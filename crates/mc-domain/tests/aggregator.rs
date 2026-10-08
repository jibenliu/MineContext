//! 活动聚合器。
//!
//! 这是整个产品最核心的一段逻辑。经典用例：
//! ```text
//! VSCode → Chrome(3s) → VSCode
//! ```
//! 期望是**一个**「写代码」活动，而不是 `开发 → 浏览网页 → 开发`；没有这层聚合，用户看到的时间线就是一堆碎片。
//!
//! 全部是纯状态机：时间由调用方注入，不读时钟、不做 IO。

use mc_common::time::Timestamp;
use mc_domain::activity::{AggregationPolicy, Aggregator, AggregatorEvent, EndReason, Provenance};
use mc_domain::rules::AiPreference;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn policy() -> AggregationPolicy {
    AggregationPolicy {
        debounce_secs: 45,
        min_duration_secs: 60,
        merge_gap_secs: 120,
        max_duration_secs: 7200,
        idle_gap_secs: 300,
    }
}

fn coding() -> Provenance {
    Provenance::Rule {
        rule_id: "coding".to_string(),
    }
}

fn candidate(title: &str) -> mc_domain::activity::ActivityCandidate {
    mc_domain::activity::ActivityCandidate {
        title: title.to_string(),
        category: Some("开发".to_string()),
        origin: coding(),
        confidence: 0.9,
        ai: AiPreference::None,
    }
}

fn other_candidate(title: &str) -> mc_domain::activity::ActivityCandidate {
    mc_domain::activity::ActivityCandidate {
        title: title.to_string(),
        category: Some("浏览".to_string()),
        origin: coding(),
        confidence: 0.9,
        ai: AiPreference::None,
    }
}

