# tui-ime — 终端嵌入式中文输入法

在 WezTerm + tmux 中直接输入中文，无需 ibus/fcitx/XIM，headless 服务器可用。

## 一句话

把 Rime 输入法引擎塞进 PTY proxy，在终端里用 tmux popup 弹候选窗——Shell 完全无感知。

## 架构（Phase 3）

```
tui-ime (proxy) ──Unix socket──► tui-ime-daemon (librime)
     │                                ▲
     │  按键拦截 / IME 渲染           │  Unix socket
     ▼                                │
 PTY slave ──► tmux ──► zsh/bash    tui-ime-popup (候选窗骨架)
```

- **daemon**: systemd user service, 单例运行。管理 librime 生命周期 + session pool + IPC 服务
- **proxy**: 每终端会话一个实例。PTY 拦截 → CSI u 解析 → daemon IPC → inline ANSI 渲染
- **popup**: tmux display-popup 候选窗（Phase 3 骨架，Phase 4 完善交互）

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
├── README.md              ← 你在这里
├── AGENTS.md              ← AI agent 工作规范
├── STATUS.md              ← 当前项目状态
├── tui-ime-daemon.service ← systemd user service
├── docs/reports/          ← 分析报告（不进 git）
├── thirdpart/             ← 上游仓库本地参考（不进 git）
└── src/
    ├── lib.rs             ← library root
    ├── protocol.rs        ← IPC 消息类型
    ├── ipc.rs             ← Unix socket 传输层
    ├── config.rs          ← tui-ime.toml 加载
    ├── daemon.rs          ← session pool + 消息分发
    ├── ime.rs             ← librime 会话封装
    ├── keyevent.rs        ← CSI u / SS3 解析
    ├── keymap.rs          ← 按键 → rime keycode 映射
    ├── proxy.rs           ← PTY proxy 核心
    ├── render.rs          ← inline ANSI 候选条
    ├── main.rs            ← tui-ime（proxy）入口
    └── bin/
        ├── daemon.rs      ← tui-ime-daemon 入口
        └── popup.rs       ← tui-ime-popup 入口（骨架）
```
## 快速开始

### Phase 3：daemon + proxy 分离

```bash
# 1. 安装依赖
sudo apt install librime-dev libclang-dev rime-data-luna-pinyin

# 2. 编译
cargo build --release

# 3. 启动 daemon（librime 后端，首次部署需 1-3 秒）
./target/release/tui-ime-daemon &

# 4. 在 WezTerm + tmux 中启动 proxy
./target/release/tui-ime

# 默认切换键：Ctrl+\ （不冲突系统输入法）
# 输入时光标处显示淡色 preedit + 单行候选条
```

### 切换键配置

默认 `Ctrl+\`（codepoint=92, modifiers=5 即 Ctrl）。
如需改为 `Ctrl+Space` 或其他键：

```bash
# 环境变量方式（立即生效）
TUI_IME_TOGGLE=32:5 ./target/release/tui-ime   # Ctrl+Space
TUI_IME_TOGGLE=96:5 ./target/release/tui-ime   # Ctrl+`

# 配置文件方式（~/.config/tui-ime/tui-ime.toml）
[proxy]
toggle_codepoint = 32   # Space
toggle_modifiers = 5     # Ctrl（Kitty 编码 = bitmask + 1；Ctrl=5, Alt=3, Shift=2）
```

modifiers 使用 Kitty keyboard protocol 的原始编码值（实际修饰键 bitmask + 1）。
常用值：无修饰=1, Shift=2, Alt=3, Ctrl=5, Ctrl+Shift=7。

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
