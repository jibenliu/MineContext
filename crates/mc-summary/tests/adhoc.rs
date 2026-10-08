//! **任意时段总结**。
//! 这是用户明确要求的入口：「总结我 10 点到 12 点半」。
//! 与阶段总结共用同一套引擎与兜底，因此也有同一条不变量：
//! **只要范围内有数据，就一定拿得到一份非空总结**。
//!
//! 四条设计约束在这里被钉住：
//! - 空范围**拒绝**而不是产出一条空总结（垃圾数据比没有更糟）；
//! - 时间范围对外按**用户本地时区**解释，对内一律 UTC；
//! - 同一范围 + 同一筛选 + 事件没变 → 命中缓存（不重复烧 token）；
//! - 事件变了 / 显式要求重生成 → 绕过缓存。

use std::sync::Arc;
use std::time::Duration;

use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::{ChatProvider, Role};
use mc_storage::Database;
use mc_summary::adhoc::{
    generate, preview, AdhocConfig, AdhocOutcome, AdhocRange, AdhocRequest, AdhocScope,
};
use mc_summary::generator::SummaryGenerator;
use mc_summary::model::{Quality, SummaryLocale};
use mc_summary::template::SummaryTemplate;
use mc_testkit::provider::ScriptedTransport;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn chat_response(content: &str) -> String {
    serde_json::json!({
        "choices": [{ "message": { "content": content }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 200, "completion_tokens": 60 }
    })
    .to_string()
}

fn open_db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Database::open(dir.path().join("mc.sqlite")).expect("db");
    (dir, db)
}

fn seed_activities(db: &Database, specs: &[(&str, i64, i64, bool)]) {
    let views: Vec<mc_domain::activity::ActivityView> = specs
        .iter()
        .map(
            |(id, start, end, inferred)| mc_domain::activity::ActivityView {
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
                origin: if *inferred {
                    Provenance::Inferred {
                        model: "stub".to_string(),
                    }
                } else {
                    Provenance::Rule {
                        rule_id: "coding".to_string(),
                    }
                },
                confidence: 1.0,
                is_user_modified: false,
            },
        )
        .collect();

    let projection = mc_domain::projector::Projection {
        activities: views,
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };
    mc_storage::projectors::activities::store(db, &projection, 0, at(0)).expect("存活动");
}

fn seed_observation(db: &Database, id: &str, offset_secs: i64, verdict: &str) {
    db.insert_observation(&mc_storage::observations::NewObservation {
        id: id.to_string(),
        ts: at(offset_secs),
        source_id: "macos:screen".to_string(),
        kind: "screen".to_string(),
        app_name: Some("VSCode".to_string()),
        app_bundle_id: None,
        window_title: Some("main.rs".to_string()),
        domain: None,
        display_id: None,
        scale_factor: None,
        image: None,
        text_content: None,
        text_origin: None,
        change_kind: "pixel_major".to_string(),
        privacy_verdict: verdict.to_string(),
        phash: None,
        idempotency: format!("idem-{id}"),
    })
    .expect("写观测");
}

fn config() -> AdhocConfig {
    AdhocConfig {
        timezone: "Asia/Shanghai".to_string(),
        locale: SummaryLocale::ZhCn,
        chunk_threshold_secs: 4 * 3600,
        max_chunks: 24,
    }
}

fn request(from: i64, to: i64) -> AdhocRequest {
    AdhocRequest {
        range: AdhocRange {
            from: at(from),
            to: at(to),
        },
        scope: AdhocScope::default(),
        template_id: None,
        template_yaml: None,
        title: None,
        save_to_vault: false,
        force_regenerate: false,
    }
}

fn generator(failures: usize) -> SummaryGenerator {
    let mut transport = ScriptedTransport::new();
    for _ in 0..failures {
        transport = transport.push_failure(mc_providers::transport::TransportError::Timeout);
    }
    let transport = Arc::new(transport.push_json(200, chat_response("这段时间主要在写代码。")));

    let provider = OpenAiCompatibleProvider::new(
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
    .expect("provider");

    SummaryGenerator::new(
        Arc::new(provider) as Arc<dyn ChatProvider>,
        SummaryTemplate::default_work_stage(),
        SummaryLocale::ZhCn,
    )
    .with_max_attempts(2)
}

#[tokio::test]
async fn adhoc_summary_for_an_explicit_range() {
    let (_dir, db) = open_db();
    seed_activities(
        &db,
        &[("act-1", 0, 1800, false), ("act-2", 1800, 3600, false)],
    );
    seed_observation(&db, "obs-1", 60, "allowed");

    let outcome = generate(&db, &generator(0), &config(), request(0, 3600), at(4000))
        .await
        .expect("生成");

    let AdhocOutcome::Generated {
        summary_id,
        summary,
        ..
    } = outcome
    else {
        panic!("应当生成新的总结：{outcome:?}");
    };
    assert_eq!(summary.quality, Quality::Model);
    assert!(
        summary_id.starts_with("sum-adhoc-"),
        "总结 id 要能看出是任意时段总结：{summary_id}"
    );

    let stored = db.read_summaries(None, Some("adhoc")).expect("读总结");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].kind, "adhoc");
    assert_eq!(stored[0].start, at(0));
    assert_eq!(stored[0].end, at(3600));
    assert!(!stored[0].body_markdown.trim().is_empty());
}

