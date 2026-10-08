//! 日报与周报，以及归档进笔记树，形状必须逐字对齐兼容面客户端的查询：
//!
//! ```sql
//! SELECT * FROM vaults WHERE document_type IN ('DailyReport', 'vaults')
//! AND is_deleted = 0 ORDER BY id DESC
//! ```
//!
//! 对不齐这份查询，日报在笔记树里就不可见。

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;
use mc_memory::vault_writer::{archive_daily_report, archive_weekly_report};
use mc_memory::weekly::{generate_weekly, WeeklyConfig};
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::{ChatProvider, Role};
use mc_storage::vaults::{DOCUMENT_TYPE_DAILY_REPORT, DOCUMENT_TYPE_VAULTS, FOLDER_SUMMARY};
use mc_storage::Database;
use mc_summary::daily::{generate_daily, DailyConfig};
use mc_summary::generator::SummaryGenerator;
use mc_summary::model::SummaryLocale;
use mc_summary::source::FallbackOnly;
use mc_summary::template::SummaryTemplate;
use mc_testkit::provider::ScriptedTransport;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn open_db() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Database::open(dir.path().join("mc.sqlite")).expect("db");
    (dir, db)
}

fn daily_config() -> DailyConfig {
    DailyConfig {
        timezone: "Asia/Shanghai".to_string(),
        locale: SummaryLocale::ZhCn,
    }
}

fn weekly_config() -> WeeklyConfig {
    WeeklyConfig {
        timezone: "Asia/Shanghai".to_string(),
        locale: SummaryLocale::ZhCn,
    }
}

fn fallback() -> FallbackOnly {
    FallbackOnly::new(SummaryTemplate::default_work_stage())
}

