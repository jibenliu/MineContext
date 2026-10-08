//! 采集调度与背压。
//!
//! 这是 的最小实现。
//! 全部用 `TestClock` 驱动，**不 sleep、不碰 OS、不碰磁盘**，
//! 因此「漂移修正」「空闲降频」「队列满降频」这些时间敏感逻辑都能确定性验证。

use mc_capture::scheduler::{
    CapturePolicy, CaptureScheduler, CaptureSignals, SkipReason, StateChange,
};
use mc_common::time::{Clock, Timestamp};
use mc_testkit::TestClock;

fn ms(value: i64) -> Timestamp {
    Timestamp::from_millis(value)
}

fn policy() -> CapturePolicy {
    CapturePolicy {
        interval_secs: 1,
        idle_interval_secs: 10,
        idle_threshold_secs: 5,
        queue_capacity: 4,
        ..Default::default()
    }
}

fn running() -> CaptureSignals {
    CaptureSignals {
        locked: false,
        suspended: false,
        idle_for_secs: 0,
    }
}

/// 用户已经空闲 `secs` 秒（真实信号来自 OS，是一个时长而不是布尔值）。
fn idle_for(secs: u64) -> CaptureSignals {
    CaptureSignals {
        locked: false,
        suspended: false,
        idle_for_secs: secs,
    }
}

fn scheduler_with(targets: &[&str]) -> CaptureScheduler {
    let mut scheduler = CaptureScheduler::new(policy());
    scheduler.set_targets(targets.iter().map(|s| s.to_string()).collect());
    scheduler
}

// ---------------------------------------------------------------- 1.17 漂移修正

#[test]
fn capture_loop_is_drift_corrected() {
    let mut scheduler = scheduler_with(&["display-1"]);

    // 每次 tick 都比理想时刻晚 10ms（模拟调度延迟）。
    // 若按「now + interval」推进，10 次后会累积 100ms 漂移；
    // 按「名义时刻 + interval」推进则不会。
    let base = 1_000_000i64;
    for i in 0..10 {
        let now = ms(base + i * 1_000 + 10);
        let outcome = scheduler.tick(now, &running());
        assert_eq!(outcome.started, vec!["display-1"], "第 {i} 次应当采集");
        scheduler.mark_captured("display-1");
        scheduler.drain_pending(1);
    }

    let next_due = scheduler.next_due().expect("应当有下次到期时刻");
    let drift = next_due.as_millis() - (base + 10 * 1_000);
    assert!(
        drift.abs() < 50,
        "漂移必须被修正，实际漂移 {drift}ms（next_due={next_due}）"
    );
}

#[test]
fn tick_before_due_does_nothing() {
    let mut scheduler = scheduler_with(&["display-1"]);

    let first = scheduler.tick(ms(1_000), &running());
    assert_eq!(first.started, vec!["display-1"]);

    let early = scheduler.tick(ms(1_500), &running());
    assert!(early.started.is_empty(), "未到期的 tick 不应采集");
    assert_eq!(early.skipped, Some(SkipReason::NotDue));
}

// ---------------------------------------------------------------- 并发上限

// 12 ：同一目标不得并发采集（ PQueue(3) 无 per-source 在途保护，
// 正是「同一张截图被重复请求」的来源之一）
#[test]
fn same_target_never_captured_concurrently() {
    let mut scheduler = scheduler_with(&["display-1"]);

    let first = scheduler.tick(ms(1_000), &running());
    assert_eq!(first.started, vec!["display-1"]);

    // 上一张还没落盘，即使到期了也不能再抓同一个目标
    let second = scheduler.tick(ms(2_000), &running());
    assert!(
        second.started.is_empty(),
        "同一目标在途时不得并发采集，实际启动了 {:?}",
        second.started
    );
    assert_eq!(second.skipped, Some(SkipReason::AllInflight));

    // 采集完成后可以继续
    scheduler.mark_captured("display-1");
    let third = scheduler.tick(ms(3_000), &running());
    assert_eq!(third.started, vec!["display-1"]);
}

#[test]
fn different_targets_capture_in_parallel() {
    let mut scheduler = scheduler_with(&["display-1", "display-2"]);

    let outcome = scheduler.tick(ms(1_000), &running());

    assert_eq!(outcome.started.len(), 2, "不同目标应当并行采集");
    assert_eq!(scheduler.pending_count(), 2);
}

