//! Stage 状态机（纯函数）。
//!
//! 这是**用户原始诉求**（「一直在截图但没有阶段总结」）的入口：
//! 没有阶段划分，就无从谈阶段总结。
//!
//! 状态机刻意做成纯函数 + 注入时间：
//! - 六种结束条件（切换/空闲/锁屏/睡眠/超长/跨天）+ 手动 + 崩溃，
//!   每一条都能用固定时间戳确定性地测出来；
//! - 相同输入必须给出**相同的 effect 序列**（4.14），否则重放无法复现。

use mc_common::time::Timestamp;
use mc_domain::stage::{
    ActivitySignal, EndReason, StageDetector, StageEffect, StagePolicy, StageState,
};

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn policy() -> StagePolicy {
    StagePolicy {
        // 时间缩短，测试才好读；比例关系与默认值一致
        min_duration_secs: 600,
        max_duration_secs: 7200,
        switch_grace_secs: 300,
        idle_threshold_secs: 300,
        min_activity_stable_secs: 60,
        summary_deadline_secs: 600,
        timezone: "Asia/Shanghai".to_string(),
    }
}

fn signal(id: &str, title: &str, start_secs: i64, end_secs: i64) -> ActivitySignal {
    ActivitySignal {
        id: id.to_string(),
        title: title.to_string(),
        start: at(start_secs),
        end: at(end_secs),
    }
}

/// `(id, 结束原因, 结束时刻)` —— 结束时刻是断言时长正确性的关键。
fn closed_spans(effects: &[StageEffect]) -> Vec<(String, EndReason, Timestamp)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            StageEffect::Closed {
                id, reason, end, ..
            } => Some((id.clone(), *reason, *end)),
            StageEffect::Opened { .. } => None,
        })
        .collect()
}

fn closed(effects: &[StageEffect]) -> Vec<(&str, EndReason, Vec<String>)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            StageEffect::Closed {
                id,
                reason,
                activities,
                ..
            } => Some((id.as_str(), *reason, activities.clone())),
            StageEffect::Opened { .. } => None,
        })
        .collect()
}

fn opened(effects: &[StageEffect]) -> Vec<&str> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            StageEffect::Opened { id, .. } => Some(id.as_str()),
            StageEffect::Closed { .. } => None,
        })
        .collect()
}

#[test]
fn idle_to_observing_on_first_observation() {
    let mut detector = StageDetector::new(policy());
    assert_eq!(detector.state(), StageState::Idle);

    let effects = detector.observe(&signal("act-1", "写代码", 0, 0), at(0));

    assert_eq!(detector.state(), StageState::Observing);
    assert!(effects.is_empty(), "还没稳定，不该开阶段：{effects:?}");
}

#[test]
fn observing_to_open_after_activity_stable() {
    let mut detector = StageDetector::new(policy());
    detector.observe(&signal("act-1", "写代码", 0, 0), at(0));

    // 未到稳定阈值
    assert!(detector.tick(at(30)).is_empty());
    assert_eq!(detector.state(), StageState::Observing);

    // 到阈值 → 开阶段
    let effects = detector.tick(at(60));
    assert_eq!(detector.state(), StageState::Open);
    assert_eq!(opened(&effects), vec!["stage-act-1"]);
}

#[test]
fn activity_switch_begins_ending() {
    let mut detector = open_stage();

    let effects = detector.observe(&signal("act-2", "看网页", 600, 600), at(600));

    assert_eq!(
        detector.state(),
        StageState::Ending,
        "切换先进入宽限，不能立刻关阶段（用户经常切走再切回）"
    );
    assert!(closed(&effects).is_empty(), "宽限期内不该关阶段");
}

#[test]
fn grace_cancel_returns_to_open() {
    let mut detector = open_stage();
    detector.observe(&signal("act-2", "看网页", 600, 600), at(600));

    // 宽限期内切回来
    let effects = detector.observe(&signal("act-1", "写代码", 660, 660), at(660));

    assert_eq!(detector.state(), StageState::Open, "回到原活动应当取消结束");
    assert!(closed(&effects).is_empty());
    assert!(opened(&effects).is_empty(), "不该开新阶段");
}

#[test]
fn grace_expiry_closes_stage() {
    let mut detector = open_stage();
    detector.observe(&signal("act-2", "看网页", 600, 600), at(600));

    let effects = detector.tick(at(900));

    let closed_stages = closed(&effects);
    assert_eq!(closed_stages.len(), 1, "{effects:?}");
    assert_eq!(closed_stages[0].1, EndReason::Switched);
    assert_eq!(closed_stages[0].2, vec!["act-1"]);

    // 新活动已经持续了整个宽限期（≥ 稳定阈值），因此立刻开新阶段 ——
    // 两个阶段首尾相接（半开区间），中间不留缝也不重叠。
    assert_eq!(detector.state(), StageState::Open);
    assert_eq!(
        opened(&effects),
        vec!["stage-act-2"],
        "新阶段应当由切过去的活动开启：{effects:?}"
    );
}

