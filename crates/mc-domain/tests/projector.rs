//! 投影器与重放。
//!
//! 投影器是整个重构里**唯一**把「事件」变成「活动」的地方。
//! 之所以强调纯函数 + 确定性：
//!
//! - 算法升级后要重跑历史，两次重放必须逐字节一致，
//!   否则用户每次升级都会看到时间线「自己变了」；
//! - 实时写入与重放重建必须走同一段代码（3.39），
//!   否则线上数据与重放数据会悄悄分叉，排查时无从下手。

use mc_common::time::Timestamp;
use mc_domain::activity::{Activity, OverrideKind, Provenance};
use mc_domain::observation::ObservationSummary;
use mc_domain::projector::{project, DomainEvent, ProjectionOptions, Projector};
use mc_domain::rules::RuleSet;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn observation(id: &str, at_secs: i64, app: &str, title: &str) -> DomainEvent {
    DomainEvent::ObservationRecorded {
        observation: ObservationSummary {
            id: id.to_string(),
            at: at(at_secs),
            app_name: Some(app.to_string()),
            window_title: Some(title.to_string()),
            domain: None,
            text: None,
        },
    }
}

/// 只认 VSCode 的规则：命中就用规则的名字，没命中的靠进程名兜底。
fn rules() -> RuleSet {
    RuleSet::parse_yaml(
        r#"
version: 1
activities:
  - id: coding
    name: 写代码
    category: 开发
    triggers:
      apps: [VSCode]
"#,
    )
    .expect("规则文件应当能解析")
}

fn options() -> ProjectionOptions {
    ProjectionOptions::default()
}

fn titles(activities: &[mc_domain::activity::ActivityView]) -> Vec<String> {
    activities
        .iter()
        .map(|activity| activity.title.clone())
        .collect()
}

#[test]
fn projection_is_deterministic() {
    let events = vec![
        observation("obs-1", 0, "VSCode", "main.rs"),
        observation("obs-2", 60, "VSCode", "main.rs"),
        observation("obs-3", 600, "Chrome", "Hacker News"),
        observation("obs-4", 660, "Chrome", "Hacker News"),
    ];

    let first = project(&events, &rules(), options());
    let second = project(&events, &rules(), options());

    assert_eq!(
        serde_json::to_string(&first.activities).unwrap(),
        serde_json::to_string(&second.activities).unwrap(),
        "两次重放必须逐字节一致，否则用户每次升级时间线都会「自己变」"
    );
    assert_eq!(titles(&first.activities), vec!["写代码", "Chrome"]);
}

#[test]
fn live_projection_equals_replay_projection() {
    let events = vec![
        observation("obs-1", 0, "VSCode", "main.rs"),
        observation("obs-2", 60, "VSCode", "main.rs"),
        observation("obs-3", 600, "Chrome", "Hacker News"),
        observation("obs-4", 660, "Chrome", "Hacker News"),
    ];

    // 实时：一条一条吃进去
    let rule_set = rules();
    let mut live = Projector::new(&rule_set, options());
    for event in &events {
        live.apply(event);
    }
    let live = live.finish();

    // 重放：一次性重建
    let replay = project(&events, &rule_set, options());

    assert_eq!(
        serde_json::to_string(&live.activities).unwrap(),
        serde_json::to_string(&replay.activities).unwrap(),
        "实时写入与重放重建必须是同一段代码，否则两者会悄悄分叉"
    );
}

#[test]
fn user_override_survives_replay() {
    let events = vec![
        observation("obs-1", 0, "VSCode", "main.rs"),
        observation("obs-2", 60, "VSCode", "main.rs"),
        // 用户改名的修正**也是事件**，因此重放时会重新生效
        DomainEvent::ActivityOverridden {
            override_: OverrideKind::Rename {
                activity_id: "act-obs-1".to_string(),
                title: "重构活动引擎".to_string(),
                at: at(120),
            },
        },
    ];

    let projection = project(&events, &rules(), options());

    assert_eq!(projection.activities.len(), 1);
    assert_eq!(
        projection.activities[0].title, "重构活动引擎",
        "算法升级重算后，用户的改名必须还在"
    );
    assert_eq!(
        projection.activities[0].original_title, "写代码",
        "同时要保留算法原本的标题，UI 才能解释「为什么是它」"
    );
    assert!(projection.activities[0].is_user_modified);
}

