//! 分级调度与预算闸门。
//!
//! 这一层回答一个问题：**这一帧值不值得花钱？**
//! 朴素答案是「每次都值」—— 于是 2 小时烧掉 300 万 token。
//!
//! `decide()` 是**纯函数**，因此可以用表驱动穷举所有分支，
//! 不需要模型、不需要网络、不需要真实队列。

use mc_capture::change::ChangeKind;
use mc_common::time::Timestamp;
use mc_pipeline::budget::{BudgetAction, BudgetEvent, BudgetPolicy, BudgetTracker};
use mc_pipeline::schedule::{
    decide, AiPreference, PressureLevel, SchedulingDecision, SchedulingInput, SkipReason, TaskKind,
    Watermarks,
};

use mc_testkit::fixtures::FIXTURE_EPOCH_MS;
fn ms(value: i64) -> Timestamp {
    Timestamp::from_millis(value)
}

fn input(change: ChangeKind) -> SchedulingInput {
    SchedulingInput {
        change_kind: change,
        rule_matched: false,
        ai_preference: AiPreference::None,
        pressure: 0.0,
        budget_exceeded: false,
        privacy_blocked: false,
    }
}

// ---------------------------------------------------------------- 压力水位

#[test]
fn pressure_levels_follow_the_four_tier_design() {
    let watermarks = Watermarks::default(); // 0.50 / 0.80 / 0.95

    assert_eq!(watermarks.level(0.0), PressureLevel::Normal);
    assert_eq!(watermarks.level(0.49), PressureLevel::Normal);
    assert_eq!(watermarks.level(0.50), PressureLevel::Coalesce);
    assert_eq!(watermarks.level(0.79), PressureLevel::Coalesce);
    assert_eq!(watermarks.level(0.80), PressureLevel::Degrade);
    assert_eq!(watermarks.level(0.94), PressureLevel::Degrade);
    assert_eq!(watermarks.level(0.95), PressureLevel::Shed);
    assert_eq!(watermarks.level(1.0), PressureLevel::Shed);
}

#[test]
fn watermarks_are_configurable_and_validated() {
    let watermarks = Watermarks::new(0.3, 0.6, 0.9).expect("合法水位");
    assert_eq!(watermarks.level(0.5), PressureLevel::Coalesce);

    // 顺序错乱必须报错，否则降级档位会互相覆盖
    assert!(Watermarks::new(0.9, 0.6, 0.3).is_err());
    assert!(Watermarks::new(0.0, 0.6, 0.9).is_err());
}

// ---------------------------------------------------------------- 2.41–2.44

#[test]
fn unchanged_screen_does_not_call_vlm() {
    let decision = decide(&input(ChangeKind::PixelMinor), &Watermarks::default());

    assert!(!decision.tasks.contains(&TaskKind::L2Vision));
    assert_eq!(decision.skip_reason, Some(SkipReason::Unchanged));
    assert!(
        decision.tasks.is_empty(),
        "画面没变时连元数据都不必写（观测仍会落库，只是不产生分析作业）"
    );
}

#[test]
fn title_only_change_uses_l0() {
    let decision = decide(&input(ChangeKind::TitleOnly), &Watermarks::default());

    assert_eq!(decision.tasks, vec![TaskKind::L0Metadata]);
    assert!(!decision.tasks.contains(&TaskKind::L2Vision));
}

#[test]
fn pixel_major_triggers_l2() {
    for change in [ChangeKind::PixelMajor, ChangeKind::New] {
        let decision = decide(&input(change), &Watermarks::default());
        assert!(
            decision.tasks.contains(&TaskKind::L2Vision),
            "{change:?} 应当触发视觉分析"
        );
        assert!(decision.tasks.contains(&TaskKind::L0Metadata));
        assert_eq!(decision.skip_reason, None);
    }
}

#[test]
fn rule_hit_skips_vlm() {
    let mut scheduling = input(ChangeKind::PixelMajor);
    scheduling.rule_matched = true;
    scheduling.ai_preference = AiPreference::None;

    let decision = decide(&scheduling, &Watermarks::default());

    assert!(
        !decision.tasks.contains(&TaskKind::L2Vision),
        "规则命中且规则声明不需要 AI 时，绝不能调模型"
    );
    assert!(decision.tasks.contains(&TaskKind::L0Metadata));
    assert_eq!(decision.skip_reason, Some(SkipReason::RuleDecided));
}

#[test]
fn rule_with_deep_preference_still_uses_vlm() {
    let mut scheduling = input(ChangeKind::PixelMajor);
    scheduling.rule_matched = true;
    scheduling.ai_preference = AiPreference::Deep;

    let decision = decide(&scheduling, &Watermarks::default());
    assert!(decision.tasks.contains(&TaskKind::L2Vision));
}

