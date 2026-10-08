//! 作业队列的消费者与补偿作业的入队点。
//!
//! 队列表、`JobKind`、幂等键与租约早就有了（崩溃不丢、重启不重跑、可观测），
//! 但此前**没有任何生产者与消费者** —— 表是死的。这里补上两条：
//! 入队（`enqueue_backfill`）与消费（`run_once`），并把每种 kind 的去向写清楚：
//! 没接线的类型**明确跳过并留下原因**，而不是让它在队列里烂着。

use std::sync::Arc;
use std::time::Duration;

use mc_common::error::{AppError, ErrorCode};
use mc_common::observability::{info, warn};
use mc_common::time::{Clock, SystemClock, Timestamp};
use mc_storage::jobs::{Job, JobKind, JobState, NewJob, Priority};

use crate::state::ServerState;
use mc_pipeline::activity_ai::ActivityAiWorker;

/// 消费者标识（写进 `lease_owner`，诊断页能看到是谁在跑）。
pub const WORKER_NAME: &str = "daemon-jobs";
/// 补偿作业的幂等键前缀：`backfill:{from_ms}:{to_ms}`。
pub const BACKFILL_PREFIX: &str = "backfill:";
/// 租约时长：跑一段历史推断可能超过默认值，但也不能太长（崩溃后要能被别人捡起来）。
const LEASE: Duration = Duration::from_secs(120);
/// 补偿作业单轮处理的活动上限（与巡检一致：分批做，剩下的下一轮继续）。
const BACKFILL_LIMIT: usize = 50;

/// 入队一次补偿推断。同一范围重复提交只会有**一条**作业（幂等键去重）。
pub fn enqueue_backfill(
    state: &ServerState,
    from: Timestamp,
    to: Timestamp,
    now: Timestamp,
) -> Result<(i64, bool), AppError> {
    if from >= to {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            "补偿作业的时间范围必须 from < to",
        ));
    }
    let outcome = state.db.enqueue_job(
        &NewJob {
            kind: JobKind::Backfill,
            priority: Priority::Backfill,
            observation_id: None,
            stage_id: None,
            // 范围存在幂等键里：表结构不动（迁移已发过版），同时天然去重
            adhoc_id: Some(format!("{}:{}", from.as_millis(), to.as_millis())),
            idempotency: format!("{BACKFILL_PREFIX}{}:{}", from.as_millis(), to.as_millis()),
            weight_bytes: 0,
        },
        now,
    )?;
    Ok(match outcome {
        mc_storage::jobs::EnqueueOutcome::Enqueued { job_id } => (job_id, false),
        mc_storage::jobs::EnqueueOutcome::Deduped { job_id } => (job_id, true),
    })
}

/// 解析补偿作业的范围（从幂等键里取，入队时写进去的）。
fn backfill_range(job: &Job) -> Result<(Timestamp, Timestamp), AppError> {
    let rest = job
        .idempotency
        .strip_prefix(BACKFILL_PREFIX)
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::DomainInvariantViolated,
                format!("补偿作业的幂等键形状不对：{}", job.idempotency),
            )
        })?;
    let (from, to) = rest.split_once(':').ok_or_else(|| {
        AppError::new(
            ErrorCode::DomainInvariantViolated,
            format!("补偿作业缺少时间范围：{}", job.idempotency),
        )
    })?;
    let parse = |value: &str| {
        value
            .parse::<i64>()
            .map(Timestamp::from_millis)
            .map_err(|_| {
                AppError::new(
                    ErrorCode::DomainInvariantViolated,
                    format!("补偿作业的时间戳无法解析：{value}"),
                )
            })
    };
    Ok((parse(from)?, parse(to)?))
}

/// 取一条作业并处理；返回是否真的处理了（循环据此决定要不要继续立刻取）。
///
/// `worker` 由调用方持有：构造一次、循环复用（与投影循环一致，
/// 避免每轮重新解析密钥与组装 provider）。
pub async fn run_once(
    state: &Arc<ServerState>,
    worker: &mut Option<ActivityAiWorker>,
) -> Result<bool, AppError> {
    let now = SystemClock.now();
    let Some(job) = state.db.reserve_job(WORKER_NAME, now, LEASE)? else {
        return Ok(false);
    };

    match job.kind {
        JobKind::Backfill => run_backfill(state, worker, &job, now).await?,
        other => {
            // 不假装能跑：明确记下「这个类型还没接线」，诊断页能查到
            let reason = format!(
                "{} 类型尚未接线（队列消费者只实现了补偿推断）",
                other.as_str()
            );
            state.db.skip_job(job.id, &reason, now)?;
            warn!(
                component = "jobs",
                event = "skipped",
                kind = other.as_str(),
                job_id = job.id,
                "作业类型未接线，已跳过"
            );
        }
    }
    Ok(true)
}

async fn run_backfill(
    state: &Arc<ServerState>,
    worker: &mut Option<ActivityAiWorker>,
    job: &Job,
    now: Timestamp,
) -> Result<(), AppError> {
    let (from, to) = backfill_range(job)?;
    let Some(worker) = worker.as_mut() else {
        // 没配模型时「跑一遍」什么也不会发生：如实跳过，而不是记成成功
        state
            .db
            .skip_job(job.id, "未配置视觉模型，补偿推断无法执行", now)?;
        return Ok(());
    };

    let batch =
        crate::activities::infer_range(state, worker, now, from, to, BACKFILL_LIMIT).await?;
    state.db.complete_job(job.id, now)?;
    info!(
        component = "jobs",
        event = "completed",
        kind = "backfill",
        job_id = job.id,
        from_ms = from.as_millis(),
        to_ms = to.as_millis(),
        suggestions = batch.suggestions.len(),
        "补偿推断完成"
    );
    Ok(())
}

/// 消费者循环：空闲时按 `idle` 间隔轮询队列。
///
/// 与采集/投影一样是 daemon 生命周期的一部分：没有它，入队的作业永远不会被处理。
pub fn spawn_jobs_worker(state: Arc<ServerState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let secrets = mc_providers::credentials::KeychainCommand::default();
        // `build_vision_worker` 自己会返回 `Option`（未配置模型时是 None），
        // 这里保持同样的形状，不要再包一层。
        let mut worker =
            crate::activities::build_vision_worker(&state.config.current().config, &secrets)
                .unwrap_or(None);
        let idle = Duration::from_secs(2);

        loop {
            match run_once(&state, &mut worker).await {
                Ok(true) => continue,
                Ok(false) => tokio::time::sleep(idle).await,
                Err(error) => {
                    // 队列本身出错（数据库不可用等）不该让循环退出：
                    // 退出等于从此不再处理任何作业，且没有人会知道
                    warn!(
                        component = "jobs",
                        event = "worker_error",
                        code = error.code().as_str(),
                        "作业消费者出错，稍后重试"
                    );
                    tokio::time::sleep(idle).await;
                }
            }
        }
    })
}

/// 读取一条作业（HTTP 状态接口用）。
pub fn job_json(job: &Job) -> serde_json::Value {
    serde_json::json!({
        "id": job.id,
        "kind": job.kind.as_str(),
        "state": job.state.as_str(),
        "attempts": job.attempts,
        "idempotency": job.idempotency,
        "skip_reason": job.skip_reason,
        "last_error": job.last_error,
        "created_at": job.created_at.as_millis(),
    })
}

/// 终态判断：消费者与接口都用它，避免两处对「什么算结束」理解不一致。
pub const fn is_terminal(state: JobState) -> bool {
    matches!(
        state,
        JobState::Succeeded | JobState::Failed | JobState::Skipped | JobState::Cancelled
    )
}
