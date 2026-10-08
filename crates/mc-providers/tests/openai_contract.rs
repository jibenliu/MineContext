//! OpenAI 兼容 Provider 的契约。
//!
//! 这一组测试钉住两条契约：
//! - **错误不能说谎**：真实的 401 必须变成 `AuthFailed`，而不是 UI 上的 "timeout"
//! - **只走标准契约**：请求路径/字段必须是 OpenAI 的标准形状，
//!   绝不能出现厂商特化（`multimodal_embeddings` / `thinking` 之类）

use std::sync::Arc;
use std::time::Duration;

use mc_providers::error::ProviderError;
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::transport::TransportError;
use mc_providers::{
    ChatMessage, ChatProvider, ChatRequest, EmbeddingProvider, Provider, Role, VisionProvider,
};
use mc_testkit::provider::ScriptedTransport;

fn config(base_url: &str) -> EndpointConfig {
    EndpointConfig {
        base_url: base_url.to_string(),
        model: "qwen3-vl".to_string(),
        api_key: Some("sk-test-key".to_string()),
        timeout: Duration::from_secs(30),
        max_image_edge: Some(1024),
        max_concurrency: 2,
    }
}

fn provider(
    base_url: &str,
    transport: Arc<ScriptedTransport>,
    role: Role,
) -> OpenAiCompatibleProvider {
    OpenAiCompatibleProvider::new(config(base_url), transport, role).expect("Provider 必须可构造")
}

fn chat_request() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage::user("你好")],
        max_tokens: Some(128),
        temperature: Some(0.2),
        json_mode: false,
    }
}

// ---------------------------------------------------------------- 错误映射

async fn expect_error(base_url: &str, transport: Arc<ScriptedTransport>) -> ProviderError {
    let provider = provider(base_url, transport, Role::Chat);
    provider
        .complete(&chat_request())
        .await
        .expect_err("应当失败")
}