#[test]
fn rule_with_assist_preference_uses_cheap_tier_first() {
    let mut scheduling = input(ChangeKind::PixelMajor);
    scheduling.rule_matched = true;
    scheduling.ai_preference = AiPreference::Assist;

    let decision = decide(&scheduling, &Watermarks::default());

    assert!(
        decision.tasks.contains(&TaskKind::L1Text),
        "assist 应当先走便宜的 L1"
    );
    assert!(
        !decision.tasks.contains(&TaskKind::L2Vision),
        "assist 不应直接上最贵的 L2"
    );
}

// ---------------------------------------------------------------- 隐私

#[test]
fn privacy_blocked_never_escalates() {
    let mut scheduling = input(ChangeKind::PixelMajor);
    scheduling.privacy_blocked = true;

    let decision = decide(&scheduling, &Watermarks::default());

    assert!(
        decision.tasks.is_empty(),
        "被拦截的内容不得产生任何分析作业"
    );
    assert_eq!(decision.skip_reason, Some(SkipReason::PrivacyBlocked));
}

// ---------------------------------------------------------------- 2.45–2.46 预算

#[test]
fn budget_exceeded_degrades_to_l0_only() {
    let mut scheduling = input(ChangeKind::PixelMajor);
    scheduling.budget_exceeded = true;

    let decision = decide(&scheduling, &Watermarks::default());

    assert!(
        !decision.tasks.contains(&TaskKind::L2Vision),
        "超预算后必须停止调用模型"
    );
    assert!(
        decision.tasks.contains(&TaskKind::L0Metadata),
        "但廉价元数据仍要记录 —— 降级不是停摆"
    );
    assert_eq!(decision.skip_reason, Some(SkipReason::BudgetExceeded));
}

#[test]
fn budget_policy_defaults_are_conservative() {
    let policy = BudgetPolicy::default();
    assert_eq!(policy.max_vlm_calls_per_hour, 240);
    assert!(policy.max_tokens_per_day > 0);
    assert_eq!(policy.on_exceeded, BudgetAction::Degrade);
}

#[test]
fn budget_resets_when_the_window_passes() {
    let policy = BudgetPolicy {
        max_vlm_calls_per_hour: 3,
        max_tokens_per_hour: u64::MAX,
        max_tokens_per_day: u64::MAX,
        on_exceeded: BudgetAction::Degrade,
    };
    let mut tracker = BudgetTracker::new(policy);

    let start = ms(FIXTURE_EPOCH_MS);
    for _ in 0..3 {
        tracker.record_call(100, start);
    }
    assert!(tracker.state(start).exceeded, "达到上限");

    // 同一小时内仍然超限
    assert!(tracker.state(ms(start.as_millis() + 30 * 60_000)).exceeded);

    // 越过一小时窗口后恢复
    let after = ms(start.as_millis() + 61 * 60_000);
    assert!(!tracker.state(after).exceeded, "窗口滑过后应恢复");
}

#[test]
fn token_budget_is_tracked_separately_from_call_count() {
    let policy = BudgetPolicy {
        max_vlm_calls_per_hour: 1000,
        max_tokens_per_hour: 500,
        max_tokens_per_day: u64::MAX,
        on_exceeded: BudgetAction::Degrade,
    };
    let mut tracker = BudgetTracker::new(policy);
    let now = ms(FIXTURE_EPOCH_MS);

    tracker.record_call(300, now);
    assert!(!tracker.state(now).exceeded);

    tracker.record_call(300, now);
    assert!(tracker.state(now).exceeded, "token 超限同样要降级");
}

#[test]
fn daily_budget_caps_even_when_hourly_is_fine() {
    let policy = BudgetPolicy {
        max_vlm_calls_per_hour: 1000,
        max_tokens_per_hour: u64::MAX,
        max_tokens_per_day: 1000,
        on_exceeded: BudgetAction::Degrade,
    };
    let mut tracker = BudgetTracker::new(policy);

    let start = FIXTURE_EPOCH_MS;
    // 每小时只用 300 token，但连续 4 小时累计 1200 > 1000
    for hour in 0..4 {
        tracker.record_call(300, ms(start + hour * 3_600_000));
    }

    let state = tracker.state(ms(start + 3 * 3_600_000));
    assert!(state.exceeded, "日预算必须能单独触发降级");
    assert_eq!(state.reason.as_deref(), Some("daily_tokens"));
}

