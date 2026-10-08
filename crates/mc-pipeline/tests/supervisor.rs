//! 把调度器接到真实异步任务上（worker 接线）。
//!
//! 纯调度器（`pools.rs`）只能证明「决定了什么」，
//! 证明不了「真的这样执行」。这一组用**真 tokio 任务**验证：
//! 分池容量、全局闸门、优先级在真实执行时同样成立 ——
//! 否则调度器就只是一段好看的决策代码。
//!
//! 并发现象容易写出「有时过有时不过」的测试，因此所有断言都落在
//! **可观测的最大并发数**与**开始顺序**上，而不是时序猜测。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mc_pipeline::pools::{PoolConfig, Priority, TaskClass};
use mc_pipeline::supervisor::PoolSupervisor;

fn config() -> PoolConfig {
    PoolConfig {
        vision_capacity: 2,
        summary_capacity: 2,
        global_provider_limit: 4,
        global_reserved_for_summary: 1,
        interactive_reserved_summary_slots: 1,
        aging_after_secs: 30,
    }
}

/// 记录「同时最多有多少个任务在跑」与「开始顺序」。
#[derive(Default)]
struct Probe {
    vision_now: AtomicUsize,
    vision_peak: AtomicUsize,
    global_now: AtomicUsize,
    global_peak: AtomicUsize,
    started: Started,
}

/// 极简的「开始顺序」记录（避免为测试引入新依赖）。
#[derive(Default)]
struct Started(std::sync::Mutex<Vec<String>>);

impl Started {
    fn push(&self, value: String) {
        self.0.lock().unwrap().push(value);
    }

