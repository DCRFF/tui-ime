//! 内部 PTY 前台进程身份镜像（tmux 兼容层）。
//!
//! tmux 观察 pane 时只能看到 proxy 进程，看不到 proxy 自建 PTY 内的程序
//! （docs/reports/2026-07-24-01-tmux-window-name-and-cwd-issues.md）：
//! - `pane_current_path` 取 /proc/<fg_pgrp>/cwd → 把内部前台 cwd 镜像到
//!   proxy 自身即可修复（已实测生效）；
//! - 窗口名（automatic-rename）取 fg 进程 /proc/<pid>/cmdline 的 argv[0]
//!   basename（tmux 3.5a osdep-linux.c `osdep_get_name`）。proxy 的 argv[0]
//!   恒为 tui-ime，而改写自身 argv 需要 unsafe（本 crate forbid(unsafe_code)），
//!   转义序列改名又被用户配置 `allow-rename off` 拦截——改为在 fg 程序变化
//!   时执行 `tmux rename-window` 驱动窗口名（TMUX_PANE 存在时启用）。
//!
//! 检查挂在 master 读循环上、100ms 限流：fg 切换 / cd 后 shell 必重绘
//! （产生输出），无需独立定时器；终端静默时零开销。

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// 两次镜像检查的最小间隔
const SYNC_INTERVAL: Duration = Duration::from_millis(100);

/// proxy 子进程（内部 PTY session leader）的前台身份镜像
pub struct IdentityMirror {
    child_pid: u32,
    last_check: Option<Instant>,
    last_cwd: Option<PathBuf>,
    /// TMUX_PANE；不在 tmux 内则为 None（改名逻辑停用）
    tmux_pane: Option<String>,
    /// 上次由我们设置的窗口名（兼作 fg comm 变化缓存）
    last_set_name: Option<String>,
    /// 改名失败或检测到用户手动改名后永久停驶
    namer_disabled: bool,
}

impl IdentityMirror {
    pub fn new(child_pid: u32) -> Self {
        Self {
            child_pid,
            last_check: None,
            last_cwd: None,
            tmux_pane: std::env::var("TMUX_PANE").ok().filter(|s| !s.is_empty()),
            last_set_name: None,
            namer_disabled: false,
        }
    }

    /// 主线程读循环每轮调用；按 SYNC_INTERVAL 限流
    pub fn maybe_sync(&mut self) {
        let now = Instant::now();
        if self
            .last_check
            .is_some_and(|t| now.duration_since(t) < SYNC_INTERVAL)
        {
            return;
        }
        self.last_check = Some(now);
        self.sync();
    }

    /// 同步内部 PTY 前台进程的 cwd 与窗口名。
    /// 前台进程随时可能退出（读 stat 与读 comm/cwd 之间存在竞态），
    /// 所有失败静默忽略、保留上次有效值，下一轮检查自愈。
    fn sync(&mut self) {
        let Some(pgid) = self.foreground_pgrp() else {
            return;
        };

        // cwd 镜像：跟随内部 shell cd，tmux pane_current_path 即同步
        if let Ok(cwd) = fs::read_link(format!("/proc/{pgid}/cwd")) {
            if self.last_cwd.as_deref() != Some(cwd.as_path())
                && std::env::set_current_dir(&cwd).is_ok()
            {
                self.last_cwd = Some(cwd);
            }
        }

        // 窗口名：fg 程序 comm 变化时驱动 tmux rename-window
        if let Ok(comm) = fs::read_to_string(format!("/proc/{pgid}/comm")) {
            let name = comm.trim_end();
            if !name.is_empty() && self.last_set_name.as_deref() != Some(name) {
                self.rename_tmux_window(name);
            }
        }
    }

    /// 内部终端的前台进程组 id：读子进程 /proc/<pid>/stat 的 tpgid
    ///（字段 8）。等价于对 master fd 做 TIOCGPGRP，但无需 unsafe 持 fd。
    fn foreground_pgrp(&self) -> Option<i32> {
        let stat = fs::read_to_string(format!("/proc/{}/stat", self.child_pid)).ok()?;
        // comm（字段 2）可含空格/右括号：从最后一个 ')' 之后按空白切分，
        // 余下字段自 field 3 (state) 起，tpgid (field 8) 即索引 5
        let rest = &stat[stat.rfind(')')? + 1..];
        let tpgid: i32 = rest.split_whitespace().nth(5)?.parse().ok()?;
        (tpgid > 0).then_some(tpgid)
    }

    /// fg 程序变化 → `tmux rename-window`。失败（tmux 不可用/pane 失效）
    /// 或检测到用户手动改名（当前名与上次我们设置的值不符）即永久停驶，
    /// 退回 tmux 默认命名行为。
    fn rename_tmux_window(&mut self, name: &str) {
        let Some(pane) = self.tmux_pane.clone() else {
            return;
        };
        if self.namer_disabled {
            return;
        }
        if let Some(prev) = self.last_set_name.clone() {
            let cur = Command::new("tmux")
                .args(["display-message", "-t", &pane, "-p", "#W"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string());
            if cur.as_deref() != Some(prev.as_str()) {
                self.namer_disabled = true;
                return;
            }
        }
        let ok = Command::new("tmux")
            .args(["rename-window", "-t", &pane, "--", name])
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            self.last_set_name = Some(name.to_string());
        } else {
            self.namer_disabled = true;
        }
    }
}
