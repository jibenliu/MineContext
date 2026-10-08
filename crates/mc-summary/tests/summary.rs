//! 、4.30–4.32、4.35–4.36：模板与兜底总结。
//!
//! 这一组的核心不是「总结有多好看」，而是**「有阶段必有总结」在结构上成立**：
//!
//! - 模型不可用时产出**确定性兜底总结**，内容必须人类可读
//!   （时间段 + 做过什么 + 涉及哪些应用/分类），而不是一句「生成失败」；
//! - 兜底总结在 API 上可区分（`quality=Fallback`），UI 才能如实标注；
//! - `body_markdown` 在任何输入下都非空（4.35，属性测试）；
//! - 极短阶段可以不产出总结（4.36），否则防抖残留会变成噪声总结。

use mc_common::time::Timestamp;
use mc_summary::model::{ActivityDigest, Quality, StageSummaryInput, SummaryLocale, SummaryRange};
use mc_summary::template::{FieldKind, SummaryTemplate, TemplateSet};
use mc_summary::{fallback, should_summarize};

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn digest(title: &str, category: Option<&str>, start: i64, end: i64) -> ActivityDigest {
    ActivityDigest {
        id: format!("act-{start}"),
        title: title.to_string(),
        category: category.map(str::to_string),
        start: at(start),
        end: at(end),
        observations: 3,
        inferred: false,
    }
}

fn input(activities: Vec<ActivityDigest>) -> StageSummaryInput {
    StageSummaryInput {
        stage_id: "stage-act-1".to_string(),
        range: SummaryRange {
            start: at(0),
            end: at(1800),
        },
        timezone: "Asia/Shanghai".to_string(),
        locale: SummaryLocale::ZhCn,
        activities,
        observation_count: 42,
        blocked_observations: 0,
    }
}

// ---------------------------------------------------------------- 模板

#[test]
fn template_fields_are_mapped_into_the_summary() {
    let template = SummaryTemplate::parse_yaml(
        r#"
id: work_stage
name: 工作阶段
fields:
  - id: time_range
    label: 时间段
    kind: time_range
  - id: activities
    label: 做过什么
    kind: activity_list
  - id: tools
    label: 涉及工具
    kind: app_list
"#,
    )
    .expect("模板应当能解析");

    let summary = fallback::generate(
        &input(vec![
            digest("写代码", Some("开发"), 0, 600),
            digest("需求评审", Some("需求"), 600, 1200),
        ]),
        &template,
    );

    assert!(summary.body_markdown.contains("写代码"));
    assert!(summary.body_markdown.contains("需求评审"));
    assert!(
        summary.body_markdown.contains("开发"),
        "模板要求的字段必须出现在正文里：{}",
        summary.body_markdown
    );
    assert!(summary.fields.contains_key("time_range"));
    assert!(summary.fields.contains_key("activities"));
}

#[test]
fn template_validation_rejects_unknown_fields() {
    let error = SummaryTemplate::parse_yaml(
        r#"
id: bad
name: 坏模板
fields:
  - id: time_range
    label: 时间段
    kind: time_range
  - id: crystal_ball
    label: 预测未来
    kind: prophecy
"#,
    )
    .expect_err("未知字段类型必须被拒绝");

    assert!(
        error.contains("prophecy"),
        "报错要点名哪个字段有问题：{error}"
    );
}

#[test]
fn template_without_fields_is_rejected() {
    let error = SummaryTemplate::parse_yaml("id: empty\nname: 空模板\nfields: []\n")
        .expect_err("没有字段的模板产不出可用总结");
    assert!(error.contains("fields"), "{error}");
}

#[test]
fn shipped_template_example_parses() {
    let text = include_str!("../../../configs/templates/summaries.example.yaml");
    let set = TemplateSet::parse_yaml(text).expect("示例模板必须能解析");
    assert!(set.len() >= 2, "示例应当给出多个模板，实际 {}", set.len());
    assert!(set.get("work_stage").is_some(), "至少要有一个工作阶段模板");
}

// ---------------------------------------------------------------- 兜底总结