#[test]
fn budget_exceeded_emits_a_transition_event_exactly_once() {
    let policy = BudgetPolicy {
        max_vlm_calls_per_hour: 2,
        max_tokens_per_hour: u64::MAX,
        max_tokens_per_day: u64::MAX,
        on_exceeded: BudgetAction::Degrade,
    };
    let mut tracker = BudgetTracker::new(policy);
    let now = ms(FIXTURE_EPOCH_MS);

    let mut events = Vec::new();
    for _ in 0..5 {
        if let Some(event) = tracker.record_call(10, now) {
            events.push(event);
        }
    }

    assert_eq!(
        events,
        vec![BudgetEvent::Exceeded {
            scope: "hourly_calls".to_string()
        }],
        "超限事件必须只发一次（否则 UI 会被刷屏）"
    );
}

#[test]
fn budget_recovery_emits_a_recovered_event() {
    let policy = BudgetPolicy {
        max_vlm_calls_per_hour: 1,
        max_tokens_per_hour: u64::MAX,
        max_tokens_per_day: u64::MAX,
        on_exceeded: BudgetAction::Degrade,
    };
    let mut tracker = BudgetTracker::new(policy);
    let start = FIXTURE_EPOCH_MS;

    // 上限是「最多 1 次」，因此第 1 次调用就把额度用满 → 立即超限
    assert!(
        matches!(
            tracker.record_call(10, ms(start)),
            Some(BudgetEvent::Exceeded { .. })
        ),
        "用满额度即视为超限（max=1 时第一次调用就满了）"
    );
    assert_eq!(
        tracker.record_call(10, ms(start)),
        None,
        "已经超限后不再重复发事件"
    );

    // 窗口滑过、**且没有新调用**时才谈得上「恢复」——
    // 由 tick 探测，而不是由某次调用顺带发现（那次调用本身会再次用掉额度）。
    let later = ms(start + 61 * 60_000);
    assert_eq!(
        tracker.tick(later),
        Some(BudgetEvent::Recovered {
            scope: "hourly_calls".to_string()
        }),
        "窗口滑过后必须报告恢复"
    );
    assert_eq!(tracker.tick(later), None, "恢复事件也只发一次");
}

#[test]
fn budget_usage_is_reportable_for_the_ui() {
    let policy = BudgetPolicy {
        max_vlm_calls_per_hour: 100,
        max_tokens_per_hour: 10_000,
        max_tokens_per_day: 100_000,
        on_exceeded: BudgetAction::Degrade,
    };
    let mut tracker = BudgetTracker::new(policy);
    let now = ms(FIXTURE_EPOCH_MS);

    tracker.record_call(250, now);
    tracker.record_call(750, now);

    let usage = tracker.usage(now);
    assert_eq!(usage.calls_last_hour, 2);
    assert_eq!(usage.tokens_last_hour, 1000);
    assert_eq!(usage.tokens_last_day, 1000);
    assert_eq!(usage.hourly_call_limit, 100);
}

// ---------------------------------------------------------------- 2.48 表驱动