/// 从事件流里数出「开始过几个活动」。
fn started_titles(events: &[AggregatorEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            AggregatorEvent::Started { title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect()
}

fn closed_count(events: &[AggregatorEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, AggregatorEvent::Ended { .. }))
        .count()
}

// ---------------------------------------------------------------- 3.13 防抖

#[test]
fn debounce_app_switch_produces_single_activity() {
    let mut aggregator = Aggregator::new(policy());

    let mut events = Vec::new();
    events.extend(aggregator.observe("o1", &candidate("写代码"), at(0)));
    events.extend(aggregator.observe("o2", &other_candidate("浏览网页"), at(60)));
    events.extend(aggregator.observe("o3", &candidate("写代码"), at(63))); // 3 秒后切回

    // 3 秒的打断低于 45 秒防抖阈值，因此不应产生新活动
    assert_eq!(
        started_titles(&events),
        vec!["写代码".to_string()],
        "短暂切走再回来必须只产生一个活动，实际事件：{events:?}"
    );
    assert_eq!(closed_count(&events), 0, "不应关闭任何活动");

    // 被打断期间的观测仍然归属原活动（不能变成孤儿）
    let open = aggregator.open().expect("应当有一个进行中的活动");
    assert_eq!(open.title, "写代码");
    assert!(open.observations.contains(&"o2".to_string()));
}

#[test]
fn sustained_switch_commits_after_the_debounce_window() {
    let mut aggregator = Aggregator::new(policy());

    aggregator.observe("o1", &candidate("写代码"), at(0));
    aggregator.observe("o2", &other_candidate("浏览网页"), at(60));

    // 还未到防抖阈值
    let quiet = aggregator.tick(at(100));
    assert!(quiet.is_empty(), "阈值未到不应有任何动作：{quiet:?}");
    assert_eq!(aggregator.open().unwrap().title, "写代码");

    // 超过阈值 → 提交切换
    let events = aggregator.tick(at(106));
    assert_eq!(started_titles(&events), vec!["浏览网页".to_string()]);
    assert_eq!(closed_count(&events), 1, "应当关闭原活动");
    assert_eq!(aggregator.open().unwrap().title, "浏览网页");
}

// ---------------------------------------------------------------- 3.14 合并

#[test]
fn same_title_within_merge_gap_is_merged() {
    let mut aggregator = Aggregator::new(policy());

    // 写代码(0..60) → 切到浏览网页(60..150) → 又回到写代码
    aggregator.observe("o1", &candidate("写代码"), at(0));
    aggregator.observe("o2", &other_candidate("浏览网页"), at(60));
    aggregator.tick(at(120)); // 提交切换：写代码在 60 结束

    aggregator.observe("o3", &candidate("写代码"), at(150));
    let events = aggregator.tick(at(200)); // 提交切回

    // 写代码上次结束于 60，这次起于 150 —— 间隔 90s < merge_gap(120s)，应当合并
    assert!(
        started_titles(&events).is_empty(),
        "间隔在 merge_gap 内应当合并，而不是新开活动：{events:?}"
    );
    let open = aggregator.open().expect("应当仍在原活动上");
    assert_eq!(open.title, "写代码");
    assert!(open.observations.contains(&"o1".to_string()));
    assert!(open.observations.contains(&"o3".to_string()));
}

#[test]
fn same_title_after_merge_gap_starts_a_new_activity() {
    let mut aggregator = Aggregator::new(policy());

    aggregator.observe("o1", &candidate("写代码"), at(0));
    aggregator.observe("o2", &other_candidate("浏览网页"), at(60));
    aggregator.tick(at(120)); // 写代码结束于 60

    // 回来得太晚：60 → 400 间隔 340s > merge_gap(120s)
    aggregator.observe("o3", &candidate("写代码"), at(400));
    let events = aggregator.tick(at(500));

    assert_eq!(
        started_titles(&events),
        vec!["写代码".to_string()],
        "间隔超过 merge_gap 应当开启新活动：{events:?}"
    );
}

// ---------------------------------------------------------------- 3.15 噪声

#[test]
fn short_activity_between_different_titles_is_dropped_as_noise() {
    let mut aggregator = Aggregator::new(policy());

    let mut events = Vec::new();
    events.extend(aggregator.observe("o1", &candidate("写代码"), at(0)));
    // 只持续 5 秒的另一个活动，且之后没有回到「写代码」
    events.extend(aggregator.observe("o2", &other_candidate("看一眼邮件"), at(100)));
    events.extend(aggregator.observe("o3", &other_candidate("看文档"), at(105)));
    events.extend(aggregator.tick(at(200)));

    let dropped: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            AggregatorEvent::NoiseDropped {
                observation_ids, ..
            } => Some(observation_ids.clone()),
            _ => None,
        })
        .flatten()
        .collect();

    assert!(
        dropped.contains(&"o2".to_string()),
        "过短且未持续的活动应作为噪声丢弃并记录：{events:?}"
    );
}

#[test]
fn observations_are_never_orphaned() {
    let mut aggregator = Aggregator::new(policy());

    aggregator.observe("o1", &candidate("写代码"), at(0));
    aggregator.observe("o2", &other_candidate("看一眼邮件"), at(100));
    aggregator.observe("o3", &other_candidate("看文档"), at(105));
    aggregator.tick(at(400));
    aggregator.flush(at(500));

    let attached: Vec<String> = aggregator.all_observations();
    let dropped: Vec<String> = aggregator.dropped_observations();

    let mut accounted: Vec<String> = attached.into_iter().chain(dropped).collect();
    accounted.sort();
    accounted.dedup();

    for id in ["o1", "o2", "o3"] {
        assert!(
            accounted.contains(&id.to_string()),
            "观测 {id} 既没归属任何活动、也不在丢弃记录里 —— 这就是「观测丢失」"
        );
    }
}

// ---------------------------------------------------------------- 3.17 超长切分

