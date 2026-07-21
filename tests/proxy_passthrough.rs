//! 集成测试：tui-ime proxy 的字节透传与退出码传播。
//!
//! harness 结构：test → 外层 PTY → tui-ime（proxy）→ 内层 PTY → 子进程。

use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

struct Harness {
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    reader_rx: mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

fn spawn_proxy(child_cmd: &[&str]) -> Harness {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_tui-ime"));
    cmd.arg("--");
    cmd.args(child_cmd);
    let child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer = pair.master.take_writer().unwrap();

    // 读线程：把外层 master 的输出搬进 channel，主测试线程带超时消费
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    Harness {
        master: pair.master,
        reader_rx: rx,
        writer,
        child,
    }
}

/// 在超时时间内收集输出，直到包含 expected 或超时返回
fn read_until(h: &Harness, expected: &[u8], timeout: Duration) -> Vec<u8> {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    while Instant::now() < deadline {
        match h.reader_rx.recv_timeout(deadline - Instant::now()) {
            Ok(chunk) => {
                out.extend_from_slice(&chunk);
                if out.windows(expected.len()).any(|w| w == expected) {
                    return out;
                }
            }
            Err(_) => break,
        }
    }
    out
}

/// 子串匹配（windows 长度必须等于 needle 长度）
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// 等 proxy 就绪：启动完成后会向终端请求扩展按键（\e[>4;2m\e[>1u）。
/// rime 首次部署较慢，给足超时。
fn wait_ready(h: &Harness) {
    const MARKER: &[u8] = b"\x1b[>4;2m";
    let out = read_until(h, MARKER, Duration::from_secs(15));
    assert!(
        out.windows(MARKER.len()).any(|w| w == MARKER),
        "proxy 未就绪: {out:?}"
    );
    // 等子进程（stty/cat）完成自身初始化
    thread::sleep(Duration::from_millis(200));
}

/// 字节级透传：写入 proxy 的内容应被子进程（cat）原样回显
#[test]
fn passthrough_bytes() {
    let mut h = spawn_proxy(&["sh", "-c", "stty raw -echo; exec cat"]);
    wait_ready(&h);

    let payload = "hello, 世界\n";
    h.writer.write_all(payload.as_bytes()).unwrap();
    h.writer.flush().unwrap();

    let out = read_until(&h, payload.as_bytes(), Duration::from_secs(5));
    assert!(
        out.windows(payload.len()).any(|w| w == payload.as_bytes()),
        "expected echo of {payload:?}, got {out:?}"
    );
}

/// 子进程退出码应逐层传播回 harness
#[test]
fn exit_code_propagates() {
    let mut h = spawn_proxy(&["sh", "-c", "exit 42"]);
    let status = h.child.wait().unwrap();
    assert_eq!(status.exit_code(), 42);
}

/// IME 全链路：toggle 开 → 输入被 rime 消费（无裸回显）→ 候选条渲染 →
/// Space commit 注入 → toggle 关 → 恢复透传
#[test]
fn ime_compose_commit_passthrough() {
    let mut h = spawn_proxy(&["sh", "-c", "stty raw -echo; exec cat"]);
    wait_ready(&h);

    // 开启前：正常透传
    h.writer.write_all(b"ab").unwrap();
    h.writer.flush().unwrap();
    let out = read_until(&h, b"ab", Duration::from_secs(5));
    assert!(out.windows(2).any(|w| w == b"ab"), "got {out:?}");

    // toggle on + 输入 nihao：按键进 rime，候选条渲染（含 你好 候选）
    h.writer.write_all(b"\x1b[32;5u").unwrap();
    h.writer.write_all(b"nihao").unwrap();
    h.writer.flush().unwrap();
    let out = read_until(&h, "你好".as_bytes(), Duration::from_secs(5));
    assert!(
        contains(&out, "你好".as_bytes()),
        "候选条应包含 你好: {out:?}"
    );
    // 候选条绘制标记（\e7 保存光标）
    assert!(contains(&out, b"\x1b7"), "应出现候选条绘制序列: {out:?}");
    // 按键未被转发：preedit 分词为 "ni hao"，裸 "nihao" 只可能来自 cat 回显
    assert!(
        !contains(&out, b"nihao"),
        "IME 开启期间按键不应转发, got {out:?}"
    );

    // Space 首选上屏：commit 注入 master → cat 回显 你好
    h.writer.write_all(b" ").unwrap();
    h.writer.flush().unwrap();
    let out = read_until(&h, "你好".as_bytes(), Duration::from_secs(5));
    assert!(
        contains(&out, "你好".as_bytes()),
        "commit 后应回显 你好: {out:?}"
    );

    // toggle off → 恢复透传
    h.writer.write_all(b"\x1b[32;5u").unwrap();
    thread::sleep(Duration::from_millis(100));
    h.writer.write_all(b"ef").unwrap();
    h.writer.flush().unwrap();
    let out = read_until(&h, b"ef", Duration::from_secs(5));
    assert!(out.windows(2).any(|w| w == b"ef"), "got {out:?}");
}
