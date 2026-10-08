//! 幂等、持久化的作业队列。
//!
//! 这一组直接对应两个历史问题：
//! - 同一张截图在 2 秒内被请求 3 次 → 幂等键 + 唯一约束
//! - 过载时降频而不丢观测 → 有界、可观测、可恢复
//!
//! 队列放在 SQLite 而不是内存里，因此**崩溃后仍在**、**重启后不重跑**。

use std::time::Duration;

use mc_common::error::ErrorCode;
use mc_common::time::Timestamp;
use mc_storage::jobs::{EnqueueOutcome, JobKind, JobState, NewJob, Priority};
use mc_storage::Database;

fn open() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("minecontext.db");
    let db = Database::open(&path).unwrap();
    (dir, db)
}

fn ms(value: i64) -> Timestamp {
    Timestamp::from_millis(value)
}

fn job(kind: JobKind, idempotency: &str, priority: Priority) -> NewJob {
    NewJob {
        kind,
        priority,
        observation_id: None,
        stage_id: None,
        adhoc_id: None,
        idempotency: idempotency.to_string(),
        weight_bytes: 1024,
    }
}

fn vision(idempotency: &str) -> NewJob {
    job(JobKind::VisionL2, idempotency, Priority::VisionMajor)
}

// ---------------------------------------------------------------- 入队与幂等

// 2.31 的基础
#[test]
fn enqueue_creates_a_queued_job() {
    let (_dir, db) = open();

    let outcome = db.enqueue_job(&vision("obs-1"), ms(1_000)).unwrap();

    assert!(matches!(outcome, EnqueueOutcome::Enqueued { .. }));
    assert_eq!(db.queue_stats(JobKind::VisionL2).unwrap().depth, 1);
}

#[test]
fn same_screenshot_is_enqueued_exactly_once() {
    let (_dir, db) = open();

    let first = db
        .enqueue_job(&vision("content-hash-abc"), ms(1_000))
        .unwrap();
    let second = db
        .enqueue_job(&vision("content-hash-abc"), ms(1_001))
        .unwrap();
    let third = db
        .enqueue_job(&vision("content-hash-abc"), ms(1_002))
        .unwrap();

    assert!(matches!(first, EnqueueOutcome::Enqueued { .. }));
    assert!(
        matches!(second, EnqueueOutcome::Deduped { .. }),
        "同一幂等键必须被去重（同一张图 2 秒内被请求 3 次）"
    );
    assert!(matches!(third, EnqueueOutcome::Deduped { .. }));

    assert_eq!(db.queue_stats(JobKind::VisionL2).unwrap().depth, 1);
}

#[test]
fn dedup_returns_the_existing_job_id() {
    let (_dir, db) = open();

    let first = db.enqueue_job(&vision("h"), ms(1_000)).unwrap();
    let second = db.enqueue_job(&vision("h"), ms(1_000)).unwrap();

    assert_eq!(first.job_id(), second.job_id(), "去重后应返回同一个 job id");
}

#[test]
fn different_kinds_may_share_an_idempotency_key() {
    let (_dir, db) = open();

    db.enqueue_job(&vision("same-key"), ms(1_000)).unwrap();
    let other = db
        .enqueue_job(
            &job(JobKind::SummaryStage, "same-key", Priority::StageSummary),
            ms(1_000),
        )
        .unwrap();

    assert!(
        matches!(other, EnqueueOutcome::Enqueued { .. }),
        "唯一约束是 (kind, idempotency)，不同 kind 不应互相去重"
    );
}

#[test]
fn jobs_survive_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("minecontext.db");

    {
        let db = Database::open(&path).unwrap();
        db.enqueue_job(&vision("h"), ms(1_000)).unwrap();
    }

    // 队列在 SQLite 里，因此崩溃/重启后仍在
    let db = Database::open(&path).unwrap();
    assert_eq!(db.queue_stats(JobKind::VisionL2).unwrap().depth, 1);

    // 重启后也不会把已完成的重新排队
    let reserved = db
        .reserve_job("worker-1", ms(2_000), Duration::from_secs(60))
        .unwrap()
        .expect("应当能取到任务");
    db.complete_job(reserved.id, ms(2_100)).unwrap();
    drop(db);

    let db = Database::open(&path).unwrap();
    assert_eq!(
        db.queue_stats(JobKind::VisionL2).unwrap().depth,
        0,
        "已完成的作业不应在重启后复活"
    );
    assert!(db
        .reserve_job("worker-1", ms(3_000), Duration::from_secs(60))
        .unwrap()
        .is_none());
}

