//! **「有阶段必有总结」**。
//!
//! 这是整个项目的最高优先级不变量：用户原始诉求就是「一直在截图，但没有阶段总结」。
//! 因此这里用两道测试把它钉死：
//!
//! 1. **属性测试（256 例）**：任意失败率下，生成入口都产出一份非空总结 —— 返回类型里没有 `Err`，只可能降级；
//! 2. **巡检集成测试**：数据库里「已关闭却没有总结」的阶段会被补齐，哨兵指标最终回到 0。

use std::sync::Arc;
use std::time::Duration;

use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::{ChatProvider, Role};
use mc_storage::projectors::activities::StoredActivity;
use mc_storage::projectors::stages::StageRow;
use mc_storage::Database;
use mc_summary::generator::{GenerateOutcome, SummaryGenerator};
use mc_summary::model::{ActivityDigest, StageSummaryInput, SummaryLocale, SummaryRange};
use mc_summary::patrol::{patrol_once, PatrolPolicy};
use mc_summary::template::SummaryTemplate;
use mc_testkit::provider::ScriptedTransport;
use proptest::prelude::*;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn chat_response(content: &str) -> String {
    serde_json::json!({
        "choices": [{ "message": { "content": content }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 100, "completion_tokens": 30 }
    })
    .to_string()
}

fn provider(transport: Arc<ScriptedTransport>) -> Arc<dyn ChatProvider> {
    Arc::new(
        OpenAiCompatibleProvider::new(
            EndpointConfig {
                base_url: "https://api.example.com/v1".to_string(),
                model: "qwen-max".to_string(),
                api_key: Some("sk-test".to_string()),
                timeout: Duration::from_secs(10),
                max_image_edge: None,
                max_concurrency: 1,
            },
            transport,
            Role::Chat,
        )
        .expect("provider"),
    )
}

fn generator(transport: Arc<ScriptedTransport>) -> SummaryGenerator {
    SummaryGenerator::new(
        provider(transport),
        SummaryTemplate::default_work_stage(),
        SummaryLocale::ZhCn,
    )
    .with_max_attempts(2)
}

fn input(index: usize) -> StageSummaryInput {
    let start = at(index as i64 * 3600);
    StageSummaryInput {
        stage_id: format!("stage-{index}"),
        range: SummaryRange {
            start,
            end: Timestamp::from_millis(start.as_millis() + 1_800_000),
        },
        timezone: "Asia/Shanghai".to_string(),
        locale: SummaryLocale::ZhCn,
        activities: vec![ActivityDigest {
            id: format!("act-{index}"),
            title: format!("活动 {index}"),
            category: Some("开发".to_string()),
            start,
            end: Timestamp::from_millis(start.as_millis() + 600_000),
            observations: 10,
            inferred: false,
        }],
        observation_count: 10,
        blocked_observations: 0,
    }
}

/// 按失败率构造脚本化传输：前 `failures` 次失败，之后成功。
fn transport_with_failure_rate(failures: usize) -> Arc<ScriptedTransport> {
    let mut transport = ScriptedTransport::new();
    for _ in 0..failures {
        transport = transport.push_failure(mc_providers::transport::TransportError::Timeout);
    }
    // 解析失败也走同一条降级链，因此这里顺带覆盖坏输出
    Arc::new(transport.push_json(200, chat_response("模型生成的总结正文。")))
}

// 256 例属性测试：任意阶段数 × 任意失败率，都有非空总结
proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn no_stage_without_summary(
        stage_count in 1usize..6,
        failure_rate in 0u32..=100,
    ) {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            for index in 0..stage_count {
                // 把失败率映射成「前 N 次调用失败」
                let failures = (failure_rate as usize * 4) / 100;
                let transport = transport_with_failure_rate(failures);
                let generator = generator(transport);

                let outcome = generator.generate(&input(index)).await;
                let summary = outcome.summary();

                prop_assert!(
                    !summary.body_markdown.trim().is_empty(),
                    "失败率 {}% 时产出了空总结",
                    failure_rate
                );
                prop_assert!(!summary.title.trim().is_empty());

                // 降级路径必须被明确标记（UI 才能如实显示）
                if outcome.is_fallback() {
                    prop_assert_eq!(summary.quality, mc_summary::model::Quality::Fallback);
                } else {
                    prop_assert_eq!(summary.quality, mc_summary::model::Quality::Model);
                }
            }
            Ok(())
        })?;
    }
}

