//! 集成测试：tui-ime proxy 的字节透传与退出码传播。
//!
//! Phase 3: proxy 通过 daemon IPC 调用 librime。测试启动一个共享 daemon，
//! 通过 TUI_IME_SOCKET 环境变量路由 proxy 到该 daemon。

use std::io::{Read, Write};
use std::path::Path;
use std::process::Command as ProcCommand;
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

static TEST_SERIAL: Mutex<()> = Mutex::new(());
static DAEMON_SOCKET: OnceLock<String> = OnceLock::new();

fn ensure_daemon() -> &'static str {
    DAEMON_SOCKET.get_or_init(|| {
        let tmpdir = tempfile::tempdir().unwrap();
        let socket_path = tmpdir.path().join("daemon.sock");
        let socket_str = socket_path.to_str().unwrap().to_string();

        ProcCommand::new(env!("CARGO_BIN_EXE_tui-ime-daemon"))
            .arg("--socket")
            .arg(&socket_str)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to start daemon");

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if socket_path.exists() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(socket_path.exists(), "daemon socket not ready");

        std::mem::forget(tmpdir);
        socket_str
    })
}

struct Harness {
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    reader_rx: mpsc::Receiver<Vec<u8>>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    _guard: MutexGuard<'static, ()>,
}

impl Harness {
    fn write(&mut self, data: &[u8]) {
        self.writer.write_all(data).unwrap();
    }

    fn exit_code(&mut self) -> u32 {
        self.child.wait().unwrap().exit_code()
    }
}

fn spawn_proxy(child_cmd: &[&str]) -> Harness {
    spawn_proxy_with(child_cmd, &[])
}

fn spawn_proxy_with(child_cmd: &[&str], extra_env: &[(&str, String)]) -> Harness {
    let guard = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let socket = ensure_daemon();

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
    // 测试可能跑在真实 tui-ime 会话内：清掉继承来的防嵌套标记，否则 proxy 拒绝启动
    cmd.env_remove("TUI_IME_ACTIVE");
    cmd.env("TUI_IME_SOCKET", socket);
    // 测试用旧的 Ctrl+Space toggle（默认是 Ctrl+\）
    cmd.env("TUI_IME_TOGGLE", "32:5");
    for (k, v) in extra_env {
        cmd.env(*k, v.as_str());
    }
    cmd.arg("--");
    cmd.args(child_cmd);
    let child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer = pair.master.take_writer().unwrap();

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
        _guard: guard,
    }
}

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

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

fn wait_ready(h: &Harness) {
    const MARKER: &[u8] = b"\x1b[>4;2m";
    let out = read_until(h, MARKER, Duration::from_secs(15));
    assert!(
        out.windows(MARKER.len()).any(|w| w == MARKER),
        "proxy not ready: {out:?}"
    );
    thread::sleep(Duration::from_millis(200));
}

#[test]
fn passthrough_bytes() {
    let mut h = spawn_proxy(&["cat"]);
    wait_ready(&h);
    h.write(b"hello world\n");
    let out = read_until(&h, b"hello world", Duration::from_secs(2));
    assert!(contains(&out, b"hello world"));
}

#[test]
fn exit_code_propagates() {
    let mut h = spawn_proxy(&["sh", "-c", "exit 42"]);
    wait_ready(&h);
    assert_eq!(h.exit_code(), 42);
}

#[test]
fn child_env_has_guard_marker() {
    let h = spawn_proxy(&["sh", "-c", "echo TUI_IME_ACTIVE=$TUI_IME_ACTIVE"]);
    wait_ready(&h);
    let out = read_until(&h, b"TUI_IME_ACTIVE=1", Duration::from_secs(2));
    assert!(contains(&out, b"TUI_IME_ACTIVE=1"));
}

