//! 运行日志：统一出口 + 脱敏策略。
//!
//! 三条要求：
//! 1. **只写事实** —— 事件名、计数、耗时、错误码；内容、密钥、路径一律不进日志；
//! 2. **可定位** —— 字段名稳定（`component` / `event` / `code` / `count`），
//!    支持按 `MC_LOG` 环境变量调级，可同时落文件；
//! 3. **不中断业务** —— 日志写不出去只影响日志本身，调用方不需要处理错误。
//!
//! 跨 crate 统一用 `mc_common::observability::{info, warn, error, debug}`，
//! 需要把可能带路径或密钥的文本写进日志时先过 [`redact_text`]。

use std::fs::{File, OpenOptions};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use regex::Regex;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;

use crate::error::{AppError, ErrorCode};

pub use tracing::{debug, error, info, trace, warn};

/// 单文件上限。超过就在初始化时归档成 `<名字>.1`，新文件从空开始。
pub const DEFAULT_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// 日志落点与级别。
#[derive(Debug, Clone)]
pub struct LogOptions {
    pub level: LevelFilter,
    /// `None` = 只写 stderr。
    pub file: Option<PathBuf>,
    pub stderr: bool,
    /// 每行一个 JSON 对象（便于采集与检索）。
    pub json: bool,
    pub max_bytes: u64,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            level: LevelFilter::INFO,
            file: None,
            stderr: true,
            json: false,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

impl LogOptions {
    /// 级别取自 `MC_LOG`（缺省 `info`），其余用默认值。
    pub fn from_env() -> Self {
        Self {
            level: level_from_env(std::env::var("MC_LOG").ok().as_deref()),
            ..Self::default()
        }
    }

    /// 命令行默认值：**没设 `MC_LOG` 时只记警告及以上** ——
    /// CLI 的 stdout/stderr 是给人和脚本看的，不能被常规日志挤占。
    pub fn for_cli() -> Self {
        match std::env::var("MC_LOG") {
            Ok(raw) => Self {
                level: level_from_env(Some(&raw)),
                ..Self::default()
            },
            Err(_) => Self {
                level: LevelFilter::WARN,
                ..Self::default()
            },
        }
    }

    /// 追加文件落点（与 stderr 并存）。
    pub fn with_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file = Some(path.into());
        self
    }
}

/// 已安装的落点。`init` 幂等：第二次调用不会再装一个 subscriber。
static INSTALLED: OnceLock<Option<SharedFile>> = OnceLock::new();

/// 日志句柄：进程退出前调用 [`LogGuard::flush`]。
#[derive(Clone, Default)]
pub struct LogGuard {
    file: Option<SharedFile>,
}

impl LogGuard {
    /// 刷盘。失败只被忽略 —— 退出路径不该因为日志失败而改变行为。
    pub fn flush(&self) {
        if let Some(file) = &self.file {
            let _ = file.0.lock().map(|mut handle| handle.flush());
        }
    }
}

/// 安装全局落点。幂等：重复调用返回已有句柄。
pub fn init(options: LogOptions) -> Result<LogGuard, AppError> {
    if let Some(existing) = INSTALLED.get() {
        return Ok(LogGuard {
            file: existing.clone(),
        });
    }

    let file = match &options.file {
        Some(path) => Some(SharedFile::open(path, options.max_bytes)?),
        None => None,
    };
    let writer = SinkWriter {
        file: file.clone(),
        stderr: options.stderr,
    };

    let subscriber = tracing_subscriber::fmt()
        .with_max_level(options.level)
        .with_target(true)
        // 落文件时一律不上色：支持包里要能直接 grep。
        .with_ansi(options.stderr && options.file.is_none() && io::stderr().is_terminal())
        .with_writer(writer);

    let installed = if options.json {
        subscriber.json().try_init()
    } else {
        subscriber.try_init()
    };
    installed.map_err(|error| {
        AppError::new(
            ErrorCode::ConfigInvalid,
            format!("日志落点安装失败：{error}"),
        )
    })?;

    let _ = INSTALLED.set(file.clone());
    Ok(LogGuard { file })
}

/// 从 `MC_LOG` 取值（`debug` / `info` / `warn` / `error` / `off`）。
///
/// 解析不出来时退回 `info`：一个写错的级别不该让进程起不来，
/// 但也不该静默变成「什么都不记」。
pub fn level_from_env(raw: Option<&str>) -> LevelFilter {
    // 大小写不敏感：`MC_LOG=WARN` 与 `warn` 是同一件事。
    match raw
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("trace") => LevelFilter::TRACE,
        Some("debug") => LevelFilter::DEBUG,
        Some("info") => LevelFilter::INFO,
        Some("warn") | Some("warning") => LevelFilter::WARN,
        Some("error") => LevelFilter::ERROR,
        Some("off") => LevelFilter::OFF,
        _ => LevelFilter::INFO,
    }
}

