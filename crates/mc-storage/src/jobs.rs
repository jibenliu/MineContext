//! 幂等、持久化的作业队列。
//!
//! 放在 SQLite 而不是内存里，换来三个性质：
//! - **崩溃后仍在**：进程被 kill 也不会丢任务
//! - **重启后不重跑**：已完成的作业有终态且幂等键占位
//! - **可观测**：队列深度/在途/最老任务年龄都是查询出来的，不是猜的
//!
//! 去重的动机很具体：同一张截图 2 秒内会被请求 3 次，
//! 与「过载时降频，绝不丢观测」相冲。

use std::time::Duration;

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::map_sqlite_error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// 视觉分析（贵）
    VisionL2,
    /// 轻量分析
    VisionL1,
    /// 阶段结束触发的总结
    SummaryStage,
    /// 用户主动请求的任意时段总结
    SummaryAdhoc,
    /// 低负载时的补偿处理
    Backfill,
}

impl JobKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VisionL2 => "vision_l2",
            Self::VisionL1 => "vision_l1",
            Self::SummaryStage => "summary_stage",
            Self::SummaryAdhoc => "summary_adhoc",
            Self::Backfill => "backfill",
        }
    }

    /// 从数据库里的字符串还原。刻意不叫 `from_str`，避免与 `std::str::FromStr` 混淆。
    pub fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "vision_l2" => Some(Self::VisionL2),
            "vision_l1" => Some(Self::VisionL1),
            "summary_stage" => Some(Self::SummaryStage),
            "summary_adhoc" => Some(Self::SummaryAdhoc),
            "backfill" => Some(Self::Backfill),
            _ => None,
        }
    }
}

/// 数字越小越优先。
///
/// 顺序是刻意的：**用户主动请求 > 阶段总结 > 重要图片分析 > 次要分析 > 回填**。
/// 没有这一层，用户点了「总结最近两小时」却排在几百张图后面。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Interactive,
    StageSummary,
    VisionMajor,
    VisionMinor,
    Backfill,
}

impl Priority {
    pub const fn as_i32(self) -> i32 {
        match self {
            Self::Interactive => 0,
            Self::StageSummary => 1,
            Self::VisionMajor => 2,
            Self::VisionMinor => 3,
            Self::Backfill => 4,
        }
    }

