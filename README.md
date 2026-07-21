# tui-ime — 终端嵌入式中文输入法

在 WezTerm + tmux 中直接输入中文，无需 ibus/fcitx/XIM，headless 服务器可用。

## 一句话

把 Rime 输入法引擎塞进 PTY proxy，在终端里用 tmux popup 弹候选窗——Shell 完全无感知。

## 架构

```
WezTerm → PTY master → [IME Proxy] → PTY slave → zsh/bash
              ↑
         Kitty keyboard protocol
         所有按键以 CSI u 上报
              │
         ┌────┴────┐
         │ 透传     │ IME 激活
         ▼          ▼
       Shell    Rime → tmux popup 候选窗 → commit 中文注入 slave
```

## 技术栈

| 层 | 技术 |
|---|---|
| 键盘协议 | Kitty keyboard protocol (WezTerm 内置) |
| 候选窗 | tmux `display-popup` |
| 输入引擎 | librime (Rime, C API) |
| PTY 管理 | Rust `portable-pty` |
| 实现语言 | Rust |

## 仓库结构

```
tui-ime/
├── README.md          ← 你在这里
├── AGENTS.md          ← AI agent 工作规范
├── STATUS.md          ← 当前项目状态
├── docs/reports/      ← 分析报告
├── thirdpart/
│   ├── librime/       ← Rime 核心引擎 (C，参考用，构建走系统包 librime-dev)
│   ├── plum/          ← 输入方案 (朙月拼音, 双拼, ...)
│   └── librime-rs/    ← Rust FFI 封装 (path 依赖)
└── src/               ← tui-ime 源码 (proxy / keyevent / keymap / ime)
```

## 快速开始

```bash
# 1. 安装依赖（Debian trixie；libclang-dev 供 bindgen 生成 FFI 绑定）
sudo apt install librime-dev libclang-dev rime-data-luna-pinyin

# 2. 编译
cargo build --release

# 3. 在 WezTerm + tmux 中运行
./target/release/tui-ime --log /tmp/tui-ime.log

# Ctrl+Space 切换中英文；输入时光标处显示淡色 preedit + 单行候选条；
# 数字选字、Space 首选、↑↓ 高亮、PageUp/Down 翻页、Esc 取消
```

tmux 配置要求（~/.tmux.conf）：

```tmux
set -s -g extended-keys on                          # 窗格侧扩展按键（server 选项）
set -g extended-keys-format csi-u                   # 以 CSI u 格式上报
set -as terminal-features ",xterm-256color:extkeys" # 声明外层终端支持扩展按键
```

改完需 detach + reattach 生效（`extended-keys always` 可替代第三行）。

## 相关文档

- [可行性分析报告](docs/reports/2026-07-21-01-terminal-embedded-chinese-ime-feasibility.md) — 完整的技术分析
- [STATUS.md](STATUS.md) — 当前进度
- [AGENTS.md](AGENTS.md) — AI agent 规范
