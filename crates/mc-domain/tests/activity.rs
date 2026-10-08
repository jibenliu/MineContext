//! 活动不变量与用户修正。
//!
//! 两组目标：
//! - **provenance 必须可区分**：
//!   「AI 认为发生了什么」与「用户实际上做了什么」混在一起，
//!   整个记忆系统的可信度就没了。
//! - **用户修正必须活得比重算久**：
//!   算法升级后重跑历史数据，用户的改名/分类/合并不能丢。

use mc_common::time::Timestamp;
use mc_domain::activity::{Activity, ActivityError, ObservationRef, OverrideKind, Provenance};

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn observations(offsets: &[i64]) -> Vec<ObservationRef> {
    offsets
        .iter()
        .map(|offset| ObservationRef {
            id: format!("obs-{offset}"),
            at: at(*offset),
        })
        .collect()
}

fn base(id: &str) -> Activity {
    Activity {
        id: id.to_string(),
        start: at(0),
        end: at(300),
        title: "写代码".to_string(),
        category: Some("开发".to_string()),
        // 刻意取 0/60/180/300：这样在 150s 处切分会得到 2 + 2
        observations: observations(&[0, 60, 180, 300]),
        origin: Provenance::Rule {
            rule_id: "coding".to_string(),
        },
        confidence: 0.9,
    }
}

// ---------------------------------------------------------------- 3.23–3.27

#[test]
fn evidence_is_never_empty() {
    let mut activity = base("act-1");
    activity.observations.clear();

    assert_eq!(
        activity.validate(),
        Err(ActivityError::MissingEvidence),
        "没有证据的活动等于凭空断言"
    );
}

// 3.26b
#[test]
fn evidence_must_lie_inside_the_activity_window() {
    let mut activity = base("act-1");
    // 观测在 300s，窗口却只到 120s —— 这条观测就成了孤儿：
    // 界面上点它选不中任何活动，合并/切分也会把它算漏。
    activity.end = at(120);

    assert_eq!(
        activity.validate(),
        Err(ActivityError::EvidenceOutsideRange),
        "观测落不到活动窗口里，说明活动窗口和证据对不上"
    );

    let mut activity = base("act-1");
    activity.start = at(60); // 最早一条观测在 0s
    assert_eq!(
        activity.validate(),
        Err(ActivityError::EvidenceOutsideRange)
    );

    assert_eq!(base("act-1").validate(), Ok(()));
}

#[test]
fn inferred_activity_must_carry_model_and_confidence() {
    let mut activity = base("act-1");
    activity.origin = Provenance::Inferred {
        model: String::new(),
    };
    assert_eq!(
        activity.validate(),
        Err(ActivityError::InferredWithoutModel)
    );

    let mut activity = base("act-1");
    activity.origin = Provenance::Inferred {
        model: "qwen3-vl".to_string(),
    };
    activity.confidence = 0.0;
    assert_eq!(
        activity.validate(),
        Err(ActivityError::InferredWithoutConfidence),
        "推测出来的活动必须带置信度，否则 UI 无法标注「这是猜的」"
    );

    let mut ok = base("act-1");
    ok.origin = Provenance::Inferred {
        model: "qwen3-vl".to_string(),
    };
    ok.confidence = 0.72;
    assert_eq!(ok.validate(), Ok(()));
}

#[test]
fn rule_activity_references_the_rule_id() {
    let mut activity = base("act-1");
    activity.origin = Provenance::Rule {
        rule_id: String::new(),
    };
    assert_eq!(activity.validate(), Err(ActivityError::RuleWithoutRuleId));

    // 有 rule_id 就合法
    assert_eq!(base("act-1").validate(), Ok(()));
}

#[test]
fn observed_activity_requires_no_model() {
    let mut activity = base("act-1");
    activity.origin = Provenance::Observed;
    // 直接把窗口标题当活动名，没有任何模型参与
    activity.title = "Google Chrome".to_string();
    activity.confidence = 0.0;

    assert_eq!(
        activity.validate(),
        Ok(()),
        "纯元数据活动不需要模型与置信度"
    );
}

#[test]
fn end_before_start_is_rejected() {
    let mut activity = base("act-1");
    activity.end = at(-10);

    assert_eq!(activity.validate(), Err(ActivityError::EndBeforeStart));
}

#[test]
fn serialization_keeps_origins_distinguishable() {
    let observed = Activity {
        origin: Provenance::Observed,
        ..base("a")
    };
    let ruled = base("b");
    let inferred = Activity {
        origin: Provenance::Inferred {
            model: "qwen3-vl".to_string(),
        },
        confidence: 0.6,
        ..base("c")
    };

    let json = serde_json::to_value([&observed, &ruled, &inferred]).unwrap();

    assert_eq!(json[0]["origin"]["kind"], "observed");
    assert_eq!(json[1]["origin"]["kind"], "rule");
    assert_eq!(json[1]["origin"]["rule_id"], "coding");
    assert_eq!(json[2]["origin"]["kind"], "inferred");
    assert_eq!(json[2]["origin"]["model"], "qwen3-vl");

    // UI 能据此把「推测」标出来
    assert!(inferred.is_inferred());
    assert!(!ruled.is_inferred());
}

