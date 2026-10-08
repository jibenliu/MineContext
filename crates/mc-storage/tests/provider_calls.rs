//! 模型调用记账与成本可见性。
//!
//! 成本必须可回答：「2 小时 300 万 token，而 UI 没有明显产出」这类质疑要用账本说话。
//! 单看 token 数没有意义 —— 有意义的是「每个产出花了多少」。

use mc_common::time::Timestamp;
use mc_storage::observations::{NewObservation, ObservationQuery};
use mc_storage::provider_calls::{CallResult, CostReport, ProviderCall, Purpose, UsageReport};
use mc_storage::Database;

fn open() -> (tempfile::TempDir, Database) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("minecontext.db");
    let db = Database::open(&path).unwrap();
    (dir, db)
}

fn ms(value: i64) -> Timestamp {
    Timestamp::from_millis(value)
}

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as DAY;

fn call(at_ms: i64, model: &str, purpose: Purpose, tokens: u32) -> ProviderCall {
    ProviderCall {
        at: ms(at_ms),
        provider_id: "openai_compatible".to_string(),
        model: model.to_string(),
        purpose,
        observation_id: None,
        stage_id: None,
        prompt_tokens: tokens,
        completion_tokens: tokens / 2,
        latency_ms: 800,
        result: CallResult::Ok,
        error_code: None,
    }
}

// ---------------------------------------------------------------- 2.49

#[test]
fn provider_call_is_recorded_with_tokens() {
    let (_dir, db) = open();

    let id = db
        .record_provider_call(&call(DAY, "qwen3-vl", Purpose::Vision, 1_000))
        .unwrap();
    assert!(id > 0);

    let usage = db.provider_usage(ms(DAY - 1), ms(DAY + 1)).unwrap();
    assert_eq!(usage.calls, 1);
    assert_eq!(usage.prompt_tokens, 1_000);
    assert_eq!(usage.completion_tokens, 500);
    assert_eq!(usage.total_tokens(), 1_500);
}

#[test]
fn usage_splits_by_model() {
    let (_dir, db) = open();

    db.record_provider_call(&call(DAY, "qwen3-vl", Purpose::Vision, 1_000))
        .unwrap();
    db.record_provider_call(&call(DAY, "qwen3-vl", Purpose::Vision, 2_000))
        .unwrap();
    db.record_provider_call(&call(DAY, "deepseek-chat", Purpose::Chat, 100))
        .unwrap();

    let usage = db.provider_usage(ms(DAY - 1), ms(DAY + 1)).unwrap();
    assert_eq!(usage.by_model.len(), 2);

    // 按 token 总量降序
    assert_eq!(usage.by_model[0].model, "qwen3-vl");
    assert_eq!(usage.by_model[0].calls, 2);
    assert_eq!(usage.by_model[0].prompt_tokens, 3_000);
    assert_eq!(usage.by_model[1].model, "deepseek-chat");
}

#[test]
fn usage_splits_by_purpose() {
    let (_dir, db) = open();

    db.record_provider_call(&call(DAY, "m", Purpose::Vision, 1_000))
        .unwrap();
    db.record_provider_call(&call(DAY, "m", Purpose::Chat, 200))
        .unwrap();
    db.record_provider_call(&call(DAY, "m", Purpose::Embedding, 50))
        .unwrap();

    let usage = db.provider_usage(ms(DAY - 1), ms(DAY + 1)).unwrap();
    let by_purpose: Vec<(&str, u64)> = usage
        .by_purpose
        .iter()
        .map(|p| (p.purpose.as_str(), p.calls))
        .collect();

    assert_eq!(
        by_purpose,
        vec![("chat", 1), ("embedding", 1), ("vision", 1)]
    );
}

#[test]
fn failed_calls_are_counted_separately() {
    let (_dir, db) = open();

    db.record_provider_call(&call(DAY, "m", Purpose::Vision, 100))
        .unwrap();

    let mut failed = call(DAY, "m", Purpose::Vision, 0);
    failed.result = CallResult::RateLimited;
    failed.error_code = Some("provider_rate_limited".to_string());
    db.record_provider_call(&failed).unwrap();

    let usage = db.provider_usage(ms(DAY - 1), ms(DAY + 1)).unwrap();
    assert_eq!(usage.calls, 2);
    assert_eq!(usage.successful_calls, 1);
    assert_eq!(usage.failed_calls, 1);
    assert!(
        (usage.failure_rate() - 0.5).abs() < 1e-9,
        "失败率是运营上最该盯的数字之一（失败也照样烧钱）"
    );
}

#[test]
fn usage_respects_the_time_range() {
    let (_dir, db) = open();

    db.record_provider_call(&call(DAY, "m", Purpose::Vision, 100))
        .unwrap();
    db.record_provider_call(&call(DAY + 86_400_000, "m", Purpose::Vision, 999))
        .unwrap();

    let today = db.provider_usage(ms(DAY - 1), ms(DAY + 1)).unwrap();
    assert_eq!(today.calls, 1);
    assert_eq!(today.prompt_tokens, 100);
}