// ---------------------------------------------------------------- 巡检（第三道防线）

fn open_db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Database::open(dir.path().join("mc.sqlite")).expect("db");
    (dir, db)
}

/// 一次投影写入多个活动。
///
/// `activities::store` 是**覆盖式写入**（派生表整份重写），
/// 因此分多次调用只会留下最后一个 —— 分两次「造数据」得到的是一个活动。
fn seed_activities(db: &Database, specs: &[(&str, i64, i64)]) {
    let views: Vec<mc_domain::activity::ActivityView> = specs
        .iter()
        .map(|(id, start, end)| mc_domain::activity::ActivityView {
            id: id.to_string(),
            start: at(*start),
            end: at(*end),
            title: format!("活动 {id}"),
            original_title: format!("活动 {id}"),
            category: Some("开发".to_string()),
            observations: vec![mc_domain::activity::ObservationRef {
                id: format!("obs-{id}"),
                at: at(*start),
            }],
            origin: Provenance::Rule {
                rule_id: "coding".to_string(),
            },
            confidence: 1.0,
            is_user_modified: false,
        })
        .collect();

    let projection = mc_domain::projector::Projection {
        activities: views,
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };
    mc_storage::projectors::activities::store(db, &projection, 0, at(specs.last().unwrap().2))
        .expect("存活动");
}

fn seed_activity(db: &Database, id: &str, start: i64, end: i64) {
    seed_activities(db, &[(id, start, end)]);
}

fn closed_stage(id: &str, start: i64, end: i64, activities: &[&str]) -> StageRow {
    StageRow {
        id: id.to_string(),
        start: at(start),
        end: Some(at(end)),
        state: "closed".to_string(),
        end_reason: Some("switched".to_string()),
        day: "2026-09-30".to_string(),
        activities: activities.iter().map(|id| id.to_string()).collect(),
    }
}

#[tokio::test]
async fn patrol_creates_missing_summary_within_deadline() {
    let (_dir, db) = open_db();
    seed_activity(&db, "act-1", 0, 3600);
    db.upsert_stage(&closed_stage("stage-1", 0, 3600, &["act-1"]), 0)
        .expect("写阶段");

    // 关闭后还没到 deadline → 不该被催
    let policy = PatrolPolicy {
        deadline_secs: 600,
        min_stage_duration_secs: 900,
        timezone: "Asia/Shanghai".to_string(),
    };
    let transport = transport_with_failure_rate(0);
    let generator = generator(Arc::clone(&transport));

    let early = patrol_once(&db, &generator, &policy, at(3900))
        .await
        .expect("巡检");
    assert!(
        early.repaired.is_empty(),
        "还没到 deadline 不该补：{early:?}"
    );
    assert_eq!(transport.call_count(), 0);

    // 超过 deadline → 必须补
    let late = patrol_once(&db, &generator, &policy, at(4300))
        .await
        .expect("巡检");
    assert_eq!(late.repaired, vec!["stage-1"], "{late:?}");
    assert_eq!(transport.call_count(), 1);

    let summaries = db.stage_summaries("stage-1").expect("读总结");
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].quality, "model");
    assert!(!summaries[0].body_markdown.trim().is_empty());
    assert!(
        summaries[0].body_markdown.contains("模型生成的总结正文"),
        "存的应当是模型产物"
    );
}