#[test]
fn activity_id_is_derived_from_evidence_not_from_order() {
    let first_events = vec![
        observation("obs-1", 0, "VSCode", "main.rs"),
        observation("obs-2", 60, "VSCode", "main.rs"),
    ];

    // 同一段历史，前面插入了别的东西 —— 第一个活动的 id 不能跟着漂
    let shifted_events = vec![
        observation("obs-9", 0, "Terminal", "cargo build"),
        observation("obs-1", 60, "VSCode", "main.rs"),
        observation("obs-2", 120, "VSCode", "main.rs"),
    ];

    let first = project(&first_events, &rules(), options());
    let shifted = project(&shifted_events, &rules(), options());

    assert_eq!(first.activities[0].id, "act-obs-1");
    assert_eq!(
        shifted.activities[1].id, "act-obs-1",
        "活动 id 必须由证据决定：前端和用户修正都拿着这个 id，漂了就全指错"
    );
}

#[test]
fn replay_from_zero_rebuilds_the_same_activities() {
    let events = vec![
        observation("obs-1", 0, "VSCode", "main.rs"),
        observation("obs-2", 60, "VSCode", "main.rs"),
        observation("obs-3", 600, "Chrome", "Hacker News"),
        observation("obs-4", 660, "Chrome", "Hacker News"),
    ];

    let first = project(&events, &rules(), options());
    let rebuilt = project(&events, &rules(), options());

    let ids: Vec<&str> = first.activities.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(
        ids,
        rebuilt
            .activities
            .iter()
            .map(|a| a.id.as_str())
            .collect::<Vec<_>>()
    );
    // 全量重建要覆盖到每一条观测
    let mut observed: Vec<String> = first
        .activities
        .iter()
        .flat_map(|a| a.observation_ids())
        .collect();
    observed.sort();
    assert_eq!(observed, vec!["obs-1", "obs-2", "obs-3", "obs-4"]);
}

// 3.20（投影视角）
#[test]
fn every_observation_is_accounted_for() {
    let events = vec![
        observation("obs-1", 0, "VSCode", "main.rs"),
        // 3 秒的切走 —— 应当被当作噪声丢弃，但必须被**记录**为噪声
        observation("obs-2", 3, "Chrome", "Hacker News"),
        observation("obs-3", 6, "VSCode", "main.rs"),
        observation("obs-4", 60, "VSCode", "main.rs"),
    ];

    let projection = project(&events, &rules(), options());

    let mut owned: Vec<String> = projection
        .activities
        .iter()
        .flat_map(|a| a.observation_ids())
        .collect();
    owned.extend(projection.noise.iter().cloned());
    owned.sort();

    assert_eq!(
        owned,
        vec!["obs-1", "obs-2", "obs-3", "obs-4"],
        "每条观测要么属于某个活动，要么被显式标成噪声，不能凭空消失"
    );
    assert_eq!(
        projection.activities.len(),
        1,
        "经典防抖用例：切走 3 秒不算新活动"
    );
}

#[test]
fn no_rule_match_falls_back_to_observed_without_ai() {
    let events = vec![
        observation("obs-1", 0, "iTerm2", "cargo test"),
        observation("obs-2", 60, "iTerm2", "cargo test"),
    ];

    let projection = project(&events, &rules(), options());

    assert_eq!(projection.activities.len(), 1);
    assert_eq!(
        projection.activities[0].origin,
        Provenance::Observed,
        "规则缺失时系统仍要产出活动，只是标注为「只看见、没推断」"
    );
    assert_eq!(
        projection.activities[0].title, "iTerm2",
        "兜底标题用进程名：窗口标题一直在变，用它会碎成一片"
    );
}

#[test]
fn unmatched_observation_requests_no_deep_ai_call() {
    let events = vec![observation("obs-1", 0, "iTerm2", "cargo test")];

    // 没有预算时的降级路径：即使想调模型也不许调
    let projection = project(
        &events,
        &rules(),
        ProjectionOptions {
            allow_inference: false,
            ..options()
        },
    );

    assert_eq!(projection.activities.len(), 1);
    assert_eq!(
        projection.ai_requests, 0,
        "没有预算就一次模型调用都不该发生"
    );
    assert_eq!(projection.activities[0].origin, Provenance::Observed);
}

