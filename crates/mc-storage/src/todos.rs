//! 待办与提示（`todo` / `tips`）。
//! 直接服务首页的「任务卡片」与「提示列表」（`use-home-info.ts`）。
//!
//! **时间列写 ISO-8601**：调用方用 `dayjs` 的 ISO 字符串作为范围边界；
//! 若这里写兼容层的 `YYYY-MM-DD HH:MM:SS`，
//! `start_time >= '2026-09-30T00:00:00.000Z'` 会**永远为假**（`T` 的字典序
//! 大于空格），用户看到的是「今天的任务一个都没有」—— 格式不一致不报错，
//! 只是结果为空。
//!
//! 排序：`urgency DESC, created_at DESC`。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::db::Database;

/// `todo` 的一行，列与兼容层的 `SELECT` 一一对应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TodoRow {
    pub id: i64,
    pub content: String,
    pub created_at: String,
    pub start_time: String,
    pub end_time: Option<String>,
    /// 0 = 待办，1 = 已完成（只有这两档语义）
    pub status: i64,
    pub urgency: i64,
    pub assignee: Option<String>,
    pub reason: Option<String>,
}

/// 新建待办。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTodo {
    pub content: String,
    /// 缺省 = 当前时间
    pub start_time: Option<Timestamp>,
    pub end_time: Option<Timestamp>,
    pub status: i64,
    pub urgency: i64,
    pub assignee: Option<String>,
    pub reason: Option<String>,
}

/// 查询条件。时间范围过滤的是 **`start_time`**。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoQuery {
    pub start: Option<Timestamp>,
    pub end: Option<Timestamp>,
    pub status: Option<i64>,
}

/// 局部更新。`Some(None)` 表示把可空字段设回 NULL。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodoPatch {
    pub content: Option<String>,
    pub start_time: Option<Option<Timestamp>>,
    pub end_time: Option<Option<Timestamp>>,
    pub status: Option<i64>,
    pub urgency: Option<i64>,
    pub assignee: Option<Option<String>>,
    pub reason: Option<Option<String>>,
}

impl TodoPatch {
    pub fn is_empty(&self) -> bool {
        self.content.is_none()
            && self.start_time.is_none()
            && self.end_time.is_none()
            && self.status.is_none()
            && self.urgency.is_none()
            && self.assignee.is_none()
            && self.reason.is_none()
    }
}

/// `tips` 的一行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TipRow {
    pub id: i64,
    pub content: String,
    pub created_at: String,
}

const TODO_COLUMNS: &str =
    "id, content, created_at, start_time, end_time, status, urgency, assignee, reason";

fn row_to_todo(row: &rusqlite::Row<'_>) -> rusqlite::Result<TodoRow> {
    Ok(TodoRow {
        id: row.get(0)?,
        content: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        created_at: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        start_time: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        end_time: row.get(4)?,
        status: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
        urgency: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
        assignee: row.get(7)?,
        reason: row.get(8)?,
    })
}

