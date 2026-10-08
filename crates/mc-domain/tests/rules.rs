//! 用户自定义活动规则。
//!
//! 这是「规则 + 行为 + AI 三者共同判断」里**规则**的那一半
//! 规则命中就不必调模型 —— 这是省 token 的主要手段，
//! 也是让用户真正掌控「系统认为我在干什么」的唯一途径。
//!
//! 全部是纯函数：不读文件、不连数据库、不调模型。

use mc_common::time::Timestamp;
use mc_domain::observation::ObservationSummary;
use mc_domain::rules::{AiPreference, RuleMatch, RuleSet};
use mc_testkit::fixtures::FIXTURE_EPOCH_MS;

fn obs(app: Option<&str>, title: Option<&str>, text: Option<&str>) -> ObservationSummary {
    ObservationSummary {
        id: "obs-1".to_string(),
        at: Timestamp::from_millis(FIXTURE_EPOCH_MS),
        app_name: app.map(str::to_string),
        window_title: title.map(str::to_string),
        domain: None,
        text: text.map(str::to_string),
    }
}

const RULES: &str = r#"
version: 1
activities:
  - id: coding
    name: "写代码"
    category: "开发"
    priority: 10
    ai: none
    minimum_duration_secs: 120
    triggers:
      apps: ["VSCode", "GoLand", "RustRover"]
      keywords: ["impl ", "fn "]
    exclude:
      apps: ["Terminal"]

  - id: requirement_analysis
    name: "需求分析"
    category: "需求"
    priority: 20
    ai: assist
    triggers:
      apps: ["Chrome", "Safari"]
      domains: ["jira.company.com", "confluence.company.com"]
      keywords: ["需求", "APEX"]

  - id: terminal_debug
    name: "终端调试"
    category: "排查"
    priority: 5
    ai: deep
    triggers:
      apps: ["Terminal", "iTerm2"]
"#;

fn rules() -> RuleSet {
    RuleSet::parse_yaml(RULES).expect("规则必须能解析")
}

// ---------------------------------------------------------------- 3.1 应用名

#[test]
fn app_rule_matches_by_name() {
    let matched = rules().classify(&obs(Some("VSCode"), Some("main.rs"), Some("impl Foo")));

    match matched {
        RuleMatch::Matched { rule_id, name, .. } => {
            assert_eq!(rule_id, "coding");
            assert_eq!(name, "写代码");
        }
        other => panic!("应当命中 coding 规则，实际 {other:?}"),
    }
}

#[test]
fn matching_is_case_insensitive_for_apps() {
    for app in ["VSCode", "vscode", "VSCODE"] {
        assert!(
            rules()
                .classify(&obs(Some(app), Some("x.rs"), Some("impl A")))
                .is_matched(),
            "{app} 应当命中（应用名大小写不应敏感）"
        );
    }
}

// ---------------------------------------------------------------- 3.2 窗口标题

#[test]
fn window_title_rule_matches_regex() {
    let yaml = r#"
version: 1
activities:
  - id: rust_edit
    name: "改 Rust"
    triggers:
      window_patterns: ['\.rs\b']
"#;
    let set = RuleSet::parse_yaml(yaml).unwrap();

    assert!(set
        .classify(&obs(None, Some("main.rs — project"), None))
        .is_matched());
    assert!(!set
        .classify(&obs(None, Some("notes.md — project"), None))
        .is_matched());
}

// ---------------------------------------------------------------- 3.3 域名

#[test]
fn domain_rule_matches_window_title_or_explicit_domain() {
    let set = rules();

    // 域名出现在窗口标题里（浏览器常见形态）
    assert!(set
        .classify(&obs(
            Some("Chrome"),
            Some("APEX-389 - jira.company.com"),
            Some("需求")
        ))
        .is_matched());

    // 也支持结构化提供的 domain 字段
    let mut with_domain = obs(Some("Chrome"), Some("APEX-389"), Some("需求"));
    with_domain.domain = Some("jira.company.com".to_string());
    assert!(set.classify(&with_domain).is_matched());
}