fn model(text: &str) -> SummaryGenerator {
    let transport = Arc::new(
        ScriptedTransport::new().push_json(
            200,
            serde_json::json!({
                "choices": [{ "message": { "content": text }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 200, "completion_tokens": 50 }
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

/// 在本地某天造一段活动（`day_offset` 相对 2026-09-30）。
fn seed_day(db: &Database, day_offset: i64, id: &str) {
    // 上海时间 09:00 = UTC 01:00
    seed_at(db, day_offset * 86_400 - 8 * 3600, id);
}

/// 在指定的 UTC 秒偏移处造一段活动。
///
/// **注意**：`activities::store` 是覆盖式写入（派生表整份重写），
/// 因此想造多段活动必须**一次传完**（用 `seed_many`），
/// 分多次调用只会留下最后一次，因此夹具必须一次写全。
fn seed_at(db: &Database, base_secs: i64, id: &str) {
    seed_many(db, &[(base_secs, id)]);
}

fn seed_many(db: &Database, specs: &[(i64, &str)]) {
    let views: Vec<mc_domain::activity::ActivityView> = specs
        .iter()
        .map(|(base_secs, id)| mc_domain::activity::ActivityView {
            id: id.to_string(),
            start: at(*base_secs),
            end: at(*base_secs + 1800),
            title: format!("活动 {id}"),
            original_title: format!("活动 {id}"),
            category: Some("开发".to_string()),
            observations: vec![mc_domain::activity::ObservationRef {
                id: format!("obs-{id}"),
                at: at(*base_secs),
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
    let last = specs.iter().map(|(secs, _)| *secs).max().unwrap_or(0);
    mc_storage::projectors::activities::store(db, &projection, 0, at(last + 1800)).expect("存活动");
}

// 周报聚合这一周的日报
#[tokio::test]
async fn weekly_report_aggregates_daily_summaries() {
    let (_dir, db) = open_db();
    // 周一到周三各一段活动（2026-09-28 是周一）
    let monday = NaiveDate::from_ymd_opt(2026, 9, 28).unwrap();
    for (offset, id) in [(0i64, "mon"), (1, "tue"), (2, "wed")] {
        seed_day(&db, offset - 2, id);
        let day = NaiveDate::from_ymd_opt(2026, 9, 28 + offset as u32).unwrap();
        generate_daily(&db, &fallback(), &daily_config(), day, at(3 * 86_400))
            .await
            .expect("日报");
    }

    let weekly = generate_weekly(&db, &fallback(), &weekly_config(), monday, at(7 * 86_400))
        .await
        .expect("生成周报")
        .expect("这一周有内容");

    assert_eq!(weekly.kind, "weekly");
    assert_eq!(weekly.id, "sum-weekly-2026-09-28");
    for id in ["mon", "tue", "wed"] {
        assert!(
            weekly.body_markdown.contains(&format!("活动 {id}")),
            "周报要包含周内每一天的内容，缺了 {id}：{}",
            weekly.body_markdown
        );
    }
    assert!(
        weekly.body_markdown.contains("第 1 天"),
        "按天分段：{}",
        weekly.body_markdown
    );
    assert_eq!(weekly.title, "2026-09-28 周报");
}

// 5.23（周报版）—— 同一周重复生成不重复写
#[tokio::test]
async fn weekly_report_is_idempotent() {
    let (_dir, db) = open_db();
    seed_day(&db, -2, "mon");
    let monday = NaiveDate::from_ymd_opt(2026, 9, 28).unwrap();
    generate_daily(
        &db,
        &fallback(),
        &daily_config(),
        NaiveDate::from_ymd_opt(2026, 9, 28).unwrap(),
        at(86_400),
    )
    .await
    .unwrap();

    let first = generate_weekly(&db, &fallback(), &weekly_config(), monday, at(86_400 * 7))
        .await
        .unwrap()
        .expect("第一次");
    let second = generate_weekly(&db, &fallback(), &weekly_config(), monday, at(86_400 * 8))
        .await
        .unwrap()
        .expect("第二次");

    assert_eq!(first.id, second.id);
    let weeklies = db.read_summaries(None, Some("weekly")).unwrap();
    assert_eq!(weeklies.len(), 1, "同一周只该有一条周报");
}

// 空周不产出（与「空白天不出日报」同一条原则）
#[tokio::test]
async fn empty_week_yields_no_report() {
    let (_dir, db) = open_db();
    let monday = NaiveDate::from_ymd_opt(2026, 9, 28).unwrap();

    let outcome = generate_weekly(&db, &fallback(), &weekly_config(), monday, at(86_400 * 7))
        .await
        .expect("空周不是错误");
    assert!(outcome.is_none());
    assert_eq!(db.summary_count().unwrap(), 0);
}

// 日报归档进笔记树，形状与渲染层查询逐字对齐
#[tokio::test]
async fn daily_report_is_written_as_a_vault_document() {
    let (_dir, db) = open_db();
    seed_day(&db, 0, "act-1");
    let day = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
    let summary = generate_daily(&db, &fallback(), &daily_config(), day, at(86_400))
        .await
        .unwrap()
        .expect("日报");

    archive_daily_report(&db, &summary, at(86_400)).expect("归档");

    // 渲染层的查询：document_type IN ('DailyReport', 'vaults')
    let documents = db
        .read_vault_documents(&[DOCUMENT_TYPE_DAILY_REPORT, DOCUMENT_TYPE_VAULTS])
        .expect("读笔记树");

    let report = documents
        .iter()
        .find(|doc| doc.title == "2026-09-30 日报")
        .expect("日报必须出现在渲染层的查询结果里");
    assert_eq!(report.document_type, DOCUMENT_TYPE_DAILY_REPORT);
    assert!(
        report
            .content
            .as_deref()
            .is_some_and(|content| content.contains("活动 act-1")),
        "笔记正文就是日报正文：{report:?}"
    );
    assert!(
        report
            .summary
            .as_deref()
            .is_some_and(|summary| !summary.is_empty()),
        "列表里要有一句摘要，否则笔记树里只看到标题"
    );

    // 归档在 Summary 文件夹下
    let folder = documents
        .iter()
        .find(|doc| doc.is_folder && doc.title == FOLDER_SUMMARY)
        .expect("必须有一个 Summary 文件夹");
    assert_eq!(report.parent_id, Some(folder.id));
}

// 归档是幂等的：同一天归档两次不该出现两篇
#[tokio::test]
async fn archiving_the_same_day_twice_updates_one_document() {
    let (_dir, db) = open_db();
    seed_day(&db, 0, "act-1");
    let day = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
    let summary = generate_daily(&db, &fallback(), &daily_config(), day, at(86_400))
        .await
        .unwrap()
        .expect("日报");

    let first = archive_daily_report(&db, &summary, at(86_400)).unwrap();
    let second = archive_daily_report(&db, &summary, at(86_400 + 60)).unwrap();

    assert_eq!(first, second, "同一篇文档要更新而不是新建");
    let documents = db
        .read_vault_documents(&[DOCUMENT_TYPE_DAILY_REPORT])
        .unwrap();
    assert_eq!(documents.len(), 1);
    // Summary 文件夹也只有一个
    let folders = db.read_vault_documents(&[DOCUMENT_TYPE_VAULTS]).unwrap();
    assert_eq!(
        folders.iter().filter(|doc| doc.is_folder).count(),
        1,
        "文件夹不能重复创建：{folders:?}"
    );
}

// 周报归档为普通笔记（旧枚举里只有 DailyReport 与 vaults），并带标签
#[tokio::test]
async fn weekly_report_is_archived_as_a_tagged_note() {
    let (_dir, db) = open_db();
    seed_day(&db, -2, "mon");
    generate_daily(
        &db,
        &fallback(),
        &daily_config(),
        NaiveDate::from_ymd_opt(2026, 9, 28).unwrap(),
        at(86_400),
    )
    .await
    .unwrap();
    let weekly = generate_weekly(
        &db,
        &fallback(),
        &weekly_config(),
        NaiveDate::from_ymd_opt(2026, 9, 28).unwrap(),
        at(86_400 * 7),
    )
    .await
    .unwrap()
    .expect("周报");

    archive_weekly_report(&db, &weekly, at(86_400 * 7)).expect("归档");

    let documents = db.read_vault_documents(&[DOCUMENT_TYPE_VAULTS]).unwrap();
    let note = documents
        .iter()
        .find(|doc| doc.title == weekly.title)
        .expect("周报要出现在笔记树里");
    assert!(
        note.tags
            .as_deref()
            .is_some_and(|tags| tags.contains("weekly")),
        "用标签区分周报（旧枚举里没有 WeeklyReport 类型）：{note:?}"
    );
    assert_eq!(note.document_type, DOCUMENT_TYPE_VAULTS);
}

// 日报必须覆盖当天**所有**阶段的内容（不能漏）
#[tokio::test]
async fn daily_report_includes_every_stage_of_the_day() {
    let (_dir, db) = open_db();
    // 同一天里的三段（本地 09:00 / 10:00 / 11:00）—— 必须一次写入
    seed_many(
        &db,
        &[
            (-8 * 3600, "a"),
            (-8 * 3600 + 3600, "b"),
            (-8 * 3600 + 7200, "c"),
        ],
    );
    let day = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();

    let summary = generate_daily(&db, &fallback(), &daily_config(), day, at(86_400))
        .await
        .unwrap()
        .expect("日报");

    for id in ["a", "b", "c"] {
        assert!(
            summary.body_markdown.contains(&format!("活动 {id}")),
            "漏了 {id}：{}",
            summary.body_markdown
        );
    }
}

// 周报同样有兜底（模型不可用不是「没有周报」的理由）
#[tokio::test]
async fn weekly_report_falls_back_when_the_model_fails() {
    let (_dir, db) = open_db();
    seed_day(&db, -2, "mon");
    generate_daily(
        &db,
        &fallback(),
        &daily_config(),
        NaiveDate::from_ymd_opt(2026, 9, 28).unwrap(),
        at(86_400),
    )
    .await
    .unwrap();

    // 模型第一次成功、周报这次失败 → 周报走兜底
    let failing = {
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
        SummaryGenerator::new(
            Arc::new(provider) as Arc<dyn ChatProvider>,
            SummaryTemplate::default_work_stage(),
            SummaryLocale::ZhCn,
        )
    };

    let weekly = generate_weekly(
        &db,
        &failing,
        &weekly_config(),
        NaiveDate::from_ymd_opt(2026, 9, 28).unwrap(),
        at(86_400 * 7),
    )
    .await
    .unwrap()
    .expect("模型挂了也要有周报");

    assert_eq!(weekly.quality, "fallback");
    assert!(!weekly.body_markdown.trim().is_empty());
    let _ = model("不参与本用例");
}
