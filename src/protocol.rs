//! IPC 协议消息类型（proxy/daemon/popup 共享）。
//!
//! 所有消息使用 `serde(tag = "type")` 的 tagged enum，JSON 中通过 `"type"` 字段区分。
//!
//! 候选选择（数字、Space、方向键、翻页等）全部通过 `process_key` 走 rime 原生键位处理，
//! 不需要单独的 select/change_page 消息。

use serde::{Deserialize, Serialize};

// ── Proxy → Daemon ────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProxyRequest {
    CreateSession,
    DestroySession {
        session_id: u32,
    },
    ProcessKey {
        session_id: u32,
        keycode: i32,
        modifiers: i32,
    },
    PollCommit {
        session_id: u32,
    },
}

// ── Daemon → Proxy ────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProxyResponse {
    SessionCreated {
        session_id: u32,
    },
    SessionDestroyed,
    ProcessKeyResult {
        consumed: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<ContextSnapshot>,
    },
    CommitReady {
        #[serde(skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
    },
    Error {
        message: String,
    },
}

// ── Popup → Daemon ────────────────────────────────────────

/// Popup 只发送两种消息：subscribe（获取初始 context）和 process_key（候选交互）。
/// 数字选字、Space 首选、方向键、翻页等全部通过 process_key 走 rime 原生键位。
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PopupRequest {
    Subscribe {
        session_id: u32,
    },
    ProcessKey {
        session_id: u32,
        keycode: i32,
        modifiers: i32,
    },
}

// ── Daemon → Popup ────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PopupResponse {
    Subscribed {
        context: ContextSnapshot,
    },
    ProcessKeyResult {
        consumed: bool,
        /// popup 内 process_key 产生的 commit（如 Space 选字），
        /// popup 退出前将该 commit 文本输出到 stdout，由 proxy 通过 PTY→shell 路径注入
        #[serde(skip_serializing_if = "Option::is_none")]
        commit: Option<String>,
        context: ContextSnapshot,
    },
    Error {
        message: String,
    },
}

// ── Daemon push ───────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonPush {
    SessionClosed { session_id: u32 },
}

// ── Shared types ──────────────────────────────────────────

/// Rime context 快照
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub preedit: String,
    pub candidates: Vec<String>,
    pub highlighted: usize,
    pub page_no: usize,
    pub is_last_page: bool,
}
