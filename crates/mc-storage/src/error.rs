//! SQLite 错误 → `AppError` 的映射。
//!
//! 没有这一层，用户会看到 `database is locked` 这种既不知道哪个组件、
//! 也不知道该做什么的原始字符串。

use mc_common::error::{AppError, ErrorCode};

pub fn map_sqlite_error(error: rusqlite::Error) -> AppError {
    let text = error.to_string();
    let lower = text.to_ascii_lowercase();

    let code = if lower.contains("not a database")
        || lower.contains("malformed")
        || lower.contains("corrupt")
    {
        ErrorCode::StorageCorrupt
    } else if lower.contains("disk") && lower.contains("full") || lower.contains("enospc") {
        ErrorCode::StorageDiskFull
    } else {
        // 包含 locked / busy / readonly / io 等：都是「暂时不可用，可重试」
        ErrorCode::StorageUnavailable
    };

    AppError::new(code, text)
}
