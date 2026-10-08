//! AI 辅助识别（Inferred 路径）。
//!
//! 这条路径的**唯一目的**是「规则判不了的时候才花钱」，因此五条测试全部围绕「什么时候**不**调模型」：
//! - 规则能判 → 一次都不调；
//! - 没预算 → 不调，活动照样产出（降级不是停摆）；
//! - 输出不可用 → 降级到 Observed，不把坏数据写进库；
//! - 短时间重复 → 限流 + 同一活动窗口不重复分析（对应「重复请求」这一类浪费）。
//!
//! 全部用脚本化传输替身，不碰网络、不碰真实模型。

use std::sync::Arc;
use std::time::Duration;

use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;
use mc_domain::observation::ObservationSummary;
use mc_pipeline::activity_ai::{
    ActivityAiOutcome, ActivityAiPolicy, ActivityAiWorker, ActivityRequest,
};
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::Role;
use mc_testkit::provider::ScriptedTransport;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn observation(id: &str, offset_secs: i64, app: &str, title: &str) -> ObservationSummary {
    ObservationSummary {
        id: id.to_string(),
        at: at(offset_secs),
        app_name: Some(app.to_string()),
        window_title: Some(title.to_string()),
        domain: None,
        text: None,
    }
}

fn vision_response(content: &str) -> String {
    serde_json::json!({
        "choices": [{ "message": { "content": content }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 120, "completion_tokens": 20 }
    })
    .to_string()
}

fn worker(transport: Arc<ScriptedTransport>, policy: ActivityAiPolicy) -> ActivityAiWorker {
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

    ActivityAiWorker::new(Arc::new(provider), policy)
}

fn policy() -> ActivityAiPolicy {
    ActivityAiPolicy {
        ai_enabled: true,
        max_calls_per_window: 3,
        window_secs: 3600,
        // 同一个活动窗口在这段时间内不重复分析
        reanalyze_after_secs: 1800,
    }
}

fn request<'a>(
    observation: &'a ObservationSummary,
    window_key: &'a str,
    offset_secs: i64,
) -> ActivityRequest<'a> {
    ActivityRequest {
        observation,
        window_key,
        image: b"fake-png-bytes",
        mime: "image/png",
        at: at(offset_secs),
        rule_resolved: false,
    }
}

fn good_json() -> &'static str {
    r#"{"title":"架构评审","category":"需求","confidence":0.82}"#
}

#[tokio::test]
async fn unknown_app_is_resolved_by_the_vision_model() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, vision_response(good_json())));
    let mut worker = worker(Arc::clone(&transport), policy());

    let obs = observation("obs-1", 0, "Aurora", "unknown window");
    let outcome = worker
        .resolve(request(&obs, "act-1", 0))
        .await
        .expect("推断路径不应报错");

    match outcome {
        ActivityAiOutcome::Inferred { suggestion, usage } => {
            assert_eq!(suggestion.title, "架构评审");
            assert_eq!(suggestion.category.as_deref(), Some("需求"));
            assert!((suggestion.confidence - 0.82).abs() < 1e-6);
            // 记的是 provider id（含 provider 类型）而不只是模型名：
            // 同一个模型名可能由不同 provider 提供，出问题时必须能区分。
            assert_eq!(
                suggestion.origin,
                Provenance::Inferred {
                    model: "openai_compatible:qwen3-vl".to_string()
                },
                "推断结果必须带上是哪个模型推断的"
            );
            assert_eq!(usage.prompt_tokens, 120, "用量要回传，供记账与预算使用");
        }
        other => panic!("应当得到 Inferred，实际 {other:?}"),
    }

    assert_eq!(transport.call_count(), 1);
}

// 规则已经判出来的活动不该再花钱（这是省 token 的主要手段）
#[tokio::test]
async fn known_activity_never_reaches_the_model() {
    let transport = Arc::new(ScriptedTransport::new());
    let mut worker = worker(Arc::clone(&transport), policy());

    let obs = observation("obs-1", 0, "VSCode", "main.rs");
    let outcome = worker
        .resolve(ActivityRequest {
            observation: &obs,
            window_key: "act-1",
            image: b"fake-png-bytes",
            mime: "image/png",
            at: at(0),
            // 规则已经给出了结论：只保留「规则说的」，不再问模型
            rule_resolved: true,
        })
        .await
        .expect("不该出错");

    assert!(matches!(outcome, ActivityAiOutcome::Skipped { .. }));
    assert_eq!(
        transport.call_count(),
        0,
        "规则命中还调模型，就是「2 小时 300 万 token」的复现"
    );
}

