//! 长时段总结的分块与续跑。
//!
//! 长范围（例如「总结我上个月」）不能一次性喂给模型。分块有两个好处：
//! 每块都在模型的能力范围内；块还能**单独缓存** ——
//! 中途取消不必从头再来，数据变了也只需要重算变了的那几块。
//!
//! 三条被测试钉住的约束：
//! - 分块**确定性**：同一范围永远切成同样的块（否则缓存永远命中不了）；
//! - **取消保留已完成块**（4.67）：取消后重新发起，只补没做完的块；
//! - 归并结果包含所有块的内容，不丢块。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mc_common::time::Timestamp;
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::{ChatProvider, Role};
use mc_storage::Database;
use mc_summary::adhoc::{
    generate_chunked, AdhocConfig, AdhocOutcome, AdhocRange, AdhocRequest, AdhocScope, CancelFlag,
};
use mc_summary::generator::SummaryGenerator;
use mc_summary::model::SummaryLocale;
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

/// 每块一个标题的脚本化模型：调用次数可数，用来验证「续跑只补缺的块」。
fn counting_source(calls: Arc<AtomicUsize>) -> SummaryGenerator {
    let transport = Arc::new(ScriptedTransport::new().reply_with(move |_request| {
        let index = calls.fetch_add(1, Ordering::SeqCst) + 1;
        serde_json::json!({
            "choices": [{ "message": { "content": format!("第 {index} 次调用") }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 100, "completion_tokens": 20 }
        })
        .to_string()
    }));

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

/// 阈值 1 小时 → 3 小时的范围切成 3 块。
fn config() -> AdhocConfig {
    AdhocConfig {
        timezone: "Asia/Shanghai".to_string(),
        locale: SummaryLocale::ZhCn,
        chunk_threshold_secs: 3600,
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

fn seed_activities(db: &Database, count: i64) {
    let views: Vec<mc_domain::activity::ActivityView> = (0..count)
        .map(|index| {
            let start = index * 1800;
            mc_domain::activity::ActivityView {
                id: format!("act-{index}"),
                start: at(start),
                end: at(start + 900),
                title: format!("活动 {index}"),
                original_title: format!("活动 {index}"),
                category: Some("开发".to_string()),
                observations: vec![mc_domain::activity::ObservationRef {
                    id: format!("obs-{index}"),
                    at: at(start),
                }],
                origin: mc_domain::activity::Provenance::Rule {
                    rule_id: "coding".to_string(),
                },
                confidence: 1.0,
                is_user_modified: false,
            }
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

// 分块：3 小时的范围切成 3 块，每块一次调用，最后归并
#[tokio::test]
async fn long_range_is_split_into_chunks_and_merged() {
    let (_dir, db) = open_db();
    seed_activities(&db, 6);
    let calls = Arc::new(AtomicUsize::new(0));

    let outcome = generate_chunked(
        &db,
        &counting_source(Arc::clone(&calls)),
        &config(),
        request(0, 3 * 3600),
        at(4 * 3600),
        &CancelFlag::new(),
    )
    .await
    .expect("分块生成");

    let AdhocOutcome::Generated {
        summary, preview, ..
    } = outcome
    else {
        panic!("应当生成：{outcome:?}");
    };
    assert_eq!(preview.estimated_chunks, 3);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "每块一次调用，不做多余请求"
    );

    // 归并结果必须包含三块的内容
    for index in 1..=3 {
        assert!(
            summary
                .body_markdown
                .contains(&format!("第 {index} 次调用")),
            "归并结果少了第 {index} 块：{}",
            summary.body_markdown
        );
    }
}

// 取消保留已完成块，重新发起只补没做完的
#[tokio::test]
async fn cancel_keeps_completed_chunks_and_resumes() {
    let (_dir, db) = open_db();
    seed_activities(&db, 6);

    // 第一块做完之后就取消
    let cancel = CancelFlag::new();
    let cancel_for_source = cancel.clone();
    let transport = Arc::new(ScriptedTransport::new().reply_with(move |_| {
        cancel_for_source.cancel();
        serde_json::json!({
            "choices": [{ "message": { "content": "第一块" }, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 100, "completion_tokens": 20 }
        })
        .to_string()
    }));
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
    let cancelling_source = SummaryGenerator::new(
        Arc::new(provider) as Arc<dyn ChatProvider>,
        SummaryTemplate::default_work_stage(),
        SummaryLocale::ZhCn,
    );

    let outcome = generate_chunked(
        &db,
        &cancelling_source,
        &config(),
        request(0, 3 * 3600),
        at(4 * 3600),
        &cancel,
    )
    .await
    .expect("取消不是错误");

    match outcome {
        AdhocOutcome::Cancelled {
            chunks_done,
            chunks_total,
        } => {
            assert_eq!(chunks_done, 1, "第一块应当已经完成");
            assert_eq!(chunks_total, 3);
        }
        other => panic!("应当返回已取消：{other:?}"),
    }
    assert_eq!(
        db.read_summaries(None, Some("adhoc_chunk")).unwrap().len(),
        1,
        "已完成的块必须留在库里（否则取消等于白跑）"
    );

    // 重新发起：只补剩下的两块
    let resumed_calls = Arc::new(AtomicUsize::new(0));
    let resumed = generate_chunked(
        &db,
        &counting_source(Arc::clone(&resumed_calls)),
        &config(),
        request(0, 3 * 3600),
        at(5 * 3600),
        &CancelFlag::new(),
    )
    .await
    .expect("续跑");

    assert_eq!(
        resumed_calls.load(Ordering::SeqCst),
        2,
        "续跑只该补没做完的两块"
    );
    let summary = resumed.summary().expect("续跑应当产出归并结果");
    assert!(
        summary.body_markdown.contains("第一块"),
        "续跑要带上已完成块"
    );
}

// 分块必须确定性：同一范围永远切成同样的块，否则缓存永远命中不了
#[tokio::test]
async fn chunk_boundaries_are_deterministic() {
    let (_dir, db) = open_db();
    seed_activities(&db, 6);

    let first = mc_summary::adhoc::plan_chunks(request(0, 3 * 3600).range, &config());
    let second = mc_summary::adhoc::plan_chunks(request(0, 3 * 3600).range, &config());

    assert_eq!(first, second);
    assert_eq!(first.len(), 3);
    assert_eq!(first[0].from, at(0));
    assert_eq!(first.last().unwrap().to, at(3 * 3600));
    // 块之间首尾相接，不留缝不重叠
    for pair in first.windows(2) {
        assert_eq!(pair[0].to, pair[1].from);
    }
}

// 短范围仍然是一条总结（不分块），避免给小请求增加开销
#[tokio::test]
async fn short_range_stays_single_chunk() {
    let (_dir, db) = open_db();
    seed_activities(&db, 1);
    let calls = Arc::new(AtomicUsize::new(0));

    let outcome = generate_chunked(
        &db,
        &counting_source(Arc::clone(&calls)),
        &config(),
        request(0, 600),
        at(1000),
        &CancelFlag::new(),
    )
    .await
    .expect("生成");

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let AdhocOutcome::Generated { preview, .. } = outcome else {
        panic!("应当生成");
    };
    assert_eq!(preview.estimated_chunks, 1);
    assert_eq!(db.read_summaries(None, Some("adhoc")).unwrap().len(), 1);
}
