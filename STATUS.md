# STATUS.md — 项目状态

**更新**: 2026-07-21

## 当前阶段: Phase 2 候选窗 — 代码完成，待手动验收 M4

Phase 1 ✅（2026-07-21 DoD 全部达成）。Phase 2 采用 inline 单行候选条方案
（popup 推迟到 Phase 3，见 [Phase 2 计划](docs/reports/2026-07-21-03-phase2-candidate-window-plan.md) D1）。

## 已完成（Phase 2）

- [x] DSR（`\e[6n`）查询/响应解析：组合开始时获取光标列，精确截断候选条宽度
- [x] `src/render.rs`：inline ANSI 渲染——preedit 下划线淡色、候选 dim、高亮反显、页码指示、尾部擦除
- [x] proxy 接线：stdout 互斥共享（主线程 shell 输出 / 输入线程 IME 渲染）、commit 先擦 UI 再注入、
      toggle off 自动 Escape 取消残留组合
- [x] 候选选择交互：数字/Space/↑↓/PageUp·Down/Esc 全部经 rime 原生键位处理
- [x] 自动化验收：`cargo test` 30 项全绿（含 IME 全链路集成测试：候选条渲染 → 无裸回显 →
      commit 注入 → toggle off 恢复），fmt/clippy 干净
- [x] headless 验证（python PTY harness）：候选条字节序列正确、擦除先于 commit、toggle off 无残留

## 待用户手动验收

- [ ] **M4**: tmux 内 `--log` 运行，`Ctrl+Space` 后输入 `nihao`：光标处见淡色 preedit + 候选条；
      数字选字、翻页（PageUp/Down 或 -/=）、↑↓ 高亮正常；Space 上屏后屏幕无残留；Esc 取消正常

## 已完成（Phase 1）

- [x] 技术可行性分析（[报告](docs/reports/2026-07-21-01-terminal-embedded-chinese-ime-feasibility.md)）、
      实施计划（[全阶段框架 + Phase 1 详细](docs/reports/2026-07-21-02-implementation-plan.md)）
- [x] 第三方仓库 clone（`thirdpart/librime`、`plum`、`librime-rs`）；apt librime-dev 1.13.1
- [x] 最小 PTY proxy：PTY pair → spawn shell → 三线程透传（计划 D7）
- [x] 扩展按键双协议请求（modifyOtherKeys mode 2 + kitty 0b1，计划 v1.4）+ CSI u 增量解析 + kitty→legacy 回译
- [x] `Ctrl+Space` IME 开关（`\e[32;5u` 嗅探）
- [x] librime FFI 集成 + 最小上屏闭环（commit 写入 PTY master）
- [x] librime 退出收尾（先 `session.close()` 后 `RimeFinalize()`，计划 v1.3）

## 手动验收（Phase 1，全部通过 2026-07-21）

- [x] **M1** ✅：`ls`/`vim`/`top`/`Ctrl+C` 正常，`exit` 后终端无残留
- [x] **M2** ✅：日志出现 `\e[32;5u` + `toggle: ime on/off`，CSI u 穿透确认
- [x] **M3** ✅：候选随击键更新（preedit 链完整），`commit: 你好`，shell 行内上屏成功

**M2 排障记录**：首次失败原因是 tmux 窗格侧不认 kitty push，只认 modifyOtherKeys
mode 2（`\e[>4;2m`）；且外层终端需声明 `extkeys` 特性。proxy 已改为双协议请求（计划 v1.4），
tmux 配置要求见 README（用户已持久化 `terminal-features` 于 ~/.tmux.conf:35）。

## 下一步: Phase 3 产品化

daemon + proxy IPC（Unix socket）、`tui-ime.toml` 配置、多 session 管理、systemd user service；
tmux `display-popup` 候选窗在有 IPC 后升级（popup 按键转发需要它，见 Phase 2 计划 D1）。

## 技术决策（当前有效版）

| 决策 | 结论 | 依据 |
|---|---|---|
| 扩展按键 | modifyOtherKeys mode 2 + kitty 0b1 双请求 | 计划 v1.4：tmux 窗格侧只认前者 |
| IME 关闭透明性 | kitty→legacy 回译层 | readline/zle 不懂 CSI u，必须回译 |
| 事件模型 | 三线程（输出/输入/SIGWINCH） | 计划 D7：避开 poll 所需的 unsafe |
| 组合 UI | inline 单行候选条（ANSI） | Phase 2 计划 D1：popup 需 IPC，推迟 Phase 3 |
| librime | apt `librime-dev` 1.13.1 + rime-api path 依赖 | 计划 D2/D3 |
| 退出收尾 | 先 `session.close()` 后 `RimeFinalize()` | 计划 v1.3：否则 exit 时 librime 静态析构 SIGSEGV |
| 架构 | 单体进程，Phase 3 再拆 daemon | 计划 D1 |

## 阻塞项

无。

## 相关文档

- [README.md](README.md) — 项目概述
- [AGENTS.md](AGENTS.md) — AI agent 工作规范
- [可行性分析报告](docs/reports/2026-07-21-01-terminal-embedded-chinese-ime-feasibility.md)
- [实施计划（全阶段框架）](docs/reports/2026-07-21-02-implementation-plan.md)
- [Phase 2 计划](docs/reports/2026-07-21-03-phase2-candidate-window-plan.md)
