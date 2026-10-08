//! `pipeline_failures`：失败必须**可见**。
//!
//! 只把错误打成一行日志的话，用户看到的是「功能没了，但什么都没有」。
//! 这里要求每条失败都留下：组件、稳定错误码、严重级别、人话说明、可执行建议。

use mc_common::error::AppError;
use mc_common::time::Timestamp;

use crate::db::Database;

impl Database {
    /// 记录一条失败。返回自增 id。
    pub fn record_failure(
        &self,
        at: Timestamp,
        component: &str,
        error: &AppError,
        severity: &str,
    ) -> Result<i64, AppError> {
        let remediation = error.remediation().map(|item| item.text.to_string());
        let message = error.user_message().to_string();

        self.with_write(|conn| {
            conn.execute(
                "INSERT INTO pipeline_failures
                   (at_utc_ms, component, error_code, severity, message, remediation,
                    context, retryable)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    at.as_millis(),
                    component,
                    error.code().as_str(),
                    severity,
                    message,
                    remediation,
                    error.detail(),
                    error.code().retryable() as i64,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }
}