#[tokio::test]
async fn without_budget_the_system_degrades_instead_of_stalling() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, vision_response(good_json())));
    let mut worker = worker(
        Arc::clone(&transport),
        ActivityAiPolicy {
            ai_enabled: false,
            ..policy()
        },
    );

    let obs = observation("obs-1", 0, "Aurora", "unknown window");
    let outcome = worker
        .resolve(request(&obs, "act-1", 0))
        .await
        .expect("没预算不是错误");

    match outcome {
        ActivityAiOutcome::Degraded { reason, .. } => {
            assert!(
                reason.contains("budget") || reason.contains("预算"),
                "原因要说清是预算用尽，实际 {reason}"
            );
        }
        other => panic!("应当降级，实际 {other:?}"),
    }
    assert_eq!(transport.call_count(), 0, "没有预算就一次都不能调");
}

#[tokio::test]
async fn unusable_model_output_degrades_to_observed() {
    for bad in [
        // 不是 JSON
        "我觉得这是一次架构评审",
        // JSON 但没有标题
        r#"{"category":"需求","confidence":0.9}"#,
        // 置信度越界
        r#"{"title":"架构评审","confidence":7.5}"#,
        // 标题是空白
        r#"{"title":"   ","confidence":0.9}"#,
        // 空响应
        "",
    ] {
        let transport = Arc::new(ScriptedTransport::new().push_json(200, vision_response(bad)));
        let mut worker = worker(Arc::clone(&transport), policy());

        let obs = observation("obs-1", 0, "Aurora", "unknown window");
        let outcome = worker
            .resolve(request(&obs, "act-1", 0))
            .await
            .expect("坏输出不该让管线报错");

        match outcome {
            ActivityAiOutcome::Degraded { raw, reason } => {
                assert!(!reason.is_empty(), "降级必须说明原因");
                // 原始输出要留着，便于事后回答「模型到底吐了什么」
                assert_eq!(raw.as_deref(), Some(bad));
            }
            other => panic!("坏输出 `{bad}` 应当降级，实际 {other:?}"),
        }
    }
}

#[tokio::test]
async fn ai_calls_are_rate_limited_within_the_window() {
    let transport = Arc::new(
        ScriptedTransport::new()
            .push_json(200, vision_response(good_json()))
            .push_json(200, vision_response(good_json()))
            .push_json(200, vision_response(good_json()))
            .push_json(200, vision_response(good_json())),
    );
    let mut worker = worker(Arc::clone(&transport), policy());

    // 每次都用不同的活动窗口，避免命中「同一窗口不重复分析」
    for step in 0..4 {
        let obs = observation(&format!("obs-{step}"), step, "Aurora", "unknown window");
        let outcome = worker
            .resolve(request(&obs, &format!("act-{step}"), step))
            .await
            .expect("不该报错");

        if step < 3 {
            assert!(
                matches!(outcome, ActivityAiOutcome::Inferred { .. }),
                "第 {step} 次应当放行，实际 {outcome:?}"
            );
        } else {
            match outcome {
                ActivityAiOutcome::Degraded { reason, .. } => assert!(
                    reason.contains("rate") || reason.contains("限流"),
                    "第 4 次应当被限流，原因 {reason}"
                ),
                other => panic!("第 4 次应当被限流，实际 {other:?}"),
            }
        }
    }

    assert_eq!(transport.call_count(), 3, "窗口内最多只能发 3 次请求");
}

// 窗口滑过之后额度要恢复，否则限流就成了永久停用
#[tokio::test]
async fn rate_limit_window_slides() {
    let transport = Arc::new(
        ScriptedTransport::new()
            .push_json(200, vision_response(good_json()))
            .push_json(200, vision_response(good_json())),
    );
    let mut worker = worker(
        Arc::clone(&transport),
        ActivityAiPolicy {
            max_calls_per_window: 1,
            window_secs: 600,
            reanalyze_after_secs: 0,
            ..policy()
        },
    );

    let first = observation("obs-1", 0, "Aurora", "unknown");
    assert!(matches!(
        worker.resolve(request(&first, "act-1", 0)).await.unwrap(),
        ActivityAiOutcome::Inferred { .. }
    ));

    let second = observation("obs-2", 60, "Aurora", "unknown");
    assert!(matches!(
        worker.resolve(request(&second, "act-2", 60)).await.unwrap(),
        ActivityAiOutcome::Degraded { .. }
    ));

    // 11 分钟后，窗口已滑过
    let third = observation("obs-3", 660, "Aurora", "unknown");
    assert!(
        matches!(
            worker.resolve(request(&third, "act-3", 660)).await.unwrap(),
            ActivityAiOutcome::Inferred { .. }
        ),
        "窗口滑过后必须恢复额度"
    );
}