// ---------------------------------------------------------------- 1.19 / 1.20 背压

#[test]
fn bounded_queue_applies_backpressure() {
    // 队列容量 2（policy 里给的 4，这里用小容量更直观）
    let mut scheduler = scheduler_with(&["a", "b", "c", "d"]);
    scheduler.set_queue_capacity(2);

    let first = scheduler.tick(ms(1_000), &running());
    assert_eq!(first.started.len(), 2, "应当在容量内启动采集");
    assert!(!first.throttled);

    let second = scheduler.tick(ms(2_000), &running());
    assert!(
        second.started.is_empty(),
        "队列满时不得启动新的抓取，实际 {:?}",
        second.started
    );
    assert!(second.throttled, "队列满必须被标记为 throttled");
    assert_eq!(second.skipped, Some(SkipReason::QueueFull));
}

// 1.19b —— 关键：满时是「不再抓」而不是「丢掉已抓到的」
#[test]
fn queue_full_throttles_and_never_drops() {
    let mut scheduler = scheduler_with(&["a", "b", "c", "d"]);
    scheduler.set_queue_capacity(1);

    scheduler.tick(ms(1_000), &running());
    assert_eq!(scheduler.pending_count(), 1);

    for i in 1..10 {
        scheduler.tick(ms(1_000 + i * 1_000), &running());
    }

    assert_eq!(
        scheduler.pending_count(),
        1,
        "已抓到的图必须留在队列里，不能被丢弃"
    );
    assert_eq!(scheduler.dropped_total(), 0, "任何情况下都不应丢弃观测");
}

#[test]
fn queue_overflow_is_accounted_not_silent() {
    let mut scheduler = scheduler_with(&["a"]);
    scheduler.set_queue_capacity(1);

    scheduler.tick(ms(1_000), &running());
    assert_eq!(scheduler.throttled_total(), 0);

    scheduler.tick(ms(2_000), &running());
    scheduler.tick(ms(3_000), &running());

    assert_eq!(
        scheduler.throttled_total(),
        2,
        "每次因队列满而降频都必须计数（否则用户看不到「一直在记录但没在分析」）"
    );
}

#[test]
fn capture_rate_recovers_after_backlog_clears() {
    let mut scheduler = scheduler_with(&["a"]);
    scheduler.set_queue_capacity(1);

    scheduler.tick(ms(1_000), &running());
    let throttled = scheduler.tick(ms(2_000), &running());
    assert!(throttled.throttled);

    // 下游把积压消费掉
    scheduler.drain_pending(1);
    scheduler.mark_captured("a");

    let recovered = scheduler.tick(ms(3_000), &running());
    assert_eq!(recovered.started, vec!["a"], "积压清除后必须自动恢复采集");
    assert!(!recovered.throttled);
}

// ---------------------------------------------------------------- 1.18 慢持久化

#[test]
fn slow_persistence_does_not_block_tick_cadence() {
    // 下游完全不消费（persist 卡住），tick 的节拍仍必须准点推进
    let mut scheduler = scheduler_with(&["a", "b"]);
    scheduler.set_queue_capacity(64);

    let base = 2_000_000i64;
    for i in 0..20 {
        let now = ms(base + i * 1_000);
        scheduler.tick(now, &running());
        // 采集完成，但 drain 从不调用 → 模拟慢持久化
        scheduler.mark_captured("a");
        scheduler.mark_captured("b");
    }

    let expected = base + 20 * 1_000;
    let next_due = scheduler.next_due().unwrap().as_millis();
    assert!(
        (next_due - expected).abs() < 50,
        "下游再慢也不能让采集节拍漂移：next_due={next_due} 期望≈{expected}"
    );
    assert_eq!(scheduler.pending_count(), 40, "20 轮 × 2 个目标");
}

// ---------------------------------------------------------------- 1.21 锁屏