// 前向兼容：不认识的事件不能把重放搞崩
#[test]
fn unknown_event_kinds_are_skipped_and_counted() {
    let events = vec![
        observation("obs-1", 0, "VSCode", "main.rs"),
        DomainEvent::Unknown {
            kind: "future.thing".to_string(),
        },
    ];

    let projection = project(&events, &rules(), options());

    assert_eq!(projection.activities.len(), 1);
    assert_eq!(
        projection.unknown_event_kinds, 1,
        "跳过要计数，否则新旧版本混跑查不出问题"
    );
}

// 抗乱序：重放时事件可能按到达顺序而非发生顺序排列
#[test]
fn out_of_order_observations_do_not_produce_orphans() {
    let events = vec![
        observation("obs-2", 60, "VSCode", "main.rs"),
        observation("obs-1", 0, "VSCode", "main.rs"),
    ];

    let projection = project(&events, &rules(), options());

    let mut owned: Vec<String> = projection
        .activities
        .iter()
        .flat_map(|a| a.observation_ids())
        .collect();
    owned.sort();
    assert_eq!(owned, vec!["obs-1", "obs-2"]);
}

#[test]
fn user_override_wins_over_ai_suggestion() {
    let events = vec![
        observation("obs-1", 0, "VSCode", "main.rs"),
        observation("obs-2", 60, "VSCode", "main.rs"),
        DomainEvent::ActivityOverridden {
            override_: OverrideKind::SetCategory {
                activity_id: "act-obs-1".to_string(),
                category: Some("需求分析".to_string()),
                at: at(120),
            },
        },
    ];

    let projection = project(&events, &rules(), options());

    assert_eq!(
        projection.activities[0].category.as_deref(),
        Some("需求分析"),
        "用户设定的分类必须压过规则给的分类"
    );
}

// 空输入不能 panic
#[test]
fn projecting_nothing_yields_nothing() {
    let projection = project(&[], &rules(), options());
    assert!(projection.activities.is_empty());
    assert!(projection.noise.is_empty());
}

// 每个活动都要能过不变量校验（否则写进库里就是脏数据）
#[test]
fn projected_activities_satisfy_domain_invariants() {
    let events = vec![
        observation("obs-1", 0, "VSCode", "main.rs"),
        observation("obs-2", 60, "VSCode", "main.rs"),
        observation("obs-3", 600, "Chrome", "Hacker News"),
        observation("obs-4", 660, "Chrome", "Hacker News"),
        observation("obs-5", 9000, "iTerm2", "cargo test"),
    ];

    let projection = project(&events, &rules(), options());

    for view in &projection.activities {
        let activity = Activity {
            id: view.id.clone(),
            start: view.start,
            end: view.end,
            title: view.title.clone(),
            category: view.category.clone(),
            observations: view.observations.clone(),
            origin: view.origin.clone(),
            confidence: view.confidence,
        };
        assert_eq!(
            activity.validate(),
            Ok(()),
            "活动 {id} 不满足不变量",
            id = view.id
        );
    }
}

// 重放时也必须执行「超长强制切分」与「空闲结束」：
// 这两条只在聚合器的 tick 里判定，而重放如果从不 tick，
// 一段跨夜的观测会被算成「连续工作了 9 小时」。
#[test]
fn replay_force_splits_activities_longer_than_max_duration() {
    let events: Vec<DomainEvent> = (0..37)
        .map(|step| observation(&format!("obs-{step}"), step * 300, "VSCode", "main.rs"))
        .collect();

    let projection = project(&events, &rules(), options());

    assert!(
        projection.activities.len() >= 3,
        "5 小时连续观测、单活动上限 2 小时，至少该切成 3 段，实际 {}",
        projection.activities.len()
    );

    for view in &projection.activities {
        let span = view.end.as_millis() - view.start.as_millis();
        assert!(
            span <= 7_200_000,
            "活动 {id} 时长 {span}ms 超过上限",
            id = view.id
        );
    }
}

#[test]
fn replay_closes_activity_across_an_idle_gap() {
    let events = vec![
        observation("obs-1", 0, "VSCode", "main.rs"),
        observation("obs-2", 60, "VSCode", "main.rs"),
        // 中间空了 9 小时：这是两次不同的工作，不是一次 9 小时的活动
        observation("obs-3", 32_460, "VSCode", "main.rs"),
        observation("obs-4", 32_520, "VSCode", "main.rs"),
    ];

    let projection = project(&events, &rules(), options());

    assert_eq!(
        projection.activities.len(),
        2,
        "空闲超过 idle_gap 必须断开，否则用户看到的是「工作了一整天」"
    );
}