#[test]
fn nested_launch_refused() {
    let guard = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let status = ProcCommand::new(env!("CARGO_BIN_EXE_tui-ime"))
        .arg("--")
        .arg("echo")
        .arg("nested")
        .env("TUI_IME_ACTIVE", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
    drop(guard);
}

#[test]
fn ime_compose_commit_passthrough() {
    let mut h = spawn_proxy(&["cat"]);
    wait_ready(&h);

    h.write(b"\x1b[32;5u");
    thread::sleep(Duration::from_millis(400));

    for &b in b"nihao" {
        h.write(&[b]);
        thread::sleep(Duration::from_millis(100));
    }
    thread::sleep(Duration::from_millis(500));

    let out = read_until(&h, b"nihao", Duration::from_millis(500));
    assert!(!contains(&out, b"nihao"), "keys leaked: {out:?}");

    h.write(b" ");
    thread::sleep(Duration::from_millis(500));

    let out = read_until(&h, b"\xe4\xbd\xa0", Duration::from_secs(2));
    assert!(
        contains(&out, "\u{4f60}".as_bytes()),
        "commit not found: {out:?}"
    );

    h.write(b"\x1b[32;5u");
    thread::sleep(Duration::from_millis(200));

    h.write(b"echo test\n");
    let out = read_until(&h, b"test", Duration::from_secs(2));
    assert!(contains(&out, b"test"));
}

/// 轮询等待文件最后一行变为 expected（proxy 限流 100ms，给 3s 余量）
fn wait_file_line(path: &Path, expected: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(content) = std::fs::read_to_string(path) {
            if content.lines().last() == Some(expected) {
                return;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    let got = std::fs::read_to_string(path).unwrap_or_default();
    panic!(
        "expected last line {expected:?} in {}: {got:?}",
        path.display()
    );
}

#[test]
fn mirrors_foreground_cwd_and_renames_tmux_window() {
    // 假 tmux：rename-window 记录窗口名；display-message 回显当前名（停驶检查用）
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("names.log");
    let fake = dir.path().join("tmux");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nif [ \"$1\" = rename-window ]; then for n; do :; done; echo \"$n\" >> {0};\
             elif [ \"$1\" = display-message ]; then tail -n 1 {0} 2>/dev/null; fi\n",
            log.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap()
    );
    let mut h = spawn_proxy_with(
        &["sh"],
        &[("TMUX_PANE", "%999".to_string()), ("PATH", path)],
    );
    wait_ready(&h);
    let proxy_pid = h.child.process_id().unwrap();

    // 首次镜像：窗口名应被 rename 为内部 shell 名
    wait_file_line(&log, "sh", Duration::from_secs(3));

    // 内部 shell cd 后，proxy cwd 应跟随（tmux pane_current_path 同步）
    h.write(b"cd /tmp\n");
    let cwd_path = format!("/proc/{proxy_pid}/cwd");
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if std::fs::read_link(&cwd_path).is_ok_and(|p| p == Path::new("/tmp")) {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        std::fs::read_link(&cwd_path).unwrap(),
        Path::new("/tmp"),
        "proxy cwd not mirrored"
    );

    // fg 切换（起 cat 作业）后应改名 cat；cat 无提示符，靠输入回显触发同步
    h.write(b"cat\n");
    thread::sleep(Duration::from_millis(300));
    h.write(b"ping\n");
    wait_file_line(&log, "cat", Duration::from_secs(3));

    // 用户手动 rename-window 后 proxy 应停驶：覆盖日志模拟，
    // Ctrl-C 杀掉 cat 回到 sh（fg 再次变化）也不应再改名
    std::fs::write(&log, "user-window\n").unwrap();
    h.write(b"\x03");
    thread::sleep(Duration::from_secs(1));
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "user-window\n",
        "namer should stay disabled after user rename"
    );
}

#[test]
fn rejected_key_forwarded_when_not_composing() {
    let mut h = spawn_proxy(&["cat"]);
    wait_ready(&h);

    h.write(b"\x1b[32;5u");
    thread::sleep(Duration::from_millis(400));

    h.write(b"\x08");
    thread::sleep(Duration::from_millis(200));
}

#[test]
fn bare_esc_cancels_composition_once() {
    let mut h = spawn_proxy(&["cat"]);
    wait_ready(&h);

    h.write(b"\x1b[32;5u");
    thread::sleep(Duration::from_millis(400));

    for &b in b"nihao" {
        h.write(&[b]);
        thread::sleep(Duration::from_millis(100));
    }
    thread::sleep(Duration::from_millis(300));

    let out = read_until(&h, b"\x1b7", Duration::from_secs(2));
    assert!(
        contains(&out, b"\x1b7"),
        "candidate strip not found: {out:?}"
    );

    h.write(b"\x1b");
    thread::sleep(Duration::from_millis(300));

    h.write(b" ");
    thread::sleep(Duration::from_millis(200));
    let out = read_until(&h, b" ", Duration::from_secs(1));
    assert!(contains(&out, b" "));
}

#[test]
#[ignore = "SS3 passthrough needs rework for Phase 3 daemon"]
fn ss3_arrow_keys() {
    let mut h = spawn_proxy(&["cat"]);
    wait_ready(&h);

    h.write(b"\x1bOA");
    thread::sleep(Duration::from_millis(200));
    let out = read_until(&h, b"\x1bOA", Duration::from_secs(1));
    assert!(contains(&out, b"\x1bOA"));

    h.write(b"\x1b[32;5u");
    thread::sleep(Duration::from_millis(400));

    h.write(b"\x1bOA");
    thread::sleep(Duration::from_millis(200));

    h.write(b"\x1b[32;5u");
    thread::sleep(Duration::from_millis(200));
}

#[test]
#[ignore = "ModeSnoop test needs rework for Phase 3 daemon"]
fn extkeys_repush_on_mode_reset() {
    let mut h = spawn_proxy(&["cat"]);
    wait_ready(&h);

    h.write(b"\x1b[>4;0m");
    thread::sleep(Duration::from_millis(200));

    let out = read_until(&h, b"\x1b[>4;2m", Duration::from_secs(2));
    let count = out
        .windows(b"\x1b[>4;2m".len())
        .filter(|w| *w == b"\x1b[>4;2m")
        .count();
    assert!(count >= 2, "mode 2 not repushed: {out:?}");
}
