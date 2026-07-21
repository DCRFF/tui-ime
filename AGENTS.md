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

### 6. Git 提交

- Commit message: `type(scope): [YYYY-MM-DD] description`（英文），提交前 `git diff --stat` 确认。

### 7. 目录与文件

- `docs/` 已加入 `.gitignore` 不提交；分析文档命名: `YYYY-MM-DD-<序号> <简述>.md`。

## 架构参考

详细设计见 [可行性分析报告](docs/reports/2026-07-21-01-terminal-embedded-chinese-ime-feasibility.md)。

关键架构决策:
- **PTY proxy 模式**: 创建 PTY pair，proxy 在 master 端拦截 I/O
- **Kitty keyboard protocol**: WezTerm 内置支持，flag `0b1111`
- **tmux display-popup**: 候选窗渲染，不侵入 shell 输出
- **Daemon + Proxy 分离**: daemon 管理 librime 生命周期，proxy 管理 PTY

## 目标环境

- **主力**: WezTerm + tmux + zsh/bash + Linux
- **可按需适配**: kitty / foot / Ghostty / Alacritty
- **不支持**: xterm / urxvt / Linux console / 非 Kitty protocol 终端

## 第三方依赖

| 路径 | 说明 | 状态 |
|---|---|---|
| `thirdpart/librime/` | Rime 核心引擎 (C, `rime_api.h`) | 只读 |
| `thirdpart/plum/` | 输入方案 + 配置管理 | 只读 |
| `thirdpart/librime-rs/` | Rust FFI 封装 (`rime-sys` + `librime`) | 只读 |