#[test]
fn long_activity_is_force_split() {
    let mut policy_with_short_max = policy();
    policy_with_short_max.max_duration_secs = 600;
    let mut aggregator = Aggregator::new(policy_with_short_max);

    aggregator.observe("o1", &candidate("写代码"), at(0));
    aggregator.observe("o2", &candidate("写代码"), at(300));
    // 切分点（600 秒）之后**必须还有观测**才谈得上「超长」：
    // 证据停在 300 秒的话，正确的结束原因是「空闲」而不是「超长」——
    // 用户其实是干到 5 分钟就停了。
    aggregator.observe("o3", &candidate("写代码"), at(620));

    // 超过上限 → 应当关闭并开一个同标题的续接活动
    let events = aggregator.tick(at(700));

    assert_eq!(closed_count(&events), 1, "超长必须切分：{events:?}");
    match events.iter().find_map(|e| match e {
        AggregatorEvent::Ended { reason, at, .. } => Some((*reason, *at)),
        _ => None,
    }) {
        Some((EndReason::MaxDuration, ended_at)) => {
            assert_eq!(
                ended_at,
                at(600),
                "结束时刻必须是**切分点**，不是 tick 的当前时刻 —— \
                 否则「干到 5 分钟就离开」会被记成 10 分钟"
            );
        }
        other => panic!("结束原因应为 MaxDuration，实际 {other:?}"),
    }
    assert_eq!(started_titles(&events), vec!["写代码".to_string()]);
}

// ---------------------------------------------------------------- 3.18 空闲

#[test]
fn idle_gap_closes_activity() {
    let mut aggregator = Aggregator::new(policy());
    aggregator.observe("o1", &candidate("写代码"), at(0));

    // 空闲不足阈值
    assert!(aggregator.tick(at(100)).is_empty());

    // 超过 idle_gap（300s）
    let events = aggregator.tick(at(400));
    assert_eq!(closed_count(&events), 1);
    match events.iter().find_map(|e| match e {
        AggregatorEvent::Ended { reason, .. } => Some(*reason),
        _ => None,
    }) {
        Some(EndReason::Idle) => {}
        other => panic!("结束原因应为 Idle，实际 {other:?}"),
    }
    assert!(aggregator.open().is_none());
}

// ---------------------------------------------------------------- 3.19 锁屏

#[test]
fn lock_closes_activity_immediately() {
    let mut aggregator = Aggregator::new(policy());
    aggregator.observe("o1", &candidate("写代码"), at(0));

    let events = aggregator.pause(at(30));

    assert_eq!(closed_count(&events), 1, "锁屏必须立即结束当前活动");
    match events.iter().find_map(|e| match e {
        AggregatorEvent::Ended { reason, .. } => Some(*reason),
        _ => None,
    }) {
        Some(EndReason::Locked) => {}
        other => panic!("结束原因应为 Locked，实际 {other:?}"),
    }
    assert!(aggregator.open().is_none(), "锁屏后不应有进行中的活动");
}

#[test]
fn flush_closes_activity_on_shutdown() {
    let mut aggregator = Aggregator::new(policy());
    aggregator.observe("o1", &candidate("写代码"), at(0));

    let events = aggregator.flush(at(10));
    assert_eq!(closed_count(&events), 1);
    assert!(matches!(
        events.iter().find_map(|e| match e {
            AggregatorEvent::Ended { reason, .. } => Some(*reason),
            _ => None,
        }),
        Some(EndReason::Flushed)
    ));

    // 幂等：已经空了再 flush 不应报错或重复产出
    assert!(aggregator.flush(at(20)).is_empty());
}

// ---------------------------------------------------------------- 3.16 间隔合并

#[test]
fn brief_gap_within_merge_gap_does_not_split() {
    let mut aggregator = Aggregator::new(policy());
    aggregator.observe("o1", &candidate("写代码"), at(0));

    // 200 秒没有观测，但小于 idle_gap(300) —— 活动不该结束
    assert!(aggregator.tick(at(200)).is_empty());

    aggregator.observe("o2", &candidate("写代码"), at(250));
    assert_eq!(aggregator.open().unwrap().observations.len(), 2);
}

