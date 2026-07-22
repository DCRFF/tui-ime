//! PTY proxy：终端与子进程之间的字节透传层 + IME UI 渲染。
//!
//! 线程模型（计划 D7）：
//! - 主线程：PTY master → stdout（子进程输出）；ModeSnoop 侦测输出中的
//!   终端模式重置并补发 extkeys 请求
//! - 输入线程：stdin → IME 过滤器 → PTY master（用户按键）；IME 组合 UI
//!   渲染也在此线程（写 stdout，与主线程经互斥锁共享，Phase 2 计划 D5）。
//!   解析器有未完成序列时 poll 30ms 超时冲刷（裸 Esc 判定）
//! - SIGWINCH 线程：终端尺寸变化 → `master.resize()`

use std::fs::File;
use std::io::{Read, Stdout, Write};
use std::os::unix::io::{AsFd, AsRawFd};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use nix::errno::Errno;
use nix::poll::{poll, PollFd, PollFlags, PollTimeout};
use nix::unistd::read;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use signal_hook::consts::SIGWINCH;
use signal_hook::iterator::Signals;

/// 裸 Esc 判定超时：解析器持有未完成序列时，超过该时长无后续字节即冲刷。
/// 30ms 与常见编辑器 esctimeout 同级；序列字节总在同一数据包内到达，不受影响。
const ESC_FLUSH_MS: u16 = 30;

use crate::daemon;
use crate::ipc::IpcClient;
use crate::keyevent::{to_legacy, InputEvent, Parser};
use crate::keymap::{byte_to_rime, key_to_rime, XK_ESCAPE};
use crate::protocol::{ContextSnapshot, ProxyRequest, ProxyResponse};
use crate::render::{Renderer, Strip};