// ---------------------------------------------------------------- 3.28–3.31

#[test]
fn user_can_rename_activity() {
    let activities = vec![base("act-1")];
    let overrides = vec![OverrideKind::Rename {
        activity_id: "act-1".to_string(),
        title: "排查 APEX-389".to_string(),
        at: at(400),
    }];

    let views = mc_domain::activity::apply_overrides(&activities, &overrides);

    assert_eq!(views.len(), 1);
    assert_eq!(views[0].title, "排查 APEX-389");
    assert!(views[0].is_user_modified, "必须标记为「用户改过」");
    assert_eq!(views[0].original_title, "写代码");
}

#[test]
fn user_can_set_category() {
    let activities = vec![base("act-1")];
    let overrides = vec![OverrideKind::SetCategory {
        activity_id: "act-1".to_string(),
        category: Some("需求分析".to_string()),
        at: at(400),
    }];

    let views = mc_domain::activity::apply_overrides(&activities, &overrides);

    assert_eq!(views[0].category.as_deref(), Some("需求分析"));
    assert!(views[0].is_user_modified);
}

// 3.30b
#[test]
fn merge_survives_absorbed_activities_sitting_before_the_primary() {
    // 被吸收的活动排在主活动**之前**：每次移除都会让主活动的下标前移，
    // 若沿用第一次算出的下标，第二次就会搬运错对象（或越界 panic）。
    let mut earlier_a = base("act-a");
    earlier_a.observations = observations(&[0]);
    earlier_a.start = at(0);
    earlier_a.end = at(0);

    let mut earlier_b = base("act-b");
    earlier_b.observations = observations(&[60]);
    earlier_b.start = at(60);
    earlier_b.end = at(60);

    let primary = base("act-1");
    let activities = vec![earlier_a, earlier_b, primary];

    let overrides = vec![OverrideKind::Merge {
        primary: "act-1".to_string(),
        absorbed: vec!["act-a".to_string(), "act-b".to_string()],
        at: at(600),
    }];

    let views = mc_domain::activity::apply_overrides(&activities, &overrides);

    assert_eq!(views.len(), 1, "两个活动都该被吸收");
    assert_eq!(views[0].id, "act-1", "活下来必须是主活动，不能搬错");
    assert_eq!(views[0].observations.len(), 6, "证据一条都不能丢");
    assert_eq!(views[0].start, at(0), "合并后要覆盖最早的证据");
    assert_eq!(views[0].end, at(300), "合并后要覆盖最晚的证据");
}

#[test]
fn user_can_merge_two_activities() {
    let mut second = base("act-2");
    second.title = "看文档".to_string();
    second.observations = observations(&[400, 460]);
    // 窗口必须跟着证据走，否则 validate() 会拒绝
    second.start = at(400);
    second.end = at(460);

    let activities = vec![base("act-1"), second];
    let overrides = vec![OverrideKind::Merge {
        primary: "act-1".to_string(),
        absorbed: vec!["act-2".to_string()],
        at: at(600),
    }];

    let views = mc_domain::activity::apply_overrides(&activities, &overrides);

    assert_eq!(views.len(), 1, "被合并掉的活动不应再单独出现");
    assert_eq!(views[0].id, "act-1");
    assert_eq!(
        views[0].observations.len(),
        6,
        "被吸收活动的观测必须归到主活动上，否则观测成了孤儿"
    );
    assert_eq!(views[0].end, at(460), "合并后时间范围应当扩展");
}

#[test]
fn user_can_split_activity() {
    let activities = vec![base("act-1")];
    // 在 150 秒处切开
    let overrides = vec![OverrideKind::Split {
        activity_id: "act-1".to_string(),
        at: at(150),
        tail_title: "转向其它任务".to_string(),
    }];

    let views = mc_domain::activity::apply_overrides(&activities, &overrides);

    assert_eq!(views.len(), 2, "切分后应当有两个活动");

    let head = views.iter().find(|v| v.id == "act-1").unwrap();
    let tail = views.iter().find(|v| v.id != "act-1").unwrap();

    assert_eq!(head.observations.len(), 2, "150s 之前的两条观测");
    assert_eq!(tail.observations.len(), 2, "150s 之后的两条观测");
    assert_eq!(tail.title, "转向其它任务");
    assert!(tail.start >= at(150));
    assert!(head.end <= at(120));
}

// ---------------------------------------------------------------- 3.32–3.34