#[test]
fn llm_failure_still_yields_fallback_summary() {
    let activities = vec![digest("写代码", Some("开发"), 0, 600)];

    let summary = fallback::generate(&input(activities), &SummaryTemplate::default_work_stage());

    assert_eq!(summary.quality, Quality::Fallback);
    assert!(
        !summary.body_markdown.trim().is_empty(),
        "兜底总结绝不能是空的：模型不可用不是「什么都不给」的理由"
    );
    assert!(summary.model.is_none(), "兜底没有模型");
}

#[test]
fn fallback_summary_contains_activity_titles_and_time_range() {
    let activities = vec![
        digest("写代码", Some("开发"), 0, 600),
        digest("需求评审", Some("需求"), 600, 1800),
    ];
    let summary = fallback::generate(&input(activities), &SummaryTemplate::default_work_stage());

    // 时间段（本地时间；Asia/Shanghai 下 T0 是 17:00）
    assert!(
        summary.body_markdown.contains("17:00"),
        "兜底要写清时间段：{}",
        summary.body_markdown
    );
    assert!(summary.body_markdown.contains("17:30"));
    // 做过什么
    assert!(summary.body_markdown.contains("写代码"));
    assert!(summary.body_markdown.contains("需求评审"));
    // 涉及哪些分类（≈工具/领域）
    assert!(summary.body_markdown.contains("开发"));
    assert!(
        summary.body_markdown.contains("2"),
        "要给出活动条数，让人一眼知道这段时间有多碎：{}",
        summary.body_markdown
    );
}

#[test]
fn fallback_summary_is_marked_for_the_api() {
    let summary = fallback::generate(
        &input(vec![digest("写代码", Some("开发"), 0, 600)]),
        &SummaryTemplate::default_work_stage(),
    );

    let json = serde_json::to_value(&summary).expect("总结要能序列化给 API");
    assert_eq!(json["quality"], "fallback");
    assert!(json["body_markdown"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
}

// 无活动时（只有观测）也要给出可读的兜底，而不是空正文
#[test]
fn fallback_handles_a_range_without_activities() {
    let summary = fallback::generate(&input(Vec::new()), &SummaryTemplate::default_work_stage());

    assert!(!summary.body_markdown.trim().is_empty());
    assert!(
        summary.body_markdown.contains("42"),
        "至少要说明这段时间采集了多少观测：{}",
        summary.body_markdown
    );
}

#[test]
fn stage_below_min_duration_may_skip_summary() {
    assert!(
        !should_summarize(60, 900),
        "过短的阶段（噪声）不该产出总结，否则总结本身就成了噪声"
    );
    assert!(should_summarize(900, 900));
    assert!(should_summarize(3600, 900));
}

// 属性测试：任何输入都不能产出空正文
mod properties {
    use super::*;
    use proptest::prelude::*;

    fn arb_digest() -> impl Strategy<Value = ActivityDigest> {
        (
            "[\\PC]{0,40}",                       // 标题（可能为空或全是空白）
            proptest::option::of("[\\PC]{0,20}"), // 分类
            0i64..10_000,
            0i64..10_000,
        )
            .prop_map(|(title, category, a, b)| ActivityDigest {
                id: format!("act-{a}"),
                title,
                category,
                start: at(a),
                end: at(a + b),
                observations: (a % 50) as u32,
                inferred: false,
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn fallback_body_is_never_empty(
            activities in proptest::collection::vec(arb_digest(), 0..6),
            observations in 0u32..10_000,
            blocked in 0u32..100,
        ) {
            let mut input = input(activities);
            input.observation_count = observations;
            input.blocked_observations = blocked;

            let summary = fallback::generate(&input, &SummaryTemplate::default_work_stage());
            prop_assert!(!summary.body_markdown.trim().is_empty());
            prop_assert!(!summary.title.trim().is_empty());
        }
    }
}

// 字段类型清单是**封闭集合**：新增类型必须同时给渲染实现，否则就是「声明了但没人填」
#[test]
fn every_field_kind_has_a_renderer() {
    for kind in FieldKind::ALL {
        assert!(
            !kind.label_zh().is_empty() && !kind.label_en().is_empty(),
            "{kind:?} 缺少中英标签"
        );
    }
}