// ---------------------------------------------------------------- 3.43–3.45 AI 推断落地

fn inferred(activity_id: &str, title: &str, confidence: f32) -> DomainEvent {
    DomainEvent::ActivityInferred {
        suggestion: mc_domain::projector::InferredSuggestion {
            activity_id: activity_id.to_string(),
            title: title.to_string(),
            category: Some("需求".to_string()),
            confidence,
            model: "openai_compatible:qwen3-vl".to_string(),
        },
    }
}

// 规则判不了的时段，模型给出结论后要能落到活动上
#[test]
fn inferred_event_upgrades_an_observed_activity() {
    let events = vec![
        observation("obs-1", 0, "Aurora", "unknown window"),
        observation("obs-2", 60, "Aurora", "unknown window"),
        inferred("act-obs-1", "架构评审", 0.82),
    ];

    let projection = project(&events, &rules(), options());

    assert_eq!(projection.activities.len(), 1);
    let activity = &projection.activities[0];
    assert_eq!(activity.title, "架构评审");
    assert_eq!(activity.category.as_deref(), Some("需求"));
    assert!((activity.confidence - 0.82).abs() < 1e-6);
    assert_eq!(
        activity.origin,
        Provenance::Inferred {
            model: "openai_compatible:qwen3-vl".to_string()
        },
        "推断出来的活动必须标注 provenance，UI 才能显示「这是猜的」"
    );
    assert!(!activity.is_user_modified, "模型推断不是用户改的");
}

// 指向不存在的活动（例如活动已被重算掉）不能把重放搞崩
#[test]
fn inferred_event_for_unknown_activity_is_ignored() {
    let events = vec![
        observation("obs-1", 0, "Aurora", "unknown window"),
        observation("obs-2", 60, "Aurora", "unknown window"),
        inferred("act-does-not-exist", "架构评审", 0.9),
    ];

    let projection = project(&events, &rules(), options());

    assert_eq!(projection.activities.len(), 1);
    assert_eq!(
        projection.activities[0].origin,
        Provenance::Observed,
        "没有对应活动时不该凭空造一个结论"
    );
    assert_eq!(projection.activities[0].title, "Aurora");
}

// 规则命中是确定性的，不能被模型的猜测覆盖（3.34 的 AI 侧）
#[test]
fn inferred_event_does_not_overwrite_a_rule_hit() {
    let events = vec![
        observation("obs-1", 0, "VSCode", "main.rs"),
        observation("obs-2", 60, "VSCode", "main.rs"),
        inferred("act-obs-1", "模型说这是写文档", 0.99),
    ];

    let projection = project(&events, &rules(), options());

    assert_eq!(
        projection.activities[0].title, "写代码",
        "规则命中优先于模型推断：规则是用户自己定的，模型只是在猜"
    );
    assert_eq!(
        projection.activities[0].origin,
        Provenance::Rule {
            rule_id: "coding".to_string()
        }
    );
}

// 用户修正 > 模型推断
#[test]
fn user_rename_beats_the_ai_suggestion() {
    let events = vec![
        observation("obs-1", 0, "Aurora", "unknown window"),
        observation("obs-2", 60, "Aurora", "unknown window"),
        inferred("act-obs-1", "架构评审", 0.82),
        DomainEvent::ActivityOverridden {
            override_: OverrideKind::Rename {
                activity_id: "act-obs-1".to_string(),
                title: "我自己起的名字".to_string(),
                at: at(120),
            },
        },
    ];

    let projection = project(&events, &rules(), options());

    assert_eq!(projection.activities[0].title, "我自己起的名字");
    assert_eq!(
        projection.activities[0].original_title, "架构评审",
        "算法结论应当保留模型给出的标题，而不是被抹掉"
    );
    assert!(projection.activities[0].is_user_modified);
}

// 推断事件必须能经事件日志往返（它和用户修正一样是事件）
#[test]
fn inferred_event_round_trips_through_storage_shape() {
    let event = inferred("act-obs-1", "架构评审", 0.82);
    let payload = event.payload();
    let restored = DomainEvent::from_stored(event.kind(), at(0), &payload);

    assert_eq!(restored, event, "payload/from_stored 必须对称");
    assert_eq!(event.kind(), "activity.inferred");
}
