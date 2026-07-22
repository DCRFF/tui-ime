# STATUS.md — 项目状态

**更新**: 2026-07-22

## 当前阶段: Phase 3 产品化 — ✅ core 完成，popup 骨架就绪

Phase 1 ✅（2026-07-21）。Phase 2 ✅（2026-07-21 M4 通过）。
Phase 3 核心交付：daemon + proxy IPC 拆分、`tui-ime.toml` 配置、可配置切换键（默认 `Ctrl+\`）。

## 已完成（Phase 3）

- [x] crate 重构：`src/lib.rs` 共享 library + 三 binary target（`tui-ime` / `tui-ime-daemon` / `tui-ime-popup`）
- [x] IPC 传输层：Unix socket + JSON 4B 长度前缀消息帧，`IpcClient` / `IpcServer`，6 项单测
- [x] IPC 协议：`ProxyRequest/Response`、`PopupRequest/Response`、`ContextSnapshot`（`src/protocol.rs`）
- [x] Daemon 核心：librime session pool（≤16）、accept-per-client 线程模型、消息分发（`src/daemon.rs`）
- [x] Daemon CLI：`tui-ime-daemon --config PATH --socket PATH`，`TUI_IME_SOCKET` 环境变量覆盖
- [x] Config 模块：`tui-ime.toml` 加载，全字段默认值回退（`src/config.rs`）
- [x] Proxy 改造：`InputFilter` 移除直接 librime，所有 IME 操作经 daemon IPC
- [x] 切换键可配置：`TUI_IME_TOGGLE=cp:mod` 环境变量 / `tui-ime.toml` [proxy] 节；
      默认 `Ctrl+\`（92:5），不冲突系统输入法
- [x] Popup 骨架：`tui-ime-popup --socket PATH --session ID`，subscribe daemon 获取候选
- [x] systemd user service：`tui-ime-daemon.service`
- [x] `cargo test` 45 项全绿（38 lib + 7 integration），3 项 ignored（SS3/ModeSnoop 待适配）

## 已知限制（Phase 3）

- **L1**: popup 为骨架，无交互式候选选择（tdb → Phase 4）
- **L2**: SS3 方向键 passthrough 在 daemon 路径下需适配（parser 产生的 `KeyEvent.raw` 不含 `\eO` 前缀）
- **L3**: ModeSnoop 集成测试需重写（按键经 IME 路径后不再原样 echo）

## 已完成（Phase 2）

所有 Phase 2 条目见 [Phase 2 计划](docs/reports/2026-07-21-03-phase2-candidate-window-plan.md)；摘要：
- inline ANSI 候选条渲染 + DSR 光标列查询
- 候选选择交互（全经 rime 原生键位）
- stdout 互斥共享、commit 先擦 UI 再注入
- ModeSnoop 终端模式重置侦测
- 防嵌套启动保护（`TUI_IME_ACTIVE`）
- 4 bug 修复（rime 拒绝键透传 / SS3 解析 / 裸 Esc / extkeys 补发）

## 已完成（Phase 1）

- PTY proxy + 三线程透传 / 扩展按键双协议 / CSI u 解析 + legacy 回译
- librime FFI 集成 + 最小上屏闭环
- M1-M3 手动验收通过

## 下一步: Phase 4 打磨

popup 交互式候选窗、双拼验证、用户词典同步、SSH 端到端测试、安装脚本 / 打包。

## 技术决策（当前有效，Phase 3 新增 *）

| 决策 | 结论 | 依据 |
|---|---|---|
| 扩展按键 | modifyOtherKeys mode 2 + kitty 0b1 双请求 | Phase 1 v1.4 |
| 组合 UI | inline 单行候选条（ANSI）；popup 骨架就绪 | Phase 2 D1 |
| IPC 协议* | Unix socket + JSON 4B 长度前缀帧，Hub-and-spoke 拓扑 | Phase 3 D2/D3 |
| 进程模型* | daemon（librime）+ proxy（PTY）+ popup（候选窗），单 package 多 binary | Phase 3 D1 |
| 切换键* | 默认 `Ctrl+\`（codepoint=92, modifiers=5），`TUI_IME_TOGGLE` 环境变量 / toml 可配 | Phase 3：避免与系统 IME 冲突 |
| librime 所有权* | daemon 独占 librime 全局初始化；proxy 不再直接链接 librime | Phase 3 D3 |
| 退出收尾* | daemon 管理 session 生命周期；proxy 断开时 daemon 自动 `session.close()` | Phase 3 |
| Kitty 编码* | 配置/环境变量使用 kitty 原始修饰键编码（bitmask+1），parser 存解码后值 | Phase 3：`TUI_IME_TOGGLE=32:5` 即 Ctrl+Space |
| 其余决策 | 同 Phase 1/2 | — |

## 阻塞项

无。

## 相关文档

- [README.md](README.md) — 项目概述
- [AGENTS.md](AGENTS.md) — AI agent 工作规范
- [可行性分析报告](docs/reports/2026-07-21-01-terminal-embedded-chinese-ime-feasibility.md)
- [实施计划（全阶段框架）](docs/reports/2026-07-21-02-implementation-plan.md)
- [Phase 2 计划](docs/reports/2026-07-21-03-phase2-candidate-window-plan.md)
- [Phase 3 计划](docs/reports/2026-07-22-01-phase3-productization-plan.md)
