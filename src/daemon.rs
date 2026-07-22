//! tui-ime daemon：librime 后端 + session pool + Unix socket 服务。

use std::collections::HashMap;
use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::config::Config;
use crate::ime::Ime;
use crate::ipc::{self, read_frame, write_frame, IpcServer};
use crate::protocol::{ContextSnapshot, PopupRequest, PopupResponse, ProxyRequest, ProxyResponse};

struct SessionState {
    ime: Ime,
    pending_commit: Option<String>,
}

struct Shared {
    sessions: HashMap<u32, SessionState>,
    next_id: u32,
    max_sessions: usize,
}

fn context_from_ime(ime: &Ime) -> ContextSnapshot {
    let snap = ime.snapshot();
    ContextSnapshot {
        preedit: snap.preedit,
        candidates: snap.candidates,
        highlighted: snap.highlighted,
        page_no: snap.page_no,
        is_last_page: snap.is_last_page,
    }
}

fn handle_client(stream: UnixStream, shared: Arc<Mutex<Shared>>) -> io::Result<()> {
    let mut reader = stream
        .try_clone()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let mut writer = stream;
    let mut session_id: Option<u32> = None;

    loop {
        let raw = match read_frame(&mut reader) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e),
        };

        if let Ok(req) = serde_json::from_slice::<ProxyRequest>(&raw) {
            let resp = handle_proxy_request(req, &mut session_id, &shared);
            let resp_bytes = serde_json::to_vec(&resp).map_err(ipc::json_err)?;
            write_frame(&mut writer, &resp_bytes)?;
            continue;
        }

        if let Ok(req) = serde_json::from_slice::<PopupRequest>(&raw) {
            let resp = handle_popup_request(req, session_id, &shared);
            let resp_bytes = serde_json::to_vec(&resp).map_err(ipc::json_err)?;
            write_frame(&mut writer, &resp_bytes)?;
            continue;
        }

        let err = ProxyResponse::Error {
            message: "unknown message type".to_string(),
        };
        let err_bytes = serde_json::to_vec(&err).map_err(ipc::json_err)?;
        write_frame(&mut writer, &err_bytes)?;
    }

    if let Some(sid) = session_id {
        let mut shared = shared.lock().expect("shared mutex");
        shared.sessions.remove(&sid);
    }
    Ok(())
}

fn handle_proxy_request(
    req: ProxyRequest,
    session_id: &mut Option<u32>,
    shared: &Arc<Mutex<Shared>>,
) -> ProxyResponse {
    let mut shared = shared.lock().expect("shared mutex");

    match req {
        ProxyRequest::CreateSession => {
            if shared.sessions.len() >= shared.max_sessions {
                return ProxyResponse::Error {
                    message: format!("max sessions ({}) reached", shared.max_sessions),
                };
            }
            let id = shared.next_id;
            shared.next_id += 1;
            let user_dir = PathBuf::from(crate::ime::default_user_data_dir());
            let ime = match Ime::new(&user_dir) {
                Ok(ime) => ime,
                Err(e) => {
                    return ProxyResponse::Error {
                        message: format!("failed to create rime session: {e}"),
                    };
                }
            };
            shared.sessions.insert(
                id,
                SessionState {
                    ime,
                    pending_commit: None,
                },
            );
            *session_id = Some(id);
            ProxyResponse::SessionCreated { session_id: id }
        }

        ProxyRequest::DestroySession { session_id: sid } => {
            shared.sessions.remove(&sid);
            if *session_id == Some(sid) {
                *session_id = None;
            }
            ProxyResponse::SessionDestroyed
        }

        ProxyRequest::ProcessKey {
            session_id: sid,
            keycode,
            modifiers,
        } => {
            let state = match shared.sessions.get(&sid) {
                Some(s) => s,
                None => {
                    return ProxyResponse::Error {
                        message: format!("session {sid} not found"),
                    };
                }
            };
            let consumed = state.ime.process_key(keycode, modifiers);
            let commit = if consumed {
                state.ime.take_commit()
            } else {
                None
            };
            let context = if consumed {
                Some(context_from_ime(&state.ime))
            } else {
                None
            };
            ProxyResponse::ProcessKeyResult {
                consumed,
                commit,
                context,
            }
        }

        ProxyRequest::PollCommit { session_id: sid } => {
            let state = match shared.sessions.get_mut(&sid) {
                Some(s) => s,
                None => {
                    return ProxyResponse::Error {
                        message: format!("session {sid} not found"),
                    };
                }
            };
            ProxyResponse::CommitReady {
                commit: state.pending_commit.take(),
            }
        }
    }
}

fn handle_popup_request(
    req: PopupRequest,
    session_id: Option<u32>,
    shared: &Arc<Mutex<Shared>>,
) -> PopupResponse {
    let mut shared = shared.lock().expect("shared mutex");

    let sid = match session_id {
        Some(id) => id,
        None => {
            return PopupResponse::Error {
                message: "popup must subscribe with a session_id".to_string(),
            };
        }
    };

    let state = match shared.sessions.get_mut(&sid) {
        Some(s) => s,
        None => {
            return PopupResponse::Error {
                message: format!("session {sid} not found"),
            };
        }
    };

    match req {
        PopupRequest::Subscribe { .. } => {
            let context = context_from_ime(&state.ime);
            PopupResponse::Subscribed { context }
        }

        PopupRequest::ProcessKey {
            keycode, modifiers, ..
        } => {
            let consumed = state.ime.process_key(keycode, modifiers);
            let commit = if consumed {
                state.ime.take_commit()
            } else {
                None
            };
            if commit.is_some() {
                state.pending_commit = commit.clone();
            }
            let context = context_from_ime(&state.ime);
            PopupResponse::ProcessKeyResult {
                consumed,
                commit,
                context,
            }
        }
    }
}

/// Daemon socket 路径（优先 `TUI_IME_SOCKET` 环境变量）
pub fn default_socket_path() -> PathBuf {
    if let Ok(path) = std::env::var("TUI_IME_SOCKET") {
        return PathBuf::from(path);
    }
    let runtime = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
            PathBuf::from(home).join(".local").join("share")
        });
    runtime.join("tui-ime").join("daemon.sock")
}

/// 启动 daemon 主循环（阻塞）。
pub fn run(config: &Config, socket_path: &Path) -> io::Result<()> {
    let shared = Arc::new(Mutex::new(Shared {
        sessions: HashMap::new(),
        next_id: 1,
        max_sessions: config.daemon.max_sessions,
    }));

    let server = IpcServer::bind(socket_path)?;
    eprintln!("tui-ime-daemon: listening on {}", socket_path.display());

    loop {
        let (stream, addr) = server.accept()?;
        eprintln!("tui-ime-daemon: client connected from {:?}", addr);
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            if let Err(e) = handle_client(stream, shared) {
                eprintln!("tui-ime-daemon: client error: {e}");
            }
        });
    }
}