#[test]
fn domain_rule_does_not_match_unrelated_domain() {
    let matched = rules().classify(&obs(Some("Chrome"), Some("Hacker News"), Some("需求")));
    assert!(
        !matched.is_matched(),
        "不在白名单域名上不应命中：{matched:?}"
    );
}

// ---------------------------------------------------------------- 3.4 关键词

#[test]
fn keyword_rule_matches_text() {
    // 只用 keywords 的规则（避免 domains 类别造成干扰）
    let yaml = r#"
version: 1
activities:
  - id: keywords_only
    name: "关键词命中"
    triggers:
      keywords: ["APEX", "需求"]
"#;
    let set = RuleSet::parse_yaml(yaml).unwrap();

    // 关键词出现在窗口标题里
    assert!(set
        .classify(&obs(None, Some("APEX 需求评审"), None))
        .is_matched());

    // 关键词出现在 OCR 文本里
    assert!(set
        .classify(&obs(None, Some("Jira"), Some("这是一个 APEX 项目的页面")))
        .is_matched());

    // 两处都没有 → 不命中
    assert!(!set
        .classify(&obs(None, Some("首页"), Some("没有关键词")))
        .is_matched());
}

/// 跨类别是 AND：指定了 domains 就必须命中。
///
/// 这条也顺手记录了一个**真实的局限**：浏览器窗口标题通常不含域名
/// （`APEX-389 - Jira` 而不是 `... - jira.company.com`），
/// 因此依赖 domains 的规则需要观测里带上结构化的 `domain` 字段。
#[test]
fn specified_domain_category_is_required() {
    let set = rules();
    let mut with_domain = obs(Some("Chrome"), Some("APEX-389 - Jira"), Some("需求"));
    with_domain.domain = Some("jira.company.com".to_string());

    assert!(
        !set.classify(&obs(Some("Chrome"), Some("APEX-389 - Jira"), Some("需求")))
            .is_matched(),
        "指定了 domains 却没有任何域名信息时不应命中（否则规则形同虚设）"
    );
    assert!(set.classify(&with_domain).is_matched());
}

#[test]
fn all_trigger_categories_must_match_but_any_value_within_one() {
    let set = rules();

    // apps 命中但 domains 不命中 → 整条规则不命中（跨类别是 AND）
    assert!(!set
        .classify(&obs(Some("Chrome"), Some("example.com"), Some("需求")))
        .is_matched());

    // domains 命中但 keywords 不命中 → 同样不命中
    assert!(!set
        .classify(&obs(
            Some("Safari"),
            Some("jira.company.com"),
            Some("随便什么")
        ))
        .is_matched());

    // 三类都命中 → 命中
    assert!(set
        .classify(&obs(
            Some("Safari"),
            Some("jira.company.com"),
            Some("APEX-389 需求")
        ))
        .is_matched());

    // 类别内任一项即可
    assert!(set
        .classify(&obs(
            Some("Chrome"),
            Some("confluence.company.com"),
            Some("需求")
        ))
        .is_matched());
}

// ---------------------------------------------------------------- 3.5 排除

#[test]
fn negative_rule_has_highest_priority() {
    // 构造一个「如果没有 exclude 就一定会命中」的场景：
    // coding 的 keywords 能匹配 "impl "，且它的优先级(10) 高于 terminal_debug(5)。
    // 只有 exclude 生效，terminal_debug 才可能胜出。
    let yaml = r#"
version: 1
activities:
  - id: coding
    name: "写代码"
    priority: 10
    triggers:
      keywords: ["impl ", "fn "]
    exclude:
      apps: ["Terminal"]
  - id: terminal_debug
    name: "终端调试"
    priority: 5
    triggers:
      apps: ["Terminal"]
"#;
    let set = RuleSet::parse_yaml(yaml).unwrap();

    // 先确认没有 exclude 时 coding 会赢（否则这条测试是空转）
    let mut without_exclude: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
    without_exclude["activities"][0]
        .as_mapping_mut()
        .unwrap()
        .remove(serde_yaml::Value::String("exclude".to_string()));
    let control = RuleSet::parse_yaml(&serde_yaml::to_string(&without_exclude).unwrap()).unwrap();
    assert_eq!(
        control
            .classify(&obs(Some("Terminal"), Some("cargo build"), Some("impl A")))
            .rule_id(),
        Some("coding"),
        "对照组：没有 exclude 时 coding 必须命中（证明本用例不是空转）"
    );

    // 有 exclude 时，被排除的规则不得命中 —— 即使它优先级更高
    match set.classify(&obs(Some("Terminal"), Some("cargo build"), Some("impl A"))) {
        RuleMatch::Matched { rule_id, .. } => assert_eq!(rule_id, "terminal_debug"),
        other => panic!("应当命中 terminal_debug，实际 {other:?}"),
    }
}