// ---------------------------------------------------------------- 优先级

#[test]
fn reserve_picks_highest_priority_first() {
    let (_dir, db) = open();

    db.enqueue_job(&job(JobKind::Backfill, "b", Priority::Backfill), ms(1_000))
        .unwrap();
    db.enqueue_job(
        &job(JobKind::VisionL1, "a", Priority::VisionMinor),
        ms(1_000),
    )
    .unwrap();
    db.enqueue_job(
        &job(JobKind::VisionL2, "u", Priority::Interactive),
        ms(1_000),
    )
    .unwrap();

    let mut order = Vec::new();
    while let Some(reserved) = db
        .reserve_job("w", ms(2_000), Duration::from_secs(60))
        .unwrap()
    {
        order.push(reserved.priority);
        db.complete_job(reserved.id, ms(2_100)).unwrap();
    }

    assert_eq!(
        order,
        vec![
            Priority::Interactive,
            Priority::VisionMinor,
            Priority::Backfill
        ],
        "必须按优先级出队：用户主动请求 > 图片分析 > 回填"
    );
}

#[test]
fn reserve_respects_not_before() {
    let (_dir, db) = open();

    // 一个已经在退避中的任务
    let outcome = db.enqueue_job(&vision("backoff"), ms(1_000)).unwrap();
    let reserved = db
        .reserve_job("w", ms(1_000), Duration::from_secs(60))
        .unwrap()
        .unwrap();
    db.fail_job(
        reserved.id,
        &mc_common::error::AppError::new(ErrorCode::ProviderRateLimited, "429"),
        ms(30_000),
        ms(1_100),
    )
    .unwrap();
    assert_eq!(outcome.job_id(), reserved.id);

    // 还没到 not_before
    assert!(
        db.reserve_job("w", ms(10_000), Duration::from_secs(60))
            .unwrap()
            .is_none(),
        "退避期内不应被取出"
    );

    // 到期后可以
    assert!(
        db.reserve_job("w", ms(30_001), Duration::from_secs(60))
            .unwrap()
            .is_some(),
        "退避到期后应当重新可取"
    );
}

// ---------------------------------------------------------------- 租约

#[test]
fn reserve_marks_running_and_sets_lease() {
    let (_dir, db) = open();
    db.enqueue_job(&vision("h"), ms(1_000)).unwrap();

    let reserved = db
        .reserve_job("worker-1", ms(2_000), Duration::from_secs(60))
        .unwrap()
        .unwrap();

    assert_eq!(reserved.state, JobState::Running);
    assert_eq!(reserved.lease_owner.as_deref(), Some("worker-1"));
    assert_eq!(reserved.attempts, 1, "取出即计一次尝试");

    let stats = db.queue_stats(JobKind::VisionL2).unwrap();
    assert_eq!(stats.depth, 0, "在途的不算排队中");
    assert_eq!(stats.inflight, 1);
}

#[test]
fn expired_lease_is_requeued() {
    let (_dir, db) = open();
    db.enqueue_job(&vision("h"), ms(1_000)).unwrap();

    // worker 取走后崩溃（从未 complete）
    db.reserve_job("doomed-worker", ms(2_000), Duration::from_secs(30))
        .unwrap()
        .unwrap();

    // 租约未到期：不该被别人抢走
    assert!(db
        .reserve_job("other", ms(10_000), Duration::from_secs(30))
        .unwrap()
        .is_none());

    // 到期后回收
    let requeued = db.requeue_expired_leases(ms(40_000)).unwrap();
    assert_eq!(requeued, 1);

    let recovered = db
        .reserve_job("other", ms(41_000), Duration::from_secs(30))
        .unwrap()
        .expect("崩溃 worker 的任务必须被重新拾取");
    assert_eq!(recovered.lease_owner.as_deref(), Some("other"));
    assert_eq!(recovered.attempts, 2, "重新拾取算作新的一次尝试");
}

// ---------------------------------------------------------------- 完成与失败

