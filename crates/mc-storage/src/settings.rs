//! 通用设置 KV。
//!
//! 值一律按 **JSON** 存取：渲染层存的是布尔、对象数组、嵌套对象，
//! 只支持字符串会让 `true` 变成 `"true"`（`if (isFinished)` 永远为真）。
//!
//! 用表而不是 JSON 文件：单写者 + 事务。「读-改-写整个文件」的形态下，
//! 两个窗口同时写就会丢更新。

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use rusqlite::OptionalExtension;
use serde_json::Value;

use crate::db::Database;

/// 键的长度上限。设置键是给人写的短标识，超长基本可以断定是调用方拼错了。
pub const MAX_SETTING_KEY_LEN: usize = 128;

fn validate_key(key: &str) -> Result<&str, AppError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            "设置键不能为空",
        ));
    }
    if key.chars().count() > MAX_SETTING_KEY_LEN {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            format!(
                "设置键过长（{} 字符，上限 {MAX_SETTING_KEY_LEN}）",
                key.chars().count()
            ),
        ));
    }
    Ok(key)
}

impl Database {
    /// 读取一个设置。键不存在返回 `None`。
    ///
    /// 存的值解析不出来时返回 [`ErrorCode::StorageCorrupt`] ——
    /// 静默当成「没有这个设置」会让用户以为设置被重置了，
    /// 而那与「数据库里有一段坏数据」是完全不同的两件事。
    pub fn get_setting(&self, key: &str) -> Result<Option<Value>, AppError> {
        let key = validate_key(key)?;
        let raw: Option<String> = self.with_read(|conn| {
            conn.query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                [key],
                |row| row.get(0),
            )
            .optional()
        })?;

        let Some(raw) = raw else {
            return Ok(None);
        };

        serde_json::from_str(&raw).map(Some).map_err(|error| {
            AppError::new(
                ErrorCode::StorageCorrupt,
                format!("设置 {key} 的值不是合法 JSON：{error}"),
            )
            .with_context("key", key.to_string())
        })
    }

    pub fn set_setting(&self, key: &str, value: &Value, at: Timestamp) -> Result<(), AppError> {
        let key = validate_key(key)?;
        let text = serde_json::to_string(value).map_err(|error| {
            AppError::new(
                ErrorCode::DomainInvalidRange,
                format!("设置值无法序列化为 JSON：{error}"),
            )
        })?;
        let updated_at = at.to_legacy_datetime();

        self.with_write(|conn| {
            conn.execute(
                "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                                               updated_at = excluded.updated_at",
                rusqlite::params![key, text, updated_at],
            )?;
            Ok(())
        })
    }

    /// 删除一个设置，返回删除行数（重复删除是 0，不报错）。
    pub fn clear_setting(&self, key: &str) -> Result<usize, AppError> {
        let key = validate_key(key)?;
        self.with_write(|conn| conn.execute("DELETE FROM app_settings WHERE key = ?1", [key]))
    }

    /// 全部键（升序）。用于诊断与导出 —— 值可能含敏感信息，因此不返回。
    pub fn setting_keys(&self) -> Result<Vec<String>, AppError> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare("SELECT key FROM app_settings ORDER BY key ASC")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }
}
