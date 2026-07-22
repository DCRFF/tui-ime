use std::env;
use std::fs::File;
use std::process::ExitCode;

use tui_ime::{self, NEST_GUARD_ENV};

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
            // 已在 tui-ime proxy 内：拒绝嵌套启动（在 rime 部署前快速退出）
            if env::var_os(NEST_GUARD_ENV).is_some() {
                eprintln!(
                    "tui-ime: already inside a tui-ime proxy ({NEST_GUARD_ENV} is set); nested launch refused"
                );
                return ExitCode::FAILURE;
            }
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
            // IME 后端由 daemon 管理，proxy 启动时自动连接 daemon socket；
            // 若 daemon 不可用则纯透传降级
            match tui_ime::proxy::run(&args.command, log) {
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
