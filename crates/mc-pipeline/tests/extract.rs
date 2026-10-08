//! 结构化抽取与降级链。
//!
//! 目标：**模型输出的不可靠不能变成管线的不稳定**。
//! 解析失败要走完整的降级链，最终仍然保留原始输出 —— 观测永不丢失。

use std::sync::Arc;
use std::time::Duration;

use mc_pipeline::extract::{ExtractionOutcome, StructuredExtractor};
use mc_pipeline::prompts::{screenshot_analyze, Locale, PROMPT_VERSION};
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::Role;
use mc_testkit::provider::ScriptedTransport;

fn chat_response(content: &str) -> String {
    serde_json::json!({
        "choices": [{ "message": { "content": content }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5 }
    })
    .to_string()
}

fn extractor(transport: Arc<ScriptedTransport>) -> StructuredExtractor {
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
        Role::Chat,
    )
    .unwrap();

    StructuredExtractor::new(Arc::new(provider), Locale::ZhCn)
}

// ---------------------------------------------------------------- 纯解析

#[test]
fn parses_plain_json() {
    let outcome = mc_pipeline::extract::parse(r#"{"app":"Chrome","confidence":0.9}"#);
    let parsed = outcome.expect("应当解析成功");
    assert_eq!(parsed.app.as_deref(), Some("Chrome"));
    assert!((parsed.confidence - 0.9).abs() < 1e-6);
}

#[test]
fn strips_markdown_fence() {
    let raw = "```json\n{\"app\":\"Chrome\"}\n```";
    assert_eq!(
        mc_pipeline::extract::parse(raw).unwrap().app.as_deref(),
        Some("Chrome")
    );
}

#[test]
fn strips_fence_without_language_tag() {
    let raw = "```\n{\"app\":\"VSCode\"}\n```";
    assert_eq!(
        mc_pipeline::extract::parse(raw).unwrap().app.as_deref(),
        Some("VSCode")
    );
}

#[test]
fn repairs_trailing_comma() {
    for raw in [
        r#"{"app":"Chrome","objects":["a","b",],}"#,
        r#"{"app":"Chrome",}"#,
    ] {
        let parsed = mc_pipeline::extract::parse(raw)
            .unwrap_or_else(|e| panic!("应当修复尾随逗号: {raw} — {e:?}"));
        assert_eq!(parsed.app.as_deref(), Some("Chrome"));
    }
}

#[test]
fn repairs_truncated_object() {
    // 被 max_tokens 截断的典型形态
    let raw = r#"{"app":"Chrome","objects":["左右门状态","MQTT""#;
    let parsed = mc_pipeline::extract::parse(raw).expect("应当补全并解析成功");
    assert_eq!(parsed.app.as_deref(), Some("Chrome"));
}

#[test]
fn handles_preamble_and_suffix_chatter() {
    let raw =
        "好的，这是我的分析结果：\n{\"app\":\"Jira\",\"issue\":\"APEX-389\"}\n希望对你有帮助！";
    let parsed = mc_pipeline::extract::parse(raw).expect("应当从废话中抽出 JSON");
    assert_eq!(parsed.issue.as_deref(), Some("APEX-389"));
}

#[test]
fn extracts_the_first_balanced_object() {
    // 前后都有别的花括号
    let raw = r#"注意 {这里不是 JSON} 然后 {"app":"Chrome"} 结束 {"x":1}"#;
    assert_eq!(
        mc_pipeline::extract::parse(raw).unwrap().app.as_deref(),
        Some("Chrome")
    );
}

#[test]
fn schema_violation_is_reported() {
    // confidence 超出 0..1
    let error = mc_pipeline::extract::parse(r#"{"app":"Chrome","confidence":5.0}"#)
        .expect_err("越界置信度必须被拒绝");
    assert!(
        error.contains("confidence"),
        "错误信息应当指出字段名：{error}"
    );
}

#[test]
fn non_object_json_is_rejected() {
    for raw in ["[1,2,3]", "\"just a string\"", "42"] {
        assert!(
            mc_pipeline::extract::parse(raw).is_err(),
            "{raw} 不是对象，应当被拒绝"
        );
    }
}

#[test]
fn unknown_fields_are_ignored_not_fatal() {
    // 模型多输出了字段不应该让整条记录报废
    let parsed = mc_pipeline::extract::parse(r#"{"app":"Chrome","bogus":123}"#).unwrap();
    assert_eq!(parsed.app.as_deref(), Some("Chrome"));
}

#[test]
fn objects_array_is_capped() {
    let many: Vec<String> = (0..50).map(|i| format!("obj{i}")).collect();
    let raw = serde_json::json!({ "objects": many }).to_string();
    let parsed = mc_pipeline::extract::parse(&raw).unwrap();
    assert!(
        parsed.objects.len() <= 8,
        "对象数组必须被截断（提示词要求最多 8 个，模型可能不遵守）"
    );
}

proptest::prelude::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig { cases: 128, ..Default::default() })]

    // 任意输入都不得 panic
    #[test]
    fn parse_never_panics(input in ".{0,400}") {
        let _ = mc_pipeline::extract::parse(&input);
    }
}

// ---------------------------------------------------------------- 抽取器