#[test]
fn excluded_app_alone_yields_no_match() {
    let yaml = r#"
version: 1
activities:
  - id: coding
    name: "写代码"
    triggers:
      apps: ["VSCode"]
    exclude:
      apps: ["VSCode"]
"#;
    let set = RuleSet::parse_yaml(yaml).unwrap();
    assert!(
        !set.classify(&obs(Some("VSCode"), None, None)).is_matched(),
        "排除条件优先级最高"
    );
}

// ---------------------------------------------------------------- 3.6 优先级

#[test]
fn multiple_rules_resolved_by_priority() {
    // 两条规则都能命中，priority 高的胜出
    let yaml = r#"
version: 1
activities:
  - id: low
    name: "低优先级"
    priority: 1
    triggers:
      apps: ["Chrome"]
  - id: high
    name: "高优先级"
    priority: 99
    triggers:
      apps: ["Chrome"]
"#;
    let set = RuleSet::parse_yaml(yaml).unwrap();
    match set.classify(&obs(Some("Chrome"), None, None)) {
        RuleMatch::Matched { rule_id, .. } => assert_eq!(rule_id, "high"),
        other => panic!("应当按优先级选出 high，实际 {other:?}"),
    }
}

#[test]
fn conflicting_same_priority_falls_back_to_ai() {
    let yaml = r#"
version: 1
activities:
  - id: a
    name: "甲"
    priority: 5
    triggers:
      apps: ["Chrome"]
  - id: b
    name: "乙"
    priority: 5
    triggers:
      apps: ["Chrome"]
"#;
    let set = RuleSet::parse_yaml(yaml).unwrap();

    match set.classify(&obs(Some("Chrome"), None, None)) {
        RuleMatch::Conflict { rule_ids } => {
            let mut ids = rule_ids.clone();
            ids.sort();
            assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
        }
        other => panic!("同优先级冲突必须交给 AI 消解，实际 {other:?}"),
    }
}

// ---------------------------------------------------------------- 3.9 不命中

#[test]
fn no_match_yields_none_not_an_error() {
    let matched = rules().classify(&obs(Some("Calculator"), Some("计算器"), None));
    assert!(matches!(matched, RuleMatch::None));
    assert!(!matched.is_matched());
}

#[test]
fn observation_without_any_metadata_matches_nothing() {
    assert!(matches!(
        rules().classify(&obs(None, None, None)),
        RuleMatch::None
    ));
}

// ---------------------------------------------------------------- 3.10–3.12 校验

#[test]
fn invalid_yaml_is_rejected_with_a_reason() {
    let error = RuleSet::parse_yaml("activities: [").unwrap_err();
    assert!(!error.is_empty(), "必须给出可读的失败原因");
}

#[test]
fn rule_without_id_is_rejected() {
    let yaml = r#"
version: 1
activities:
  - name: "没有 id"
    triggers:
      apps: ["VSCode"]
"#;
    let error = RuleSet::parse_yaml(yaml).unwrap_err();
    assert!(error.contains("id"), "错误信息应指出缺什么：{error}");
}

#[test]
fn duplicate_rule_ids_are_rejected() {
    let yaml = r#"
version: 1
activities:
  - id: dup
    name: "甲"
    triggers: { apps: ["A"] }
  - id: dup
    name: "乙"
    triggers: { apps: ["B"] }
"#;
    let error = RuleSet::parse_yaml(yaml).unwrap_err();
    assert!(error.contains("dup"), "应当指出重复的 id：{error}");
}

