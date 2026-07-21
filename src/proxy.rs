//! PTY proxy：终端与子进程之间的字节透传层 + IME UI 渲染。
//!
//! 线程模型（计划 D7）：
//! - 主线程：PTY master → stdout（子进程输出）
//! - 输入线程：stdin → IME 过滤器 → PTY master（用户按键）；IME 组合 UI
//!   渲染也在此线程（写 stdout，与主线程经互斥锁共享，Phase 2 计划 D5）
//! - SIGWINCH 线程：终端尺寸变化 → `master.resize()`

use std::fs::File;
use std::io::{Read, Stdout, Write};
use std::os::unix::io::AsRawFd;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context, Result};
use nix::errno::Errno;
use nix::unistd::read;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use signal_hook::consts::SIGWINCH;
use signal_hook::iterator::Signals;

use crate::ime::Ime;
use crate::keyevent::{to_legacy, InputEvent, Parser};
use crate::keymap::{byte_to_rime, key_to_rime, XK_ESCAPE};
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
    /// rime 会话（共享给主线程做退出收尾）；初始化失败时为 None
    /// （纯透传降级，toggle 不生效）
    ime: Arc<Mutex<Option<Ime>>>,
    log: Option<File>,
    /// 与主线程共享的 stdout（渲染组合 UI / 发 DSR 查询）
    stdout: Arc<Mutex<Stdout>>,
    renderer: Renderer,
    /// 已发出 DSR 查询、等待响应中（响应到达前吞掉 `\e[r;cR`）
    dsr_pending: bool,
    /// 组合是否激活（有 preedit）
    composing: bool,
}

impl InputFilter {
    fn new(ime: Arc<Mutex<Option<Ime>>>, log: Option<File>, stdout: Arc<Mutex<Stdout>>) -> Self {
        Self {
            parser: Parser::new(),
            ime_on: false,
            ime,
            log,
            stdout,
            renderer: Renderer::new(),
            dsr_pending: false,
            composing: false,
        }
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
        let guard = self.ime.lock().expect("ime mutex");
        match guard.as_ref() {
            Some(ime) => {
                let snap = ime.snapshot();
                Strip {
                    preedit: snap.preedit,
                    candidates: snap.candidates,
                    highlighted: snap.highlighted,
                    page_no: snap.page_no,
                    is_last_page: snap.is_last_page,
                }
            }
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

    /// IME 开启时：按键 → rime；commit 文本注入 PTY master（计划 D6 时序）
    fn rime_key(&mut self, key_code: i32, modifiers: i32, writer: &mut dyn Write) -> Result<()> {
        let commit = {
            let guard = self.ime.lock().expect("ime mutex");
            match guard.as_ref() {
                Some(ime) => {
                    ime.process_key(key_code, modifiers);
                    ime.take_commit()
                }
                None => None,
            }
        };
        if let Some(text) = commit {
            // 先擦 UI → 再注入 → 仍组合中则重绘（部分提交场景）
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
        Ok(())
    }

    fn process(&mut self, chunk: &[u8], writer: &mut dyn Write) -> Result<()> {
        if self.log.is_some() {
            self.log_line(&format!("in: {}", escape_bytes(chunk)));
        }
        for ev in self.parser.feed(chunk) {
            match ev {
                InputEvent::Byte(b) => {
                    if self.ime_on {
                        if let Some((kc, mask)) = byte_to_rime(b) {
                            self.rime_key(kc, mask, writer)?;
                        }
                    } else {
                        writer.write_all(&[b]).context("forward byte")?;
                    }
                }
                InputEvent::Key(k) => {
                    if k.is_toggle() && k.is_press() {
                        if self.ime_on {
                            self.ime_on = false;
                            // 取消残留组合（计划 D7）
                            {
                                let guard = self.ime.lock().expect("ime mutex");
                                if let Some(ime) = guard.as_ref() {
                                    ime.process_key(XK_ESCAPE, 0);
                                    let _ = ime.take_commit();
                                }
                            }
                            self.clear_ui()?;
                            self.dsr_pending = false;
                            self.log_line("toggle: ime off");
                        } else if self.ime.lock().expect("ime mutex").is_some() {
                            self.ime_on = true;
                            self.log_line("toggle: ime on");
                        } else {
                            self.log_line("toggle: ignored (rime unavailable)");
                        }
                    } else if self.ime_on {
                        if let Some((kc, mask)) = key_to_rime(&k) {
                            self.rime_key(kc, mask, writer)?;
                        }
                    } else {
                        match to_legacy(&k) {
                            Some(bytes) => writer.write_all(&bytes).context("forward legacy")?,
                            None => writer.write_all(&k.raw).context("forward raw")?,
                        }
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
        }
        writer.flush().context("flush master")?;
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

/// 启动子进程并运行透传主循环，返回子进程退出码。
pub fn run(command: &[String], log: Option<File>, ime: Option<Ime>) -> Result<i32> {
    let stdout = Arc::new(Mutex::new(std::io::stdout()));
    let _raw = RawModeGuard::enter()?;
    let _extkeys = ExtendedKeysGuard::push(Arc::clone(&stdout))?;

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(term_size()).context("openpty")?;

    let mut cmd = CommandBuilder::new(&command[0]);
    cmd.args(&command[1..]);
    let mut child = pair.slave.spawn_command(cmd).context("spawn child")?;
    // 子进程已持有 slave；关闭 proxy 侧引用，保证子进程退出后 master 读端收到 EOF/EIO
    drop(pair.slave);

    let master = pair.master;
    let mut reader = master.try_clone_reader().context("clone master reader")?;
    let mut writer = master.take_writer().context("take master writer")?;
    let ime = Arc::new(Mutex::new(ime));

    // SIGWINCH 线程：重设 PTY 尺寸（TIOCSWINSZ 由 portable-pty 完成，内核负责通知子进程）
    let mut signals = Signals::new([SIGWINCH]).context("register SIGWINCH")?;
    thread::spawn(move || {
        for _ in signals.forever() {
            let _ = master.resize(term_size());
        }
    });

    // 输入线程：stdin → IME 过滤器 → PTY master
    let input_ime = Arc::clone(&ime);
    let input_stdout = Arc::clone(&stdout);
    thread::spawn(move || -> Result<()> {
        let stdin = std::io::stdin();
        let mut filter = InputFilter::new(input_ime, log, input_stdout);
        let mut buf = [0u8; 16384];
        loop {
            let n = read(stdin.as_raw_fd(), &mut buf).context("read stdin")?;
            if n == 0 {
                // 终端侧 EOF：进程随 SIGHUP 退出，此处不做额外清理
                return Ok(());
            }
            filter.process(&buf[..n], &mut *writer)?;
        }
    });

    // 主线程：PTY master → stdout；EOF/EIO 视为子进程退出
    let mut buf = [0u8; 16384];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let mut out = stdout.lock().expect("stdout mutex");
                out.write_all(&buf[..n])
                    .and_then(|()| out.flush())
                    .context("write stdout")?;
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

    // 关闭 rime 会话并 finalize：进程退出时 librime 的静态 Service 析构器
    // 会对残留会话执行引擎析构并崩溃（CleanupAllSessions → ConcreteEngine
    // 析构 SIGSEGV，已用 gdb 确认），必须先关会话、后 finalize。
    let taken = ime.lock().expect("ime mutex").take();
    let had_ime = taken.is_some();
    drop(taken); // Ime::drop → session.close()
    if had_ime {
        crate::ime::finalize();
    }

    Ok(status.exit_code() as i32)
}