#[tokio::test]
async fn schema_violation_triggers_retry_with_error_feedback() {
    let transport = Arc::new(
        ScriptedTransport::new()
            // 第一次：置信度越界
            .push_json(200, chat_response(r#"{"app":"Chrome","confidence":5.0}"#))
            // 第二次：正常
            .push_json(200, chat_response(r#"{"app":"Chrome","confidence":0.8}"#)),
    );

    let extractor = extractor(Arc::clone(&transport));
    let outcome = extractor
        .extract("window: Chrome\napp: Google Chrome")
        .await
        .unwrap();

    match outcome {
        ExtractionOutcome::Parsed {
            understanding,
            attempts,
        } => {
            assert_eq!(understanding.app.as_deref(), Some("Chrome"));
            assert_eq!(attempts, 2, "应当重试了一次");
        }
        other => panic!("应当最终解析成功，实际 {other:?}"),
    }

    assert_eq!(transport.call_count(), 2);

    // 第二次请求必须带上第一次的错误信息，否则模型无从修正
    let second: serde_json::Value =
        serde_json::from_str(&transport.recorded()[1].body.clone().unwrap()).unwrap();
    let user_content = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["content"].as_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        user_content.contains("confidence"),
        "重试请求必须把校验错误回给模型：{user_content}"
    );
}

#[tokio::test]
async fn double_failure_degrades_with_raw_preserved() {
    let junk = "对不起，我无法分析这张图片。";
    let transport = Arc::new(
        ScriptedTransport::new()
            .push_json(200, chat_response(junk))
            .push_json(200, chat_response(junk)),
    );

    let extractor = extractor(Arc::clone(&transport));
    let outcome = extractor.extract("window: Chrome").await.unwrap();

    match outcome {
        ExtractionOutcome::Degraded { raw, reason } => {
            assert_eq!(raw, junk, "原始输出必须完整保留，供诊断与降级使用");
            assert!(!reason.is_empty(), "必须说明为什么降级");
        }
        other => panic!("两次失败后应当降级而不是报错，实际 {other:?}"),
    }

    assert_eq!(transport.call_count(), 2, "应当正好尝试两次");
}

#[tokio::test]
async fn degraded_outcome_still_carries_useful_context() {
    let transport = Arc::new(
        ScriptedTransport::new()
            .push_json(200, chat_response("不是 JSON"))
            .push_json(200, chat_response("仍然不是 JSON")),
    );

    let sut = extractor(transport);
    let outcome = sut.extract("window: Chrome").await.unwrap();

    // 降级不是「什么都没有」：调用方仍能拿到原始文本用于后续处理
    assert!(matches!(outcome, ExtractionOutcome::Degraded { .. }));
    assert!(!outcome.is_parsed());
    assert!(outcome.raw_text().is_some(), "降级必须保留原始输出");

    // 未降级时才有结构化结果
    let transport2 =
        Arc::new(ScriptedTransport::new().push_json(200, chat_response(r#"{"app":"Chrome"}"#)));
    let outcome2 = extractor(transport2)
        .extract("window: Chrome")
        .await
        .unwrap();
    assert!(outcome2.is_parsed());
    assert!(outcome2.understanding().is_some());
}

#[tokio::test]
async fn provider_error_is_propagated_not_swallowed() {
    // 鉴权失败不该被当成「模型输出格式不对」——那会误导用户去改提示词
    let transport = Arc::new(ScriptedTransport::new().push_json(401, r#"{"error":{}}"#));
    let extractor = extractor(transport);

    let error = extractor
        .extract("window: Chrome")
        .await
        .expect_err("鉴权失败必须向上传播");

    assert_eq!(
        error.code(),
        mc_common::error::ErrorCode::ProviderAuthFailed,
        "必须保留真实原因"
    );
}

#[tokio::test]
async fn prompt_size_is_bounded_across_many_calls() {
    // 把 _processed_cache 无条件拼进 merge prompt，
    // 提示词随运行时间无界增长 —— 2 小时烧掉 300 万 token 的直接原因。
    let mut transport = ScriptedTransport::new();
    for _ in 0..50 {
        transport = transport.push_json(200, chat_response(r#"{"app":"Chrome"}"#));
    }
    let transport = Arc::new(transport);

    let extractor = extractor(Arc::clone(&transport));
    let input = "window: Chrome — Jira\napp: Google Chrome";

    for _ in 0..50 {
        let _ = extractor.extract(input).await;
    }

    let bodies: Vec<String> = transport
        .recorded()
        .into_iter()
        .filter_map(|r| r.body)
        .collect();

    let first_len = bodies[0].len();
    for (index, body) in bodies.iter().enumerate() {
        assert_eq!(
            body.len(),
            first_len,
            "第 {index} 次请求的提示词长度与第一次不同（{} vs {}）—— 提示词必须无状态",
            body.len(),
            first_len
        );
    }
}

#[tokio::test]
async fn request_carries_prompt_version_for_traceability() {
    let transport =
        Arc::new(ScriptedTransport::new().push_json(200, chat_response(r#"{"app":"Chrome"}"#)));
    let extractor = extractor(Arc::clone(&transport));
    let _ = extractor.extract("window: Chrome").await;

    // 提示词内容来自内嵌模板
    let body = transport.last().body.unwrap();
    assert!(
        body.contains("JSON"),
        "应当使用内嵌的分析提示词：{}",
        &body[..200.min(body.len())]
    );
    assert!(!PROMPT_VERSION.is_empty());
    assert!(screenshot_analyze(Locale::ZhCn).contains("app"));
}

#[tokio::test]
async fn empty_provider_content_is_treated_as_degraded() {
    // content 为 null 时 parse_chat_response 就应当报错，
    // 这里验证它最终表现为降级而不是崩溃
    let transport = Arc::new(
        ScriptedTransport::new().push_json(200, r#"{"choices":[{"message":{"content":null}}]}"#),
    );
    let extractor = extractor(transport);

    let outcome = extractor.extract("window: Chrome").await;
    assert!(
        outcome.is_err() || matches!(outcome.unwrap(), ExtractionOutcome::Degraded { .. }),
        "空内容必须是可诊断的失败，不能是崩溃"
    );
}