#[test]
fn pause_resume_on_lock_unlock() {
    let mut scheduler = scheduler_with(&["display-1"]);

    scheduler.tick(ms(1_000), &running());
    scheduler.mark_captured("display-1");
    scheduler.drain_pending(1);

    let locked = CaptureSignals {
        locked: true,
        ..running()
    };
    let outcome = scheduler.tick(ms(2_000), &locked);
    assert!(outcome.started.is_empty(), "锁屏期间不得采集");
    assert_eq!(outcome.skipped, Some(SkipReason::Locked));

    let unlocked = scheduler.tick(ms(3_000), &running());
    assert_eq!(unlocked.started, vec!["display-1"], "解锁后必须恢复采集");
}

#[test]
fn lock_reports_state_change_with_away_duration() {
    let mut scheduler = scheduler_with(&["display-1"]);
    scheduler.tick(ms(1_000), &running());
    scheduler.mark_captured("display-1");
    scheduler.drain_pending(1);

    // 锁屏 30 秒
    scheduler.tick(
        ms(2_000),
        &CaptureSignals {
            locked: true,
            ..running()
        },
    );
    scheduler.tick(
        ms(32_000),
        &CaptureSignals {
            locked: true,
            ..running()
        },
    );

    let resumed = scheduler.tick(ms(33_000), &running());

    match resumed.state_change {
        Some(StateChange::Resumed { away_for_secs }) => {
            assert!(
                (30..=31).contains(&away_for_secs),
                "必须报告离开了多久，供 Stage 引擎决定是否强制结束阶段，实际 {away_for_secs}s"
            );
        }
        other => panic!("解锁必须产生 Resumed 状态变化，实际 {other:?}"),
    }
}

#[test]
fn resume_does_not_produce_catch_up_burst() {
    let mut scheduler = scheduler_with(&["display-1"]);
    scheduler.tick(ms(1_000), &running());
    scheduler.mark_captured("display-1");
    scheduler.drain_pending(1);

    // 锁屏一小时
    scheduler.tick(
        ms(2_000),
        &CaptureSignals {
            locked: true,
            ..running()
        },
    );
    scheduler.tick(
        ms(3_602_000),
        &CaptureSignals {
            locked: true,
            ..running()
        },
    );

    // 解锁后第一帧
    let first = scheduler.tick(ms(3_603_000), &running());
    assert_eq!(first.started.len(), 1);

    // 不能「补采」这一小时里错过的帧 —— 那会在解锁瞬间打爆队列
    let immediate = scheduler.tick(ms(3_603_100), &running());
    assert!(
        immediate.started.is_empty(),
        "解锁后不应补采错过的帧（会瞬间打爆队列）"
    );
}

// ---------------------------------------------------------------- 挂起/唤醒

#[test]
fn suspend_resume_forces_stage_boundary() {
    let mut scheduler = scheduler_with(&["display-1"]);
    scheduler.tick(ms(1_000), &running());
    scheduler.mark_captured("display-1");
    scheduler.drain_pending(1);

    scheduler.tick(
        ms(2_000),
        &CaptureSignals {
            suspended: true,
            ..running()
        },
    );
    let resumed = scheduler.tick(ms(60_000), &running());

    assert!(
        matches!(resumed.state_change, Some(StateChange::Resumed { .. })),
        "睡眠唤醒必须产生状态变化，供 Stage 引擎结束跨睡眠的阶段"
    );
    assert_eq!(resumed.started, vec!["display-1"]);
}

// ---------------------------------------------------------------- 1.23 空闲降频

#[test]
fn idle_detection_reduces_capture_rate() {
    let mut scheduler = scheduler_with(&["display-1"]);

    // 空闲时长从 0 增长到 8 秒（阈值是 5 秒）
    let mut now = 1_000_000i64;
    for secs in 0..=8u64 {
        scheduler.tick(ms(now), &idle_for(secs));
        scheduler.mark_captured("display-1");
        scheduler.drain_pending(1);
        now += 1_000;
    }

    now += 6_000;
    let outcome = scheduler.tick(ms(now), &idle_for(20));
    assert!(scheduler.is_idle(), "空闲超过阈值必须被识别");
    assert_eq!(
        outcome.interval_secs,
        policy().idle_interval_secs,
        "空闲时应当降频"
    );
}

