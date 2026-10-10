//! 隐私脱敏必须在**出网之前**发生（`privacy.redact_patterns`）。
//!
//! 与拦截的区别：脱敏不阻止记录，只改写外发内容 —— 本地仍然保留完整信息，
//! 因此脱敏只能做在「构造提示词」的位置，不能做在采集或存储处。
//!
//! 三条断言：
//! 1. 命中模式的内容不出现在**实际发出的请求体**里；
//! 2. 命中次数可见（诊断里能看到「这次会话脱敏了几次」）；
//! 3. 模式写错时**不放行**（fail-closed：退化为本地引擎，而不是照发原文）。

use std::sync::Arc;
use std::time::Duration;

use mc_common::redact::Redactor;
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::ChatProvider;
use mc_server::chat::{ChatEngine, ChatInput, Citation, ExtractOnlyEngine, ProviderChatEngine};
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
use mc_testkit::provider::ScriptedTransport;

const CARD: &str = "6222021234567890";

fn engine_with(patterns: &[&str], transport: Arc<ScriptedTransport>) -> ProviderChatEngine {
    let provider = OpenAiCompatibleProvider::new(
        EndpointConfig {
            base_url: "https://api.example.com/v1".to_string(),
            model: "qwen3-max".to_string(),
            api_key: Some("sk-test".to_string()),
            timeout: Duration::from_secs(5),
            max_image_edge: None,
            max_concurrency: 1,
        },
        transport,
        mc_providers::Role::Chat,
    )
    .expect("provider");

    let redactor = Redactor::new(&patterns.iter().map(|p| p.to_string()).collect::<Vec<_>>())
        .expect("模式合法");

    ProviderChatEngine::with_redactor(
        Arc::new(provider) as Arc<dyn ChatProvider>,
        mc_summary::model::SummaryLocale::from_config("zh-CN"),
        redactor,
    )
}

fn scripted(answer: &str) -> Arc<ScriptedTransport> {
    Arc::new(
        ScriptedTransport::new().push_json(
            200,
            serde_json::json!({
                "choices": [{ "message": { "content": answer }, "finish_reason": "stop" }],
                "usage": { "prompt_tokens": 10, "completion_tokens": 5 }
            })
            .to_string(),
        ),
    )
}

fn input(query: &str, title: &str) -> ChatInput {
    ChatInput {
        query: query.to_string(),
        citations: vec![Citation {
            document_id: "act-1".to_string(),
            title: title.to_string(),
            kind: "activity".to_string(),
        }],
        history: Vec::new(),
    }
}

// ---------------------------------------------------------------- 出网内容

/// **核心断言**：请求体里不出现命中模式的内容（提问与引用标题都算）。
#[tokio::test]
async fn outbound_request_never_contains_redacted_content() {
    let transport = scripted("好的");
    let engine = engine_with(&[r"\d{16}"], Arc::clone(&transport));

    engine
        .answer(&input(
            &format!("我的卡号是 {CARD}，帮我记一下"),
            &format!("处理退款 {CARD}"),
        ))
        .await
        .expect("回答成功");

    let body = transport.last().body.expect("必须发出请求");
    assert!(!body.contains(CARD), "请求体里不得出现被脱敏的内容：{body}");
    assert!(body.contains("[已脱敏]"), "应当留下脱敏占位：{body}");
    assert_eq!(engine.redaction_count(), 2, "提问与引用标题各命中一次");
}

/// 没配规则时原文照发（脱敏不能变成「默认改写用户内容」）
#[tokio::test]
async fn without_patterns_text_is_sent_unchanged() {
    let transport = scripted("好的");
    let engine = engine_with(&[], Arc::clone(&transport));

    engine
        .answer(&input(&format!("卡号 {CARD}"), "普通标题"))
        .await
        .expect("回答成功");

    let body = transport.last().body.expect("必须发出请求");
    assert!(body.contains(CARD), "没有规则时不该改写内容：{body}");
    assert_eq!(engine.redaction_count(), 0);
}

/// 引用标题单独命中也要脱敏（不只是提问）
#[tokio::test]
async fn citation_titles_are_redacted_too() {
    let transport = scripted("好的");
    let engine = engine_with(&["退款".to_string().as_str()], Arc::clone(&transport));

    engine
        .answer(&input("这个怎么处理？", "处理退款申请"))
        .await
        .expect("回答成功");

    let body = transport.last().body.expect("必须发出请求");
    assert!(!body.contains("处理退款申请"), "{body}");
    assert!(body.contains("[已脱敏]"), "{body}");
}

// ---------------------------------------------------------------- 失败即关闭

/// 模式写错时**不能放行**：静默忽略一条写错的正则，等于用户以为自己脱敏了。
#[test]
fn invalid_patterns_do_not_produce_a_working_redactor() {
    let error = Redactor::new(&["[未闭合".to_string()]).expect_err("非法正则必须报错");
    assert_eq!(error.code(), mc_common::error::ErrorCode::ConfigInvalid);
}

