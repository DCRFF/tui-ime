# tui-ime — Terminal-Embedded Chinese IME

**English** | [中文](README_zh.md)

Type Chinese directly in your terminal — no ibus/fcitx/XIM required. Works on
headless servers over SSH, with or without tmux.

The Rime engine is embedded behind a PTY proxy: keystrokes are intercepted,
composed by librime, and candidates are rendered as an inline strip right at
the cursor — the shell underneath notices nothing.

## Screenshot

![inline candidate strip](screenshot/input_test.png)

Typing `shurufaceshi` shows an underlined preedit plus a single-line candidate
strip (`1.輸入法測試 2.輸入法 3.輸入`); `你好！` on the left was already committed
straight into the shell prompt.

## Architecture (Phase 3)

```
tui-ime (proxy) ──Unix socket──► tui-ime-daemon (librime)
     │                                ▲
     │  keystroke interception        │  Unix socket
     │  IME rendering                 │
     ▼                                │
 PTY slave ──► zsh/bash              tui-ime-popup (skeleton)
(runs under a bare terminal or tmux)
```

- **daemon**: systemd user service, single instance. Owns the librime lifecycle,
  a session pool, and the IPC service.
- **proxy**: one instance per terminal session. PTY interception → CSI u parsing
  → daemon IPC → inline ANSI rendering.
- **popup**: tmux `display-popup` candidate window (skeleton in Phase 3,
  interaction lands in Phase 4).

## Tech stack

| Layer | Tech |
|---|---|
| Keyboard protocol | Kitty keyboard protocol (built into WezTerm) |
| Candidate window | tmux `display-popup` |
| Input engine | librime (Rime, C API) |
| PTY management | Rust `portable-pty` |
| Language | Rust |

**Target environment**: WezTerm + zsh/bash on Linux, optionally inside tmux.
Other kitty-protocol terminals (kitty / foot / Ghostty / Alacritty) can be
adapted on demand. xterm / urxvt / Linux console are not supported.

## Quick start

```bash
# 1. Install dependencies
sudo apt install librime-dev libclang-dev rime-data-luna-pinyin

# 2. Build and install the binaries
cargo build --release
install -Dm755 target/release/tui-ime-daemon ~/.local/bin/tui-ime-daemon
install -Dm755 target/release/tui-ime ~/.local/bin/tui-ime

# 3. Install and start the daemon (systemd user service: autostart on login,
#    auto-restart on crash)
install -Dm644 tui-ime-daemon.service ~/.config/systemd/user/tui-ime-daemon.service
systemctl --user daemon-reload
systemctl --user enable --now tui-ime-daemon

# 4. Wrap every interactive shell in the proxy (works with or without tmux).
#    Append to the END of ~/.zshrc — nothing after `exec` will run:
cat >> ~/.zshrc <<'EOF'

# tui-ime: terminal-embedded Chinese IME (daemon managed by systemd --user)
if [[ -o interactive && -z "$TUI_IME_ACTIVE" \
   && -S "${XDG_RUNTIME_DIR:-$HOME/.local/share}/tui-ime/daemon.sock" \
   && -x "$HOME/.local/bin/tui-ime" ]]; then
  exec "$HOME/.local/bin/tui-ime"
fi
EOF

# bash users: append the same block to ~/.bashrc and replace the
# interactive check with [[ $- == *i* ]]
# You can also skip the shell integration and just run `tui-ime` manually.

# Default toggle key: Ctrl+\ (does not clash with system IMEs)
# While composing, an inline preedit + candidate strip appears at the cursor.
```

About the guards: `TUI_IME_ACTIVE` is injected by the proxy to prevent nested
wrapping (a shell started inside the proxy skips re-wrapping); if the socket or
the binary is missing, the shell falls back to normal operation.

When the daemon is down, the proxy silently degrades to plain passthrough
(toggle does nothing). To troubleshoot:

```bash
systemctl --user status tui-ime-daemon   # should be active (running)
ls /run/user/$UID/tui-ime/daemon.sock    # socket should exist
```

## Toggle key configuration

The default is `Ctrl+\` (codepoint=92, modifiers=5 i.e. Ctrl).
To use `Ctrl+Space` or another key:

```bash
# Environment variable (takes effect immediately)
TUI_IME_TOGGLE=32:5 tui-ime   # Ctrl+Space
TUI_IME_TOGGLE=96:5 tui-ime   # Ctrl+`

# Config file (~/.config/tui-ime/tui-ime.toml)
[proxy]
toggle_codepoint = 32   # Space
toggle_modifiers = 5    # Ctrl (kitty encoding = bitmask + 1; Ctrl=5, Alt=3, Shift=2)
```

Modifiers use the raw Kitty keyboard protocol encoding (actual modifier bitmask
+ 1). Common values: none=1, Shift=2, Alt=3, Ctrl=5, Ctrl+Shift=7.

## Using inside tmux (optional)

In a bare terminal the proxy negotiates extended keys with WezTerm directly —
no configuration needed. Inside tmux, tmux must forward and re-encode extended
keys (`~/.tmux.conf`):

```tmux
set -s -g extended-keys on                          # extended keys on the pane side (server option)
set -g extended-keys-format csi-u                   # report in CSI u format
set -as terminal-features ",xterm-256color:extkeys" # declare outer terminal support
```

Detach and reattach for the change to take effect (`extended-keys always` can
replace the third line).

## Repository layout

```
tui-ime/
├── README.md              ← you are here
├── README_zh.md           ← 中文说明
├── AGENTS.md              ← AI agent conventions
├── STATUS.md              ← current project status
├── tui-ime-daemon.service ← systemd user service
├── screenshot/            ← usage screenshots
├── docs/reports/          ← analysis reports (not committed)
├── thirdpart/             ← local upstream references (not committed)
└── src/
    ├── lib.rs             ← library root
    ├── protocol.rs        ← IPC message types
    ├── ipc.rs             ← Unix socket transport
    ├── config.rs          ← tui-ime.toml loading
    ├── daemon.rs          ← session pool + dispatch
    ├── ime.rs             ← librime session wrapper
    ├── keyevent.rs        ← CSI u / SS3 / modifyOtherKeys parsing
    ├── keymap.rs          ← key → rime keycode mapping
    ├── proxy.rs           ← PTY proxy core
    ├── render.rs          ← inline ANSI candidate strip
    ├── main.rs            ← tui-ime (proxy) entry
    └── bin/
        ├── daemon.rs      ← tui-ime-daemon entry
        └── popup.rs       ← tui-ime-popup entry (skeleton)
```