#[tokio::test]
async fn same_activity_window_is_not_analyzed_twice() {
    let transport = Arc::new(
        ScriptedTransport::new()
            .push_json(200, vision_response(good_json()))
            .push_json(200, vision_response(good_json())),
    );
    let mut worker = worker(Arc::clone(&transport), policy());

    let obs = observation("obs-1", 0, "Aurora", "unknown");
    let first = worker.resolve(request(&obs, "act-1", 0)).await.unwrap();
    assert!(matches!(first, ActivityAiOutcome::Inferred { .. }));

    // 同一个活动窗口，30 秒后又来一条观测
    let again = observation("obs-2", 30, "Aurora", "unknown");
    let second = worker.resolve(request(&again, "act-1", 30)).await.unwrap();

    match second {
        ActivityAiOutcome::Cached { suggestion } => {
            assert_eq!(suggestion.title, "架构评审", "复用的必须是上次的结论");
        }
        other => panic!("同一活动窗口应当复用结论，实际 {other:?}"),
    }
    assert_eq!(
        transport.call_count(),
        1,
        "同一活动窗口内重复分析 = 重复请求"
    );

    // 超过 reanalyze_after 之后允许重新分析（活动可能已经变样）
    let later = observation("obs-3", 1801, "Aurora", "unknown");
    assert!(matches!(
        worker
            .resolve(request(&later, "act-1", 1801))
            .await
            .unwrap(),
        ActivityAiOutcome::Inferred { .. }
    ));
}

// 请求本身要带上足以判断的上下文：只看一张图，模型无从知道这是哪个应用
#[tokio::test]
async fn request_carries_observation_context_and_json_mode() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, vision_response(good_json())));
    let mut worker = worker(Arc::clone(&transport), policy());

    let obs = observation("obs-1", 0, "Aurora", "Quarterly planning");
    worker.resolve(request(&obs, "act-1", 0)).await.unwrap();

    let sent = transport.last();
    let body: serde_json::Value =
        serde_json::from_str(sent.body.as_deref().expect("请求体应当存在")).expect("请求体是 JSON");
    assert_eq!(body["model"], "qwen3-vl");
    assert_eq!(
        body["response_format"]["type"], "json_object",
        "必须要求 JSON 输出，否则解析率会低到把降级率顶上去"
    );

    let content = body["messages"][0]["content"].to_string();
    assert!(
        content.contains("Aurora") && content.contains("Quarterly planning"),
        "提示词必须带上进程名与窗口标题：{content}"
    );
}

// ---------------------------------------------------------------- 批次推断（接线用）

/// 从一批活动里挑出「规则没判出来的」，逐个问模型。
///
/// 这一步是省钱的**真正所在**：规则命中的活动在这里就已经被排除掉了。
#[tokio::test]
async fn batch_only_asks_about_activities_without_a_rule() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, vision_response(good_json())));
    let mut worker = worker(Arc::clone(&transport), policy());

    let activities = vec![
        activity_view(
            "act-rule",
            "写代码",
            Provenance::Rule {
                rule_id: "coding".to_string(),
            },
        ),
        activity_view("act-unknown", "Aurora", Provenance::Observed),
    ];

    let batch = mc_pipeline::activity_ai::infer_activities(
        &mut worker,
        &activities,
        |observation_id| Some((b"png".to_vec(), format!("image/png:{observation_id}"))),
        at(0),
    )
    .await
    .expect("批次推断不该报错");

    assert_eq!(transport.call_count(), 1, "规则命中的活动一次都不该问");
    assert_eq!(batch.suggestions.len(), 1);
    assert_eq!(
        batch.suggestions[0].activity_id, "act-unknown",
        "结论必须挂回正确的活动"
    );
    assert_eq!(batch.suggestions[0].title, "架构评审");
    assert_eq!(
        batch.skipped, 1,
        "被跳过的规则活动要计数，便于解释「为什么这次没调模型」"
    );
}

// 没有像素就没有可推断的依据：宁可退回 Observed，也不要凭进程名让模型编
#[tokio::test]
async fn batch_skips_activities_without_pixels() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, vision_response(good_json())));
    let mut worker = worker(Arc::clone(&transport), policy());

    let activities = vec![activity_view("act-unknown", "Aurora", Provenance::Observed)];
    let batch =
        mc_pipeline::activity_ai::infer_activities(&mut worker, &activities, |_| None, at(0))
            .await
            .expect("不该报错");

    assert_eq!(transport.call_count(), 0, "没有图就别调模型");
    assert!(batch.suggestions.is_empty());
    assert_eq!(batch.skipped, 1);
}