#[test]
fn completing_removes_from_queue() {
    let (_dir, db) = open();
    db.enqueue_job(&vision("h"), ms(1_000)).unwrap();
    let reserved = db
        .reserve_job("w", ms(2_000), Duration::from_secs(60))
        .unwrap()
        .unwrap();

    db.complete_job(reserved.id, ms(2_100)).unwrap();

    let stats = db.queue_stats(JobKind::VisionL2).unwrap();
    assert_eq!(stats.depth, 0);
    assert_eq!(stats.inflight, 0);

    // 幂等键仍然占位，因此不会重复入队
    assert!(matches!(
        db.enqueue_job(&vision("h"), ms(3_000)).unwrap(),
        EnqueueOutcome::Deduped { .. }
    ));
}

#[test]
fn failing_requeues_only_retryable_errors() {
    let (_dir, db) = open();
    db.enqueue_job(&vision("h"), ms(1_000)).unwrap();
    let reserved = db
        .reserve_job("w", ms(2_000), Duration::from_secs(60))
        .unwrap()
        .unwrap();

    // 429 可重试 → 回到队列
    db.fail_job(
        reserved.id,
        &mc_common::error::AppError::new(ErrorCode::ProviderRateLimited, "429"),
        ms(5_000),
        ms(2_100),
    )
    .unwrap();

    assert_eq!(db.queue_stats(JobKind::VisionL2).unwrap().depth, 1);

    // 401 不可重试 → 直接终态
    let reserved = db
        .reserve_job("w", ms(6_000), Duration::from_secs(60))
        .unwrap()
        .unwrap();
    db.fail_job(
        reserved.id,
        &mc_common::error::AppError::new(ErrorCode::ProviderAuthFailed, "401"),
        ms(9_000),
        ms(6_100),
    )
    .unwrap();

    assert_eq!(
        db.queue_stats(JobKind::VisionL2).unwrap().depth,
        0,
        "不可重试的错误不应再排队（否则会无限重试烧钱）"
    );
    assert_eq!(
        db.job(reserved.id).unwrap().unwrap().state,
        JobState::Failed
    );
}

// 2.37 的一半（另一半是退避算法本身）
#[test]
fn failing_beyond_max_attempts_marks_failed() {
    let (_dir, db) = open();
    db.enqueue_job(&vision("h"), ms(1_000)).unwrap();

    let mut now = 2_000i64;
    for attempt in 1..=5 {
        let Some(reserved) = db
            .reserve_job("w", ms(now), Duration::from_secs(60))
            .unwrap()
        else {
            break;
        };
        assert_eq!(reserved.attempts, attempt);
        db.fail_job(
            reserved.id,
            &mc_common::error::AppError::new(ErrorCode::ProviderTimeout, "timeout"),
            ms(now),
            ms(now),
        )
        .unwrap();
        now += 1_000;
    }

    let stats = db.queue_stats(JobKind::VisionL2).unwrap();
    assert_eq!(stats.depth, 0, "超过重试上限后必须停止重试");
}

#[test]
fn failed_job_keeps_its_own_identity_when_new_items_arrive() {
    // 的缺陷形态：`continue` 写在 `clear()` 之前，失败批次会在下一轮
    // **连同新到达的数据一起**被重新投递 —— 同一份工作被重复做，而且边界模糊。
    // 正确行为是：失败的任务作为**它自己**重试，与其它任务互不干扰。
    let (_dir, db) = open();

    let old = db.enqueue_job(&vision("old"), ms(1_000)).unwrap().job_id();
    let reserved = db
        .reserve_job("w", ms(2_000), Duration::from_secs(60))
        .unwrap()
        .unwrap();
    assert_eq!(reserved.id, old);

    db.fail_job(
        reserved.id,
        &mc_common::error::AppError::new(ErrorCode::ProviderTimeout, "timeout"),
        ms(3_000),
        ms(2_100),
    )
    .unwrap();

    // 失败的任务必须仍然存在，且保持自己的身份
    let after_fail = db.job(old).unwrap().expect("失败的任务不能被丢弃");
    assert_eq!(after_fail.state, JobState::Queued);
    assert_eq!(after_fail.attempts, 1);
    assert_eq!(after_fail.idempotency, "old");

    // 新任务入队后，两者是**独立**的两条
    let fresh = db.enqueue_job(&vision("new"), ms(2_500)).unwrap().job_id();
    assert_ne!(old, fresh);
    assert_eq!(db.queue_stats(JobKind::VisionL2).unwrap().depth, 2);

    // 任意取走一个，另一个仍独立排队（没有被「合并处理」）
    let taken = db
        .reserve_job("w", ms(3_001), Duration::from_secs(60))
        .unwrap()
        .unwrap();
    assert!(taken.id == old || taken.id == fresh);

    let remaining = db.queue_stats(JobKind::VisionL2).unwrap();
    assert_eq!(remaining.depth, 1, "另一个任务应独立等待，而不是被一起消费");

    let other_id = if taken.id == old { fresh } else { old };
    let other = db.job(other_id).unwrap().expect("另一个任务必须还在");
    assert_eq!(other.state, JobState::Queued);

    // 它没有被本次 reserve 碰过：既没被取走，attempts 也与取走前一致
    // （old 之前已经失败过一次，因此 attempts=1 是它自己的历史，不是被顺带处理的痕迹）
    assert!(other.lease_owner.is_none(), "它不该被顺带取走");
    let expected_attempts = if other_id == old { 1 } else { 0 };
    assert_eq!(
        other.attempts, expected_attempts,
        "attempts 应当保持它自己的历史，不因别的任务被取走而改变"
    );
}

