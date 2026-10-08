//! 、4.22、4.26、4.30：总结生成与降级链。
//!
//! 「有阶段必有总结」的第一道防线是正常路径，第二道是兜底。
//! 这里把两条路径放在**同一个入口**里测：
//! `generate()` 的返回类型里没有 `Err` —— 它不可能失败，
//! 只可能降级。这样调用方就没办法「忘记处理失败」。

use std::sync::Arc;
use std::time::Duration;

use mc_common::time::Timestamp;
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::{ChatProvider, Role};
use mc_summary::generator::{GenerateOutcome, SummaryGenerator};
use mc_summary::model::{ActivityDigest, Quality, StageSummaryInput, SummaryLocale, SummaryRange};
use mc_summary::template::SummaryTemplate;
use mc_testkit::provider::ScriptedTransport;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn input() -> StageSummaryInput {
    StageSummaryInput {
        stage_id: "stage-act-1".to_string(),
        range: SummaryRange {
            start: at(0),
            end: at(3600),
        },
        timezone: "Asia/Shanghai".to_string(),
        locale: SummaryLocale::ZhCn,
        activities: vec![
            ActivityDigest {
                id: "act-1".to_string(),
                title: "写代码".to_string(),
                category: Some("开发".to_string()),
                start: at(0),
                end: at(1800),
                observations: 30,
                inferred: false,
            },
            ActivityDigest {
                id: "act-2".to_string(),
                title: "需求评审".to_string(),
                category: Some("需求".to_string()),
                start: at(1800),
                end: at(3600),
                observations: 20,
                inferred: true,
            },
        ],
        observation_count: 50,
        blocked_observations: 2,
    }
}

fn chat_response(content: &str) -> String {
    serde_json::json!({
        "choices": [{ "message": { "content": content }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 300, "completion_tokens": 80 }
    })
    .to_string()
}

fn generator(transport: Arc<ScriptedTransport>, attempts: u32) -> SummaryGenerator {
    let provider = OpenAiCompatibleProvider::new(
        EndpointConfig {
            base_url: "https://api.example.com/v1".to_string(),
            model: "qwen-max".to_string(),
            api_key: Some("sk-test".to_string()),
            timeout: Duration::from_secs(30),
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
    .with_max_attempts(attempts)
}

#[tokio::test]
async fn model_path_produces_a_model_quality_summary() {
    let transport = Arc::new(ScriptedTransport::new().push_json(
        200,
        chat_response("这段时间主要在重构活动引擎，中途参加了需求评审。"),
    ));
    let generator = generator(Arc::clone(&transport), 3);

    let outcome = generator.generate(&input()).await;

    match outcome {
        GenerateOutcome::Model { summary } => {
            assert_eq!(summary.quality, Quality::Model);
            assert!(summary.body_markdown.contains("重构活动引擎"));
            assert_eq!(summary.model.as_deref(), Some("openai_compatible:qwen-max"));
            assert_eq!(summary.prompt_tokens, 300);
            assert_eq!(summary.completion_tokens, 80);
        }
        other => panic!("应当走模型路径，实际 {other:?}"),
    }
    assert_eq!(transport.call_count(), 1);
}

// 结构化字段由模板决定，模型只负责正文
#[tokio::test]
async fn model_path_still_fills_template_fields() {
    let transport =
        Arc::new(ScriptedTransport::new().push_json(200, chat_response("重构与评审各占一半。")));

    let outcome = generator(transport, 1).generate(&input()).await;
    let summary = outcome.summary();

    assert!(
        summary.fields.contains_key("time_range"),
        "{:?}",
        summary.fields
    );
    assert!(summary.fields.contains_key("activities"));
    assert!(summary.fields.contains_key("captured"));
    assert!(
        summary.title.contains("17:00") && summary.title.contains("18:00"),
        "标题用本地时间（Asia/Shanghai）：{}",
        summary.title
    );
}

#[tokio::test]
async fn provider_failure_degrades_to_fallback_instead_of_failing() {
    // 三次都失败：降级前必须把重试机会用满
    let transport = Arc::new(
        ScriptedTransport::new()
            .push_failure(mc_providers::transport::TransportError::Timeout)
            .push_failure(mc_providers::transport::TransportError::Timeout)
            .push_failure(mc_providers::transport::TransportError::Timeout),
    );
    let generator = generator(Arc::clone(&transport), 3);

    let outcome = generator.generate(&input()).await;

    match outcome {
        GenerateOutcome::Fallback { summary, reason } => {
            assert_eq!(summary.quality, Quality::Fallback);
            assert!(!summary.body_markdown.trim().is_empty());
            assert!(
                reason.contains("超时") || reason.contains("timeout"),
                "{reason}"
            );
        }
        other => panic!("provider 失败必须降级，实际 {other:?}"),
    }
    assert_eq!(
        transport.call_count(),
        3,
        "降级前应当按策略重试，而不是一次失败就放弃"
    );
}

#[tokio::test]
async fn empty_model_output_degrades_to_fallback() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, chat_response("   \n  ")));
    let outcome = generator(transport, 2).generate(&input()).await;

    assert!(matches!(outcome, GenerateOutcome::Fallback { .. }));
    assert!(!outcome.summary().body_markdown.trim().is_empty());
}