/// 降级路径：本地引擎不组装 provider，因此不可能把原文发出去。
#[tokio::test]
async fn local_engine_never_uses_a_model() {
    let engine: Box<dyn ChatEngine> = Box::new(ExtractOnlyEngine::default());
    let answer = engine
        .answer(&ChatInput {
            query: "测试".to_string(),
            citations: Vec::new(),
            history: Vec::new(),
        })
        .await
        .expect("本地引擎必须能回答");
    assert!(answer.model.is_none(), "本地引擎不该有模型参与");
}

// ---------------------------------------------------------------- 总结路径

/// 总结提示词里含活动标题与正文，同样必须在**出网前**脱敏。
///
/// 断言的是实际发出的请求体 —— 与对话路径同一条标准。
#[tokio::test]
async fn summary_prompt_is_redacted_before_upload() {
    use mc_summary::generator::SummaryGenerator;
    use mc_summary::model::{ActivityDigest, StageSummaryInput, SummaryLocale, SummaryRange};
    use mc_summary::template::SummaryTemplate;

    let transport = scripted("总结正文");
    let provider = OpenAiCompatibleProvider::new(
        EndpointConfig {
            base_url: "https://api.example.com/v1".to_string(),
            model: "qwen3-max".to_string(),
            api_key: Some("sk-test".to_string()),
            timeout: Duration::from_secs(5),
            max_image_edge: None,
            max_concurrency: 1,
        },
        Arc::clone(&transport) as Arc<dyn mc_providers::transport::HttpTransport>,
        mc_providers::Role::Chat,
    )
    .expect("provider");

    let redactor = Redactor::new(&[r"\d{16}".to_string()]).expect("模式合法");
    let generator = SummaryGenerator::new(
        Arc::new(provider) as Arc<dyn ChatProvider>,
        SummaryTemplate::default_work_stage(),
        SummaryLocale::from_config("zh-CN"),
    )
    .with_redactor(redactor);

    let at = mc_common::time::Timestamp::from_millis(T0);
    let input = StageSummaryInput {
        stage_id: "stage-1".to_string(),
        range: SummaryRange {
            start: at,
            end: at.plus_millis(600_000),
        },
        timezone: "Asia/Shanghai".to_string(),
        locale: SummaryLocale::from_config("zh-CN"),
        activities: vec![ActivityDigest {
            id: "act-1".to_string(),
            // 标题里带 16 位数字（模拟卡号/订单号）
            title: format!("处理订单 {CARD} 的退款"),
            category: Some("开发".to_string()),
            start: at,
            end: at.plus_millis(300_000),
            observations: 3,
            inferred: false,
        }],
        observation_count: 3,
        blocked_observations: 0,
    };

    let outcome = generator.generate(&input).await;
    assert!(!outcome.is_fallback(), "模型可用时不该走兜底");

    let body = transport.last().body.expect("必须发出请求");
    assert!(
        !body.contains(CARD),
        "总结提示词里不得出现被脱敏的内容：{body}"
    );
    assert!(body.contains("[已脱敏]"), "{body}");
    assert_eq!(generator.redaction_count(), 1);
}

// ---------------------------------------------------------------- 提示词语言（D3）

/// 提示词语言跟随 `general.locale`：设置成英文的用户不该被要求「用中文回答」。
#[tokio::test]
async fn system_prompt_language_follows_locale() {
    let transport = scripted("Sure.");
    // 显式类型：让 unsized coercion 发生在实参位置（与 engine_with 一致）
    let transport_for_engine: Arc<ScriptedTransport> = Arc::clone(&transport);
    let provider = OpenAiCompatibleProvider::new(
        EndpointConfig {
            base_url: "https://api.example.com/v1".to_string(),
            model: "qwen3-max".to_string(),
            api_key: Some("sk-test".to_string()),
            timeout: Duration::from_secs(5),
            max_image_edge: None,
            max_concurrency: 1,
        },
        transport_for_engine,
        mc_providers::Role::Chat,
    )
    .expect("provider");
    let engine = ProviderChatEngine::with_redactor(
        Arc::new(provider) as Arc<dyn ChatProvider>,
        mc_summary::model::SummaryLocale::from_config("en-US"),
        Redactor::new(&[]).expect("模式合法"),
    );

    engine
        .answer(&input("what did I do today", "standup notes"))
        .await
        .expect("回答成功");

    let body = transport.last().body.expect("必须发出请求");
    assert!(
        body.contains("Answer in English"),
        "英文 locale 下必须要求用英文回答：{body}"
    );
    assert!(
        !body.contains("回答用中文"),
        "英文 locale 下不该再要求中文：{body}"
    );
}
