//! tui-ime 配置文件加载。
//!
//! 配置文件路径：`$XDG_CONFIG_HOME/tui-ime/tui-ime.toml`（默认 `~/.config/tui-ime/tui-ime.toml`）。
//! 所有字段均有默认值，不强制用户创建文件。

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// 完整配置结构
#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub popup: PopupConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            daemon: DaemonConfig::default(),
            proxy: ProxyConfig::default(),
            popup: PopupConfig::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DaemonConfig {
    /// Unix socket 路径（留空则自动推导）
    #[serde(default)]
    pub socket_path: Option<String>,
    /// 最大并发 session 数
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
    /// librime 共享数据目录
    #[serde(default = "default_shared_data_dir")]
    pub rime_shared_data_dir: String,
    /// librime 用户数据目录
    #[serde(default = "default_user_data_dir")]
    pub rime_user_data_dir: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: None,
            max_sessions: 16,
            rime_shared_data_dir: default_shared_data_dir(),
            rime_user_data_dir: default_user_data_dir(),
        }
    }
}

fn default_max_sessions() -> usize {
    16
}

fn default_shared_data_dir() -> String {
    "/usr/share/rime-data".to_string()
}

fn default_user_data_dir() -> String {
    crate::ime::default_user_data_dir()
}

#[derive(Debug, Deserialize)]
pub struct ProxyConfig {
    /// IME toggle 键 codepoint（Kitty CSI-u 编码值）
    #[serde(default = "default_toggle_codepoint")]
    pub toggle_codepoint: u32,
    /// IME toggle 键 modifiers（Kitty 原始编码 = bitmask + 1；Ctrl=5, Alt=3, Shift=2）
    #[serde(default = "default_toggle_modifiers")]
    pub toggle_modifiers: u8,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            toggle_codepoint: 92, // Ctrl+\ (backslash) — doesn't conflict with system IME
            toggle_modifiers: 5,
        }
    }
}

fn default_toggle_codepoint() -> u32 {
    92 // backslash — Ctrl+\ doesn't conflict with system IME
}

fn default_toggle_modifiers() -> u8 {
    5 // Ctrl
}

/// 从环境变量 `TUI_IME_TOGGLE` 读取 toggle 键（格式 `codepoint:modifiers`，如 `92:5`）。
/// 未设置时使用 config 默认值。
pub fn toggle_key(cfg: &ProxyConfig) -> (u32, u8) {
    if let Ok(val) = std::env::var("TUI_IME_TOGGLE") {
        if let Some((cp, m)) = val.split_once(':') {
            if let (Ok(cp), Ok(m)) = (cp.parse::<u32>(), m.parse::<u8>()) {
                return (cp, m);
            }
        }
    }
    (cfg.toggle_codepoint, cfg.toggle_modifiers)
}

#[derive(Debug, Deserialize)]
pub struct PopupConfig {
    #[serde(default = "default_popup_width")]
    pub width: u16,
    #[serde(default = "default_popup_height")]
    pub height: u16,
}

impl Default for PopupConfig {
    fn default() -> Self {
        Self {
            width: 56,
            height: 8,
        }
    }
}

fn default_popup_width() -> u16 {
    56
}

fn default_popup_height() -> u16 {
    8
}

/// 查找配置文件路径
pub fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".config")
        });
    base.join("tui-ime").join("tui-ime.toml")
}

/// 加载配置：文件不存在时返回默认值
pub fn load(path: &Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
            eprintln!(
                "tui-ime: failed to parse config {}: {e}; using defaults",
                path.display()
            );
            Config::default()
        }),
        Err(_) => Config::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_sensible() {
        let c = Config::default();
        assert_eq!(c.proxy.toggle_codepoint, 92);
        assert_eq!(c.proxy.toggle_modifiers, 5);
        assert_eq!(c.popup.width, 56);
        assert_eq!(c.popup.height, 8);
    }

    #[test]
    fn parse_minimal_toml() {
        let toml_str = r#"
[daemon]
max_sessions = 8

[proxy]
toggle_codepoint = 48
"#;
        let c: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(c.daemon.max_sessions, 8);
        assert_eq!(c.proxy.toggle_codepoint, 48);
        // 未指定的字段应回到默认值
        assert_eq!(c.proxy.toggle_modifiers, 5);
        assert_eq!(c.popup.width, 56);
    }

    #[test]
    fn load_missing_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.toml");
        let c = load(&path);
        assert_eq!(c.daemon.max_sessions, 16);
    }
}
