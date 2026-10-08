//! 运行日志的脱敏策略与落点。
//!
//! 脱敏是**纯函数**，因此可以逐条穷举；落点是全局的，只能有一个测试安装它。

use mc_common::error::{AppError, ErrorCode};
use mc_common::observability::{self, LogOptions};
use tracing_subscriber::filter::LevelFilter;

const KEY: &str = "sk-live-abcdefghijklmnopqrstuvwxyz0123456789";
const UNIX_PATH: &str = "/Users/someone/Library/Application Support/MineContext/minecontext.db";
const WINDOWS_PATH: &str = r"C:\Users\someone\AppData\Local\MineContext\shot.png";
const HASH: &str = "9f2c1ab77d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8";

#[test]
fn absolute_paths_are_replaced() {
    let text = observability::redact_text(&format!("无法打开 {UNIX_PATH}"));
    assert!(text.contains("<path>"), "{text}");
    assert!(!text.contains("Users"), "{text}");
    assert!(!text.contains("minecontext.db"), "{text}");
}

#[test]
fn home_and_windows_paths_are_replaced() {
    let home = observability::redact_text("写入 ~/Library/Logs/minecontext.log 失败");
    assert!(!home.contains("Library"), "{home}");

    let windows = observability::redact_text(&format!("截图落盘失败：{WINDOWS_PATH}"));
    assert!(!windows.contains("AppData"), "{windows}");
    assert!(windows.contains("截图落盘失败"), "{windows}");
}

#[test]
fn secrets_are_replaced() {
    let key = observability::redact_text(&format!("请求头 {KEY} 被拒"));
    assert!(!key.contains("sk-live"), "{key}");
    assert!(key.contains("<secret>"), "{key}");

    let bearer =
        observability::redact_text("Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig");
    assert!(!bearer.contains("eyJhbGciOiJIUzI1NiJ9"), "{bearer}");

    let hash = observability::redact_text(&format!("内容哈希 {HASH}"));
    assert!(!hash.contains(HASH), "{hash}");
}

#[test]
fn ordinary_text_is_left_alone() {
    let text = observability::redact_text("截图分析失败：模型未配置");
    assert_eq!(text, "截图分析失败：模型未配置");
}

#[test]
fn identifiers_are_shortened() {
    assert_eq!(observability::redact_id(HASH), "9f2c1ab7");
    assert_eq!(
        observability::redact_id("018f2c1a-b77d-7e5f-8071-8293a4b5c6d7"),
        "018f2c1a"
    );
    // 不是 id 形状的输入不原样回显：日志里出现任意文本本身就是泄漏面。
    assert_eq!(observability::redact_id("用户输入的搜索词"), "<id>");
    assert_eq!(observability::redact_id(""), "<id>");
}

#[test]
fn error_summary_keeps_the_code_and_drops_the_path() {
    let error = AppError::new(
        ErrorCode::StorageUnavailable,
        format!("无法写入 {UNIX_PATH}: disk full"),
    );
    let summary = observability::error_summary(&error);
    assert!(summary.contains("storage_unavailable"), "{summary}");
    assert!(summary.contains("disk full"), "{summary}");
    assert!(!summary.contains("Users"), "{summary}");
}

#[test]
fn level_comes_from_configuration_and_falls_back_to_info() {
    assert_eq!(
        observability::level_from_env(Some("debug")),
        LevelFilter::DEBUG
    );
    assert_eq!(
        observability::level_from_env(Some("WARN")),
        LevelFilter::WARN
    );
    assert_eq!(observability::level_from_env(Some("off")), LevelFilter::OFF);
    assert_eq!(
        observability::level_from_env(Some("不是级别")),
        LevelFilter::INFO
    );
    assert_eq!(observability::level_from_env(None), LevelFilter::INFO);
}

/// 唯一安装全局落点的测试：其余用例只用纯函数，避免并行时互相抢全局。
#[test]
fn init_writes_events_to_the_file_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daemon.log");
    // 预置一个超限文件：初始化时应当归档成 `.1`，新文件从空开始。
    std::fs::write(&path, vec![b'x'; 64]).unwrap();

    let options = LogOptions {
        level: LevelFilter::INFO,
        file: Some(path.clone()),
        stderr: false,
        json: false,
        max_bytes: 16,
    };
    let guard = observability::init(options).unwrap();
    observability::info!(
        component = "capture",
        event = "loop_started",
        "采集环已启动"
    );
    observability::warn!(
        component = "provider",
        code = "provider_timeout",
        "调用超时"
    );
    guard.flush();

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("采集环已启动"), "{text}");
    assert!(text.contains("loop_started"), "{text}");
    assert!(text.contains("provider_timeout"), "{text}");

    let rotated = std::fs::read_to_string(dir.path().join("daemon.log.1")).unwrap();
    assert_eq!(rotated.len(), 64, "旧的超限日志应被整体归档");

    // 幂等：第二次调用不报错，也不换掉已经装好的落点。
    let again = observability::init(LogOptions::default()).unwrap();
    again.flush();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("采集环已启动"), "{text}");
}