#[test]
fn user_override_survives_replay() {
    let overrides = vec![
        OverrideKind::Rename {
            activity_id: "act-1".to_string(),
            title: "排查 APEX-389".to_string(),
            at: at(400),
        },
        OverrideKind::SetCategory {
            activity_id: "act-1".to_string(),
            category: Some("排查".to_string()),
            at: at(401),
        },
    ];

    // 第一次：基于原始事件算出的活动
    let first = mc_domain::activity::apply_overrides(&[base("act-1")], &overrides);

    // 「重算」：同样的输入重新得出同一批活动（活动本身不携带用户修正）
    let replayed = mc_domain::activity::apply_overrides(&[base("act-1")], &overrides);

    assert_eq!(first, replayed, "重放后用户修正必须仍然生效且结果一致");

    // 关键：用户修正没有写进 Activity 本身，因此重算不会把它抹掉
    let underlying = base("act-1");
    assert_eq!(underlying.title, "写代码", "底层活动保持算法产出");
    assert_eq!(underlying.category.as_deref(), Some("开发"));
}

#[test]
fn user_override_is_serializable_as_an_event() {
    let kinds = vec![
        OverrideKind::Rename {
            activity_id: "a".to_string(),
            title: "t".to_string(),
            at: at(1),
        },
        OverrideKind::SetCategory {
            activity_id: "a".to_string(),
            category: None,
            at: at(2),
        },
        OverrideKind::Merge {
            primary: "a".to_string(),
            absorbed: vec!["b".to_string()],
            at: at(3),
        },
        OverrideKind::Split {
            activity_id: "a".to_string(),
            at: at(4),
            tail_title: "尾".to_string(),
        },
    ];

    let json = serde_json::to_value(&kinds).unwrap();
    let tags: Vec<&str> = json
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value["kind"].as_str().unwrap())
        .collect();

    assert_eq!(tags, vec!["rename", "set_category", "merge", "split"]);

    // 必须能往返（它要作为事件持久化，并参与重放）
    let roundtrip: Vec<OverrideKind> = serde_json::from_value(json).unwrap();
    assert_eq!(roundtrip, kinds);
}

#[test]
fn user_override_wins_over_ai_suggestion() {
    let overrides = vec![OverrideKind::Rename {
        activity_id: "act-1".to_string(),
        title: "排查 APEX-389".to_string(),
        at: at(400),
    }];

    // 算法后来又给出一个新标题（例如换了模型重算）
    let mut reworked = base("act-1");
    reworked.title = "浏览网页（模型推断）".to_string();
    reworked.origin = Provenance::Inferred {
        model: "qwen3-vl".to_string(),
    };
    reworked.confidence = 0.55;

    let views = mc_domain::activity::apply_overrides(&[reworked], &overrides);

    assert_eq!(
        views[0].title, "排查 APEX-389",
        "用户的手动命名必须优先于模型建议"
    );
    assert_eq!(views[0].original_title, "浏览网页（模型推断）");
    assert!(
        views[0].origin.is_inferred(),
        "但来源信息仍要保留，UI 才能说明「原始判断来自模型」"
    );
}

// ---------------------------------------------------------------- 边界

#[test]
fn overrides_for_unknown_activity_are_ignored_not_fatal() {
    let overrides = vec![OverrideKind::Rename {
        activity_id: "does-not-exist".to_string(),
        title: "x".to_string(),
        at: at(1),
    }];

    let views = mc_domain::activity::apply_overrides(&[base("act-1")], &overrides);

    assert_eq!(views.len(), 1);
    assert_eq!(views[0].title, "写代码", "无关的修正不应影响别的活动");
    assert!(!views[0].is_user_modified);
}

#[test]
fn applying_overrides_does_not_mutate_input() {
    let activities = vec![base("act-1")];
    let snapshot = activities.clone();

    let _ = mc_domain::activity::apply_overrides(
        &activities,
        &[OverrideKind::Rename {
            activity_id: "act-1".to_string(),
            title: "改名".to_string(),
            at: at(1),
        }],
    );

    assert_eq!(activities, snapshot, "覆盖必须是纯函数，不得改动输入");
}

#[test]
fn later_override_wins_over_earlier_one() {
    let overrides = vec![
        OverrideKind::Rename {
            activity_id: "act-1".to_string(),
            title: "第一次改".to_string(),
            at: at(10),
        },
        OverrideKind::Rename {
            activity_id: "act-1".to_string(),
            title: "第二次改".to_string(),
            at: at(20),
        },
    ];

    let views = mc_domain::activity::apply_overrides(&[base("act-1")], &overrides);
    assert_eq!(views[0].title, "第二次改", "后发生的修正优先");
}

#[test]
fn applying_overrides_is_idempotent() {
    let activities = vec![base("act-1")];
    let overrides = vec![OverrideKind::Rename {
        activity_id: "act-1".to_string(),
        title: "改名".to_string(),
        at: at(1),
    }];

    let once = mc_domain::activity::apply_overrides(&activities, &overrides);
    let twice = mc_domain::activity::apply_overrides(&activities, &overrides);

    assert_eq!(once, twice);
}
