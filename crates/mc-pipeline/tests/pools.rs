//! **摘要不被图片积压饿死**。
//!
//! 单队列 + 单信号量时，图片一积压，用户点「总结最近 2 小时」就得排在 500 张图后面。
//! 这里是**按职责物理分池**：Vision Pool 与 Summary Pool 各自一个信号量，
//! 上面还有一层 Provider Gate 做全局闸门 —— vision 只能用 `limit - reserved`，
//! summary 有专属名额，vision 永不借用。
//!
//! 全部是确定性模拟（不依赖真实并发），因此可以在 CI 里稳定跑。

use mc_common::time::Timestamp;
use mc_pipeline::pools::{PoolConfig, PoolScheduler, PoolTask, Priority, TaskClass};

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn config() -> PoolConfig {
    PoolConfig {
        vision_capacity: 2,
        summary_capacity: 2,
        global_provider_limit: 4,
        // 全局闸门里给 summary 留 1 个名额：vision 永远吃不掉它
        global_reserved_for_summary: 1,
        // summary 池里给交互式留 1 个名额
        interactive_reserved_summary_slots: 1,
        aging_after_secs: 30,
    }
}

fn task(id: &str, class: TaskClass, priority: Priority, enqueued_secs: i64) -> PoolTask {
    PoolTask {
        id: id.to_string(),
        class,
        priority,
        enqueued_at: at(enqueued_secs),
        weight: 1,
    }
}

/// 灌 500 张图进 Vision 队列（模拟里「图片解析队列爆满」）。
fn flood_vision(scheduler: &mut PoolScheduler, count: usize) {
    for index in 0..count {
        scheduler.submit(task(
            &format!("vision-{index}"),
            TaskClass::Vision,
            Priority::Backfill,
            0,
        ));
    }
}

// 过载时降频、绝不丢观测的验收点
#[test]
fn summary_jobs_never_starved_by_vision_backlog() {
    let mut scheduler = PoolScheduler::new(config());
    flood_vision(&mut scheduler, 500);

    // 用户此刻点了「总结最近 2 小时」
    scheduler.submit(task(
        "summary-interactive",
        TaskClass::Summary,
        Priority::Interactive,
        0,
    ));

    let admitted = scheduler.admit(at(0));

    assert!(
        admitted.iter().any(|task| task.id == "summary-interactive"),
        "图片积压 500 项时，交互式总结必须立刻被放行：{admitted:?}"
    );
    assert_eq!(
        scheduler.queue_depth(TaskClass::Summary),
        0,
        "总结不该还留在队列里"
    );
    assert_eq!(
        scheduler.inflight(TaskClass::Vision),
        2,
        "vision 池仍按自己的并发跑"
    );
}

// 用模拟时间把「开始执行的等待时间」量出来
#[test]
fn interactive_summary_starts_within_sla_under_heavy_vision_load() {
    let mut scheduler = PoolScheduler::new(config());
    flood_vision(&mut scheduler, 500);
    scheduler.submit(task(
        "summary-interactive",
        TaskClass::Summary,
        Priority::Interactive,
        0,
    ));

    // 逐秒推进：vision 任务不断完成又不断补位，summary 必须在 5 秒内开始
    let mut started_at = None;
    for second in 0..5 {
        let admitted = scheduler.admit(at(second));
        if admitted.iter().any(|task| task.id == "summary-interactive") {
            started_at = Some(second);
            break;
        }
        // vision 完成一批、又补一批（模拟持续积压）
        let running: Vec<String> = scheduler
            .running_ids(TaskClass::Vision)
            .into_iter()
            .collect();
        for id in running {
            scheduler.complete(&id);
        }
        flood_vision(&mut scheduler, 2);
    }

    let started_at = started_at.expect("总结必须在 SLA 内开始");
    assert!(started_at < 5, "SLA 是 5 秒，实际第 {started_at} 秒才开始");
}

#[test]
fn stage_summary_preempts_backfill() {
    let mut scheduler = PoolScheduler::new(config());
    scheduler.submit(task("backfill", TaskClass::Summary, Priority::Backfill, 0));
    scheduler.submit(task("stage", TaskClass::Summary, Priority::StageSummary, 0));

    let admitted = scheduler.admit(at(0));
    let summary_order: Vec<&str> = admitted
        .iter()
        .filter(|task| task.class == TaskClass::Summary)
        .map(|task| task.id.as_str())
        .collect();

    assert_eq!(
        summary_order.first().copied(),
        Some("stage"),
        "阶段总结要排在补算任务前面：{summary_order:?}"
    );
}

#[test]
fn pools_never_exceed_the_global_provider_limit() {
    let mut scheduler = PoolScheduler::new(config());
    flood_vision(&mut scheduler, 50);
    for index in 0..10 {
        scheduler.submit(task(
            &format!("summary-{index}"),
            TaskClass::Summary,
            Priority::StageSummary,
            0,
        ));
    }

    scheduler.admit(at(0));

    assert!(
        scheduler.global_inflight() <= config().global_provider_limit,
        "全局闸门被突破：{} > {}",
        scheduler.global_inflight(),
        config().global_provider_limit
    );
    assert!(
        scheduler.inflight(TaskClass::Vision) <= config().vision_capacity,
        "vision 池自己的容量也生效"
    );
    assert!(
        scheduler.inflight(TaskClass::Vision)
            <= config().global_provider_limit - config().global_reserved_for_summary,
        "vision 只能用「全局名额 - 给 summary 保留的名额」"
    );
    assert!(
        scheduler.inflight(TaskClass::Summary) >= 1,
        "vision 打满时 summary 仍然必须拿到名额 —— 这正是保留名额的意义"
    );
}