    pub fn from_i32(value: i32) -> Self {
        match value {
            0 => Self::Interactive,
            1 => Self::StageSummary,
            2 => Self::VisionMajor,
            3 => Self::VisionMinor,
            _ => Self::Backfill,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

impl JobState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }

    /// 从数据库里的字符串还原。
    pub fn from_db_value(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "skipped" => Some(Self::Skipped),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    pub kind: JobKind,
    pub priority: Priority,
    pub observation_id: Option<String>,
    pub stage_id: Option<String>,
    pub adhoc_id: Option<String>,
    /// 幂等键。`(kind, idempotency)` 唯一 —— 同一份内容重复投递只会有一条。
    pub idempotency: String,
    /// 预估占用字节（用于加权信号量与批量切分）
    pub weight_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Enqueued {
        job_id: i64,
    },
    /// 命中幂等键，返回已存在那条的 id
    Deduped {
        job_id: i64,
    },
}

impl EnqueueOutcome {
    pub const fn job_id(&self) -> i64 {
        match self {
            Self::Enqueued { job_id } | Self::Deduped { job_id } => *job_id,
        }
    }

    pub const fn is_enqueued(&self) -> bool {
        matches!(self, Self::Enqueued { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: i64,
    pub kind: JobKind,
    pub priority: Priority,
    pub state: JobState,
    pub observation_id: Option<String>,
    pub stage_id: Option<String>,
    pub adhoc_id: Option<String>,
    pub idempotency: String,
    pub weight_bytes: u64,
    pub attempts: u32,
    pub not_before: Timestamp,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<Timestamp>,
    pub skip_reason: Option<String>,
    pub last_error: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueueStats {
    /// 排队中
    pub depth: i64,
    /// 已取走但尚未完成
    pub inflight: i64,
    /// 最老排队任务的创建时间（用于发现「卡住」）
    pub oldest_created_at: Option<Timestamp>,
}

impl QueueStats {
    pub const fn total(&self) -> i64 {
        self.depth + self.inflight
    }
}

const JOB_COLUMNS: &str = "id, kind, priority, state, observation_id, stage_id, adhoc_id, \
     idempotency, weight_bytes, attempts, not_before, lease_owner, lease_expires_at, \
     skip_reason, last_error, created_at";

impl Database {
    /// 入队。命中 `(kind, idempotency)` 唯一约束时返回已存在那条的 id。
    pub fn enqueue_job(&self, job: &NewJob, now: Timestamp) -> Result<EnqueueOutcome, AppError> {
        let mut conn = self.lock_write()?;
        let tx = conn.transaction().map_err(map_sqlite_error)?;

        let changed = tx
            .execute(
                "INSERT OR IGNORE INTO jobs (
                    kind, priority, observation_id, stage_id, adhoc_id, idempotency,
                    state, weight_bytes, attempts, not_before, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7, 0, ?8, ?8, ?8)",
                rusqlite::params![
                    job.kind.as_str(),
                    job.priority.as_i32(),
                    job.observation_id,
                    job.stage_id,
                    job.adhoc_id,
                    job.idempotency,
                    job.weight_bytes as i64,
                    now.as_millis(),
                ],
            )
            .map_err(map_sqlite_error)?;

        let job_id: i64 = if changed == 1 {
            tx.last_insert_rowid()
        } else {
            tx.query_row(
                "SELECT id FROM jobs WHERE kind = ?1 AND idempotency = ?2",
                rusqlite::params![job.kind.as_str(), job.idempotency],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sqlite_error)?
            .ok_or_else(|| {
                AppError::new(
                    ErrorCode::StorageUnavailable,
                    format!(
                        "作业入队失败且找不到既有记录：kind={} idempotency={}",
                        job.kind.as_str(),
                        job.idempotency
                    ),
                )
            })?
        };

        tx.commit().map_err(map_sqlite_error)?;

        Ok(if changed == 1 {
            EnqueueOutcome::Enqueued { job_id }
        } else {
            EnqueueOutcome::Deduped { job_id }
        })
    }

    /// 取一个可执行的作业并加租约。
    ///
    /// 用单条 `UPDATE ... RETURNING` 完成「挑选 + 占用」，因此**多个 worker
    /// 并发取任务也不会拿到同一条** —— 不需要额外的分布式锁。
    pub fn reserve_job(
        &self,
        worker: &str,
        now: Timestamp,
        lease: Duration,
    ) -> Result<Option<Job>, AppError> {
        let lease_until = now.plus_millis(lease.as_millis() as i64);

        let mut conn = self.lock_write()?;
        let tx = conn.transaction().map_err(map_sqlite_error)?;

        let job = {
            let mut stmt = tx
                .prepare(&format!(
                    "UPDATE jobs
                     SET state = 'running', lease_owner = ?1, lease_expires_at = ?2,
                         attempts = attempts + 1, updated_at = ?3
                     WHERE id = (
                        SELECT id FROM jobs
                        WHERE state = 'queued' AND not_before <= ?3
                        ORDER BY priority ASC, not_before ASC, id ASC
                        LIMIT 1
                     )
                     RETURNING {JOB_COLUMNS}"
                ))
                .map_err(map_sqlite_error)?;

            stmt.query_row(
                rusqlite::params![worker, lease_until.as_millis(), now.as_millis()],
                map_job,
            )
            .optional()
            .map_err(map_sqlite_error)?
        };

        tx.commit().map_err(map_sqlite_error)?;
        Ok(job)
    }

    pub fn complete_job(&self, id: i64, now: Timestamp) -> Result<(), AppError> {
        self.update_state(
            "UPDATE jobs SET state = 'succeeded', lease_owner = NULL, lease_expires_at = NULL,
                             updated_at = ?2
             WHERE id = ?1",
            id,
            now,
        )
    }

    pub fn skip_job(&self, id: i64, reason: &str, now: Timestamp) -> Result<(), AppError> {
        let conn = self.lock_write()?;
        conn.execute(
            "UPDATE jobs SET state = 'skipped', skip_reason = ?2, lease_owner = NULL,
                             lease_expires_at = NULL, updated_at = ?3
             WHERE id = ?1",
            rusqlite::params![id, reason, now.as_millis()],
        )
        .map_err(map_sqlite_error)?;
        Ok(())
    }

    /// 失败处理。
    ///
    /// **可重试的错误**回到队列并设置 `not_before`；其余（以及超过重试上限的）
    /// 直接进终态 —— 对 401 无限重试只会浪费时间与 token。
    pub fn fail_job(
        &self,
        id: i64,
        error: &AppError,
        not_before: Timestamp,
        now: Timestamp,
    ) -> Result<(), AppError> {
        let Some(job) = self.job(id)? else {
            return Err(AppError::new(
                ErrorCode::StorageUnavailable,
                format!("作业 {id} 不存在"),
            ));
        };

        let should_retry = error.retryable() && job.attempts < self.max_job_attempts();

        if should_retry {
            let conn = self.lock_write()?;
            conn.execute(
                "UPDATE jobs SET state = 'queued', not_before = ?2, last_error = ?3,
                                 lease_owner = NULL, lease_expires_at = NULL, updated_at = ?4
                 WHERE id = ?1",
                rusqlite::params![
                    id,
                    not_before.as_millis(),
                    format!("{}: {}", error.code().as_str(), error.detail()),
                    now.as_millis(),
                ],
            )
            .map_err(map_sqlite_error)?;
            Ok(())
        } else {
            let conn = self.lock_write()?;
            conn.execute(
                "UPDATE jobs SET state = 'failed', last_error = ?2, lease_owner = NULL,
                                 lease_expires_at = NULL, updated_at = ?3
                 WHERE id = ?1",
                rusqlite::params![
                    id,
                    format!("{}: {}", error.code().as_str(), error.detail()),
                    now.as_millis(),
                ],
            )
            .map_err(map_sqlite_error)?;
            Ok(())
        }
    }

    /// 回收租约过期的在途作业（worker 崩溃的兜底）。
    ///
    /// 没有这一步，一个崩溃的 worker 会让它的任务永久卡在 running。
    pub fn requeue_expired_leases(&self, now: Timestamp) -> Result<usize, AppError> {
        let conn = self.lock_write()?;
        let changed = conn
            .execute(
                "UPDATE jobs
                 SET state = 'queued', lease_owner = NULL, lease_expires_at = NULL, updated_at = ?1
                 WHERE state = 'running'
                   AND lease_expires_at IS NOT NULL
                   AND lease_expires_at < ?1",
                [now.as_millis()],
            )
            .map_err(map_sqlite_error)?;
        Ok(changed)
    }

    pub fn job(&self, id: i64) -> Result<Option<Job>, AppError> {
        self.with_read(|conn| {
            let mut stmt =
                conn.prepare(&format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?1"))?;
            stmt.query_row([id], map_job).optional()
        })
    }

    pub fn job_by_idempotency(
        &self,
        kind: JobKind,
        idempotency: &str,
    ) -> Result<Option<Job>, AppError> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {JOB_COLUMNS} FROM jobs WHERE kind = ?1 AND idempotency = ?2"
            ))?;
            stmt.query_row(rusqlite::params![kind.as_str(), idempotency], map_job)
                .optional()
        })
    }

    pub fn queue_stats(&self, kind: JobKind) -> Result<QueueStats, AppError> {
        self.with_read(|conn| {
            let (depth, inflight, oldest): (i64, i64, Option<i64>) = conn.query_row(
                "SELECT
                    COALESCE(SUM(CASE WHEN state = 'queued' THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN state = 'running' THEN 1 ELSE 0 END), 0),
                    MIN(CASE WHEN state = 'queued' THEN created_at END)
                 FROM jobs WHERE kind = ?1",
                [kind.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;

            Ok(QueueStats {
                depth,
                inflight,
                oldest_created_at: oldest.map(Timestamp::from_millis),
            })
        })
    }

    /// 作业重试上限。
    pub fn max_job_attempts(&self) -> u32 {
        self.job_max_attempts
    }

    pub fn set_max_job_attempts(&mut self, max: u32) {
        self.job_max_attempts = max.max(1);
    }

    fn update_state(&self, sql: &str, id: i64, now: Timestamp) -> Result<(), AppError> {
        let conn = self.lock_write()?;
        conn.execute(sql, rusqlite::params![id, now.as_millis()])
            .map_err(map_sqlite_error)?;
        Ok(())
    }
}

fn map_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    // 按列名读取（下标在字段增减时会静默读串）
    let kind_text: String = row.get("kind")?;
    let state_text: String = row.get("state")?;
    let priority_value: i32 = row.get("priority")?;

    Ok(Job {
        id: row.get("id")?,
        kind: JobKind::from_db_value(&kind_text).unwrap_or(JobKind::VisionL2),
        priority: Priority::from_i32(priority_value),
        state: JobState::from_db_value(&state_text).unwrap_or(JobState::Queued),
        observation_id: row.get("observation_id")?,
        stage_id: row.get("stage_id")?,
        adhoc_id: row.get("adhoc_id")?,
        idempotency: row.get("idempotency")?,
        weight_bytes: row.get::<_, i64>("weight_bytes")?.max(0) as u64,
        attempts: row.get::<_, i64>("attempts")?.max(0) as u32,
        not_before: Timestamp::from_millis(row.get("not_before")?),
        lease_owner: row.get("lease_owner")?,
        lease_expires_at: row
            .get::<_, Option<i64>>("lease_expires_at")?
            .map(Timestamp::from_millis),
        skip_reason: row.get("skip_reason")?,
        last_error: row.get("last_error")?,
        created_at: Timestamp::from_millis(row.get("created_at")?),
    })
}
