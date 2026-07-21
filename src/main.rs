#![forbid(unsafe_code)]

mod ime;
mod keyevent;
mod keymap;
mod proxy;
mod render;

use std::env;
use std::fs::File;
use std::path::Path;
use std::process::ExitCode;

/// 命令行参数
struct Args {
    /// IME 调试日志文件路径（候选 / preedit 输出）
    log: Option<String>,
    /// 在 PTY slave 中执行的命令，默认 $SHELL
    command: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut log = None;
    let mut command = Vec::new();
    let mut it = env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--log" => {
                log = Some(it.next().ok_or("--log requires a FILE argument")?);
            }
            "--" => {
                command.extend(it.by_ref());
                break;
            }
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if command.is_empty() {
        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        command = vec![shell];
    }
    Ok(Args { log, command })
}

fn print_usage() {
    eprintln!("Usage: tui-ime [--log FILE] [-- COMMAND [ARGS...]]");
}

fn main() -> ExitCode {
    match parse_args() {
        Ok(args) => {
            let log = match &args.log {
                Some(path) => match File::create(path) {
                    Ok(f) => Some(f),
                    Err(e) => {
                        eprintln!("tui-ime: cannot create log file {path}: {e}");
                        return ExitCode::FAILURE;
                    }
                },
                None => None,
            };
            // 初始化 rime（首次部署需数秒）；失败时降级为纯透传（计划 R3/R8）
            eprintln!("tui-ime: initializing rime...");
            let rime = match ime::Ime::new(Path::new(&ime::default_user_data_dir())) {
                Ok(r) => Some(r),
                Err(e) => {
                    eprintln!("tui-ime: rime init failed, falling back to passthrough: {e:#}");
                    None
                }
            };
            match proxy::run(&args.command, log, rime) {
                Ok(code) => ExitCode::from(code as u8),
                Err(e) => {
                    eprintln!("tui-ime: {e:#}");
                    ExitCode::FAILURE
                }
            }
        }
        Err(e) => {
            eprintln!("tui-ime: {e}");
            print_usage();
            ExitCode::FAILURE
        }
    }
}