// 全局保留名额：vision 池容量再大也吃不掉 summary 的那一份
#[test]
fn vision_cannot_consume_the_summary_reserved_global_slots() {
    let mut scheduler = PoolScheduler::new(PoolConfig {
        // vision 池自己允许 5 个，但全局只有 4 个、其中 1 个留给 summary
        vision_capacity: 5,
        global_provider_limit: 4,
        global_reserved_for_summary: 1,
        ..config()
    });
    flood_vision(&mut scheduler, 50);

    scheduler.submit(task(
        "summary-stage",
        TaskClass::Summary,
        Priority::StageSummary,
        0,
    ));
    scheduler.admit(at(0));

    assert_eq!(
        scheduler.inflight(TaskClass::Vision),
        3,
        "vision 最多只能用 3 个全局名额（4 - 1 保留）"
    );
    assert_eq!(
        scheduler.inflight(TaskClass::Summary),
        1,
        "保留的那个名额是给 summary 的"
    );
    assert!(scheduler.global_weight_inflight() <= 4);
}

#[test]
fn summary_pool_reserves_a_slot_for_interactive() {
    let mut scheduler = PoolScheduler::new(config());
    // 先把 summary 池塞满非交互任务
    scheduler.submit(task(
        "backfill-1",
        TaskClass::Summary,
        Priority::Backfill,
        0,
    ));
    scheduler.submit(task(
        "backfill-2",
        TaskClass::Summary,
        Priority::Backfill,
        0,
    ));

    scheduler.admit(at(0));

    assert!(
        scheduler.inflight(TaskClass::Summary) < config().summary_capacity,
        "非交互任务不能占满 summary 池的保留名额：inflight={}",
        scheduler.inflight(TaskClass::Summary)
    );

    // 交互式任务仍然进得来
    scheduler.submit(task(
        "interactive",
        TaskClass::Summary,
        Priority::Interactive,
        0,
    ));
    let admitted = scheduler.admit(at(0));
    assert!(
        admitted.iter().any(|task| task.id == "interactive"),
        "保留名额就是给交互式任务留的：{admitted:?}"
    );
}

// 防饿死：等待够久的低优先级任务必须被提上来
#[test]
fn aging_prevents_permanent_starvation() {
    let mut scheduler = PoolScheduler::new(config());
    scheduler.submit(task(
        "old-backfill",
        TaskClass::Summary,
        Priority::Backfill,
        0,
    ));

    // 每 10 秒来一个更高优先级的任务，持续 60 秒
    for second in [10, 20, 30, 40, 50] {
        scheduler.submit(task(
            &format!("stage-{second}"),
            TaskClass::Summary,
            Priority::StageSummary,
            second,
        ));
    }

    let admitted = scheduler.admit(at(60));
    assert!(
        admitted.iter().any(|task| task.id == "old-backfill"),
        "等了 60 秒的补算任务必须被放行（否则它就永远排在后面）：{admitted:?}"
    );
}

#[test]
fn per_pool_capacity_is_respected() {
    let mut scheduler = PoolScheduler::new(config());
    flood_vision(&mut scheduler, 20);
    for index in 0..20 {
        scheduler.submit(task(
            &format!("summary-{index}"),
            TaskClass::Summary,
            Priority::StageSummary,
            0,
        ));
    }

    scheduler.admit(at(0));

    assert!(scheduler.inflight(TaskClass::Vision) <= config().vision_capacity);
    assert!(scheduler.inflight(TaskClass::Summary) <= config().summary_capacity);
}

#[test]
fn completing_a_task_frees_capacity() {
    let mut scheduler = PoolScheduler::new(config());
    flood_vision(&mut scheduler, 10);
    scheduler.admit(at(0));

    let first = scheduler
        .running_ids(TaskClass::Vision)
        .into_iter()
        .next()
        .unwrap();
    scheduler.complete(&first);
    let admitted = scheduler.admit(at(1));

    assert!(
        !admitted.is_empty(),
        "完成一个任务后应当能再放行一个（否则池会永久缩水）"
    );
    assert!(scheduler.inflight(TaskClass::Vision) <= config().vision_capacity);
}

#[test]
fn task_weights_are_accounted_for() {
    let mut scheduler = PoolScheduler::new(config());
    scheduler.submit(PoolTask {
        id: "heavy-vision".to_string(),
        class: TaskClass::Vision,
        priority: Priority::StageSummary,
        enqueued_at: at(0),
        weight: 2,
    });

    scheduler.admit(at(0));

    assert_eq!(
        scheduler.global_weight_inflight(),
        2,
        "重任务要按权重占名额，否则「一次请求 = 一个名额」会让闸门形同虚设"
    );
}