// 哨兵指标
#[tokio::test]
async fn stages_without_summary_metric_returns_to_zero() {
    let (_dir, db) = open_db();
    seed_activities(&db, &[("act-1", 0, 3600), ("act-2", 7200, 10_800)]);
    db.upsert_stages(
        &[
            closed_stage("stage-1", 0, 3600, &["act-1"]),
            closed_stage("stage-2", 7200, 10_800, &["act-2"]),
        ],
        0,
    )
    .expect("写阶段");

    assert_eq!(
        db.stages_without_summary_count(900).expect("指标"),
        2,
        "关闭但没总结的阶段必须被指标看见"
    );

    // 模型全部失败 —— 这正是要兜住的场景
    let transport = Arc::new(
        ScriptedTransport::new()
            .push_failure(mc_providers::transport::TransportError::Timeout)
            .push_failure(mc_providers::transport::TransportError::Timeout)
            .push_failure(mc_providers::transport::TransportError::Timeout)
            .push_failure(mc_providers::transport::TransportError::Timeout),
    );
    let generator = generator(transport);

    let policy = PatrolPolicy {
        deadline_secs: 600,
        min_stage_duration_secs: 900,
        timezone: "Asia/Shanghai".to_string(),
    };
    let report = patrol_once(&db, &generator, &policy, at(20_000))
        .await
        .expect("巡检");

    assert_eq!(report.repaired.len(), 2, "两个阶段都要补上");
    assert_eq!(report.degraded.len(), 2, "模型全挂 → 两条都是兜底");
    assert_eq!(
        db.stages_without_summary_count(900).expect("指标"),
        0,
        "巡检之后哨兵指标必须归零 —— 这是「有阶段必有总结」的最终保证"
    );

    for summary in db.read_summaries(None, Some("stage")).expect("读总结") {
        assert_eq!(summary.quality, "fallback");
        assert!(!summary.body_markdown.trim().is_empty(), "兜底也不能是空的");
    }

    // 降级必须可见
    let failures: Vec<String> = db
        .with_read(|conn| {
            let mut stmt = conn
                .prepare("SELECT error_code FROM pipeline_failures WHERE component = 'summary'")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .expect("读失败记录");
    assert_eq!(failures.len(), 2, "两条降级都要留痕");
    assert!(failures
        .iter()
        .all(|code| code == "provider_unavailable_fallback"));
}

#[tokio::test]
async fn patrol_skips_stages_below_min_duration() {
    let (_dir, db) = open_db();
    seed_activity(&db, "act-1", 0, 30);
    db.upsert_stage(&closed_stage("stage-tiny", 0, 30, &["act-1"]), 0)
        .expect("写阶段");

    let transport = transport_with_failure_rate(0);
    let generator = generator(Arc::clone(&transport));
    let policy = PatrolPolicy {
        deadline_secs: 600,
        min_stage_duration_secs: 900,
        timezone: "UTC".to_string(),
    };

    let report = patrol_once(&db, &generator, &policy, at(5000))
        .await
        .expect("巡检");

    assert!(report.repaired.is_empty());
    assert_eq!(
        report.skipped_too_short,
        vec!["stage-tiny"],
        "过短阶段要**明确记下被跳过**，而不是悄悄不处理"
    );
    assert_eq!(transport.call_count(), 0, "过短阶段连模型都不该调");
    assert_eq!(
        db.stages_without_summary_count(900).expect("指标"),
        0,
        "被刻意跳过的短阶段不该让哨兵指标报警 —— 否则指标永远不为 0，就没人看了"
    );
}

// 崩溃重启后补记的阶段（interrupted）同样要有总结
#[tokio::test]
async fn interrupted_stage_gets_a_summary() {
    let (_dir, db) = open_db();
    seed_activity(&db, "act-1", 0, 3600);
    let mut stage = closed_stage("stage-crashed", 0, 3600, &["act-1"]);
    stage.end_reason = Some("interrupted".to_string());
    db.upsert_stage(&stage, 0).expect("写阶段");

    // 模型全挂：崩溃恢复后也要有一份**人类可读**的总结
    let transport = transport_with_failure_rate(9);
    let generator = generator(transport);
    let policy = PatrolPolicy {
        deadline_secs: 600,
        min_stage_duration_secs: 900,
        timezone: "UTC".to_string(),
    };

    let report = patrol_once(&db, &generator, &policy, at(5000))
        .await
        .expect("巡检");

    assert_eq!(report.repaired, vec!["stage-crashed"]);
    let summaries = db.stage_summaries("stage-crashed").expect("读总结");
    assert_eq!(summaries.len(), 1);
    assert!(
        summaries[0].body_markdown.contains("活动 act-1"),
        "兜底/正文里要能看到这一阶段做了什么：{}",
        summaries[0].body_markdown
    );
}

// 巡检必须幂等：同一个阶段不会被补出两条总结
#[tokio::test]
async fn patrol_is_idempotent() {
    let (_dir, db) = open_db();
    seed_activity(&db, "act-1", 0, 3600);
    db.upsert_stage(&closed_stage("stage-1", 0, 3600, &["act-1"]), 0)
        .expect("写阶段");

    let transport = transport_with_failure_rate(0);
    let generator = generator(Arc::clone(&transport));
    let policy = PatrolPolicy {
        deadline_secs: 600,
        min_stage_duration_secs: 900,
        timezone: "UTC".to_string(),
    };

    let first = patrol_once(&db, &generator, &policy, at(5000))
        .await
        .expect("巡检");
    let second = patrol_once(&db, &generator, &policy, at(5100))
        .await
        .expect("巡检");

    assert_eq!(first.repaired.len(), 1);
    assert!(
        second.repaired.is_empty(),
        "已经有总结的阶段不该被重复补：{second:?}"
    );
    assert_eq!(db.summary_count().expect("总结数"), 1);
    assert_eq!(transport.call_count(), 1, "不该重复调用模型");
}

// 总结正文与活动的对应关系（4.31 的落库版本）
#[tokio::test]
async fn patrol_summary_is_built_from_the_stage_activities() {
    let (_dir, db) = open_db();
    seed_activities(&db, &[("act-1", 0, 1800), ("act-2", 1800, 3600)]);
    db.upsert_stage(&closed_stage("stage-1", 0, 3600, &["act-1", "act-2"]), 0)
        .expect("写阶段");

    let transport = transport_with_failure_rate(2); // 两次都失败 → 兜底
    let generator = generator(Arc::clone(&transport));
    let policy = PatrolPolicy {
        deadline_secs: 600,
        min_stage_duration_secs: 900,
        timezone: "Asia/Shanghai".to_string(),
    };

    patrol_once(&db, &generator, &policy, at(5000))
        .await
        .expect("巡检");

    let summaries = db.stage_summaries("stage-1").expect("读总结");
    let body = &summaries[0].body_markdown;
    assert!(body.contains("活动 act-1"), "{body}");
    assert!(body.contains("活动 act-2"), "{body}");
    assert_eq!(summaries[0].quality, "fallback");
}

/// 让编译器确认 StoredActivity 的反序列化形状没变（阶段活动读取依赖它）
#[allow(dead_code)]
fn _shape(row: &StoredActivity) -> (String, i64) {
    (row.id.clone(), row.legacy_id)
}

/// GenerateOutcome 的辅助方法也要被覆盖
#[test]
fn outcome_helpers_agree_with_the_variant() {
    let transport = transport_with_failure_rate(1);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let outcome = runtime.block_on(generator(transport).generate(&input(0)));

    if outcome.is_fallback() {
        assert!(matches!(outcome, GenerateOutcome::Fallback { .. }));
    }
    let owned = outcome.clone().into_summary();
    assert_eq!(&owned, outcome.summary());
}

// V3：巡检单轮上限 —— 刚配好模型时可能一次积压很多历史阶段，
// 一轮全做完会让巡检长时间占着（调用方是后台循环）；剩下的下一轮继续。
#[tokio::test]
async fn patrol_repairs_at_most_one_batch_per_round() {
    let (_dir, db) = open_db();
    let policy = PatrolPolicy {
        deadline_secs: 600,
        min_stage_duration_secs: 900,
        timezone: "Asia/Shanghai".to_string(),
    };

    // 造 MAX_REPAIRS_PER_ROUND + 3 个待补阶段（每个都超过 min_stage_duration_secs）
    let total = mc_summary::patrol::MAX_REPAIRS_PER_ROUND + 3;
    for index in 0..total {
        let id = format!("act-{index}");
        let stage = format!("stage-{index}");
        let start = (index as i64) * 3600;
        seed_activity(&db, &id, start, start + 3600);
        db.upsert_stage(&closed_stage(&stage, start, start + 3600, &[&id]), 0)
            .expect("写阶段");
    }

    let transport = transport_with_failure_rate(0);
    let generator = generator(Arc::clone(&transport));

    let report = patrol_once(&db, &generator, &policy, at(60 * 60 * 24))
        .await
        .expect("巡检");

    assert_eq!(
        report.repaired.len(),
        mc_summary::patrol::MAX_REPAIRS_PER_ROUND,
        "单轮补写数量必须被上限截住：{report:?}"
    );
    // 调用次数不能用来断言批量：transport 只脚本了一条响应，之后的阶段会走
    // 重试与兜底（fallback 也算 repaired）。这里只要求"确实问过模型"。
    assert!(
        transport.call_count() >= 1,
        "补写阶段至少要调用一次模型（或走到兜底）"
    );

    // 下一轮继续补剩下的（不是丢掉）
    let second = patrol_once(&db, &generator, &policy, at(60 * 60 * 25))
        .await
        .expect("第二轮巡检");
    assert_eq!(
        second.repaired.len(),
        3,
        "剩下的应当在下一轮补上：{second:?}"
    );
}
