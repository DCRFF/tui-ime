#![forbid(unsafe_code)]

pub mod config;
pub mod daemon;
pub mod identity;
pub mod ime;
pub mod ipc;
pub mod keyevent;
pub mod keymap;
pub mod protocol;
pub mod proxy;
pub mod render;

/// 防嵌套环境标记：proxy 给子进程注入（见 proxy::run），
/// 在 tui-ime 内再次启动时据此拒绝——多层嵌套会让最外层旧实例
/// 截获 toggle/按键，内层新实例完全失效且并发写日志互相踩坏
pub const NEST_GUARD_ENV: &str = "TUI_IME_ACTIVE";