#[test]
fn invalid_regex_is_rejected_with_the_pattern() {
    let yaml = r#"
version: 1
activities:
  - id: bad
    name: "坏正则"
    triggers:
      window_patterns: ['([unclosed']
"#;
    let error = RuleSet::parse_yaml(yaml).unwrap_err();
    assert!(
        error.contains("unclosed") || error.contains("正则"),
        "错误信息应指出是哪条正则：{error}"
    );
}

#[test]
fn rule_without_triggers_is_rejected() {
    let yaml = r#"
version: 1
activities:
  - id: empty
    name: "没有触发条件"
"#;
    assert!(
        RuleSet::parse_yaml(yaml).is_err(),
        "没有任何触发条件的规则会命中一切，必须拒绝"
    );
}

#[test]
fn empty_ruleset_is_valid_and_matches_nothing() {
    let set = RuleSet::parse_yaml("version: 1\nactivities: []\n").unwrap();
    assert!(set.is_empty());
    assert!(matches!(
        set.classify(&obs(Some("VSCode"), None, None)),
        RuleMatch::None
    ));
}

// ---------------------------------------------------------------- ai 偏好

#[test]
fn ai_preference_is_carried_from_the_rule() {
    let set = rules();

    let coding = set.classify(&obs(Some("VSCode"), Some("x"), Some("impl A")));
    assert_eq!(coding.ai_preference(), AiPreference::None);

    let terminal = set.classify(&obs(Some("Terminal"), None, None));
    assert_eq!(terminal.ai_preference(), AiPreference::Deep);
}

#[test]
fn default_priority_and_ai_are_sane() {
    let yaml = r#"
version: 1
activities:
  - id: minimal
    name: "最小规则"
    triggers:
      apps: ["VSCode"]
"#;
    let set = RuleSet::parse_yaml(yaml).unwrap();
    let matched = set.classify(&obs(Some("VSCode"), None, None));

    assert_eq!(matched.ai_preference(), AiPreference::None);
    assert_eq!(matched.priority(), 0, "未指定优先级时默认为 0");
}

proptest::prelude::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig { cases: 64, ..Default::default() })]

    #[test]
    fn classify_never_panics_on_arbitrary_text(app in ".{0,50}", title in ".{0,200}", text in ".{0,400}") {
        let set = rules();
        let observation = ObservationSummary {
            id: "o".to_string(),
            at: Timestamp::from_millis(0),
            app_name: Some(app),
            window_title: Some(title),
            domain: None,
            text: Some(text),
        };
        let _ = set.classify(&observation);
    }
}

// 随包发布的示例规则必须始终可解析：
// 一份写坏的示例文件比没有示例更糟 —— 用户复制过去只会得到「规则静默失效」。
#[test]
fn shipped_example_rules_parse_and_match_the_documented_semantics() {
    let text = include_str!("../../../configs/rules/activities.example.yaml");
    let set = RuleSet::parse_yaml(text).expect("示例规则必须能解析");

    assert!(
        set.len() >= 8,
        "示例应当覆盖足够多的场景，实际 {}",
        set.len()
    );

    // 「跨类别 AND、类别内 OR」在示例里要真的体现出来
    let vscode = obs(Some("Visual Studio Code"), Some("main.rs — project"), None);
    assert_eq!(
        set.classify(&vscode).rule_id(),
        Some("coding"),
        "VSCode 里写代码应当命中 coding 规则"
    );

    let terminal_test = obs(Some("iTerm2"), Some("cargo test"), Some("cargo test"));
    assert_eq!(
        set.classify(&terminal_test).rule_id(),
        Some("debugging"),
        "终端里跑测试应当命中 debugging，而不是被 coding 的 exclude 放过去"
    );

    // 只是开着会议客户端但窗口标题是应用名本身 → 不该算会议
    let idle_lark = obs(Some("飞书"), Some("飞书"), None);
    assert!(
        set.classify(&idle_lark).rule_id() != Some("meeting"),
        "exclude 必须能拦住「开着客户端但没在开会」"
    );
}