// ---------------------------------------------------------------- 3.21 确定性

#[test]
fn aggregation_is_deterministic() {
    let scenario: Vec<(&str, &str, i64)> = vec![
        ("o1", "写代码", 0),
        ("o2", "浏览网页", 60),
        ("o3", "写代码", 63),
        ("o4", "浏览网页", 200),
        ("o5", "浏览网页", 300),
    ];

    let run = || {
        let mut aggregator = Aggregator::new(policy());
        let mut events = Vec::new();
        for (id, title, offset) in &scenario {
            let candidate = if *title == "写代码" {
                candidate(title)
            } else {
                other_candidate(title)
            };
            events.extend(aggregator.observe(id, &candidate, at(*offset)));
        }
        events.extend(aggregator.tick(at(1000)));
        events
    };

    let first = run();
    for _ in 0..5 {
        assert_eq!(run(), first, "同样输入必须给同样事件序列（无隐藏状态）");
    }
}

// ---------------------------------------------------------------- 3.22 乱序

#[test]
fn aggregation_handles_out_of_order_arrival() {
    let mut aggregator = Aggregator::new(policy());
    aggregator.observe("o1", &candidate("写代码"), at(100));

    // 一条迟到的观测（时间早于当前活动的结束时间）
    let events = aggregator.observe("o0", &candidate("写代码"), at(50));

    let open = aggregator.open().expect("活动仍在");
    assert!(open.observations.contains(&"o0".to_string()));
    assert!(
        open.end >= at(100),
        "迟到观测不得把活动的结束时间往回拨：{:?}",
        open.end
    );
    // 迟到观测也不应产生额外的活动
    assert!(started_titles(&events).is_empty());
}

// ---------------------------------------------------------------- 边界

#[test]
fn observe_without_open_activity_starts_one() {
    let mut aggregator = Aggregator::new(policy());
    let events = aggregator.observe("o1", &candidate("写代码"), at(0));

    assert_eq!(started_titles(&events), vec!["写代码".to_string()]);
    let open = aggregator.open().unwrap();
    assert_eq!(open.start, at(0));
    assert_eq!(open.end, at(0));
    assert_eq!(open.category.as_deref(), Some("开发"));
}

#[test]
fn tick_without_activity_is_a_noop() {
    let mut aggregator = Aggregator::new(policy());
    assert!(aggregator.tick(at(10_000)).is_empty());
    assert!(aggregator.pause(at(20_000)).is_empty());
}

#[test]
fn default_policy_matches_config_defaults() {
    let policy = AggregationPolicy::default();
    assert_eq!(policy.debounce_secs, 45);
    assert_eq!(policy.min_duration_secs, 60);
    assert_eq!(policy.merge_gap_secs, 120);
    assert_eq!(policy.max_duration_secs, 7200);
    assert_eq!(policy.idle_gap_secs, 300);
}

#[test]
fn switched_activity_records_the_end_reason() {
    let mut aggregator = Aggregator::new(policy());
    let mut events = Vec::new();

    events.extend(aggregator.observe("o1", &candidate("写代码"), at(0)));
    events.extend(aggregator.observe("o2", &other_candidate("浏览网页"), at(60)));
    // 第二条同标题观测到达时，待定已持续 60s ≥ 防抖 45s → 在这里就提交了
    events.extend(aggregator.observe("o3", &other_candidate("浏览网页"), at(120)));
    events.extend(aggregator.tick(at(200)));

    assert!(
        matches!(
            events.iter().find_map(|e| match e {
                AggregatorEvent::Ended { reason, .. } => Some(*reason),
                _ => None,
            }),
            Some(EndReason::Switched)
        ),
        "切换提交后应记录 Switched 原因：{events:?}"
    );
}