#[tokio::test]
async fn maps_401_to_auth_failed() {
    let transport = Arc::new(
        ScriptedTransport::new().push_json(401, r#"{"error":{"message":"invalid api key"}}"#),
    );
    match expect_error("https://api.example.com/v1", transport).await {
        ProviderError::AuthFailed { status, .. } => assert_eq!(status, 401),
        other => panic!("401 必须映射为 AuthFailed，实际 {other:?}"),
    }
}

#[tokio::test]
async fn maps_403_to_auth_failed() {
    let transport = Arc::new(ScriptedTransport::new().push_json(403, r#"{"error":{}}"#));
    assert!(matches!(
        expect_error("https://api.example.com/v1", transport).await,
        ProviderError::AuthFailed { status: 403, .. }
    ));
}

#[tokio::test]
async fn maps_404_model_not_found() {
    let transport = Arc::new(
        ScriptedTransport::new().push_json(404, r#"{"error":{"message":"model not found"}}"#),
    );
    match expect_error("https://api.example.com/v1", transport).await {
        ProviderError::NotFound { model } => {
            assert_eq!(model, "qwen3-vl", "必须带上模型名，用户才知道改哪个");
        }
        other => panic!("404 必须映射为 NotFound，实际 {other:?}"),
    }
}

#[tokio::test]
async fn maps_429_honours_retry_after() {
    let transport = Arc::new(ScriptedTransport::new().push_json_with_retry_after(
        429,
        r#"{"error":{"message":"rate limited"}}"#,
        Duration::from_secs(7),
    ));
    match expect_error("https://api.example.com/v1", transport).await {
        ProviderError::RateLimited { retry_after } => {
            assert_eq!(
                retry_after,
                Some(Duration::from_secs(7)),
                "必须尊重 Retry-After"
            );
        }
        other => panic!("429 必须映射为 RateLimited，实际 {other:?}"),
    }
}

#[tokio::test]
async fn maps_5xx_to_retryable_errors() {
    // 普通 5xx → ServerError
    for status in [500u16, 502, 503] {
        let transport = Arc::new(ScriptedTransport::new().push_json(status, r#"{"error":{}}"#));
        let error = expect_error("https://api.example.com/v1", transport).await;
        match error {
            ProviderError::ServerError { status: got } => assert_eq!(got, status),
            other => panic!("{status} 必须映射为 ServerError，实际 {other:?}"),
        }
    }

    // 504 是**网关超时**，归到 Timeout 更准确（用户文案是「响应超时」而不是「服务异常」）；
    // 两者都可重试，因此不影响调度行为。
    for status in [408u16, 504] {
        let transport = Arc::new(ScriptedTransport::new().push_json(status, r#"{"error":{}}"#));
        let error = expect_error("https://api.example.com/v1", transport).await;
        assert!(
            matches!(error, ProviderError::Timeout),
            "{status} 应映射为 Timeout，实际 {error:?}"
        );
    }

    // 整族 5xx 都必须可重试
    for status in [500u16, 502, 503, 504] {
        let transport = Arc::new(ScriptedTransport::new().push_json(status, r#"{"error":{}}"#));
        assert!(
            expect_error("https://api.example.com/v1", transport)
                .await
                .retryable(),
            "{status} 必须可重试"
        );
    }
}

#[tokio::test]
async fn maps_connection_failure() {
    let transport = Arc::new(
        ScriptedTransport::new().push_failure(TransportError::Connect("connection refused".into())),
    );
    assert!(matches!(
        expect_error("https://api.example.com/v1", transport).await,
        ProviderError::Connection { .. }
    ));
}

#[tokio::test]
async fn maps_timeout() {
    let transport = Arc::new(ScriptedTransport::new().push_failure(TransportError::Timeout));
    assert!(matches!(
        expect_error("https://api.example.com/v1", transport).await,
        ProviderError::Timeout
    ));
}

#[tokio::test]
async fn maps_non_json_body_to_invalid_response() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, "<html>gateway error</html>"));
    assert!(matches!(
        expect_error("https://api.example.com/v1", transport).await,
        ProviderError::InvalidResponse { .. }
    ));
}

#[tokio::test]
async fn maps_missing_choices_to_invalid_response() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, r#"{"id":"x"}"#));
    assert!(matches!(
        expect_error("https://api.example.com/v1", transport).await,
        ProviderError::InvalidResponse { .. }
    ));
}

#[tokio::test]
async fn unconfigured_provider_fails_before_sending() {
    let transport = Arc::new(ScriptedTransport::new());
    let provider = OpenAiCompatibleProvider::new(
        EndpointConfig {
            base_url: String::new(),
            model: String::new(),
            api_key: None,
            ..config("")
        },
        Arc::clone(&transport) as Arc<ScriptedTransport>,
        Role::Chat,
    )
    .unwrap();

    let error = provider.complete(&chat_request()).await.unwrap_err();
    assert!(matches!(error, ProviderError::Unconfigured));
    assert_eq!(
        transport.call_count(),
        0,
        "未配置时不应发出任何请求（避免把无意义的流量打出去）"
    );
}

// ---------------------------------------------------------------- 重试语义

#[tokio::test]
async fn retryability_matches_design() {
    // 只有这些值得重试
    for retryable in [
        ProviderError::RateLimited { retry_after: None },
        ProviderError::Timeout,
        ProviderError::Connection {
            detail: String::new(),
        },
        ProviderError::ServerError { status: 503 },
    ] {
        assert!(retryable.retryable(), "{retryable:?} 应当可重试");
    }

    // 对 401/404/不支持的能力重试只会浪费时间与 token
    for fatal in [
        ProviderError::AuthFailed {
            status: 401,
            detail: String::new(),
        },
        ProviderError::NotFound {
            model: "m".to_string(),
        },
        ProviderError::Unsupported {
            feature: "vision".to_string(),
        },
        ProviderError::InvalidResponse {
            detail: String::new(),
        },
        ProviderError::Unconfigured,
    ] {
        assert!(!fatal.retryable(), "{fatal:?} 不应重试");
    }
}

#[tokio::test]
async fn every_error_maps_to_a_typed_app_error_with_remediation() {
    let samples = [
        ProviderError::AuthFailed {
            status: 401,
            detail: "bad key".to_string(),
        },
        ProviderError::NotFound {
            model: "m".to_string(),
        },
        ProviderError::RateLimited { retry_after: None },
        ProviderError::Connection {
            detail: "refused".to_string(),
        },
        ProviderError::Unsupported {
            feature: "vision".to_string(),
        },
    ];

    for sample in samples {
        let app = sample.to_app_error();
        assert_eq!(
            app.component(),
            mc_common::error::Component::Provider,
            "{sample:?} 的组件必须归类为 provider"
        );
        assert!(!app.user_message().is_empty(), "{sample:?} 缺少用户文案");
        assert!(
            app.remediation().is_some(),
            "{sample:?} 是可操作错误，必须给出建议"
        );
        // 用户文案不能把 SDK 细节漏出去
        assert!(
            !app.user_message().contains("AttributeError"),
            "用户文案泄漏技术细节：{}",
            app.user_message()
        );
    }
}

// ---------------------------------------------------------------- 请求形状

#[tokio::test]
async fn never_calls_vendor_specific_paths() {
    let transport = Arc::new(ScriptedTransport::new().push_json(
        200,
        r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#,
    ));
    let provider = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Chat,
    );
    let response = provider.complete(&chat_request()).await.unwrap();
    assert_eq!(response.model, "qwen3-vl");

    let request = transport.last();
    assert_eq!(request.method, "POST");
    assert_eq!(request.url, "https://api.example.com/v1/chat/completions");

    let body = request.body.expect("必须有请求体");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["model"], "qwen3-vl");
    // 厂商特化字段一律不许出现
    for forbidden in ["thinking", "multimodal_embeddings", "ark", "volcengine"] {
        assert!(
            !body.contains(forbidden),
            "请求体出现了厂商特化字段 `{forbidden}`：{body}"
        );
    }
}

#[tokio::test]
async fn sends_bearer_token() {
    let transport = Arc::new(
        ScriptedTransport::new().push_json(200, r#"{"choices":[{"message":{"content":"ok"}}]}"#),
    );
    let provider = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Chat,
    );
    provider.complete(&chat_request()).await.unwrap();

    let request = transport.last();
    let auth = request
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v.clone())
        .expect("必须带 Authorization 头");
    assert_eq!(auth, "Bearer sk-test-key");
}

#[tokio::test]
async fn base_url_with_and_without_v1_never_doubles_v1() {
    for (base, expected) in [
        (
            "https://api.example.com/v1",
            "https://api.example.com/v1/chat/completions",
        ),
        (
            "https://api.example.com",
            "https://api.example.com/v1/chat/completions",
        ),
        (
            "https://api.example.com/v1/",
            "https://api.example.com/v1/chat/completions",
        ),
        (
            "http://localhost:11434/v1",
            "http://localhost:11434/v1/chat/completions",
        ),
        (
            "http://localhost:1234",
            "http://localhost:1234/v1/chat/completions",
        ),
    ] {
        let transport = Arc::new(
            ScriptedTransport::new()
                .push_json(200, r#"{"choices":[{"message":{"content":"ok"}}]}"#),
        );
        let provider = provider(base, Arc::clone(&transport), Role::Chat);
        provider.complete(&chat_request()).await.unwrap();

        let url = transport.last().url;
        assert_eq!(url, expected, "base_url={base} 拼接错误");
        assert!(!url.contains("v1/v1"), "base_url={base} 出现了 v1/v1");
    }
}

#[tokio::test]
async fn json_mode_sets_response_format() {
    let transport = Arc::new(
        ScriptedTransport::new().push_json(200, r#"{"choices":[{"message":{"content":"{}"}}]}"#),
    );
    let provider = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Chat,
    );

    let mut request = chat_request();
    request.json_mode = true;
    provider.complete(&request).await.unwrap();

    let body: serde_json::Value = serde_json::from_str(&transport.last().body.unwrap()).unwrap();
    assert_eq!(body["response_format"]["type"], "json_object");
}

// ---------------------------------------------------------------- 视觉

#[tokio::test]
async fn vision_request_uses_data_url_with_correct_mime() {
    let transport = Arc::new(ScriptedTransport::new().push_json(
        200,
        r#"{"choices":[{"message":{"content":"{\"app\":\"Chrome\"}"}}]}"#,
    ));
    let provider = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Vision,
    );

    // 一张可以被编码成 JPEG 的图
    let image = image::RgbImage::from_pixel(64, 48, image::Rgb([200, 100, 50]));
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80)
        .encode(image.as_raw(), 64, 48, image::ExtendedColorType::Rgb8)
        .unwrap();

    provider
        .analyze(mc_providers::VisionRequest {
            image: jpeg,
            mime: "image/jpeg".to_string(),
            prompt: "描述这张图".to_string(),
            max_tokens: Some(256),
            json_mode: true,
        })
        .await
        .unwrap();

    let body: serde_json::Value = serde_json::from_str(&transport.last().body.unwrap()).unwrap();
    let url = body["messages"][0]["content"][1]["image_url"]["url"]
        .as_str()
        .expect("必须用 image_url 结构");

    assert!(
        url.starts_with("data:image/jpeg;base64,"),
        "MIME 必须与实际编码一致（不能无论什么文件都声称 image/png）：{}",
        &url[..40.min(url.len())]
    );
}

#[tokio::test]
async fn embedding_dimensions_come_from_the_response_not_a_constant() {
    // 故意用 3 维，而不是任何「常见」维度
    let transport = Arc::new(ScriptedTransport::new().push_json(
        200,
        r#"{"data":[{"embedding":[0.1,0.2,0.3],"index":0}],"model":"m"}"#,
    ));
    let provider = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Embedding,
    );

    let response = provider.embed(&["hello".to_string()]).await.unwrap();
    let vectors = response.vectors;

    assert_eq!(vectors.len(), 1);
    assert_eq!(
        vectors[0].len(),
        3,
        "维度必须来自响应，而不是硬编码：端点配置成 2048 时必须报 2048"
    );

    // 端点也必须是标准 embeddings 路径
    assert_eq!(
        transport.last().url,
        "https://api.example.com/v1/embeddings"
    );
}

// 5.8 的前提：Embedding 也要计入成本。
// 的成本报表里只有 chat/vision，embedding 的 token 花得像没发生过一样。
#[tokio::test]
async fn embedding_surfaces_usage_for_cost_accounting() {
    let transport = Arc::new(ScriptedTransport::new().push_json(
        200,
        r#"{"data":[{"embedding":[0.1,0.2,0.3],"index":0}],
            "usage":{"prompt_tokens":17,"total_tokens":17}}"#,
    ));
    let provider = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Embedding,
    );

    let response = provider.embed(&["hello".to_string()]).await.unwrap();

    assert_eq!(response.vectors.len(), 1);
    assert_eq!(
        response.usage.prompt_tokens, 17,
        "embedding 的用量必须能记账"
    );
    assert_eq!(
        response.usage.completion_tokens, 0,
        "embedding 没有补全 token"
    );
}

