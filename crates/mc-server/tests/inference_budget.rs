//! 推断的预算闸门：**按 token 联动**，而不只是按调用次数。
//!
//! 「2 小时 300 万 token」这类事故里，次数限制拦不住 —— 每次调用可能很贵。
//! 因此超预算之后必须真的停止调用模型，并且这件事要在记账里看得见
//! （`error_code = budget_exhausted`），而不是静默不推断。

use std::sync::Arc;
use std::time::Duration;

use mc_common::time::Timestamp;
use mc_domain::activity::{ActivityView, ObservationRef, Provenance};
use mc_pipeline::activity_ai::{ActivityAiPolicy, ActivityAiWorker};
use mc_pipeline::budget::{BudgetAction, BudgetPolicy, BudgetTracker};
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::Role;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
use mc_testkit::provider::ScriptedTransport;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn activity(id: &str) -> ActivityView {
    ActivityView {
        id: id.to_string(),
        start: at(0),
        end: at(60),
        title: "unknown window".to_string(),
        original_title: "unknown window".to_string(),
        category: None,
        observations: vec![ObservationRef {
            id: format!("obs-{id}"),
            at: at(0),
        }],
        origin: Provenance::Observed,
        confidence: 1.0,
        is_user_modified: false,
    }
}

fn worker(transport: Arc<ScriptedTransport>) -> ActivityAiWorker {
    let provider = OpenAiCompatibleProvider::new(
        EndpointConfig {
            base_url: "https://api.example.com/v1".to_string(),
            model: "qwen3-vl".to_string(),
            api_key: Some("sk-test".to_string()),
            timeout: Duration::from_secs(30),
            max_image_edge: None,
            max_concurrency: 1,
        },
        transport,
        Role::Vision,
    )
    .expect("provider");

    ActivityAiWorker::new(
        Arc::new(provider),
        ActivityAiPolicy {
            ai_enabled: true,
            max_calls_per_window: 1000,
            window_secs: 3600,
            reanalyze_after_secs: 0,
        },
    )
}

/// 一次调用要花 70 token（见 `response()`），因此 50 的配额一次就超。
fn tiny_budget() -> BudgetTracker {
    BudgetTracker::new(BudgetPolicy {
        max_vlm_calls_per_hour: 1000,
        max_tokens_per_hour: 50,
        max_tokens_per_day: 1000,
        on_exceeded: BudgetAction::Degrade,
    })
}

fn response() -> String {
    serde_json::json!({
        "choices": [{ "message": { "content": r#"{"title":"架构评审","confidence":0.8}"# }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 60, "completion_tokens": 10 }
    })
    .to_string()
}

#[tokio::test]
async fn tokens_are_fed_into_the_budget_and_close_the_gate() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, response()));
    let mut worker = worker(Arc::clone(&transport));
    let mut tracker = tiny_budget();

    // 第一次推断：预算够用，模型被调用
    let first = mc_pipeline::activity_ai::infer_activities(
        &mut worker,
        &[activity("act-1")],
        |_| Some((b"png".to_vec(), "image/png".to_string())),
        at(0),
    )
    .await
    .expect("第一次推断应当成功");
    assert_eq!(transport.call_count(), 1);

    // 记账回灌预算：这一次花了 70 token，已经超过 50 的配额
    mc_server::activities::feed_budget(&mut tracker, &first.calls, at(0));
    mc_server::activities::sync_ai_with_budget(&mut worker, &tracker, at(1));

    // 第二次推断：闸门已经关上，一次都不该发
    let second = mc_pipeline::activity_ai::infer_activities(
        &mut worker,
        &[activity("act-2")],
        |_| Some((b"png".to_vec(), "image/png".to_string())),
        at(2),
    )
    .await
    .expect("被闸门拦下不是错误");

    assert_eq!(transport.call_count(), 1, "超预算后不该再调用模型");
    assert_eq!(second.calls.len(), 1);
    assert_eq!(second.calls[0].result.as_str(), "rate_limited");
    assert_eq!(
        second.calls[0].error_code.as_deref(),
        Some("budget_exhausted"),
        "为什么没推断必须写清楚"
    );
}

#[tokio::test]
async fn budget_window_slides_back_open() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, response()));
    let mut worker = worker(Arc::clone(&transport));
    let mut tracker = tiny_budget();

    let first = mc_pipeline::activity_ai::infer_activities(
        &mut worker,
        &[activity("act-1")],
        |_| Some((b"png".to_vec(), "image/png".to_string())),
        at(0),
    )
    .await
    .expect("第一次推断应当成功");
    mc_server::activities::feed_budget(&mut tracker, &first.calls, at(0));
    mc_server::activities::sync_ai_with_budget(&mut worker, &tracker, at(1));
    assert!(!worker.policy().ai_enabled, "超预算之后闸门应当是关的");

    // 一小时后窗口滑走：闸门自动打开，不需要重启进程
    mc_server::activities::sync_ai_with_budget(&mut worker, &tracker, at(3_700));
    assert!(
        worker.policy().ai_enabled,
        "窗口滑走后应当恢复调用，否则用户得重启才有推断"
    );
}