/// 把任意文本变成可以安全写进日志的形式。
pub fn redact_text(raw: &str) -> String {
    let mut text = raw.to_string();
    for pattern in secret_patterns() {
        text = pattern.replace_all(&text, "<secret>").into_owned();
    }
    for pattern in path_patterns() {
        text = pattern.replace_all(&text, "<path>").into_owned();
    }
    text
}

/// 标识符只保留前 8 个十六进制字符；不像标识符的输入不回显。
pub fn redact_id(raw: &str) -> String {
    let compact: String = raw.chars().filter(|c| *c != '-').collect();
    if compact.len() >= 8 && compact.chars().all(|c| c.is_ascii_hexdigit()) {
        compact[..8].to_string()
    } else {
        "<id>".to_string()
    }
}

/// 错误摘要：`<error_code>: <脱敏后的 detail>`。
pub fn error_summary(error: &AppError) -> String {
    format!("{}: {}", error.code().as_str(), redact_text(error.detail()))
}

fn secret_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r"sk-[A-Za-z0-9_\-]{8,}").expect("内置正则"),
            Regex::new(r"(?i)bearer\s+[A-Za-z0-9._\-]{8,}").expect("内置正则"),
            Regex::new(r"\b[A-Za-z0-9_\-]{32,}\b").expect("内置正则"),
        ]
    })
}

/// 绝对路径：前缀限定在明确的文件系统根，body 允许空格但排除标点。
const PATH_BODY: &str = r#"(?:/Users|/home|/var|/tmp|/private|/Volumes|/Applications|/Library|/opt)[^\r\n"'`,;:)\]}<>|，。；：]*[^\s\r\n"'`,;:)\]}<>|，。；：]"#;

fn path_patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            // 只认明确的文件系统根：`/api/v1/search` 这类路由名要留在日志里。
            // 路径里可以有空格（`Application Support`），因此不能按空白截断；
            // 但冒号、引号、括号、中文标点一定是路径之外的字符。
            Regex::new(PATH_BODY).expect("内置正则"),
            Regex::new(r#"~[^\r\n"'`,;:)\]}<>|，。；：]*[^\s\r\n"'`,;:)\]}<>|，。；：]"#)
                .expect("内置正则"),
            Regex::new(r#"[A-Za-z]:\\[^\r\n"'`,;:)\]}<>|，。；：]*"#).expect("内置正则"),
        ]
    })
}

/// 文件句柄用 `Arc<Mutex<..>>` 共享：tracing 的写入发生在任意线程。
#[derive(Clone)]
struct SharedFile(Arc<Mutex<File>>);

impl SharedFile {
    fn open(path: &Path, max_bytes: u64) -> Result<Self, AppError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                AppError::new(
                    ErrorCode::StorageUnavailable,
                    format!("日志目录不可用：{}", redact_text(&error.to_string())),
                )
            })?;
        }
        if let Ok(metadata) = std::fs::metadata(path) {
            if max_bytes > 0 && metadata.len() > max_bytes {
                let rotated = path.with_file_name(format!(
                    "{}.1",
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "mc.log".to_string())
                ));
                std::fs::rename(path, rotated).map_err(|error| {
                    AppError::new(
                        ErrorCode::StorageUnavailable,
                        format!("日志归档失败：{}", redact_text(&error.to_string())),
                    )
                })?;
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| {
                AppError::new(
                    ErrorCode::StorageUnavailable,
                    format!("日志文件不可写：{}", redact_text(&error.to_string())),
                )
            })?;
        Ok(Self(Arc::new(Mutex::new(file))))
    }
}

#[derive(Clone)]
struct SinkWriter {
    file: Option<SharedFile>,
    stderr: bool,
}

struct SinkHandle {
    file: Option<SharedFile>,
    stderr: bool,
}

impl Write for SinkHandle {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if let Some(file) = &self.file {
            file.0
                .lock()
                .map_err(|_| io::Error::other("日志锁中毒"))?
                .write_all(buffer)?;
        }
        if self.stderr {
            io::stderr().write_all(buffer)?;
        }
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = &self.file {
            file.0
                .lock()
                .map_err(|_| io::Error::other("日志锁中毒"))?
                .flush()?;
        }
        if self.stderr {
            io::stderr().flush()?;
        }
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for SinkWriter {
    type Writer = SinkHandle;

    fn make_writer(&'a self) -> Self::Writer {
        SinkHandle {
            file: self.file.clone(),
            stderr: self.stderr,
        }
    }
}