// 有的自建网关不返回 usage —— 那也不能算失败
#[tokio::test]
async fn embedding_without_usage_is_still_successful() {
    let transport = Arc::new(
        ScriptedTransport::new().push_json(200, r#"{"data":[{"embedding":[1.0],"index":0}]}"#),
    );
    let provider = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Embedding,
    );

    let response = provider.embed(&["hello".to_string()]).await.unwrap();
    assert_eq!(response.usage.prompt_tokens, 0);
    assert_eq!(response.vectors[0], vec![1.0]);
}

#[tokio::test]
async fn embedding_batches_all_inputs_in_one_request() {
    let transport = Arc::new(ScriptedTransport::new().push_json(
        200,
        r#"{"data":[{"embedding":[1.0],"index":0},{"embedding":[2.0],"index":1}]}"#,
    ));
    let provider = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Embedding,
    );

    let response = provider
        .embed(&["a".to_string(), "b".to_string()])
        .await
        .unwrap();
    let vectors = response.vectors;

    assert_eq!(vectors.len(), 2);
    assert_eq!(transport.call_count(), 1, "应当一次批量请求，而不是两次");
}

// ---------------------------------------------------------------- 能力

#[tokio::test]
async fn capabilities_declare_vision_support_by_role() {
    let transport = Arc::new(ScriptedTransport::new());
    let vision = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Vision,
    );
    let chat = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Chat,
    );
    let embedding = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Embedding,
    );

    assert!(vision.capabilities().supports_images);
    assert!(!chat.capabilities().supports_images);
    assert!(embedding.capabilities().supports_embeddings);
    assert_eq!(vision.capabilities().max_concurrency, 2);
}