impl Database {
    pub fn insert_todo(&self, todo: &NewTodo, at: Timestamp) -> Result<i64, AppError> {
        let created_at = at.to_rfc3339();
        let start_time = todo.start_time.unwrap_or(at).to_rfc3339();
        self.with_write(|conn| {
            conn.execute(
                "INSERT INTO todo (content, created_at, start_time, end_time, status, urgency,
                                  assignee, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    todo.content,
                    created_at,
                    start_time,
                    todo.end_time.map(Timestamp::to_rfc3339),
                    todo.status,
                    todo.urgency,
                    todo.assignee,
                    todo.reason,
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    pub fn todo_by_id(&self, id: i64) -> Result<Option<TodoRow>, AppError> {
        self.with_read(|conn| {
            conn.query_row(
                &format!("SELECT {TODO_COLUMNS} FROM todo WHERE id = ?1"),
                [id],
                row_to_todo,
            )
            .optional()
        })
    }

    /// 列表查询。排序：`urgency DESC, created_at DESC`。
    pub fn list_todos(&self, query: &TodoQuery) -> Result<Vec<TodoRow>, AppError> {
        let mut conditions: Vec<String> = Vec::new();
        let mut values: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(start) = query.start {
            conditions.push("start_time >= ?".to_string());
            values.push(rusqlite::types::Value::Text(start.to_rfc3339()));
        }
        if let Some(end) = query.end {
            conditions.push("start_time <= ?".to_string());
            values.push(rusqlite::types::Value::Text(end.to_rfc3339()));
        }
        if let Some(status) = query.status {
            conditions.push("status = ?".to_string());
            values.push(rusqlite::types::Value::Integer(status));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let sql = format!(
            "SELECT {TODO_COLUMNS} FROM todo {where_clause} ORDER BY urgency DESC, created_at DESC, id DESC"
        );

        self.with_read(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), row_to_todo)?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn update_todo(
        &self,
        id: i64,
        patch: &TodoPatch,
        at: Timestamp,
    ) -> Result<usize, AppError> {
        if patch.is_empty() {
            return Ok(0);
        }

        let mut assignments: Vec<String> = Vec::new();
        let mut values: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(content) = &patch.content {
            assignments.push("content = ?".to_string());
            values.push(rusqlite::types::Value::Text(content.clone()));
        }
        if let Some(start) = patch.start_time {
            assignments.push("start_time = ?".to_string());
            values.push(match start {
                Some(value) => rusqlite::types::Value::Text(value.to_rfc3339()),
                None => rusqlite::types::Value::Null,
            });
        }
        if let Some(end) = patch.end_time {
            assignments.push("end_time = ?".to_string());
            values.push(match end {
                Some(value) => rusqlite::types::Value::Text(value.to_rfc3339()),
                None => rusqlite::types::Value::Null,
            });
        }
        if let Some(status) = patch.status {
            assignments.push("status = ?".to_string());
            values.push(rusqlite::types::Value::Integer(status));
        }
        if let Some(urgency) = patch.urgency {
            assignments.push("urgency = ?".to_string());
            values.push(rusqlite::types::Value::Integer(urgency));
        }
        if let Some(assignee) = &patch.assignee {
            assignments.push("assignee = ?".to_string());
            values.push(match assignee {
                Some(value) => rusqlite::types::Value::Text(value.clone()),
                None => rusqlite::types::Value::Null,
            });
        }
        if let Some(reason) = &patch.reason {
            assignments.push("reason = ?".to_string());
            values.push(match reason {
                Some(value) => rusqlite::types::Value::Text(value.clone()),
                None => rusqlite::types::Value::Null,
            });
        }

        let sql = format!("UPDATE todo SET {} WHERE id = ?", assignments.join(", "));
        values.push(rusqlite::types::Value::Integer(id));
        // `at` 目前不参与更新（`todo` 表没有 updated_at 列）——
        // `at` 仅保留签名，当前不参与写入（加审计列时不必改签名）。
        let _ = at;

        self.with_write(|conn| conn.execute(&sql, rusqlite::params_from_iter(values.iter())))
    }

    /// 勾选 / 取消勾选。
    ///
    /// 客户端会按 `1 - status` 显示，因此服务端必须自己翻转：
    /// 只按前端传的状态写，会让「取消勾选」变成又一次完成。
    /// 完成时补 `end_time`，取消完成时清掉（`updateToDoStatus` 的语义）。
    pub fn toggle_todo_status(&self, id: i64, at: Timestamp) -> Result<usize, AppError> {
        let Some(current) = self.todo_by_id(id)? else {
            return Ok(0);
        };

        let next = if current.status == 0 { 1 } else { 0 };
        let patch = TodoPatch {
            status: Some(next),
            end_time: Some(if next == 1 { Some(at) } else { None }),
            ..TodoPatch::default()
        };
        self.update_todo(id, &patch, at)
    }

    pub fn delete_todo(&self, id: i64) -> Result<usize, AppError> {
        self.with_write(|conn| conn.execute("DELETE FROM todo WHERE id = ?1", [id]))
    }

    /// 提示列表。新的在前（按 id 倒序展示）。
    pub fn list_tips(&self, limit: usize) -> Result<Vec<TipRow>, AppError> {
        self.with_read(|conn| {
            let mut stmt =
                conn.prepare("SELECT id, content, created_at FROM tips ORDER BY id DESC LIMIT ?1")?;
            let rows = stmt.query_map([limit as i64], |row| {
                Ok(TipRow {
                    id: row.get(0)?,
                    content: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    created_at: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// 写入一条提示。当前 daemon **不生成**提示，这个入口供导入使用。
    pub fn insert_tip(&self, content: &str, at: Timestamp) -> Result<i64, AppError> {
        let created_at = at.to_legacy_datetime();
        self.with_write(|conn| {
            conn.execute(
                "INSERT INTO tips (content, created_at) VALUES (?1, ?2)",
                params![content, created_at],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }
}
