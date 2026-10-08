//! 活动推断的**成本可见性**：每一次模型调用都要能对上账。
//!
//! 这条路径原来只限流不记账（`provider_calls` 只有语义索引在写），于是
//! 「推断花掉了多少 token」在报表上是零 —— 这类账目缺口
//! 「2 小时 300 万 token，而 UI 没有明显产出」最难排查的形态。

use mc_common::time::Timestamp;
use mc_pipeline::activity_ai::{AiCall, AiCallResult};
use mc_storage::Database;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn call(result: AiCallResult, tokens: u32) -> AiCall {
    AiCall {
        model: "openai_compatible:qwen3-vl".to_string(),
        prompt_tokens: tokens,
        completion_tokens: 20,
        latency_ms: 1_500,
        result,
        error_code: None,
    }
}

#[test]
fn inference_calls_land_in_the_cost_report() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("minecontext.db")).unwrap();

    let at = Timestamp::from_millis(T0);
    mc_server::activities::record_inference_calls(
        &db,
        &[
            call(AiCallResult::Ok, 120),
            call(AiCallResult::InvalidResponse, 90),
        ],
        at,
    )
    .expect("记账");

    let report = db
        .provider_usage(
            Timestamp::from_millis(T0 - 1_000),
            Timestamp::from_millis(T0 + 1_000),
        )
        .expect("用量报表");

    assert_eq!(report.calls, 2, "两次调用都要在报表里");
    assert_eq!(report.successful_calls, 1);
    assert_eq!(report.failed_calls, 1);
    assert_eq!(report.total_tokens(), 250, "120+20 与 90+20");
    assert_eq!(
        report
            .by_purpose
            .iter()
            .map(|p| p.purpose.as_str())
            .collect::<Vec<_>>(),
        vec!["vision"],
        "推断属于 vision 用途，不能混进 embedding"
    );
}

#[test]
fn gate_rejections_are_visible_but_cost_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path().join("minecontext.db")).unwrap();

    let at = Timestamp::from_millis(T0);
    mc_server::activities::record_inference_calls(
        &db,
        &[AiCall {
            prompt_tokens: 0,
            completion_tokens: 0,
            latency_ms: 0,
            result: AiCallResult::RateLimited,
            error_code: Some("budget_exhausted".to_string()),
            ..call(AiCallResult::RateLimited, 0)
        }],
        at,
    )
    .expect("记账");

    let report = db
        .provider_usage(
            Timestamp::from_millis(T0 - 1_000),
            Timestamp::from_millis(T0 + 1_000),
        )
        .expect("用量报表");

    assert_eq!(report.calls, 1, "被拦下也要留下痕迹");
    assert_eq!(report.total_tokens(), 0, "没发出去的调用不该算 token");
}