#[test]
fn idle_closes_stage() {
    let mut detector = open_stage();

    let effects = detector.tick(at(1200));

    let closed_stages = closed(&effects);
    assert_eq!(closed_stages.len(), 1, "{effects:?}");
    assert_eq!(closed_stages[0].1, EndReason::Idle);
    assert_eq!(detector.state(), StageState::Idle);
}

#[test]
fn lock_closes_stage_immediately() {
    let mut detector = open_stage();

    let effects = detector.locked(at(400));

    assert_eq!(closed(&effects)[0].1, EndReason::Locked);
    assert_eq!(
        detector.state(),
        StageState::Idle,
        "锁屏期间不该继续累积阶段"
    );
}

#[test]
fn suspend_closes_stage_immediately() {
    let mut detector = open_stage();
    let effects = detector.suspended(at(400));
    assert_eq!(closed(&effects)[0].1, EndReason::Suspended);
}

#[test]
fn max_duration_force_splits_stage() {
    let mut detector = StageDetector::new(StagePolicy {
        max_duration_secs: 1800,
        ..policy()
    });
    detector.observe(&signal("act-1", "写代码", 0, 0), at(0));
    detector.tick(at(60));
    assert_eq!(detector.state(), StageState::Open);

    // 活动一直在延续（每 60 秒一条新信号，标题不变）
    for step in 1..=40 {
        detector.observe(
            &signal("act-1", "写代码", step * 60, step * 60),
            at(step * 60),
        );
    }

    let effects = detector.tick(at(1810));
    let closed_stages = closed(&effects);
    assert_eq!(
        closed_stages.len(),
        1,
        "超过单阶段上限必须切分：{effects:?}"
    );
    assert_eq!(closed_stages[0].1, EndReason::MaxDuration);
    assert_eq!(
        detector.state(),
        StageState::Observing,
        "切分后继续观察，活动没变就还会再开一个阶段"
    );
}

#[test]
fn day_boundary_closes_stage() {
    let mut detector = StageDetector::new(policy());
    // 本地时间（Asia/Shanghai）23:50 开始，证据连续跨过午夜
    let base = Timestamp::parse_rfc3339("2026-09-30T15:50:00Z").unwrap();
    let mut all = Vec::new();

    for offset_secs in [0i64, 360, 720] {
        let at = Timestamp::from_millis(base.as_millis() + offset_secs * 1000);
        all.extend(detector.observe(
            &ActivitySignal {
                id: "act-1".to_string(),
                title: "写代码".to_string(),
                start: base,
                end: at,
            },
            at,
        ));
        all.extend(detector.tick(at));
    }
    // 跨天之后
    all.extend(detector.tick(Timestamp::parse_rfc3339("2026-09-30T16:05:00Z").unwrap()));

    let closed_spans = closed_spans(&all);
    assert_eq!(closed_spans.len(), 1, "{all:?}");
    assert_eq!(closed_spans[0].0, "stage-act-1");
    assert_eq!(closed_spans[0].1, EndReason::DayBoundary);
    assert_eq!(
        closed_spans[0].2,
        Timestamp::parse_rfc3339("2026-09-30T16:00:00Z").unwrap(),
        "结束时刻必须是**本地午夜**，而不是「发现跨天的那一刻」"
    );
}

#[test]
fn overnight_never_merges_into_one_stage() {
    let mut detector = StageDetector::new(policy());
    // 活动本身有 60 秒的证据（否则阶段会因为「零长度」被丢弃）
    detector.observe(&signal("act-1", "写代码", 0, 60), at(0));
    detector.tick(at(60));
    assert_eq!(detector.state(), StageState::Open);

    // 第二天早上（隔夜 14 小时）
    let effects = detector.tick(at(14 * 3600));
    assert_eq!(closed(&effects).len(), 1, "隔夜必须断开");

    let next = signal("act-2", "写代码", 14 * 3600, 14 * 3600);
    detector.observe(&next, at(14 * 3600));
    detector.tick(at(14 * 3600 + 60));

    assert_ne!(
        detector.current_id(),
        Some("stage-act-1"),
        "隔夜后是新的阶段，不能并回昨天的"
    );
}

#[test]
fn shutdown_closes_stage_with_interrupted_reason() {
    let mut detector = open_stage();
    let effects = detector.shutdown(at(400));

    let closed_stages = closed(&effects);
    assert_eq!(closed_stages.len(), 1);
    assert_eq!(
        closed_stages[0].1,
        EndReason::Shutdown,
        "关机路径要能被总结侧识别为「被打断」，从而产出 interrupted 总结"
    );
}

#[test]
fn user_can_close_stage_manually() {
    let mut detector = open_stage();
    let effects = detector.close_manually(at(500));

    assert_eq!(closed(&effects)[0].1, EndReason::Manual);
    assert_eq!(detector.state(), StageState::Idle);
}

#[test]
fn stage_state_machine_is_pure() {
    fn scripted() -> Vec<StageEffect> {
        let mut detector = StageDetector::new(policy());
        let mut effects = Vec::new();
        effects.extend(detector.observe(&signal("act-1", "写代码", 0, 0), at(0)));
        effects.extend(detector.tick(at(60)));
        effects.extend(detector.observe(&signal("act-2", "看网页", 500, 500), at(500)));
        effects.extend(detector.tick(at(900)));
        effects.extend(detector.observe(&signal("act-3", "看网页", 960, 960), at(960)));
        effects.extend(detector.tick(at(1500)));
        effects.extend(detector.locked(at(1600)));
        effects
    }

    assert_eq!(scripted(), scripted(), "相同输入必须给出相同的 effect 序列");
}