#[tokio::test]
async fn health_actually_probes_instead_of_reporting_a_flag() {
    // GlobalVLMClient 在初始化失败时也把 _auto_initialized 置 True，
    // 于是 /api/health 报告 llm healthy 而客户端其实是 None。
    let transport = Arc::new(ScriptedTransport::new().push_json(401, r#"{"error":{}}"#));
    let provider = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Chat,
    );

    let health = provider.health().await;
    assert!(!health.healthy, "鉴权失败时健康检查必须报告不健康");
    assert!(transport.call_count() > 0, "健康检查必须真的发出请求");
}

#[tokio::test]
async fn health_reports_healthy_on_success() {
    let transport = Arc::new(
        ScriptedTransport::new().push_json(200, r#"{"choices":[{"message":{"content":"pong"}}]}"#),
    );
    let provider = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Chat,
    );

    assert!(provider.health().await.healthy);
}

// ---------------------------------------------------------------- 流式（真 token 流）

/// 三帧增量 + 结束帧：既要有增量，也要能拼回完整答案。
fn sse_frames() -> Vec<String> {
    vec![
        "data: {\"model\":\"qwen3-vl\",\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"好\"}}]}\n\n".to_string(),
        "data: {\"choices\":[{\"delta\":{\"content\":\"！\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":3}}\n\n".to_string(),
        "data: [DONE]\n\n".to_string(),
    ]
}

