//! tui-ime-daemon — librime 后端 + Unix socket 服务。
//!
//! 用法:
//!   tui-ime-daemon [--config PATH] [--socket PATH]

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use tui_ime::config;
use tui_ime::daemon;

fn main() -> ExitCode {
    let mut config_path = config::config_path();
    let mut socket_path = daemon::default_socket_path();

    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => {
                config_path = PathBuf::from(it.next().unwrap_or_else(|| {
                    eprintln!("--config requires a PATH argument");
                    std::process::exit(1);
                }));
            }
            "--socket" => {
                socket_path = PathBuf::from(it.next().unwrap_or_else(|| {
                    eprintln!("--socket requires a PATH argument");
                    std::process::exit(1);
                }));
            }
            "-h" | "--help" => {
                eprintln!("Usage: tui-ime-daemon [--config PATH] [--socket PATH]");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::FAILURE;
            }
        }
    }

    let cfg = config::load(&config_path);

    match daemon::run(&cfg, &socket_path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tui-ime-daemon: {e}");
            ExitCode::FAILURE
        }
    }
}