/// 当前终端尺寸，读取失败或终端上报 0x0（裸 PTY 默认值）时退回 80x24
fn term_size() -> PtySize {
    let (cols, rows) = match crossterm::terminal::size() {
        Ok((c, r)) if c > 0 && r > 0 => (c, r),
        _ => (80, 24),
    };
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn term_cols() -> u16 {
    match crossterm::terminal::size() {
        Ok((c, _)) if c > 0 => c,
        _ => 80,
    }
}

/// RAII：进入时开启终端 raw mode，drop 时恢复
struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> Result<Self> {
        crossterm::terminal::enable_raw_mode().context("enable raw mode")?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// RAII：进入时向终端请求扩展按键上报（计划 D8），drop 时恢复。
/// 同时请求两种协议：
/// - modifyOtherKeys mode 2（`\e[>4;2m`）：tmux 窗格侧仅认此协议
///   （tmux 不解析 kitty push，实测 mode 2 下 Ctrl+Space 才以 CSI u 上报）
/// - kitty disambiguate（`\e[>1u`）：WezTerm 等终端原生 kitty protocol
struct ExtendedKeysGuard {
    stdout: Arc<Mutex<Stdout>>,
}

impl ExtendedKeysGuard {
    fn push(stdout: Arc<Mutex<Stdout>>) -> Result<Self> {
        {
            let mut out = stdout.lock().expect("stdout mutex");
            out.write_all(b"\x1b[>4;2m\x1b[>1u")
                .and_then(|()| out.flush())
                .context("request extended keys")?;
        }
        Ok(Self { stdout })
    }
}

impl Drop for ExtendedKeysGuard {
    fn drop(&mut self) {
        let mut out = self.stdout.lock().expect("stdout mutex");
        let _ = out.write_all(b"\x1b[<u\x1b[>4;0m");
        let _ = out.flush();
    }
}

/// 输入过滤器：解析按键、处理 IME 开关、渲染组合 UI、决定转发/消费
struct InputFilter {
    parser: Parser,
    ime_on: bool,
    /// daemon IPC client（连接失败时为 None → 纯透传降级）
    client: Option<IpcClient>,
    /// daemon session id（create_session 成功后填入）
    session_id: Option<u32>,
    /// 缓存最近一次 ProcessKey 返回的 context
    last_context: Option<ContextSnapshot>,
    /// 可配置的 toggle 键
    toggle_codepoint: u32,
    toggle_modifiers: u8,
    log: Option<File>,
    /// 与主线程共享的 stdout（渲染组合 UI / 发 DSR 查询）
    stdout: Arc<Mutex<Stdout>>,
    renderer: Renderer,
    dsr_pending: bool,
    composing: bool,
}
impl InputFilter {
    fn new(log: Option<File>, stdout: Arc<Mutex<Stdout>>) -> Self {
        let socket_path = daemon::default_socket_path();
        let mut client = IpcClient::connect(&socket_path).ok();

        // 连接成功后立即创建 session
        let session_id = match client.as_mut() {
            Some(c) => match c.request::<ProxyResponse>(&ProxyRequest::CreateSession) {
                Ok(ProxyResponse::SessionCreated { session_id }) => Some(session_id),
                Ok(other) => {
                    eprintln!("tui-ime: daemon refused session: {other:?}");
                    None
                }
                Err(e) => {
                    eprintln!("tui-ime: create_session failed: {e}");
                    None
                }
            },
            None => None,
        };

        let (toggle_codepoint, toggle_modifiers) = {
            let cfg = crate::config::Config::default();
            crate::config::toggle_key(&cfg.proxy)
        };
        Self {
            parser: Parser::new(),
            ime_on: false,
            client,
            session_id,
            last_context: None,
            toggle_codepoint,
            toggle_modifiers,
            log,
            stdout,
            renderer: Renderer::new(),
            dsr_pending: false,
            composing: false,
        }
    }

    fn is_toggle_key(&self, k: &crate::keyevent::KeyEvent) -> bool {
        let want_mod = if self.toggle_modifiers > 0 { self.toggle_modifiers - 1 } else { 0 };
        k.is_press() && k.terminator == b'u'
            && k.codepoint == self.toggle_codepoint && k.modifiers == want_mod
    }
    fn log_line(&mut self, line: &str) {
        if let Some(f) = &mut self.log {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }
    fn write_stdout(&mut self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut out = self.stdout.lock().expect("stdout mutex");
        out.write_all(bytes)
            .and_then(|()| out.flush())
            .context("write stdout (ime ui)")
    }

    /// 发出 DSR 查询（组合空→非空跃迁时调用）
    fn query_cursor_col(&mut self) -> Result<()> {
        self.dsr_pending = true;
        self.write_stdout(b"\x1b[6n")
    }

    fn snapshot_strip(&self) -> Strip {
        match &self.last_context {
            Some(ctx) => Strip {
                preedit: ctx.preedit.clone(),
                candidates: ctx.candidates.clone(),
                highlighted: ctx.highlighted,
                page_no: ctx.page_no,
                is_last_page: ctx.is_last_page,
            },
            None => Strip::default(),
        }
    }
    /// 取消组合并擦除 UI（toggle off / 组合结束时调用）
    fn clear_ui(&mut self) -> Result<()> {
        let erased = self.renderer.erase();
        self.write_stdout(&erased)?;
        self.renderer.reset();
        self.composing = false;
        Ok(())
    }

    /// 按当前快照渲染/擦除组合 UI
    fn render_ui(&mut self, strip: &Strip) -> Result<()> {
        let was_composing = self.composing;
        self.composing = strip.is_active();
        if self.composing {
            if !was_composing {
                // 空→非空跃迁：查询光标列以精确截断（计划 D3）
                self.query_cursor_col()?;
            }
            let out = self.renderer.draw(strip, term_cols());
            self.write_stdout(&out)?;
        } else if was_composing {
            self.clear_ui()?;
        }
        Ok(())
    }

    /// 返回 true = rime 消费了按键；false = rime 拒绝（调用方在非组合态应透传原按键）
    fn rime_key(&mut self, key_code: i32, modifiers: i32, writer: &mut dyn Write) -> Result<bool> {
        let session_id = match self.session_id {
            Some(id) => id,
            None => return Ok(false),
        };
        let Some(ref mut client) = self.client else {
            return Ok(false);
        };

        let resp: ProxyResponse = client
            .request(&ProxyRequest::ProcessKey {
                session_id,
                keycode: key_code,
                modifiers,
            })
            .map_err(|e| anyhow::anyhow!("daemon IPC: {e}"))?;

        match resp {
            ProxyResponse::ProcessKeyResult {
                consumed,
                commit,
                context,
            } => {
                if !consumed {
                    return Ok(false);
                }
                self.last_context = context;
                if let Some(text) = commit {
                    // 先擦 UI → 再注入 → 仍组合中则重绘
                    let erased = self.renderer.erase();
                    self.write_stdout(&erased)?;
                    self.renderer.reset();
                    writer.write_all(text.as_bytes()).context("inject commit")?;
                    self.log_line(&format!("commit: {text}"));
                }
                let strip = self.snapshot_strip();
                self.render_ui(&strip)?;
                if self.log.is_some() {
                    let preview: Vec<String> = strip.candidates.iter().take(9).cloned().collect();
                    self.log_line(&format!(
                        "preedit: {:?} candidates: {:?}",
                        strip.preedit, preview
                    ));
                }
                Ok(true)
            }
            ProxyResponse::Error { message } => {
                self.log_line(&format!("daemon error: {message}"));
                Ok(false)
            }
            other => {
                self.log_line(&format!("unexpected daemon response: {other:?}"));
                Ok(false)
            }
        }
    }

    fn process(&mut self, chunk: &[u8], writer: &mut dyn Write) -> Result<()> {
        if self.log.is_some() {
            self.log_line(&format!("in: {}", escape_bytes(chunk)));
        }
        for ev in self.parser.feed(chunk) {
            self.handle_event(ev, writer)?;
        }
        writer.flush().context("flush master")?;
        Ok(())
    }

    /// 输入静默超时（裸 Esc / 截断序列判定）：冲刷解析器并按事件处理
    fn process_pending(&mut self, writer: &mut dyn Write) -> Result<()> {
        for ev in self.parser.flush() {
            self.handle_event(ev, writer)?;
        }
        writer.flush().context("flush master")?;
        Ok(())
    }

    fn parser_has_pending(&self) -> bool {
        self.parser.has_pending()
    }

    fn handle_event(&mut self, ev: InputEvent, writer: &mut dyn Write) -> Result<()> {
        match ev {
            InputEvent::Byte(b) => {
                if self.ime_on {
                    match byte_to_rime(b) {
                        Some((kc, mask)) => {
                            let consumed = self.rime_key(kc, mask, writer)?;
                            // rime 拒绝且非组合态：透传（如空闲时 Backspace 删已上屏文字）
                            if !consumed && !self.composing {
                                writer.write_all(&[b]).context("forward rejected byte")?;
                            }
                        }
                        // 无 rime 映射（C0 控制字节等）：非组合态透传
                        None if !self.composing => {
                            writer.write_all(&[b]).context("forward unmapped byte")?;
                        }
                        None => {}
                    }
                } else {
                    writer.write_all(&[b]).context("forward byte")?;
                }
            }
            InputEvent::Key(k) => {
                if self.is_toggle_key(&k) {
                    if self.ime_on {
                        self.ime_on = false;
                        // 取消残留组合（计划 D7）：经 daemon 发送 Esc
                        if let (Some(sid), Some(ref mut client)) =
                            (self.session_id, &mut self.client)
                        {
                            let _resp: Result<ProxyResponse, _> =
                                client.request(&ProxyRequest::ProcessKey {
                                    session_id: sid,
                                    keycode: XK_ESCAPE,
                                    modifiers: 0,
                                });
                        }
                        self.last_context = None;
                        self.clear_ui()?;
                        self.dsr_pending = false;
                        self.log_line("toggle: ime off");
                    } else if self.client.is_some() {
                        self.ime_on = true;
                        self.log_line("toggle: ime on");
                    } else {
                        self.log_line("toggle: ignored (daemon unavailable)");
                    }
                } else if self.ime_on {
                    match key_to_rime(&k) {
                        Some((kc, mask)) => {
                            let consumed = self.rime_key(kc, mask, writer)?;
                            if !consumed && !self.composing {
                                forward_key(&k, writer)?;
                            }
                        }
                        None if !self.composing => forward_key(&k, writer)?,
                        None => {}
                    }
                } else {
                    forward_key(&k, writer)?;
                }
            }
            InputEvent::Dsr { col, raw, .. } => {
                if self.dsr_pending {
                    // 我们发起的查询：记录列并按精确宽度重绘
                    self.dsr_pending = false;
                    self.renderer.set_cursor_col(col);
                    if self.composing {
                        let strip = self.snapshot_strip();
                        let out = self.renderer.draw(&strip, term_cols());
                        self.write_stdout(&out)?;
                    }
                } else {
                    // 非我们发起的查询响应：转发给 shell
                    writer.write_all(&raw).context("forward dsr")?;
                }
            }
        }
        Ok(())
    }
}

/// 日志用：字节流转可读转义串（0x1b → \e，可打印 ASCII 原样，其余 \xNN）
fn escape_bytes(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        match b {
            0x1b => s.push_str("\\e"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("\\x{b:02x}")),
        }
    }
    s
}

/// 按键按 legacy 语义转发给子进程（IME 关闭 / rime 拒绝 / 无映射时）
fn forward_key(k: &crate::keyevent::KeyEvent, writer: &mut dyn Write) -> Result<()> {
    match to_legacy(k) {
        Some(bytes) => writer.write_all(&bytes).context("forward legacy")?,
        None => writer.write_all(&k.raw).context("forward raw")?,
    }
    Ok(())
}

/// 子进程输出中的终端模式重置侦测（M4 排障：nvim 退出时显式发出
/// `\e[>4;0m` 重置 modifyOtherKeys，此后 Ctrl+Space 退化为 \x00，toggle 失效）。
/// 侦测到重置即在转发后补发对应模式请求；kitty push/pop 对称补发，保持栈平衡。
struct ModeSnoop {
    /// 上一 chunk 尾部（最长模式序列 7 字节，跨 chunk 匹配用）
    tail: Vec<u8>,
}

/// modifyOtherKeys 重置（mode 0/1 都会使 Ctrl+Space 失去 CSI u 编码）
const RESET_MOK0: &[u8] = b"\x1b[>4;0m";
const RESET_MOK1: &[u8] = b"\x1b[>4;1m";
const PUSH_MOK: &[u8] = b"\x1b[>4;2m";
/// kitty 键盘栈 pop（可能弹掉 proxy 启动时 push 的 disambiguate）
const POP_KITTY: &[u8] = b"\x1b[<u";
const PUSH_KITTY: &[u8] = b"\x1b[>1u";

impl ModeSnoop {
    fn new() -> Self {
        Self { tail: Vec::new() }
    }

    /// 扫描一块子进程输出，返回需补发的模式请求字节（可为空）
    fn scan(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut hay = std::mem::take(&mut self.tail);
        hay.extend_from_slice(chunk);

        let mut repush = Vec::new();
        if contains_seq(&hay, RESET_MOK0) || contains_seq(&hay, RESET_MOK1) {
            repush.extend_from_slice(PUSH_MOK);
        }
        if contains_seq(&hay, POP_KITTY) {
            repush.extend_from_slice(PUSH_KITTY);
        }

        let keep = 6.min(hay.len());
        self.tail = hay[hay.len() - keep..].to_vec();
        repush
    }
}

fn contains_seq(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// 启动子进程并运行透传主循环，返回子进程退出码。
pub fn run(command: &[String], log: Option<File>) -> Result<i32> {
    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let _raw = RawModeGuard::enter()?;
    let _extkeys = ExtendedKeysGuard::push(Arc::clone(&stdout))?;

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(term_size()).context("openpty")?;

    let mut cmd = CommandBuilder::new(&command[0]);
    cmd.args(&command[1..]);
    // 注入防嵌套标记：子进程（及其后代）再启动 tui-ime 时据此拒绝
    cmd.env(crate::NEST_GUARD_ENV, "1");
    // 继承当前工作目录（否则 shell 默认跳到 ~/）
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }
    let mut child = pair.slave.spawn_command(cmd).context("spawn child")?;
    // 子进程已持有 slave；关闭 proxy 侧引用，保证子进程退出后 master 读端收到 EOF/EIO
    drop(pair.slave);

    let master = pair.master;
    let mut reader = master.try_clone_reader().context("clone master reader")?;
    let mut writer = master.take_writer().context("take master writer")?;

    let mut signals = Signals::new([SIGWINCH]).context("register SIGWINCH")?;
    thread::spawn(move || {
        for _ in signals.forever() {
            let _ = master.resize(term_size());
        }
    });

    // 输入线程：stdin → IME 过滤器 → PTY master。
    // 解析器有未完成序列时用 30ms poll 超时判定裸 Esc（否则裸 Esc 会被
    // 当成转义序列前缀无限攒住，需按两遍才生效）；无未完成序列则无限阻塞。
    let input_stdout = Arc::clone(&stdout);
    thread::spawn(move || -> Result<()> {
        let stdin = std::io::stdin();
        let mut filter = InputFilter::new(log, input_stdout);
        let mut buf = [0u8; 16384];
        loop {
            let timeout = if filter.parser_has_pending() {
                PollTimeout::from(ESC_FLUSH_MS)
            } else {
                PollTimeout::NONE
            };
            let mut fds = [PollFd::new(stdin.as_fd(), PollFlags::POLLIN)];
            let nready = poll(&mut fds, timeout).context("poll stdin")?;
            if nready == 0 {
                filter.process_pending(&mut *writer)?;
                continue;
            }
            let n = read(stdin.as_raw_fd(), &mut buf).context("read stdin")?;
            if n == 0 {
                // 终端侧 EOF：进程随 SIGHUP 退出，此处不做额外清理
                return Ok(());
            }
            filter.process(&buf[..n], &mut *writer)?;
        }
    });

    // 主线程：PTY master → stdout；EOF/EIO 视为子进程退出。
    // ModeSnoop 侦测子进程输出中的终端模式重置（如 nvim 退出时的 \e[>4;0m），
    // 转发后补发模式请求，保证 toggle 键在全屏程序退出后仍可用。
    let mut snoop = ModeSnoop::new();
    let mut buf = [0u8; 16384];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let repush = snoop.scan(&buf[..n]);
                let mut out = stdout.lock().expect("stdout mutex");
                out.write_all(&buf[..n])?;
                if !repush.is_empty() {
                    out.write_all(&repush)?;
                }
                out.flush().context("write stdout")?;
            }
            Err(e) => {
                // Linux PTY：slave 全部关闭后 master read 返回 EIO
                if e.raw_os_error() == Some(Errno::EIO as i32) {
                    break;
                }
                return Err(e).context("read master");
            }
        }
    }

    let status = child.wait().context("wait child")?;

    Ok(status.exit_code() as i32)
}