#[test]
fn empty_range_reports_zeroes_not_an_error() {
    let (_dir, db) = open();

    let usage = db.provider_usage(ms(DAY), ms(DAY + 1)).unwrap();

    assert_eq!(usage.calls, 0);
    assert_eq!(usage.total_tokens(), 0);
    assert_eq!(usage.failure_rate(), 0.0, "没有调用时失败率是 0 而不是 NaN");
    assert!(usage.by_model.is_empty());
}

#[test]
fn average_latency_is_reported() {
    let (_dir, db) = open();

    let mut fast = call(DAY, "m", Purpose::Vision, 10);
    fast.latency_ms = 100;
    let mut slow = call(DAY, "m", Purpose::Vision, 10);
    slow.latency_ms = 900;
    db.record_provider_call(&fast).unwrap();
    db.record_provider_call(&slow).unwrap();

    let usage = db.provider_usage(ms(DAY - 1), ms(DAY + 1)).unwrap();
    assert_eq!(usage.avg_latency_ms, 500);
}

#[test]
fn call_can_be_attributed_to_an_observation() {
    let (_dir, db) = open();

    let mut attributed = call(DAY, "m", Purpose::Vision, 10);
    attributed.observation_id = Some("obs-1".to_string());
    db.record_provider_call(&attributed).unwrap();

    // 记账行本身不参与观测查询，但必须能落库且可追溯
    let usage = db.provider_usage(ms(DAY - 1), ms(DAY + 1)).unwrap();
    assert_eq!(usage.calls, 1);
}

// ---------------------------------------------------------------- 2.50

#[test]
fn cost_report_pairs_spend_with_output() {
    let (_dir, db) = open();

    // 花掉 1 万 token
    for index in 0..10 {
        db.record_provider_call(&call(DAY + index, "qwen3-vl", Purpose::Vision, 1_000))
            .unwrap();
    }

    // 采集了 3 条观测，其中 2 条被分析
    for index in 0..3 {
        let mut obs = NewObservation {
            id: format!("obs-{index}"),
            ts: ms(DAY + index),
            source_id: "macos:screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some("VSCode".to_string()),
            app_bundle_id: None,
            window_title: None,
            domain: None,
            display_id: None,
            scale_factor: None,
            image: None,
            text_content: None,
            text_origin: None,
            change_kind: "pixel_major".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: format!("idem-{index}"),
        };
        obs.image = None;
        db.insert_observation(&obs).unwrap();
    }

    for index in 0..2 {
        db.with_write(|conn| {
            conn.execute(
                "UPDATE analyses SET status = 'done' WHERE observation_id = ?1",
                [format!("obs-{index}")],
            )?;
            Ok(())
        })
        .unwrap();
    }

    // 用「一整天」的范围，而不是 1 毫秒 —— 10 次调用横跨 10 毫秒
    let report = db.cost_report(ms(DAY - 1), ms(DAY + 86_400_000)).unwrap();

    assert_eq!(report.usage.calls, 10);
    assert_eq!(report.usage.total_tokens(), 15_000);
    assert_eq!(report.observations_captured, 3);
    assert_eq!(report.observations_analyzed, 2);

    // 每次分析 15000/2 = 7500 token —— 这个数字不该随运行时间增长
    assert_eq!(report.tokens_per_analysis(), 7_500.0);
}

#[test]
fn cost_report_reports_no_output_when_nothing_was_produced() {
    let (_dir, db) = open();
    db.record_provider_call(&call(DAY, "m", Purpose::Vision, 5_000))
        .unwrap();

    let report = db.cost_report(ms(DAY - 1), ms(DAY + 1)).unwrap();

    assert!(report.usage.total_tokens() > 0, "确实花了钱");
    assert!(
        !report.produced_anything(),
        "但没有任何产出 —— 这正是被投诉的形态，必须能被查询出来"
    );
    assert_eq!(report.tokens_per_analysis(), 0.0);
}

#[test]
fn cost_report_is_empty_and_safe_for_an_empty_range() {
    let (_dir, db) = open();

    let report = db.cost_report(ms(DAY), ms(DAY + 1)).unwrap();

    assert_eq!(report, CostReport::default());
    assert_eq!(report.tokens_per_analysis(), 0.0, "不能出现 NaN");
    assert!(!report.produced_anything());
}

#[test]
fn usage_report_serializes_for_the_diagnostics_api() {
    let (_dir, db) = open();
    db.record_provider_call(&call(DAY, "qwen3-vl", Purpose::Vision, 100))
        .unwrap();

    let usage: UsageReport = db.provider_usage(ms(DAY - 1), ms(DAY + 1)).unwrap();
    let json = serde_json::to_value(&usage).unwrap();

    assert!(json["calls"].is_number());
    assert!(json["by_model"].is_array());
    assert!(json["by_purpose"].is_array());
}

#[test]
fn observations_query_still_works_alongside_accounting() {
    // 记账表与观测表互不干扰
    let (_dir, db) = open();
    db.record_provider_call(&call(DAY, "m", Purpose::Vision, 10))
        .unwrap();

    let observations = db.query_observations(&ObservationQuery::default()).unwrap();
    assert!(observations.is_empty());
}