// 4.35 的生成器版本：无论模型怎么坏，产物都不为空
#[tokio::test]
async fn every_failure_rate_still_yields_a_summary() {
    let failing = || mc_providers::transport::TransportError::Timeout;

    // 0% / 50% / 100% 失败
    for script in [
        vec![chat_response("顺利生成。")],
        vec![chat_response("重试后成功。")], // 前两次失败见下
        vec![],
    ] {
        let mut transport = ScriptedTransport::new();
        for reply in &script {
            transport = transport.push_json(200, reply.clone());
        }
        if script.is_empty() {
            for _ in 0..3 {
                transport = transport.push_failure(failing());
            }
        }
        let transport = Arc::new(transport);

        let outcome = generator(Arc::clone(&transport), 3)
            .generate(&input())
            .await;
        let summary = outcome.summary();

        assert!(
            !summary.body_markdown.trim().is_empty(),
            "任何失败率下都必须有非空总结"
        );
    }
}

// 提示词必须带**结构化证据**，而不是把原始采集塞进去（R2：总结有用性）
#[tokio::test]
async fn request_carries_structured_evidence() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, chat_response("好。")));
    generator(Arc::clone(&transport), 1)
        .generate(&input())
        .await;

    let sent = transport.last();
    let body: serde_json::Value = serde_json::from_str(sent.body.as_deref().unwrap()).unwrap();
    let rendered = body.to_string();

    assert!(rendered.contains("写代码"), "证据里要有活动标题");
    assert!(rendered.contains("需求评审"));
    assert!(
        rendered.contains("推测") || rendered.contains("inferred"),
        "推测出来的活动必须在证据里标注，模型才能保留这个不确定性"
    );
    assert!(
        !rendered.contains("base64") && !rendered.contains("image_url"),
        "总结不该塞原始图片：又贵又没用"
    );
    assert_eq!(
        body["response_format"],
        serde_json::Value::Null,
        "总结是散文，不该强制 JSON 输出"
    );
}

// P1：证据渲染必须跟随 locale。
// 英文提示词配中文证据会让模型跨语言理解，质量不稳定（与对话提示词的语言问题同源）。
#[tokio::test]
async fn evidence_labels_follow_locale() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, chat_response("Done.")));
    // 显式类型：让 unsized coercion 发生在实参位置
    let transport_for_provider: Arc<ScriptedTransport> = Arc::clone(&transport);
    let provider = OpenAiCompatibleProvider::new(
        EndpointConfig {
            base_url: "https://api.example.com/v1".to_string(),
            model: "qwen-max".to_string(),
            api_key: Some("sk-test".to_string()),
            timeout: Duration::from_secs(30),
            max_image_edge: None,
            max_concurrency: 1,
        },
        transport_for_provider,
        Role::Chat,
    )
    .expect("provider");
    let generator = SummaryGenerator::new(
        Arc::new(provider) as Arc<dyn ChatProvider>,
        SummaryTemplate::default_work_stage(),
        SummaryLocale::EnUs,
    );

    generator.generate(&input()).await;

    let sent = transport.last();
    let rendered = sent.body.clone().unwrap_or_default();

    assert!(
        rendered.contains("Time range"),
        "英文 locale 的证据必须是英文标签：{rendered}"
    );
    assert!(
        rendered.contains("Activities (chronological)"),
        "活动标题也要跟着走：{rendered}"
    );
    assert!(
        !rendered.contains("时间范围")
            && !rendered.contains("采集量")
            && !rendered.contains("总结里需要覆盖"),
        "英文 locale 下不该再出现中文提示词片段（证据标签与字段清单头）：{rendered}"
    );
    assert!(
        rendered.contains("The summary must cover these fields"),
        "字段清单的头也要是英文：{rendered}"
    );
}
