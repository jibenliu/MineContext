//! `summaries` 投影器：总结落库。
//!
//! 总结**不是**纯派生数据：模型生成的内容无法重算（同一次生成不会复现）。
//! 因此这里的写入语义是「只增不改」——
//! 重生成走「新写一条 + 把旧的标记为被取代」，而不是就地覆盖，
//! 这样用户能看到「我上次看到的总结是什么」。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::db::Database;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSummary {
    pub id: String,
    /// `stage` | `adhoc` | `daily` | `weekly`
    pub kind: String,
    pub stage_id: Option<String>,
    pub template_id: String,
    pub start: Timestamp,
    pub end: Timestamp,
    pub title: String,
    pub body_markdown: String,
    /// `model` | `fallback`
    pub quality: String,
    pub model: Option<String>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub created_at: Timestamp,
    /// `scope_json` 原文（缓存判据与服务端诊断用）
    pub scope: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSummary {
    pub id: String,
    pub kind: String,
    pub stage_id: Option<String>,
    pub template_id: String,
    pub start: Timestamp,
    pub end: Timestamp,
    pub title: String,
    /// 模板字段 → 文本，序列化进 `fields_json`
    pub fields: std::collections::BTreeMap<String, String>,
    pub body_markdown: String,
    pub quality: String,
    pub model: Option<String>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// 作用域指纹 + 事件位点（任意时段总结的缓存判据）
    pub scope: Option<serde_json::Value>,
}

impl Database {
    /// 写入一条总结。id 冲突时**报错**而不是覆盖 ——
    /// 覆盖会静默丢掉用户可能正在看的那条总结。
    pub fn insert_summary(&self, summary: &NewSummary, at: Timestamp) -> Result<(), AppError> {
        let fields_json =
            serde_json::to_string(&summary.fields).unwrap_or_else(|_| "{}".to_string());

        self.with_write(|conn| {
            conn.execute(
                "INSERT INTO summaries
                   (id, kind, stage_id, template_id, start_utc_ms, end_utc_ms, title,
                    fields_json, body_markdown, quality, model, scope_json,
                    prompt_tokens, completion_tokens, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                rusqlite::params![
                    summary.id,
                    summary.kind,
                    summary.stage_id,
                    summary.template_id,
                    summary.start.as_millis(),
                    summary.end.as_millis(),
                    summary.title,
                    fields_json,
                    summary.body_markdown,
                    summary.quality,
                    summary.model,
                    summary.scope.as_ref().map(|scope| scope.to_string()),
                    summary.prompt_tokens,
                    summary.completion_tokens,
                    at.as_millis(),
                ],
            )?;
            Ok(())
        })
    }

    /// 某个阶段已有的总结（用于重生成时判断「已有」而不是重复写）。
    pub fn stage_summaries(&self, stage_id: &str) -> Result<Vec<StoredSummary>, AppError> {
        self.read_summaries(Some(stage_id), None)
    }

    pub fn read_summaries(
        &self,
        stage_id: Option<&str>,
        kind: Option<&str>,
    ) -> Result<Vec<StoredSummary>, AppError> {
        self.read_summaries_in_range(stage_id, kind, None, None)
    }

    pub fn read_summaries_in_range(
        &self,
        stage_id: Option<&str>,
        kind: Option<&str>,
        from: Option<Timestamp>,
        to: Option<Timestamp>,
    ) -> Result<Vec<StoredSummary>, AppError> {
        self.with_read(|conn| {
            let mut sql = String::from(
                "SELECT id, kind, stage_id, template_id, start_utc_ms, end_utc_ms, title,
                        body_markdown, quality, model, prompt_tokens, completion_tokens,
                        created_at, scope_json
                 FROM summaries WHERE 1 = 1",
            );
            let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(stage_id) = stage_id {
                sql.push_str(" AND stage_id = ?");
                values.push(Box::new(stage_id.to_string()));
            }
            if let Some(kind) = kind {
                sql.push_str(" AND kind = ?");
                values.push(Box::new(kind.to_string()));
            }
            if let Some(from) = from {
                sql.push_str(" AND start_utc_ms >= ?");
                values.push(Box::new(from.as_millis()));
            }
            if let Some(to) = to {
                sql.push_str(" AND start_utc_ms < ?");
                values.push(Box::new(to.as_millis()));
            }
            sql.push_str(" ORDER BY start_utc_ms, id");

            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(values.iter()), |row| {
                Ok(StoredSummary {
                    id: row.get("id")?,
                    kind: row.get("kind")?,
                    stage_id: row.get("stage_id")?,
                    template_id: row.get("template_id")?,
                    start: Timestamp::from_millis(row.get::<_, i64>("start_utc_ms")?),
                    end: Timestamp::from_millis(row.get::<_, i64>("end_utc_ms")?),
                    title: row.get("title")?,
                    body_markdown: row.get("body_markdown")?,
                    quality: row.get("quality")?,
                    model: row.get("model")?,
                    prompt_tokens: row.get::<_, Option<i64>>("prompt_tokens")?.unwrap_or(0) as u32,
                    completion_tokens: row.get::<_, Option<i64>>("completion_tokens")?.unwrap_or(0)
                        as u32,
                    created_at: Timestamp::from_millis(row.get::<_, i64>("created_at")?),
                    scope: row
                        .get::<_, Option<String>>("scope_json")?
                        .and_then(|text| serde_json::from_str(&text).ok()),
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn summary_count(&self) -> Result<i64, AppError> {
        self.with_read(|conn| {
            conn.query_row("SELECT COUNT(*) FROM summaries", [], |row| {
                row.get::<_, i64>(0)
            })
        })
    }
}