#[test]
fn scheduling_decision_is_pure_and_table_tested() {
    // (change, rule_matched, preference, pressure, budget_exceeded, privacy) → 期望任务集
    struct Case {
        name: &'static str,
        change: ChangeKind,
        rule_matched: bool,
        preference: AiPreference,
        pressure: f32,
        budget_exceeded: bool,
        privacy_blocked: bool,
        expect_l2: bool,
        expect_l0: bool,
    }

    let cases = vec![
        Case {
            name: "画面没变",
            change: ChangeKind::PixelMinor,
            rule_matched: false,
            preference: AiPreference::None,
            pressure: 0.0,
            budget_exceeded: false,
            privacy_blocked: false,
            expect_l2: false,
            expect_l0: false,
        },
        Case {
            name: "只改标题",
            change: ChangeKind::TitleOnly,
            rule_matched: false,
            preference: AiPreference::None,
            pressure: 0.0,
            budget_exceeded: false,
            privacy_blocked: false,
            expect_l2: false,
            expect_l0: true,
        },
        Case {
            name: "明显变化",
            change: ChangeKind::PixelMajor,
            rule_matched: false,
            preference: AiPreference::None,
            pressure: 0.0,
            budget_exceeded: false,
            privacy_blocked: false,
            expect_l2: true,
            expect_l0: true,
        },
        Case {
            name: "规则命中不调AI",
            change: ChangeKind::PixelMajor,
            rule_matched: true,
            preference: AiPreference::None,
            pressure: 0.0,
            budget_exceeded: false,
            privacy_blocked: false,
            expect_l2: false,
            expect_l0: true,
        },
        Case {
            name: "规则要求深度分析",
            change: ChangeKind::PixelMajor,
            rule_matched: true,
            preference: AiPreference::Deep,
            pressure: 0.0,
            budget_exceeded: false,
            privacy_blocked: false,
            expect_l2: true,
            expect_l0: true,
        },
        Case {
            name: "超预算",
            change: ChangeKind::PixelMajor,
            rule_matched: false,
            preference: AiPreference::None,
            pressure: 0.0,
            budget_exceeded: true,
            privacy_blocked: false,
            expect_l2: false,
            expect_l0: true,
        },
        Case {
            name: "隐私拦截",
            change: ChangeKind::PixelMajor,
            rule_matched: false,
            preference: AiPreference::None,
            pressure: 0.0,
            budget_exceeded: false,
            privacy_blocked: true,
            expect_l2: false,
            expect_l0: false,
        },
        Case {
            name: "压力到降级档",
            change: ChangeKind::PixelMinor,
            rule_matched: false,
            preference: AiPreference::None,
            pressure: 0.85,
            budget_exceeded: false,
            privacy_blocked: false,
            expect_l2: false,
            expect_l0: false,
        },
        Case {
            name: "压力到丢弃档",
            change: ChangeKind::PixelMajor,
            rule_matched: false,
            preference: AiPreference::None,
            pressure: 0.97,
            budget_exceeded: false,
            privacy_blocked: false,
            expect_l2: false,
            expect_l0: true,
        },
        Case {
            name: "压力在合并档",
            change: ChangeKind::PixelMajor,
            rule_matched: false,
            preference: AiPreference::None,
            pressure: 0.6,
            budget_exceeded: false,
            privacy_blocked: false,
            expect_l2: true,
            expect_l0: true,
        },
    ];

    for case in cases {
        let scheduling = SchedulingInput {
            change_kind: case.change,
            rule_matched: case.rule_matched,
            ai_preference: case.preference,
            pressure: case.pressure,
            budget_exceeded: case.budget_exceeded,
            privacy_blocked: case.privacy_blocked,
        };
        let decision = decide(&scheduling, &Watermarks::default());

        assert_eq!(
            decision.tasks.contains(&TaskKind::L2Vision),
            case.expect_l2,
            "用例「{}」的 L2 判断不符：{:?}",
            case.name,
            decision
        );
        assert_eq!(
            decision.tasks.contains(&TaskKind::L0Metadata),
            case.expect_l0,
            "用例「{}」的 L0 判断不符：{:?}",
            case.name,
            decision
        );
    }
}

#[test]
fn decide_does_not_mutate_anything() {
    let scheduling = input(ChangeKind::PixelMajor);
    let watermarks = Watermarks::default();

    let first = decide(&scheduling, &watermarks);
    for _ in 0..10 {
        assert_eq!(
            decide(&scheduling, &watermarks),
            first,
            "纯函数：同样输入必须给同样输出（无隐藏状态）"
        );
    }
}

#[test]
fn pressure_reason_is_reported_to_the_caller() {
    let mut scheduling = input(ChangeKind::PixelMajor);
    scheduling.pressure = 0.97;

    let decision = decide(&scheduling, &Watermarks::default());

    assert_eq!(decision.pressure_level, PressureLevel::Shed);
    assert_eq!(decision.skip_reason, Some(SkipReason::Backpressure));
}

#[test]
fn decision_helpers_are_consistent() {
    let scheduling = input(ChangeKind::PixelMajor);
    let decision: SchedulingDecision = decide(&scheduling, &Watermarks::default());

    assert_eq!(
        decision.needs_vision(),
        decision.tasks.contains(&TaskKind::L2Vision)
    );
    assert_eq!(decision.is_skipped(), decision.tasks.is_empty());
}

#[test]
fn backoff_is_exponential_with_jitter_bounds() {
    use std::time::Duration;

    let base = Duration::from_millis(500);
    let max = Duration::from_secs(30);

    // 第 1 次：基础延迟附近；后续指数增长但不超过上限
    let mut previous_upper = 0u128;
    for attempt in 1..=8u32 {
        let delay = mc_pipeline::schedule::backoff_delay(base, max, attempt, 0.5);

        assert!(
            delay <= max,
            "第 {attempt} 次的退避 {}ms 超过上限",
            delay.as_millis()
        );
        assert!(delay.as_millis() > 0, "退避必须为正");

        // 抖动范围应当在 [0.75x, 1.25x] 之间（jitter=0.5 时）
        let nominal = base.as_millis() as f64 * 2f64.powi(attempt as i32 - 1);
        let nominal = nominal.min(max.as_millis() as f64);
        let lower = (nominal * 0.75) as u128;
        let upper = (nominal * 1.25) as u128;
        let actual = delay.as_millis();
        assert!(
            actual >= lower && actual <= upper,
            "第 {attempt} 次退避 {actual}ms 不在 [{lower}, {upper}] 内"
        );
        previous_upper = upper;
    }
    assert!(previous_upper > 0);
}
