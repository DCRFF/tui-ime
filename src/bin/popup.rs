//! tui-ime-popup — tmux display-popup 候选窗（Phase 3 最小骨架）。
//!
//! 用法: tui-ime-popup --socket PATH --session ID
//!
//! 当前为骨架实现——连接 daemon、获取候选数据并打印到 stdout。
//! 完整交互式候选窗见后续版本。

use std::io;
use std::path::PathBuf;

use tui_ime::ipc::IpcClient;
use tui_ime::protocol::{PopupRequest, PopupResponse};

fn main() -> io::Result<()> {
    let mut socket = None;
    let mut session_id = None;

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--socket" => {
                socket = Some(PathBuf::from(it.next().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "--socket requires PATH")
                })?));
            }
            "--session" => {
                session_id = Some(
                    it.next()
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "--session requires ID",
                            )
                        })?
                        .parse::<u32>()
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?,
                );
            }
            "-h" | "--help" => {
                eprintln!("Usage: tui-ime-popup --socket PATH --session ID");
                return Ok(());
            }
            _ => {}
        }
    }

    let socket = socket.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "missing --socket")
    })?;
    let session_id = session_id.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "missing --session")
    })?;

    let mut client = IpcClient::connect(&socket)?;

    // Subscribe 获取初始 context
    let resp: PopupResponse = client
        .request(&PopupRequest::Subscribe { session_id })
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    match resp {
        PopupResponse::Subscribed { context } => {
            // 打印候选列表
            if !context.preedit.is_empty() {
                println!("[{}]", context.preedit);
            }
            for (i, c) in context.candidates.iter().enumerate() {
                let mark = if i == context.highlighted { ">" } else { " " };
                println!("{} {}. {}", mark, i + 1, c);
            }
            if !context.is_last_page || context.page_no > 1 {
                println!("-- ({}/{}) --", context.page_no, if context.is_last_page { "end" } else { "..." });
            }
            eprintln!("popup: received {} candidates", context.candidates.len());
        }
        other => {
            eprintln!("popup: unexpected response: {other:?}");
        }
    }

    Ok(())
}
