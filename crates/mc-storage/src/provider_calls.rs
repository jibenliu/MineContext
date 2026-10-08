//! 模型调用记账与成本可见性。
//!
//! 存在的理由很具体：
//! 「2 小时 300 万 token，而 UI 没有产生总结或明显结果」。
//!
//! 只有把**花了多少**和**产出了什么**放在同一张报表里，用户才能判断值不值。
//! 单看 token 数是没有意义的 —— 有意义的是「每个产出花了多少」。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::map_sqlite_error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Vision,
    Chat,
    Embedding,
}

impl Purpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vision => "vision",
            Self::Chat => "chat",
            Self::Embedding => "embedding",
        }
    }

    pub fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "vision" => Some(Self::Vision),
            "chat" => Some(Self::Chat),
            "embedding" => Some(Self::Embedding),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallResult {
    Ok,
    Error,
    RateLimited,
    Timeout,
    InvalidResponse,
}

impl CallResult {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::InvalidResponse => "invalid_response",
        }
    }

    pub const fn is_success(self) -> bool {
        matches!(self, Self::Ok)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCall {
    pub at: Timestamp,
    pub provider_id: String,
    pub model: String,
    pub purpose: Purpose,
    pub observation_id: Option<String>,
    pub stage_id: Option<String>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub latency_ms: u64,
    pub result: CallResult,
    pub error_code: Option<String>,
}

impl ProviderCall {
    pub const fn total_tokens(&self) -> u32 {
        self.prompt_tokens + self.completion_tokens
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model: String,
    pub calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurposeUsage {
    pub purpose: String,
    pub calls: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UsageReport {
    pub calls: u64,
    pub successful_calls: u64,
    pub failed_calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub avg_latency_ms: u64,
    pub by_model: Vec<ModelUsage>,
    pub by_purpose: Vec<PurposeUsage>,
}

impl UsageReport {
    pub const fn total_tokens(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }

    /// 失败率。运营上真正需要关心的是它 —— 失败也照样烧钱。
    pub fn failure_rate(&self) -> f64 {
        if self.calls == 0 {
            0.0
        } else {
            self.failed_calls as f64 / self.calls as f64
        }
    }
}

/// 成本 × 产出。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CostReport {
    pub usage: UsageReport,
    pub observations_captured: u64,
    pub observations_analyzed: u64,
    pub activities: u64,
    pub stages: u64,
    pub summaries: u64,
}

impl CostReport {
    /// 每次视觉分析平均消耗多少 token。
    ///
    /// 这是「成本与价值是否匹配」最直接的指标。它随运行时间增长说明提示词在
    /// 无界膨胀，需要立刻查合并与总结的输入构造。
    pub fn tokens_per_analysis(&self) -> f64 {
        if self.observations_analyzed == 0 {
            0.0
        } else {
            self.usage.total_tokens() as f64 / self.observations_analyzed as f64
        }
    }

    /// 有没有产出。用来回答「花了两小时的钱，到底做出来了什么」。
    pub fn produced_anything(&self) -> bool {
        self.activities > 0 || self.stages > 0 || self.summaries > 0
    }
}

impl Database {
    pub fn record_provider_call(&self, call: &ProviderCall) -> Result<i64, AppError> {
        let conn = self.lock_write()?;
        conn.execute(
            "INSERT INTO provider_calls (
                at_utc_ms, provider_id, model, purpose, observation_id, stage_id,
                prompt_tokens, completion_tokens, latency_ms, result, error_code
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                call.at.as_millis(),
                call.provider_id,
                call.model,
                call.purpose.as_str(),
                call.observation_id,
                call.stage_id,
                call.prompt_tokens,
                call.completion_tokens,
                call.latency_ms as i64,
                call.result.as_str(),
                call.error_code,
            ],
        )
        .map_err(map_sqlite_error)?;
        Ok(conn.last_insert_rowid())
    }

    pub fn provider_usage(&self, from: Timestamp, to: Timestamp) -> Result<UsageReport, AppError> {
        let (from_ms, to_ms) = (from.as_millis(), to.as_millis());

        self.with_read(|conn| {
            let (calls, successful, failed, prompt, completion, avg_latency): (
                i64,
                i64,
                i64,
                i64,
                i64,
                Option<f64>,
            ) = conn.query_row(
                "SELECT
                    COUNT(*),
                    COALESCE(SUM(CASE WHEN result = 'ok' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN result != 'ok' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(prompt_tokens), 0),
                    COALESCE(SUM(completion_tokens), 0),
                    AVG(latency_ms)
                 FROM provider_calls
                 WHERE at_utc_ms >= ?1 AND at_utc_ms < ?2",
                rusqlite::params![from_ms, to_ms],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )?;

            let mut by_model = Vec::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT model, COUNT(*), COALESCE(SUM(prompt_tokens),0),
                            COALESCE(SUM(completion_tokens),0)
                     FROM provider_calls
                     WHERE at_utc_ms >= ?1 AND at_utc_ms < ?2
                     GROUP BY model ORDER BY SUM(prompt_tokens + completion_tokens) DESC",
                )?;
                let rows = stmt.query_map(rusqlite::params![from_ms, to_ms], |row| {
                    Ok(ModelUsage {
                        model: row.get(0)?,
                        calls: row.get::<_, i64>(1)?.max(0) as u64,
                        prompt_tokens: row.get::<_, i64>(2)?.max(0) as u64,
                        completion_tokens: row.get::<_, i64>(3)?.max(0) as u64,
                    })
                })?;
                for row in rows {
                    by_model.push(row?);
                }
            }

            let mut by_purpose = Vec::new();
            {
                let mut stmt = conn.prepare(
                    "SELECT purpose, COUNT(*), COALESCE(SUM(prompt_tokens + completion_tokens),0)
                     FROM provider_calls
                     WHERE at_utc_ms >= ?1 AND at_utc_ms < ?2
                     GROUP BY purpose ORDER BY purpose ASC",
                )?;
                let rows = stmt.query_map(rusqlite::params![from_ms, to_ms], |row| {
                    Ok(PurposeUsage {
                        purpose: row.get(0)?,
                        calls: row.get::<_, i64>(1)?.max(0) as u64,
                        total_tokens: row.get::<_, i64>(2)?.max(0) as u64,
                    })
                })?;
                for row in rows {
                    by_purpose.push(row?);
                }
            }

            Ok(UsageReport {
                calls: calls.max(0) as u64,
                successful_calls: successful.max(0) as u64,
                failed_calls: failed.max(0) as u64,
                prompt_tokens: prompt.max(0) as u64,
                completion_tokens: completion.max(0) as u64,
                avg_latency_ms: avg_latency.unwrap_or(0.0).max(0.0) as u64,
                by_model,
                by_purpose,
            })
        })
    }

    /// 成本 × 产出。这是诊断页「今天花了多少、做出来了什么」的数据源。
    pub fn cost_report(&self, from: Timestamp, to: Timestamp) -> Result<CostReport, AppError> {
        let usage = self.provider_usage(from, to)?;
        let (from_ms, to_ms) = (from.as_millis(), to.as_millis());

        let count = |table: &str, column: &str| -> Result<u64, AppError> {
            self.with_read(|conn| {
                let sql =
                    format!("SELECT COUNT(*) FROM {table} WHERE {column} >= ?1 AND {column} < ?2");
                let value: i64 =
                    conn.query_row(&sql, rusqlite::params![from_ms, to_ms], |row| row.get(0))?;
                Ok(value.max(0) as u64)
            })
        };

        let observations_analyzed: i64 = self.with_read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM analyses
                 WHERE status IN ('done', 'degraded')
                   AND observation_id IN (
                       SELECT id FROM observations WHERE ts_utc_ms >= ?1 AND ts_utc_ms < ?2
                   )",
                rusqlite::params![from_ms, to_ms],
                |row| row.get(0),
            )
        })?;

        Ok(CostReport {
            usage,
            observations_captured: count("observations", "ts_utc_ms")?,
            observations_analyzed: observations_analyzed.max(0) as u64,
            activities: count("activities", "start_utc_ms")?,
            stages: count("stages", "start_utc_ms")?,
            summaries: count("summaries", "start_utc_ms")?,
        })
    }
}