// 用户改过的活动不再问模型：用户已经给了更好的答案
#[tokio::test]
async fn batch_does_not_reinfer_user_modified_activities() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, vision_response(good_json())));
    let mut worker = worker(Arc::clone(&transport), policy());

    let mut modified = activity_view("act-1", "我自己起的名字", Provenance::Observed);
    modified.is_user_modified = true;

    let batch = mc_pipeline::activity_ai::infer_activities(
        &mut worker,
        &[modified],
        |_| Some((b"png".to_vec(), "image/png".to_string())),
        at(0),
    )
    .await
    .unwrap();

    assert_eq!(transport.call_count(), 0);
    assert_eq!(batch.skipped, 1);
}

fn activity_view(id: &str, title: &str, origin: Provenance) -> mc_domain::activity::ActivityView {
    mc_domain::activity::ActivityView {
        id: id.to_string(),
        start: at(0),
        end: at(60),
        title: title.to_string(),
        original_title: title.to_string(),
        category: None,
        observations: vec![mc_domain::activity::ObservationRef {
            id: format!("obs-of-{id}"),
            at: at(0),
        }],
        origin,
        confidence: 1.0,
        is_user_modified: false,
    }
}

// 成本可见性：成功的推断必须留下可记账的一次调用（token / 模型 / 耗时）
#[tokio::test]
async fn successful_inference_is_accounted() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, vision_response(good_json())));
    let mut worker = worker(Arc::clone(&transport), policy());

    let activity = activity_view("act-1", "unknown window", Provenance::Observed);
    let batch = mc_pipeline::activity_ai::infer_activities(
        &mut worker,
        &[activity],
        |_| Some((b"png".to_vec(), "image/png".to_string())),
        at(0),
    )
    .await
    .unwrap();

    assert_eq!(batch.suggestions.len(), 1);
    assert_eq!(batch.calls.len(), 1, "一次模型调用要留下一条记账记录");
    let call = &batch.calls[0];
    assert_eq!(call.result.as_str(), "ok");
    assert_eq!(call.model, "openai_compatible:qwen3-vl");
    assert_eq!(
        call.prompt_tokens, 120,
        "token 数要能对着 provider_calls 表核"
    );
    assert_eq!(call.completion_tokens, 20);
    assert!(call.error_code.is_none());
}

// 输出不可用也算一次调用：钱已经花了，不能因为解析失败就看不见
#[tokio::test]
async fn unusable_output_is_accounted_as_invalid_response() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, vision_response("不是 JSON")));
    let mut worker = worker(Arc::clone(&transport), policy());

    let activity = activity_view("act-1", "unknown window", Provenance::Observed);
    let batch = mc_pipeline::activity_ai::infer_activities(
        &mut worker,
        &[activity],
        |_| Some((b"png".to_vec(), "image/png".to_string())),
        at(0),
    )
    .await
    .unwrap();

    assert!(batch.suggestions.is_empty());
    assert_eq!(batch.degraded, 1);
    assert_eq!(batch.calls.len(), 1, "解析失败也要记账");
    assert_eq!(batch.calls[0].result.as_str(), "invalid_response");
    assert_eq!(batch.calls[0].prompt_tokens, 120);
}

// 没发起请求就不该有记录：限流 / 复用结论都省下了真金白银
#[tokio::test]
async fn calls_that_never_happened_are_not_accounted() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, vision_response(good_json())));
    let mut worker = worker(Arc::clone(&transport), policy());

    let activity = activity_view("act-1", "unknown window", Provenance::Observed);
    let first = mc_pipeline::activity_ai::infer_activities(
        &mut worker,
        std::slice::from_ref(&activity),
        |_| Some((b"png".to_vec(), "image/png".to_string())),
        at(0),
    )
    .await
    .unwrap();
    assert_eq!(first.calls.len(), 1);

    // 同一个活动窗口：复用结论，不再调用模型
    let second = mc_pipeline::activity_ai::infer_activities(
        &mut worker,
        std::slice::from_ref(&activity),
        |_| Some((b"png".to_vec(), "image/png".to_string())),
        at(1),
    )
    .await
    .unwrap();
    assert_eq!(second.calls.len(), 0, "复用结论不该产生新的记账记录");

    // 规则已判：一次都不调
    let mut rule_resolved = activity;
    rule_resolved.origin = Provenance::Rule {
        rule_id: "coding".to_string(),
    };
    let third = mc_pipeline::activity_ai::infer_activities(
        &mut worker,
        &[rule_resolved],
        |_| Some((b"png".to_vec(), "image/png".to_string())),
        at(2),
    )
    .await
    .unwrap();
    assert_eq!(third.calls.len(), 0);
}