async fn stream_deltas(provider: &OpenAiCompatibleProvider) -> (Vec<String>, String) {
    let deltas = std::sync::Mutex::new(Vec::new());
    let response = provider
        .stream(&chat_request(), &mut |delta: &str| {
            deltas.lock().unwrap().push(delta.to_string())
        })
        .await
        .expect("流式调用应当成功");
    (deltas.into_inner().unwrap(), response.text)
}

#[tokio::test]
async fn stream_emits_incremental_deltas_and_reassembles_text() {
    let transport = Arc::new(ScriptedTransport::new().push_sse(200, sse_frames()));
    let provider = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Chat,
    );

    let (deltas, text) = stream_deltas(&provider).await;

    assert_eq!(
        deltas,
        vec!["你", "好", "！"],
        "增量必须逐帧到达，而不是整段一次"
    );
    assert_eq!(text, "你好！", "拼接结果必须与增量一致");
    let request = transport.last();
    assert!(
        request
            .body
            .as_deref()
            .unwrap_or_default()
            .contains("\"stream\":true"),
        "流式调用必须显式带 stream:true，否则服务端返回整段：{:?}",
        request.body
    );
}

#[tokio::test]
async fn stream_reports_model_and_usage() {
    let transport = Arc::new(ScriptedTransport::new().push_sse(200, sse_frames()));
    let provider = provider("https://api.example.com/v1", transport, Role::Chat);

    let deltas = std::sync::Mutex::new(Vec::new());
    let response = provider
        .stream(&chat_request(), &mut |delta: &str| {
            deltas.lock().unwrap().push(delta.to_string())
        })
        .await
        .expect("流式调用应当成功");

    assert_eq!(response.model, "qwen3-vl", "model 必须来自响应帧");
    assert_eq!(response.usage.prompt_tokens, 7);
    assert_eq!(response.usage.completion_tokens, 3);
    assert_eq!(response.finish_reason.as_deref(), Some("stop"));
}

#[tokio::test]
async fn stream_without_any_content_is_an_error() {
    let frames = vec![
        "data: {\"choices\":[{\"delta\":{}}]}\n\n".to_string(),
        "data: [DONE]\n\n".to_string(),
    ];
    let transport = Arc::new(ScriptedTransport::new().push_sse(200, frames));
    let provider = provider("https://api.example.com/v1", transport, Role::Chat);

    let mut calls = 0;
    let error = provider
        .stream(&chat_request(), &mut |_: &str| calls += 1)
        .await
        .expect_err("没有任何内容必须报错，而不是返回空答案");
    assert_eq!(calls, 0, "空内容不该产生增量");
    match error {
        ProviderError::InvalidResponse { detail } => {
            assert!(detail.contains("没有任何内容"), "实际：{detail}")
        }
        other => panic!("应当是 InvalidResponse，实际 {other:?}"),
    }
}

#[tokio::test]
async fn stream_maps_401_to_auth_failed() {
    let transport = Arc::new(ScriptedTransport::new().push_sse(
        401,
        vec!["{\"error\":{\"message\":\"invalid api key\"}}".to_string()],
    ));
    let provider = provider("https://api.example.com/v1", transport, Role::Chat);

    match provider
        .stream(&chat_request(), &mut |_: &str| {})
        .await
        .expect_err("401 必须失败")
    {
        ProviderError::AuthFailed { status, .. } => assert_eq!(status, 401),
        other => panic!("401 必须映射为 AuthFailed，实际 {other:?}"),
    }
}

#[tokio::test]
async fn stream_tolerates_server_that_ignores_the_stream_flag() {
    // 有些兼容实现无视 `stream: true`，直接返回整段 JSON：这时按非流式解析，
    // 整段作为**一个**增量回调 —— 宁可退化，也不要让用户拿到空答案或报错。
    let transport = Arc::new(ScriptedTransport::new().push_json(
        200,
        r#"{"choices":[{"message":{"role":"assistant","content":"完整回答"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2}}"#,
    ));
    let provider = provider(
        "https://api.example.com/v1",
        Arc::clone(&transport),
        Role::Chat,
    );

    let (deltas, text) = stream_deltas(&provider).await;

    assert_eq!(deltas, vec!["完整回答"], "退化为一次性回调");
    assert_eq!(text, "完整回答");
    // 请求本身仍然带 stream:true（我们确实要流式），是服务端选择了忽略它。
    let request = transport.last();
    assert!(
        request
            .body
            .as_deref()
            .unwrap_or_default()
            .contains("\"stream\":true"),
        "流式调用必须显式声明 stream:true"
    );
}