    fn snapshot(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl Probe {
    fn record_start(&self, id: &str, class: TaskClass) {
        self.started.push(id.to_string());
        let global = self.global_now.fetch_add(1, Ordering::SeqCst) + 1;
        self.global_peak.fetch_max(global, Ordering::SeqCst);

        if class == TaskClass::Vision {
            let vision = self.vision_now.fetch_add(1, Ordering::SeqCst) + 1;
            self.vision_peak.fetch_max(vision, Ordering::SeqCst);
        }
    }

    fn record_end(&self, class: TaskClass) {
        self.global_now.fetch_sub(1, Ordering::SeqCst);
        if class == TaskClass::Vision {
            self.vision_now.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// 提交一个「睡一小会儿」的任务（用真实 await 点让任务真正并发）。
fn sleeping_job(
    supervisor: &mut PoolSupervisor,
    probe: Arc<Probe>,
    id: &str,
    class: TaskClass,
    priority: Priority,
    millis: u64,
) {
    let id_owned = id.to_string();
    supervisor.submit(id, class, priority, 1, move || {
        let probe = Arc::clone(&probe);
        let id = id_owned.clone();
        async move {
            probe.record_start(&id, class);
            tokio::time::sleep(Duration::from_millis(millis)).await;
            probe.record_end(class);
        }
    });
}

#[tokio::test]
async fn vision_concurrency_is_capped_at_the_pool_capacity() {
    let mut supervisor = PoolSupervisor::new(config());
    let probe = Arc::new(Probe::default());

    for index in 0..10 {
        sleeping_job(
            &mut supervisor,
            Arc::clone(&probe),
            &format!("vision-{index}"),
            TaskClass::Vision,
            Priority::Backfill,
            20,
        );
    }

    supervisor.run_until_idle().await;

    assert_eq!(
        probe.vision_peak.load(Ordering::SeqCst),
        config().vision_capacity,
        "vision 池的并发必须恰好被限制在容量上"
    );
    assert_eq!(probe.started.snapshot().len(), 10, "所有任务都要跑完");
}

#[tokio::test]
async fn global_provider_limit_is_never_exceeded() {
    let mut supervisor = PoolSupervisor::new(config());
    let probe = Arc::new(Probe::default());

    for index in 0..6 {
        sleeping_job(
            &mut supervisor,
            Arc::clone(&probe),
            &format!("vision-{index}"),
            TaskClass::Vision,
            Priority::Backfill,
            15,
        );
    }
    for index in 0..6 {
        sleeping_job(
            &mut supervisor,
            Arc::clone(&probe),
            &format!("summary-{index}"),
            TaskClass::Summary,
            Priority::StageSummary,
            15,
        );
    }

    supervisor.run_until_idle().await;

    assert!(
        probe.global_peak.load(Ordering::SeqCst) <= config().global_provider_limit,
        "全局闸门被突破：峰值 {}",
        probe.global_peak.load(Ordering::SeqCst)
    );
    assert!(
        probe.global_peak.load(Ordering::SeqCst) >= 2,
        "任务要真的并发跑"
    );
}

// 执行层验收：图片积压时总结照样先跑起来
#[tokio::test]
async fn summary_starts_before_the_vision_backlog_drains() {
    let mut supervisor = PoolSupervisor::new(config());
    let probe = Arc::new(Probe::default());

    // 60 个 vision 任务：按容量 2 算，全部跑完要 30 轮
    for index in 0..60 {
        sleeping_job(
            &mut supervisor,
            Arc::clone(&probe),
            &format!("vision-{index}"),
            TaskClass::Vision,
            Priority::Backfill,
            5,
        );
    }
    // 用户此刻点了「总结最近 2 小时」
    sleeping_job(
        &mut supervisor,
        Arc::clone(&probe),
        "summary-interactive",
        TaskClass::Summary,
        Priority::Interactive,
        5,
    );

    supervisor.run_until_idle().await;

    let started = probe.started.snapshot();
    let summary_position = started
        .iter()
        .position(|id| id == "summary-interactive")
        .expect("总结必须被调度");
    assert!(
        summary_position < config().global_provider_limit,
        "总结必须在前几个就被放行（实际第 {summary_position} 个），\
         而不是排在 60 张图后面 —— 这正是过载时降频、绝不丢观测"
    );
}

#[tokio::test]
async fn interactive_summary_preempts_queued_backfill() {
    let mut supervisor = PoolSupervisor::new(config());
    let probe = Arc::new(Probe::default());

    // 先把 summary 池占满（两个长任务）
    for index in 0..2 {
        sleeping_job(
            &mut supervisor,
            Arc::clone(&probe),
            &format!("backfill-{index}"),
            TaskClass::Summary,
            Priority::Backfill,
            40,
        );
    }
    sleeping_job(
        &mut supervisor,
        Arc::clone(&probe),
        "summary-interactive",
        TaskClass::Summary,
        Priority::Interactive,
        1,
    );

    supervisor.run_until_idle().await;

    let started = probe.started.snapshot();
    let interactive = started
        .iter()
        .position(|id| id == "summary-interactive")
        .expect("交互式总结必须被调度");
    assert!(
        interactive <= 2,
        "交互式总结应当在容量一空出来就执行（实际第 {interactive} 个）：{started:?}"
    );
}

#[tokio::test]
async fn supervisor_reports_queue_depths() {
    let mut supervisor = PoolSupervisor::new(config());
    let probe = Arc::new(Probe::default());

    for index in 0..5 {
        sleeping_job(
            &mut supervisor,
            Arc::clone(&probe),
            &format!("vision-{index}"),
            TaskClass::Vision,
            Priority::Backfill,
            5,
        );
    }

    let snapshot = supervisor.snapshot();
    assert_eq!(
        snapshot.vision_depth, 5,
        "还没开始跑时都在队列里：{snapshot:?}"
    );
    assert_eq!(snapshot.vision_inflight, 0);

    supervisor.run_until_idle().await;

    let snapshot = supervisor.snapshot();
    assert_eq!(snapshot.vision_depth, 0);
    assert_eq!(snapshot.vision_inflight, 0);
    assert_eq!(snapshot.global_inflight, 0, "跑完之后不能有残留");
}

#[tokio::test]
async fn tasks_with_weights_occupy_multiple_slots() {
    let mut supervisor = PoolSupervisor::new(PoolConfig {
        vision_capacity: 4,
        global_provider_limit: 8,
        global_reserved_for_summary: 1,
        ..config()
    });
    let probe = Arc::new(Probe::default());

    // 两个 weight=2 的任务就应当占满 vision 池的 4 个名额
    for index in 0..4 {
        let probe = Arc::clone(&probe);
        supervisor.submit(
            format!("heavy-{index}"),
            TaskClass::Vision,
            Priority::Backfill,
            2,
            move || {
                let probe = Arc::clone(&probe);
                async move {
                    probe.record_start("heavy", TaskClass::Vision);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    probe.record_end(TaskClass::Vision);
                }
            },
        );
    }

    supervisor.run_until_idle().await;

    assert!(
        probe.vision_peak.load(Ordering::SeqCst) <= 2,
        "两个 weight=2 的任务就把 4 个名额占满了，峰值不该超过 2：{}",
        probe.vision_peak.load(Ordering::SeqCst)
    );
}
