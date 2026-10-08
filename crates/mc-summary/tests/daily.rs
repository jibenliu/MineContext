//! 跨天日报。
//!
//! 「今天做了什么」是用户每天都会问一次的问题。日报与阶段总结共用同一套引擎，
//! 因此也有同一条不变量：**只要那天有内容，就一定拿得到一份非空总结**。
//!
//! 三条刻意约束：
//! - 范围是**本地日**边界（`[00:00, 次日 00:00)`），不是「现在往前 24 小时」（DST 日不是 24 小时）；
//! - 同一天重复生成是**幂等**的（不能每次 tick 都写一条）；
//! - **空白天不出日报**：给没有内容的一天写总结只是噪声。

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::{ChatProvider, Role};
use mc_storage::Database;
use mc_summary::daily::{generate_daily, DailyConfig};
use mc_summary::generator::SummaryGenerator;
use mc_summary::model::SummaryLocale;
use mc_summary::source::FallbackOnly;
use mc_summary::template::SummaryTemplate;
use mc_testkit::provider::ScriptedTransport;

/// 2026-09-30T09:00:00Z（上海时间 17:00）
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn open_db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Database::open(dir.path().join("mc.sqlite")).expect("db");
    (dir, db)
}

fn config() -> DailyConfig {
    DailyConfig {
        timezone: "Asia/Shanghai".to_string(),
        locale: SummaryLocale::ZhCn,
    }
}

fn source(text: &str) -> SummaryGenerator {
    let transport = Arc::new(
        ScriptedTransport::new().push_json(
            200,
            serde_json::json!({
                "choices": [{ "message": { "content": text }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 500, "completion_tokens": 120 }
            })
            .to_string(),
        ),
    );

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
}

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
    mc_storage::projectors::activities::store(db, &projection, 0, at(0)).expect("存活动");
}

// 4.42（正常路径）
#[tokio::test]
async fn daily_summary_covers_the_local_day() {
    let (_dir, db) = open_db();
    // 上海时间 2026-09-30 的上午与下午各一段
    seed_activities(&db, &[("act-1", 0, 1800), ("act-2", 3600, 5400)]);

    let day = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
    let stored = generate_daily(&db, &source("今天主要在写代码。"), &config(), day, at(7200))
        .await
        .expect("生成日报")
        .expect("有内容的一天必须产出日报");

    assert_eq!(stored.kind, "daily");
    assert_eq!(stored.quality, "model");
    assert!(stored.body_markdown.contains("今天主要在写代码"));
    assert_eq!(stored.id, "sum-daily-2026-09-30");

    // 范围是本地日边界（上海 00:00 = 前一天 16:00Z）
    let (start, end) = Timestamp::day_bounds_for(day, "Asia/Shanghai").unwrap();
    assert_eq!(stored.start, start);
    assert_eq!(stored.end, end);
    assert_eq!(stored.start.to_rfc3339(), "2026-09-29T16:00:00.000Z");
    assert_eq!(stored.title, "2026-09-30 日报");
}

// 幂等：同一天重复生成不该写第二条
#[tokio::test]
async fn daily_summary_is_idempotent() {
    let (_dir, db) = open_db();
    seed_activities(&db, &[("act-1", 0, 1800)]);
    let day = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();

    let first = generate_daily(&db, &source("第一次。"), &config(), day, at(7200))
        .await
        .unwrap()
        .expect("第一次");
    let second = generate_daily(&db, &source("第二次不该发生。"), &config(), day, at(7300))
        .await
        .unwrap()
        .expect("第二次");

    assert_eq!(first.id, second.id);
    assert_eq!(db.summary_count().unwrap(), 1, "重复生成不该写第二条");
    assert!(
        second.body_markdown.contains("第一次"),
        "已有日报直接复用，不重新调用模型"
    );
}

// 空白天不出日报
#[tokio::test]
async fn empty_day_yields_no_summary() {
    let (_dir, db) = open_db();
    let day = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();

    let outcome = generate_daily(&db, &source("不该被调用。"), &config(), day, at(7200))
        .await
        .expect("空白天不是错误");
    assert!(outcome.is_none(), "没有内容的一天不该产出日报");
    assert_eq!(db.summary_count().unwrap(), 0);
}

// 相邻两天的活动不能串味
#[tokio::test]
async fn daily_summary_only_covers_its_own_day() {
    let (_dir, db) = open_db();
    // act-1 在上海时间 09-30 23:00（15:00Z），act-2 在 10-01 01:00（17:00Z）
    seed_activities(
        &db,
        &[
            ("act-1", 6 * 3600, 6 * 3600 + 600),
            ("act-2", 8 * 3600, 8 * 3600 + 600),
        ],
    );

    let day = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
    let stored = generate_daily(
        &db,
        &FallbackOnly::new(SummaryTemplate::default_work_stage()),
        &config(),
        day,
        at(12 * 3600),
    )
    .await
    .unwrap()
    .expect("09-30 有内容");

    assert!(
        stored.body_markdown.contains("活动 act-1"),
        "应当包含当天那段：{}",
        stored.body_markdown
    );
    assert!(
        !stored.body_markdown.contains("活动 act-2"),
        "不该把次日的活动算进今天：{}",
        stored.body_markdown
    );
}

// 模型不可用也要有日报（第二道防线同样适用于日报）
#[tokio::test]
async fn daily_falls_back_when_the_model_fails() {
    let (_dir, db) = open_db();
    seed_activities(&db, &[("act-1", 0, 1800)]);
    let day = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();

    let transport = Arc::new(
        ScriptedTransport::new()
            .push_failure(mc_providers::transport::TransportError::Timeout)
            .push_failure(mc_providers::transport::TransportError::Timeout)
            .push_failure(mc_providers::transport::TransportError::Timeout),
    );
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
    let generator = SummaryGenerator::new(
        Arc::new(provider) as Arc<dyn ChatProvider>,
        SummaryTemplate::default_work_stage(),
        SummaryLocale::ZhCn,
    );

    let stored = generate_daily(&db, &generator, &config(), day, at(7200))
        .await
        .unwrap()
        .expect("模型挂了也要有日报");

    assert_eq!(stored.quality, "fallback");
    assert!(!stored.body_markdown.trim().is_empty());
    assert!(stored.body_markdown.contains("活动 act-1"));
}