// 预算用尽不是「什么都没发生」：要能看见「因为没钱所以没推断」
#[tokio::test]
async fn budget_gate_is_accounted_as_rate_limited() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, vision_response(good_json())));
    let mut policy = policy();
    policy.ai_enabled = false;
    let mut worker = worker(Arc::clone(&transport), policy);

    let activity = activity_view("act-1", "unknown window", Provenance::Observed);
    let batch = mc_pipeline::activity_ai::infer_activities(
        &mut worker,
        &[activity],
        |_| Some((b"png".to_vec(), "image/png".to_string())),
        at(0),
    )
    .await
    .unwrap();

    assert_eq!(transport.call_count(), 0);
    assert_eq!(
        batch.calls.len(),
        1,
        "被闸门拦下也要记账（额度没花，但原因要可见）"
    );
    assert_eq!(batch.calls[0].result.as_str(), "rate_limited");
    assert_eq!(
        batch.calls[0].error_code.as_deref(),
        Some("budget_exhausted")
    );
}

// P2：分类是封闭集合 —— 模型给的近义词/未知值不能原样入库，
// 否则分类标签随时间发散，按 category 聚合时碎成一堆近义标签。
#[tokio::test]
async fn unknown_category_is_normalized_to_other() {
    let transport = Arc::new(ScriptedTransport::new().push_json(
        200,
        vision_response(r#"{"title":"写周报","category":"coding","confidence":0.8}"#),
    ));
    let mut worker = worker(Arc::clone(&transport), policy());

    let obs = observation("obs-1", 0, "VSCode", "main.rs");
    let outcome = worker
        .resolve(request(&obs, "act-1", 0))
        .await
        .expect("推断");

    match outcome {
        ActivityAiOutcome::Inferred { suggestion, .. } => {
            assert_eq!(
                suggestion.category.as_deref(),
                Some("其他"),
                "未知分类必须归一到「其他」，不能原样入库"
            );
        }
        other => panic!("应当得到 Inferred，实际 {other:?}"),
    }
}

// 英文分类要映射回同一套标签：中英环境下同一类活动不能落到不同标签
#[tokio::test]
async fn english_category_maps_to_the_same_label() {
    let transport = Arc::new(ScriptedTransport::new().push_json(
        200,
        vision_response(r#"{"title":"Design doc","category":"documentation","confidence":0.8}"#),
    ));
    let mut worker = worker(Arc::clone(&transport), policy());

    let obs = observation("obs-2", 0, "Docs", "spec.md");
    let outcome = worker
        .resolve(request(&obs, "act-2", 0))
        .await
        .expect("推断");

    match outcome {
        ActivityAiOutcome::Inferred { suggestion, .. } => {
            assert_eq!(suggestion.category.as_deref(), Some("文档"));
        }
        other => panic!("应当得到 Inferred，实际 {other:?}"),
    }
}

// P3：提示词要求「不超过 12 个字」，但约束不能只交给模型自觉 —— 超长标题要截断
#[tokio::test]
async fn overlong_title_is_truncated() {
    let long_title = "把旧库的笔记与待办导进新版数据目录并且逐条核对字段映射与迁移结果";
    let transport = Arc::new(ScriptedTransport::new().push_json(
        200,
        vision_response(&format!(
            r#"{{"title":"{long_title}","category":"开发","confidence":0.8}}"#
        )),
    ));
    let mut worker = worker(Arc::clone(&transport), policy());

    let obs = observation("obs-3", 0, "VSCode", "migrate.rs");
    let outcome = worker
        .resolve(request(&obs, "act-3", 0))
        .await
        .expect("推断");

    match outcome {
        ActivityAiOutcome::Inferred { suggestion, .. } => {
            assert!(
                suggestion.title.chars().count() <= 25,
                "超长标题必须被截断（含省略号 ≤ 25 字符）：{}",
                suggestion.title
            );
            assert!(
                suggestion.title.ends_with('…'),
                "截断要看得出来：{}",
                suggestion.title
            );
        }
        other => panic!("应当得到 Inferred，实际 {other:?}"),
    }
}