#[test]
fn input_resets_idle_and_restores_normal_rate() {
    let mut scheduler = scheduler_with(&["display-1"]);

    let mut now = 1_000_000i64;
    for secs in 0..10u64 {
        scheduler.tick(ms(now), &idle_for(secs));
        scheduler.mark_captured("display-1");
        scheduler.drain_pending(1);
        now += 1_000;
    }
    assert!(scheduler.is_idle());

    // 用户回来了
    let outcome = scheduler.tick(ms(now), &running());

    assert!(!scheduler.is_idle(), "有输入后必须退出空闲状态");
    assert_eq!(
        outcome.interval_secs,
        policy().interval_secs,
        "退出空闲后必须恢复原频率"
    );
}

// ---------------------------------------------------------------- 边界

#[test]
fn no_targets_yields_nothing_and_no_error() {
    let mut scheduler = CaptureScheduler::new(policy());

    let outcome = scheduler.tick(ms(1_000), &running());

    assert!(outcome.started.is_empty());
    assert_eq!(outcome.skipped, Some(SkipReason::NoTargets));
    assert!(!outcome.throttled, "没有目标不是背压");
}

#[test]
fn first_tick_captures_immediately() {
    let mut scheduler = scheduler_with(&["display-1"]);

    let outcome = scheduler.tick(ms(1_000), &running());

    assert_eq!(
        outcome.started,
        vec!["display-1"],
        "首次 tick 不应等待一个间隔"
    );
    assert_eq!(
        scheduler.next_due().unwrap(),
        ms(2_000),
        "下次到期应为 now + interval"
    );
}

#[test]
fn targets_added_later_join_the_next_tick() {
    let mut scheduler = scheduler_with(&["display-1"]);
    scheduler.tick(ms(1_000), &running());
    scheduler.mark_captured("display-1");
    scheduler.drain_pending(1);

    scheduler.set_targets(vec!["display-1".to_string(), "display-2".to_string()]);
    let outcome = scheduler.tick(ms(2_000), &running());

    assert_eq!(outcome.started.len(), 2, "新插入的显示器应当在下一轮被采集");
}

#[test]
fn clock_going_backwards_does_not_stall_capture() {
    let mut scheduler = scheduler_with(&["display-1"]);
    scheduler.tick(ms(1_000), &running());
    scheduler.mark_captured("display-1");
    scheduler.drain_pending(1);

    // 系统时钟被回拨（ 列出的真实场景）
    let outcome = scheduler.tick(ms(500), &running());

    assert!(
        !outcome.started.is_empty() || outcome.skipped == Some(SkipReason::NotDue),
        "时钟回拨不得导致采集永久停摆，实际 {outcome:?}"
    );
    assert!(
        scheduler.next_due().is_some(),
        "必须仍有一个合理的下次到期时刻"
    );
}

// ---------------------------------------------------------------- 与注入时钟协作

/// 展示并验证「调度器 + 可注入时钟」的配合方式。
///
/// 这是 的工程前提在采集层的落地：
/// 时间来自 `Clock`，因此可以「跳过」任意长的空闲而不真的等 5 分钟。
#[test]
fn scheduler_works_with_injected_clock() {
    let clock = TestClock::at("2026-09-30T09:00:00Z");
    let mut scheduler = scheduler_with(&["display-1"]);

    // 立刻采一帧
    let first = scheduler.tick(clock.now(), &running());
    assert_eq!(first.started, vec!["display-1"]);
    scheduler.mark_captured("display-1");
    scheduler.drain_pending(1);

    // 推进 1 秒 → 到期
    clock.advance(std::time::Duration::from_secs(1));
    let second = scheduler.tick(clock.now(), &running());
    assert_eq!(second.started, vec!["display-1"]);
    scheduler.mark_captured("display-1");
    scheduler.drain_pending(1);

    // 一次「跳过」5 分钟以上的空闲，不需要真的等待
    clock.advance(std::time::Duration::from_secs(6 * 60));
    scheduler.tick(clock.now(), &idle_for(6 * 60));

    assert!(scheduler.is_idle(), "空闲超过阈值必须被识别");
    assert_eq!(
        scheduler.tick(clock.now(), &idle_for(6 * 60)).interval_secs,
        policy().idle_interval_secs,
        "空闲后应切换到降频间隔"
    );
}
