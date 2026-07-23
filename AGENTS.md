# AGENTS.md — tui-ime AI Agent 工作规范

## 项目定位

终端嵌入式中文输入法。详见 [README.md](README.md) 和 [STATUS.md](STATUS.md)。

## 核心规则

### 1. 只做你被要求的事

- 不要"顺便"重构相邻代码、优化注释、调整格式
- 不要在没有要求的情况下创建抽象层
- 修改任何文件前，先理解它为什么存在

### 2. 不要修改 thirdpart/ 下的任何文件

`thirdpart/librime/`、`thirdpart/plum/`、`thirdpart/librime-rs/` 是上游仓库的 clone。**只读，永不编辑**。如需修改（如 build 脚本适配），在 src/ 下创建 wrapper 或 patch 文件。

### 3. 先读再写

- 读相关文件 → 理解现有模式 → 遵循现有模式 → 再写
- 不要凭记忆或猜测写代码

### 4. Rust 项目规范

- 代码风格遵循 `rustfmt` 默认配置
- 依赖尽量精简，能用标准库就不用 crate
- 不使用 `unsafe`，除 librime FFI 调用外
- 错误处理使用 `anyhow`（应用层）或 `thiserror`（库层）

### 5. 测试规范

- 修改行为 → 写测试。但只测试行为，不测试实现细节
- 测试放在 `src/` 同目录或 `tests/` 目录
- 运行测试: `cargo test`
- 集成测试位于 `tests/proxy_passthrough.rs`：通过 PTY harness 启动 daemon + proxy + 子进程全链路测试

### 6. Git 提交

- Commit message: `type(scope): [YYYY-MM-DD] description`（英文），提交前 `git diff --stat` 确认。

### 7. 目录与文件

- `docs/` 已加入 `.gitignore` 不提交；分析文档命名: `YYYY-MM-DD-<序号> <简述>.md`。

### 8. IPC 与模块边界

- `src/protocol.rs` 定义 IPC 消息类型（`ProxyRequest/Response`、`PopupRequest/Response`、`ContextSnapshot`）。新增消息类型必须在此文件定义，保持 proxy/daemon/popup 三者共享同一份协议。
- `src/ipc.rs` 是传输层：Unix socket + JSON 4B 长度前缀帧。`IpcClient` / `IpcServer` 为对外 API。
- daemon（`src/daemon.rs` + `src/bin/daemon.rs`）独占 librime 所有权，永不与其他组件共享 rime 全局状态。
- proxy 不直接链接/初始化 librime；所有 IME 操作经 daemon IPC。

## 架构参考

详细设计见 [可行性分析报告](docs/reports/2026-07-21-01-terminal-embedded-chinese-ime-feasibility.md) 和 [Phase 3 计划](docs/reports/2026-07-22-01-phase3-productization-plan.md)。

### 源码结构（Phase 3）

```
src/
  lib.rs           # library root — 声明所有模块
  protocol.rs      # IPC 消息类型定义
  ipc.rs           # Unix socket 传输层（IpcClient / IpcServer）
  config.rs        # tui-ime.toml 加载 + TUI_IME_TOGGLE 环境变量解析
  daemon.rs        # librime session pool + client 消息分发
  ime.rs           # librime 会话封装（daemon 使用；proxy 已移除直接依赖）
  keyevent.rs      # CSI u / SS3 增量解析 + kitty→legacy 回译
  keymap.rs        # 按键 → rime keycode/mask 映射
  proxy.rs         # PTY proxy 核心：透传 + InputFilter（经 daemon IPC）
  render.rs        # inline ANSI 候选条渲染（横向不足时 IL/DL 借下一行）
  bin/
    daemon.rs      # tui-ime-daemon CLI 入口
    popup.rs       # tui-ime-popup CLI 入口（骨架）
  main.rs          # tui-ime（proxy）CLI 入口
tests/
  proxy_passthrough.rs  # 集成测试（启动 daemon + proxy + PTY 子进程全链路）
```

### 关键架构决策

- **PTY proxy 模式**: 创建 PTY pair，proxy 在 master 端拦截 I/O
- **Kitty keyboard protocol**: WezTerm 内置支持，双协议请求（modifyOtherKeys mode 2 + kitty disambiguate 0b1）
- **tmux display-popup**: 候选窗渲染（Phase 3 骨架，Phase 4 完善）
- **Daemon + Proxy 分离**: daemon 管理 librime 生命周期，proxy 管理 PTY。Unix socket IPC，Hub-and-spoke 拓扑（proxy/popup 只连 daemon，彼此不直连）
- **切换键**: 默认 `Ctrl+\`，通过 `TUI_IME_TOGGLE` 环境变量或 `tui-ime.toml` [proxy] 节配置。Kitty 原始修饰键编码（bitmask+1）
- **Crate 结构**: 单 package 多 binary target；不拆 workspace

## 目标环境

- **主力**: WezTerm + tmux + zsh/bash + Linux
- **可按需适配**: kitty / foot / Ghostty / Alacritty
- **不支持**: xterm / urxvt / Linux console / 非 Kitty protocol 终端

## 第三方依赖

**构建依赖**（非 vendored）：`librime-dev` + `libclang-dev` + `rime-data-luna-pinyin`（apt 系统包）+ `rime-api`（crates.io）。

**Phase 3 新增**：`serde` + `serde_json`（IPC 序列化）、`toml`（配置解析）。

`thirdpart/` 下的上游 clone **仅为本地参考，不进 git**（已加入 `.gitignore`）：

| 路径 | 说明 | 状态 |
|---|---|---|
| `thirdpart/librime/` | Rime 核心引擎源码（参考） | 只读 |
| `thirdpart/plum/` | 输入方案（参考） | 只读 |
| `thirdpart/librime-rs/` | Rust FFI 封装（参考；实际依赖走 crates.io） | 只读 |
