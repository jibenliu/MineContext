//! 事件存储：append-only 的唯一真源。
//!
//! 存储层**不认识领域类型**：`payload` 是不透明的 JSON，`kind` 是不透明的字符串。
//! 这样领域模型演进时不需要改存储层，也避免了 `mc-storage → mc-domain` 的反向依赖。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::Database;

/// 事件 payload 的结构版本。领域事件格式演进时递增，由 upcaster 处理历史数据。
pub const EVENT_SCHEMA_VERSION: u16 = 1;

/// 待追加的事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewEvent {
    pub at: Timestamp,
    pub kind: String,
    pub payload: Value,
    /// `system` | `user` | `migration`
    pub actor: String,
}

impl NewEvent {
    pub fn new(kind: impl Into<String>, at: Timestamp, payload: Value) -> Self {
        Self {
            at,
            kind: kind.into(),
            payload,
            actor: "system".to_string(),
        }
    }

    pub fn by(mut self, actor: impl Into<String>) -> Self {
        self.actor = actor.into();
        self
    }
}

/// 已提交的事件（从 `events` 表读出）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub seq: i64,
    pub at: Timestamp,
    pub kind: String,
    pub schema_version: u16,
    pub payload: Value,
    pub actor: String,
}

impl Database {
    /// 追加事件。整批在**一个事务**内提交：要么全成功，要么全失败。
    ///
    /// 返回按提交顺序排列的 `seq`。
    pub fn append_events(&self, events: &[NewEvent]) -> Result<Vec<i64>, AppError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        self.with_write(|conn| {
            let tx = conn.transaction()?;
            let mut ids = Vec::with_capacity(events.len());
            {
                let mut stmt = tx.prepare(
                    "INSERT INTO events (at_utc_ms, kind, schema_version, payload, actor)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )?;
                for event in events {
                    let payload = serde_json::to_string(&event.payload)
                        .unwrap_or_else(|_| "null".to_string());
                    stmt.execute(rusqlite::params![
                        event.at.as_millis(),
                        event.kind,
                        EVENT_SCHEMA_VERSION,
                        payload,
                        event.actor,
                    ])?;
                    ids.push(tx.last_insert_rowid());
                }
            }
            tx.commit()?;
            Ok(ids)
        })
    }

    /// 从 `from_seq`（不含）之后读取最多 `limit` 条事件，按 seq 升序。
    pub fn read_events(&self, from_seq: i64, limit: usize) -> Result<Vec<EventEnvelope>, AppError> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT seq, at_utc_ms, kind, schema_version, payload, actor
                 FROM events WHERE seq > ?1 ORDER BY seq ASC LIMIT ?2",
            )?;
            let rows = stmt.query_map(rusqlite::params![from_seq, limit as i64], |row| {
                let seq: i64 = row.get(0)?;
                let at_ms: i64 = row.get(1)?;
                let kind: String = row.get(2)?;
                let schema_version: i64 = row.get(3)?;
                let payload: String = row.get(4)?;
                let actor: String = row.get(5)?;
                Ok(EventEnvelope {
                    seq,
                    at: Timestamp::from_millis(at_ms),
                    kind,
                    schema_version: schema_version as u16,
                    payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                    actor,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// 按时间范围读取（`[from, to)`），用于日报与任意时段总结。
    pub fn read_events_in_time_range(
        &self,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<Vec<EventEnvelope>, AppError> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT seq, at_utc_ms, kind, schema_version, payload, actor
                 FROM events WHERE at_utc_ms >= ?1 AND at_utc_ms < ?2 ORDER BY seq ASC",
            )?;
            let rows =
                stmt.query_map(rusqlite::params![from.as_millis(), to.as_millis()], |row| {
                    let seq: i64 = row.get(0)?;
                    let at_ms: i64 = row.get(1)?;
                    let kind: String = row.get(2)?;
                    let schema_version: i64 = row.get(3)?;
                    let payload: String = row.get(4)?;
                    let actor: String = row.get(5)?;
                    Ok(EventEnvelope {
                        seq,
                        at: Timestamp::from_millis(at_ms),
                        kind,
                        schema_version: schema_version as u16,
                        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                        actor,
                    })
                })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    /// 当前最大 seq（空库为 0）。投影位点的基线。
    pub fn last_seq(&self) -> Result<i64, AppError> {
        self.with_read(|conn| {
            conn.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| {
                r.get::<_, i64>(0)
            })
        })
    }

    pub fn event_count(&self) -> Result<i64, AppError> {
        self.with_read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))
        })
    }

    /// 指定时间范围内的事件条数（任意时段总结的预览用）。
    pub fn count_events_in_time_range(
        &self,
        from: Timestamp,
        to: Timestamp,
    ) -> Result<i64, AppError> {
        self.with_read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM events WHERE at_utc_ms >= ?1 AND at_utc_ms < ?2",
                rusqlite::params![from.as_millis(), to.as_millis()],
                |r| r.get::<_, i64>(0),
            )
        })
    }
}
