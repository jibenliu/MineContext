use std::process::ExitCode;

use mc_cli::{parse_args, run};

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();

    let command = match parse_args(&raw) {
        Ok(command) => command,
        Err(message) => {
            // 用法信息走 stdout；参数错误走 stderr 并返回 2
            if message.starts_with("mc-cli") {
                println!("{message}");
                return ExitCode::SUCCESS;
            }
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };

    let outcome = run(command);
    if !outcome.stdout.is_empty() {
        print!("{}", outcome.stdout);
    }
    if !outcome.stderr.is_empty() {
        eprint!("{}", outcome.stderr);
    }

    ExitCode::from(outcome.exit_code)
}
