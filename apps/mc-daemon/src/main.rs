use std::process::ExitCode;

use mc_daemon::{parse_args, run};

#[tokio::main]
async fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();

    let args = match parse_args(&raw) {
        Ok(args) => args,
        Err(message) => {
            // --help 走 stdout；参数错误走 stderr
            if message.starts_with("mc-daemon") {
                println!("{message}");
                return ExitCode::SUCCESS;
            }
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };

    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // 启动失败发生在日志装好之前（或日志本身失败），因此直接写 stderr；
            // 摘要走脱敏，路径不进终端与支持包。
            eprintln!(
                "启动失败 [{}/{}]: {}",
                error.code().as_str(),
                error.component().as_str(),
                mc_common::observability::error_summary(&error)
            );
            if let Some(remediation) = error.remediation() {
                eprintln!("建议: {}", remediation.text);
            }
            ExitCode::FAILURE
        }
    }
}