#[tokio::test]
async fn adhoc_empty_range_is_rejected_not_stored() {
    let (_dir, db) = open_db();
    // 范围内什么也没有
    let outcome = generate(&db, &generator(0), &config(), request(3600, 7200), at(8000))
        .await
        .expect("空范围不是错误，而是一种结果");

    match outcome {
        AdhocOutcome::EmptyRange { preview } => {
            assert!(!preview.has_data);
            assert_eq!(preview.activities, 0);
            assert_eq!(preview.observations, 0);
        }
        other => panic!("空范围应当被拒绝：{other:?}"),
    }
    assert_eq!(
        db.summary_count().expect("总结数"),
        0,
        "空总结绝不能落库 —— 垃圾数据比没有更糟"
    );
}

#[tokio::test]
async fn adhoc_invalid_range_is_rejected() {
    let (_dir, db) = open_db();

    let error = generate(&db, &generator(0), &config(), request(3600, 3600), at(8000))
        .await
        .expect_err("from >= to 必须报错");
    assert_eq!(
        error.code(),
        mc_common::error::ErrorCode::DomainInvalidRange
    );

    let reversed = generate(&db, &generator(0), &config(), request(7200, 3600), at(8000))
        .await
        .expect_err("起止颠倒必须报错");
    assert_eq!(
        reversed.code(),
        mc_common::error::ErrorCode::DomainInvalidRange
    );
    assert_eq!(db.summary_count().unwrap(), 0);
}

#[test]
fn adhoc_preview_reports_counts_and_estimate() {
    let (_dir, db) = open_db();
    seed_activities(&db, &[("act-1", 0, 1800, false)]);
    for step in 0..5 {
        seed_observation(&db, &format!("obs-{step}"), step * 60, "allowed");
    }
    seed_observation(&db, "obs-blocked", 300, "blocked");

    let preview = preview(&db, &config(), request(0, 3600)).expect("预览");

    assert!(preview.has_data);
    assert_eq!(preview.observations, 6);
    assert_eq!(
        preview.blocked_observations, 1,
        "预览必须报出被隐私拦截的条数：用户有权知道有多少内容没参与总结"
    );
    assert_eq!(preview.activities, 1);
    assert_eq!(preview.estimated_chunks, 1);
    assert!(preview.estimated_tokens > 0);
    assert_eq!(preview.timezone, "Asia/Shanghai");
    // T0 是 UTC 09:00 → 上海 17:00。预览必须按本地时区给人看。
    assert!(
        preview.local_from.contains("17:00") && preview.local_to.contains("18:00"),
        "预览要给出**本地时间**，用户看到的是自己的时间：{preview:?}"
    );
}

#[tokio::test]
async fn adhoc_cross_day_and_tiny_ranges_work() {
    let (_dir, db) = open_db();
    // 跨天：本地 09-30 08:00 到 10-01 08:00（UTC 00:00 → 24:00）
    seed_activities(
        &db,
        &[("act-1", 0, 600, false), ("act-2", 86_000, 86_600, false)],
    );
    seed_observation(&db, "obs-1", 60, "allowed");
    seed_observation(&db, "obs-2", 86_060, "allowed");

    let cross_day = generate(
        &db,
        &generator(0),
        &config(),
        request(0, 86_400),
        at(90_000),
    )
    .await
    .expect("跨天范围");
    assert!(matches!(cross_day, AdhocOutcome::Generated { .. }));

    // 30 秒的范围也要能用
    let tiny = generate(&db, &generator(0), &config(), request(60, 90), at(90_000))
        .await
        .expect("30 秒范围");
    match tiny {
        AdhocOutcome::Generated { preview, .. } => {
            assert_eq!(preview.estimated_chunks, 1, "再短也只有一个块");
        }
        AdhocOutcome::Cached { .. } => {}
        other => panic!("30 秒范围应当可用：{other:?}"),
    }
}

#[tokio::test]
async fn adhoc_llm_failure_yields_a_fallback_summary() {
    let (_dir, db) = open_db();
    seed_activities(&db, &[("act-1", 0, 1800, false)]);
    seed_observation(&db, "obs-1", 60, "allowed");

    let outcome = generate(&db, &generator(9), &config(), request(0, 3600), at(4000))
        .await
        .expect("模型全挂也要有总结");

    let AdhocOutcome::Generated { summary, .. } = outcome else {
        panic!("第一次生成不该命中缓存");
    };
    assert_eq!(summary.quality, Quality::Fallback);
    assert!(!summary.body_markdown.trim().is_empty());
    assert!(summary.body_markdown.contains("活动 act-1"));

    let stored = db.read_summaries(None, Some("adhoc")).expect("读总结");
    assert_eq!(stored[0].quality, "fallback");
}