#[test]
fn tick_is_idempotent_within_same_timestamp() {
    let mut detector = open_stage();

    let first = detector.tick(at(1200));
    let second = detector.tick(at(1200));

    assert_eq!(closed(&first).len(), 1);
    assert!(
        second.is_empty(),
        "同一时刻重复 tick 不该再产生效果：{second:?}"
    );
}

#[test]
fn stage_never_overlaps_another_stage() {
    let mut detector = StageDetector::new(StagePolicy {
        max_duration_secs: 600,
        ..policy()
    });

    let mut intervals: Vec<(Timestamp, Timestamp)> = Vec::new();
    let mut second = 0i64;
    while second <= 7200 {
        for effect in detector.observe(&signal("act-1", "写代码", second, second), at(second)) {
            if let StageEffect::Closed { start, end, .. } = effect {
                intervals.push((start, end));
            }
        }
        for effect in detector.tick(at(second)) {
            if let StageEffect::Closed { start, end, .. } = effect {
                intervals.push((start, end));
            }
        }
        second += 30;
    }

    assert!(intervals.len() >= 3, "应当切出多个阶段：{intervals:?}");
    for pair in intervals.windows(2) {
        assert!(
            pair[0].1 <= pair[1].0,
            "阶段区间不能重叠：{:?} 与 {:?}",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn stage_contains_only_its_own_activities() {
    let mut detector = open_stage();
    // 宽限期结束 → 关闭，活动的归属应当只有 act-1
    detector.observe(&signal("act-2", "看网页", 600, 600), at(600));
    let effects = detector.tick(at(900));

    let closed_stages = closed(&effects);
    assert_eq!(closed_stages[0].2, vec!["act-1"]);
    assert!(
        !closed_stages[0].2.contains(&"act-2".to_string()),
        "切换过去的活动属于下一段，不能算进被关闭的阶段"
    );
}

#[test]
fn dst_day_boundary_closes_stage_correctly() {
    // 美国夏令时结束当天（2026-11-01，America/New_York）有 25 小时。
    // 用固定 24 小时算日边界会在这天把阶段切错。
    let mut detector = StageDetector::new(StagePolicy {
        timezone: "America/New_York".to_string(),
        ..policy()
    });

    // 本地 2026-11-01 23:50（EST，UTC-5）→ 2026-11-02T04:50Z
    let base = Timestamp::parse_rfc3339("2026-11-02T04:50:00Z").unwrap();
    let mut all = Vec::new();

    for offset_secs in [0i64, 360, 720] {
        let at = Timestamp::from_millis(base.as_millis() + offset_secs * 1000);
        all.extend(detector.observe(
            &ActivitySignal {
                id: "act-1".to_string(),
                title: "写代码".to_string(),
                start: base,
                end: at,
            },
            at,
        ));
        all.extend(detector.tick(at));
    }
    all.extend(detector.tick(Timestamp::parse_rfc3339("2026-11-02T05:05:00Z").unwrap()));

    let closed_spans = closed_spans(&all);
    assert_eq!(
        closed_spans.len(),
        1,
        "DST 日的日边界必须按配置时区算：{all:?}"
    );
    assert_eq!(closed_spans[0].1, EndReason::DayBoundary);
    assert_eq!(
        closed_spans[0].2,
        Timestamp::parse_rfc3339("2026-11-02T05:00:00Z").unwrap(),
        "本地午夜是 EST 的 00:00（UTC-5），换算是 05:00Z"
    );
}

// 零长度的阶段不该产出：没有时长就没有证据，产物只会是噪声
#[test]
fn zero_length_stage_is_dropped() {
    // 稳定阈值为 0 时阶段会在第一条信号处立刻开启，
    // 此时任何「同一时刻的关闭」都会得到零长度阶段。
    let mut detector = StageDetector::new(StagePolicy {
        min_activity_stable_secs: 0,
        ..policy()
    });
    detector.observe(&signal("act-1", "写代码", 0, 0), at(0));
    detector.tick(at(0));
    assert_eq!(detector.state(), StageState::Open);

    let effects = detector.locked(at(0));
    assert!(effects.is_empty(), "零长度阶段不该产出：{effects:?}");
    assert_eq!(detector.state(), StageState::Idle);
}

/// 开一个「有内容」的阶段：单条信号会让阶段时长恒为 0，
/// 因此这里补一条同一活动的延续信号（真实数据里每 20–60 秒就会有一条）。
fn open_stage() -> StageDetector {
    let mut detector = StageDetector::new(policy());
    detector.observe(&signal("act-1", "写代码", 0, 0), at(0));
    detector.tick(at(60));
    assert_eq!(detector.state(), StageState::Open);
    detector.observe(&signal("act-1", "写代码", 0, 60), at(60));
    detector
}