#[test]
fn skipping_records_a_reason() {
    let (_dir, db) = open();
    db.enqueue_job(&vision("h"), ms(1_000)).unwrap();
    let reserved = db
        .reserve_job("w", ms(2_000), Duration::from_secs(60))
        .unwrap()
        .unwrap();

    db.skip_job(reserved.id, "backpressure", ms(2_100)).unwrap();

    let job = db.job(reserved.id).unwrap().unwrap();
    assert_eq!(job.state, JobState::Skipped);
    assert_eq!(job.skip_reason.as_deref(), Some("backpressure"));
    assert_eq!(db.queue_stats(JobKind::VisionL2).unwrap().depth, 0);
}

// ---------------------------------------------------------------- 可观测性

#[test]
fn queue_depth_is_reported() {
    let (_dir, db) = open();

    for index in 0..5 {
        db.enqueue_job(&vision(&format!("h{index}")), ms(1_000))
            .unwrap();
    }
    db.reserve_job("w", ms(2_000), Duration::from_secs(60))
        .unwrap()
        .unwrap();

    let stats = db.queue_stats(JobKind::VisionL2).unwrap();
    assert_eq!(stats.depth, 4, "排队中");
    assert_eq!(stats.inflight, 1, "在途");
    assert_eq!(stats.total(), 5);
}

#[test]
fn stats_are_per_kind() {
    let (_dir, db) = open();

    db.enqueue_job(&vision("v"), ms(1_000)).unwrap();
    db.enqueue_job(
        &job(JobKind::SummaryStage, "s", Priority::StageSummary),
        ms(1_000),
    )
    .unwrap();

    assert_eq!(db.queue_stats(JobKind::VisionL2).unwrap().depth, 1);
    assert_eq!(db.queue_stats(JobKind::SummaryStage).unwrap().depth, 1);
    assert_eq!(db.queue_stats(JobKind::SummaryAdhoc).unwrap().depth, 0);
}

#[test]
fn oldest_queued_job_age_is_reported() {
    let (_dir, db) = open();
    db.enqueue_job(&vision("h"), ms(1_000)).unwrap();

    let stats = db.queue_stats(JobKind::VisionL2).unwrap();
    assert_eq!(stats.oldest_created_at, Some(ms(1_000)));

    // 排空后没有「最老任务」
    let reserved = db
        .reserve_job("w", ms(2_000), Duration::from_secs(60))
        .unwrap()
        .unwrap();
    db.complete_job(reserved.id, ms(2_100)).unwrap();
    assert_eq!(
        db.queue_stats(JobKind::VisionL2).unwrap().oldest_created_at,
        None
    );
}

#[test]
fn job_lookup_by_idempotency_is_available() {
    let (_dir, db) = open();
    let outcome = db.enqueue_job(&vision("h"), ms(1_000)).unwrap();

    let found = db
        .job_by_idempotency(JobKind::VisionL2, "h")
        .unwrap()
        .expect("应当能按幂等键查到");
    assert_eq!(found.id, outcome.job_id());
    assert!(
        db.job_by_idempotency(JobKind::Backfill, "h")
            .unwrap()
            .is_none(),
        "不同 kind 下不应查到"
    );
}
