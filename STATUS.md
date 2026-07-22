# STATUS.md — 项目状态

**更新**: 2026-07-22

## 当前阶段: Phase 3 产品化 — ✅ 联调通过

Phase 1 ✅（2026-07-21）。Phase 2 ✅（2026-07-21 M4 通过）。

Phase 3 核心交付完成并联调通过：daemon + proxy IPC 拆分、配置文件、可配置切换键。
tmux 多窗格端到端验证 OK（toggle → 候选条 → 上屏）；daemon 已由 systemd user service 托管。

## 已完成（Phase 3）

- [x] crate 重构：`src/lib.rs` 共享 library + 三 binary target
- [x] IPC 传输层 + 协议（6 项单测）
- [x] Daemon 核心：librime session pool、client 线程、消息分发
- [x] Daemon CLI + `TUI_IME_SOCKET` 环境变量
- [x] Config 模块：`tui-ime.toml` 加载
- [x] Proxy 改造：移除直接 librime，IME 操作经 daemon IPC
- [x] 切换键可配置：`TUI_IME_TOGGLE=cp:mod` / `tui-ime.toml [proxy]`
- [x] `librime::initialize()` 改用 `OnceLock` 防多次初始化崩溃
- [x] daemon socket 路径修正（含 `tui-ime/` 子目录）
- [x] daemon client 线程 `catch_unwind` 防 panic 丢连接
- [x] Popup 骨架 + systemd user service
- [x] daemon 部署：`~/.local/bin/` 二进制 + `systemctl --user enable --now tui-ime-daemon`
- [x] proxy 触发点改入 `~/.zshrc`（与 tmux 解耦，`TUI_IME_ACTIVE` 防嵌套）
- [x] 裸终端 modifyOtherKeys 兼容：解析 `\e[27;<mod>;<cp>~` 并归一化为 CSI u（修复非 tmux 环境 Ctrl+\ 乱码、toggle 失效）
- [x] 移除联调期临时调试日志（`toggle_check` / `rime_key` eprintln）
- [x] `cargo test --lib` 38 项全绿

## 联调状态（2026-07-22 全部通过）

| 检查点 | 状态 | 说明 |
|---|---|---|
| daemon 启动 | ✅ | systemd user service 托管（见下"新窗口无法输入"排障） |
| proxy 连接 daemon | ✅ | IPC 实测：create_session + process_key("nihao") → preedit 正常 |
| toggle 键识别 | ✅ | 默认为 `Ctrl+\`（codepoint=92, kitty modifiers=5），`is_toggle_key` 返回 true |
| 候选条渲染 | ✅ | daemon 存活时全链路正常（/tmp/tui-ime.log 21:19 会话验证） |

### 排障记录：tmux 新窗口无法输入（2026-07-22 已解决）

**现象**: 新开 tmux 窗格启动 proxy 后 toggle 无反应、打字纯透传。

**根因**: daemon 未运行。此前 daemon 是手动在某窗格前台启动的，进程结束后
所有新 proxy 连接 socket 失败，静默降级为纯透传（`src/proxy.rs` `InputFilter::new`
中 `IpcClient::connect(...).ok()` → client=None → toggle 被忽略）。
systemd unit 与 `~/.local/bin/` 二进制此前从未安装。

**修复**: 二进制安装到 `~/.local/bin/`，`tui-ime-daemon.service` 安装并
`enable --now`（`Restart=on-failure` 崩溃自动拉起）。

**遗留改进**: proxy 对 daemon 不可用的降级过于隐蔽（raw mode 下 eprintln 不可见），
可考虑启动时可见提示或连接失败时自动 spawn daemon。

## 已完成（Phase 2）

- inline ANSI 候选条渲染 + DSR 光标列查询
- 候选选择交互（全经 rime 原生键位）
- stdout 互斥共享、commit 先擦 UI 再注入
- ModeSnoop 终端模式重置侦测
- 防嵌套启动保护（`TUI_IME_ACTIVE`）
- 4 bug 修复（rime 拒绝键透传 / SS3 解析 / 裸 Esc / extkeys 补发）

## 已完成（Phase 1）

- PTY proxy + 三线程透传 / 扩展按键双协议 / CSI u 解析 + legacy 回译
- librime FFI 集成 + 最小上屏闭环

## 技术决策（当前有效）

| 决策 | 结论 |
|---|---|
| IPC 协议 | Unix socket + JSON 4B 长度前缀帧 |
| 进程模型 | daemon + proxy + popup，单 package 多 binary |
| 切换键 | 默认 `Ctrl+\`，`TUI_IME_TOGGLE` 环境变量可配 |
| librime 初始化 | daemon 独占，`OnceLock` 防多次调用 |
| Kitty 编码 | 配置用原始值（bitmask+1），parser 存解码值 |
| 组合 UI | inline 单行候选条（popup 推迟 Phase 4） |

## 相关文档

- [README.md](README.md)
- [AGENTS.md](AGENTS.md)
- [Phase 3 计划](docs/reports/2026-07-22-01-phase3-productization-plan.md)