#[tokio::test]
async fn adhoc_cache_hits_then_invalidates_then_can_be_forced() {
    let (_dir, db) = open_db();
    seed_activities(&db, &[("act-1", 0, 1800, false)]);
    seed_observation(&db, "obs-1", 60, "allowed");

    let first = generate(&db, &generator(0), &config(), request(0, 3600), at(4000))
        .await
        .expect("首次生成");
    let AdhocOutcome::Generated { summary_id, .. } = first else {
        panic!("首次应当是 Generated");
    };

    // 同一范围、同一筛选、事件没变 → 命中缓存
    let second = generate(&db, &generator(0), &config(), request(0, 3600), at(4100))
        .await
        .expect("第二次");
    match second {
        AdhocOutcome::Cached {
            summary_id: cached, ..
        } => assert_eq!(cached, summary_id),
        other => panic!("应当命中缓存：{other:?}"),
    }
    assert_eq!(db.summary_count().unwrap(), 1, "缓存不该重复落库");

    // 事件变了 → 缓存失效
    seed_observation(&db, "obs-2", 120, "allowed");
    let third = generate(&db, &generator(0), &config(), request(0, 3600), at(4200))
        .await
        .expect("第三次");
    match third {
        AdhocOutcome::Generated {
            summary_id: new_id, ..
        } => {
            assert_ne!(new_id, summary_id, "重新生成必须是新的一条，id 不能撞车")
        }
        other => panic!("范围内数据变了就必须重新生成：{other:?}"),
    }
    assert_eq!(db.summary_count().unwrap(), 2);

    // 显式要求重生成 → 绕过缓存
    let mut forced = request(0, 3600);
    forced.force_regenerate = true;
    let fourth = generate(&db, &generator(0), &config(), forced, at(4300))
        .await
        .expect("第四次");
    assert!(matches!(fourth, AdhocOutcome::Generated { .. }));
    assert_eq!(db.summary_count().unwrap(), 3, "强制重生成要写出新的一条");
}

#[tokio::test]
async fn adhoc_marks_inferred_activities() {
    let (_dir, db) = open_db();
    seed_activities(&db, &[("act-1", 0, 1800, true)]);
    seed_observation(&db, "obs-1", 60, "allowed");

    let outcome = generate(&db, &generator(9), &config(), request(0, 3600), at(4000))
        .await
        .expect("生成");

    let summary = outcome.summary().expect("应当有总结");
    assert!(
        summary.body_markdown.contains("推测") || summary.body_markdown.contains("inferred"),
        "推断出来的活动在总结里必须保留不确定性：{}",
        summary.body_markdown
    );
}

// 结果基于快照：生成之后又有新数据，已生成的总结不跟着变
#[tokio::test]
async fn adhoc_snapshot_is_stable() {
    let (_dir, db) = open_db();
    seed_activities(&db, &[("act-1", 0, 1800, false)]);
    seed_observation(&db, "obs-1", 60, "allowed");

    let outcome = generate(&db, &generator(9), &config(), request(0, 3600), at(4000))
        .await
        .expect("生成");
    let first_body = outcome.summary().expect("应当有总结").body_markdown.clone();

    // 范围内又来了一条观测（但已经超出这个总结的范围语义）
    seed_observation(&db, "obs-2", 3540, "allowed");

    let stored = db.read_summaries(None, Some("adhoc")).expect("读总结");
    assert_eq!(
        stored[0].body_markdown, first_body,
        "已生成的总结是**快照**：不该被后续数据改写"
    );
    assert_eq!(stored.len(), 1);
}

// 24 —— 记录必须写**实际使用**的模板，而不是请求里的选择器。
// 用户模板 YAML 自带 id 时，它优先于 `template_id`；记录写错会让「这条总结
// 是哪套模板产出的」无法回答，用户按记录排查时会看到不存在的模板。
#[tokio::test]
async fn recorded_template_id_is_the_one_actually_used() {
    let (_dir, db) = open_db();
    seed_activities(&db, &[("act-1", 0, 1800, false)]);
    seed_observation(&db, "obs-1", 60, "allowed");

    let mut req = request(0, 3600);
    // 请求里写内置 id，但用户给了自定义 YAML（自带 id）——YAML 优先
    req.template_id = Some("work_stage".to_string());
    req.template_yaml = Some(
        "id: my_custom\nname: 我的模板\nfields:\n  - id: time_range\n    label: 时间段\n    kind: time_range\n"
            .to_string(),
    );

    let outcome = generate(&db, &generator(0), &config(), req, at(4000))
        .await
        .expect("生成");
    let AdhocOutcome::Generated { summary_id, .. } = outcome else {
        panic!("应当生成新的总结：{outcome:?}");
    };

    let stored = db
        .read_summaries(None, None)
        .expect("读总结")
        .into_iter()
        .find(|row| row.id == summary_id)
        .expect("总结要落库");
    assert_eq!(
        stored.template_id, "my_custom",
        "记录必须是实际使用的模板 id（用户 YAML 的 id），而不是请求里的 work_stage"
    );
}
