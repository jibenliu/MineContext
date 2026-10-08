//! embedding 批量执行（引擎侧）。
//!
//! 这一层唯一的硬要求：**embedding 的失败绝不能变成采集链路的失败**。
//!
//! 把向量化放在上传请求里同步做，模型抽风就直接表现为「截图丢了」（队列爆满时先丢的是截图）。
//! 所以这里刻意让 `embed_documents` **不返回 `Result`** —— 失败只能落在 `outcome.failure` 里，
//! 调用方想把它升级成致命错误必须显式去做，不可能「顺手」传播。其余三条：
//! 批量上限来自配置（`plan_batches`）；每次请求都要留下用量（`calls`），否则成本不可见；
//! 失败即停 —— 限流时继续打只会把配额烧光，剩下的批次记成 `skipped_batches`。

use std::sync::Arc;
use std::time::Duration;

use mc_common::error::{AppError, ErrorCode};
use mc_pipeline::embedding::{embed_documents, EmbeddingDocument};
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::EmbeddingProvider;
use mc_storage::provider_calls::CallResult;
use mc_testkit::provider::ScriptedTransport;

fn provider(base_url: &str, transport: Arc<ScriptedTransport>) -> Arc<dyn EmbeddingProvider> {
    Arc::new(
        OpenAiCompatibleProvider::new(
            EndpointConfig {
                base_url: base_url.to_string(),
                model: "embed-small".to_string(),
                api_key: Some("sk-test".to_string()),
                timeout: Duration::from_secs(5),
                max_image_edge: None,
                max_concurrency: 2,
            },
            transport,
            mc_providers::Role::Embedding,
        )
        .expect("Provider 必须可构造"),
    )
}

fn docs(count: usize) -> Vec<EmbeddingDocument> {
    (0..count)
        .map(|index| EmbeddingDocument {
            kind: "activity".to_string(),
            doc_id: format!("act-{index}"),
            text: format!("第 {index} 条活动"),
        })
        .collect()
}

fn json_embedding(vectors: &[&[f32]], prompt_tokens: u32) -> String {
    let data: Vec<serde_json::Value> = vectors
        .iter()
        .enumerate()
        .map(|(index, vector)| serde_json::json!({ "embedding": vector, "index": index }))
        .collect();
    serde_json::json!({
        "data": data,
        "usage": { "prompt_tokens": prompt_tokens, "total_tokens": prompt_tokens },
    })
    .to_string()
}

// ---------------------------------------------------------------- 5.3 切批

#[tokio::test]
async fn embedding_uses_the_configured_batch_limit() {
    let transport = Arc::new(
        ScriptedTransport::new()
            // 5 条文档、上限 2 → 3 次请求（2 + 2 + 1）
            .push_json(200, json_embedding(&[&[1.0, 0.0], &[0.0, 1.0]], 4))
            .push_json(200, json_embedding(&[&[1.0, 1.0], &[2.0, 0.0]], 4))
            .push_json(200, json_embedding(&[&[0.0, 2.0]], 2)),
    );
    let provider = provider("https://api.example.com/v1", Arc::clone(&transport));

    let outcome = embed_documents(&*provider, "embed-small", &docs(5), 2).await;

    assert!(
        outcome.failure.is_none(),
        "不该有失败：{:?}",
        outcome.failure
    );
    assert_eq!(transport.call_count(), 3, "必须按配置的上限切批");
    assert_eq!(outcome.batches, 3);
    assert_eq!(outcome.vectors.len(), 5);

    // 顺序必须与输入一致（索引重建依赖这个顺序）
    let ids: Vec<&str> = outcome
        .vectors
        .iter()
        .map(|vector| vector.doc_id.as_str())
        .collect();
    assert_eq!(ids, vec!["act-0", "act-1", "act-2", "act-3", "act-4"]);
    assert_eq!(outcome.vectors[4].values, vec![0.0, 2.0]);
    assert_eq!(outcome.vectors[4].kind, "activity");
    assert_eq!(outcome.vectors[4].model, "embed-small");
}

#[tokio::test]
async fn embedding_records_usage_for_every_request() {
    let transport = Arc::new(
        ScriptedTransport::new()
            .push_json(200, json_embedding(&[&[1.0]], 7))
            .push_json(200, json_embedding(&[&[2.0]], 11)),
    );
    let provider = provider("https://api.example.com/v1", Arc::clone(&transport));

    let outcome = embed_documents(&*provider, "embed-small", &docs(2), 1).await;

    assert_eq!(outcome.calls.len(), 2);
    assert_eq!(outcome.calls[0].prompt_tokens, 7);
    assert_eq!(outcome.calls[1].prompt_tokens, 11);
    assert_eq!(outcome.total_prompt_tokens(), 18, "成本必须可累加");
    assert!(outcome
        .calls
        .iter()
        .all(|call| call.result == CallResult::Ok));
    assert_eq!(outcome.calls[0].model, "embed-small");
    assert!(outcome.calls[0].latency_ms < 5_000);
}

// 服务端并行分片时 `data[]` 可能乱序 —— 必须按 `index` 归位
#[tokio::test]
async fn embedding_realigns_vectors_by_index() {
    let transport = Arc::new(ScriptedTransport::new().push_json(
        200,
        r#"{"data":[{"embedding":[9.0],"index":1},{"embedding":[1.0],"index":0}]}"#,
    ));
    let provider = provider("https://api.example.com/v1", Arc::clone(&transport));

    let outcome = embed_documents(&*provider, "embed-small", &docs(2), 8).await;

    assert_eq!(outcome.vectors[0].values, vec![1.0]);
    assert_eq!(outcome.vectors[1].values, vec![9.0]);
}

// ---------------------------------------------------------------- 5.4 失败不阻塞

#[tokio::test]
async fn embedding_failure_is_reported_and_never_a_panic() {
    let transport = Arc::new(
        ScriptedTransport::new()
            .push_json(200, json_embedding(&[&[1.0], &[2.0]], 5))
            .push_json(429, r#"{"error":{"message":"rate limited"}}"#)
            .push_json(200, json_embedding(&[&[3.0], &[4.0]], 5)),
    );
    let provider = provider("https://api.example.com/v1", Arc::clone(&transport));

    let outcome = embed_documents(&*provider, "embed-small", &docs(6), 2).await;

    let failure = outcome.failure.as_ref().expect("限流必须被报告");
    assert_eq!(failure.code(), ErrorCode::ProviderRateLimited);

    // 第一批的成果必须保留（不做无意义的丢弃）
    assert_eq!(outcome.vectors.len(), 2);
    assert_eq!(outcome.vectors[0].values, vec![1.0]);

    // 限流后**不再继续打请求**：剩下的批次只记数，不烧配额
    assert_eq!(transport.call_count(), 2, "限流后必须停止后续批次");
    assert_eq!(outcome.skipped_batches, 1);
    assert_eq!(outcome.calls.len(), 2);
    assert_eq!(outcome.calls[1].result, CallResult::RateLimited);
    assert_eq!(
        outcome.calls[1].error_code.as_deref(),
        Some("provider_rate_limited")
    );
}

#[tokio::test]
async fn unconfigured_provider_reports_failure_without_panicking() {
    let transport = Arc::new(ScriptedTransport::new());
    // 空 base_url = 尚未配置（用户还没填模型）
    let provider = provider("", Arc::clone(&transport));

    let outcome = embed_documents(&*provider, "embed-small", &docs(3), 8).await;

    assert_eq!(transport.call_count(), 0, "没配置就不该发请求");
    assert_eq!(outcome.vectors.len(), 0);
    assert_eq!(
        outcome.failure.as_ref().map(AppError::code),
        Some(ErrorCode::ProviderUnconfigured)
    );
    // 不确定的错误也不能让调用方误以为成功
    assert!(!outcome.is_success());
}

#[tokio::test]
async fn transport_failure_is_classified_not_swallowed() {
    let transport = Arc::new(
        ScriptedTransport::new().push_failure(mc_providers::transport::TransportError::Timeout),
    );
    let provider = provider("https://api.example.com/v1", Arc::clone(&transport));

    let outcome = embed_documents(&*provider, "embed-small", &docs(2), 2).await;

    let failure = outcome.failure.as_ref().expect("超时必须被报告");
    assert_eq!(failure.code(), ErrorCode::ProviderTimeout);
    assert!(
        failure.retryable(),
        "超时可重试 —— 调用方据此决定下一轮重来"
    );
}

#[tokio::test]
async fn zero_batch_limit_is_reported_as_a_config_error() {
    let transport = Arc::new(ScriptedTransport::new());
    let provider = provider("https://api.example.com/v1", Arc::clone(&transport));

    let outcome = embed_documents(&*provider, "embed-small", &docs(3), 0).await;

    assert_eq!(transport.call_count(), 0);
    assert_eq!(
        outcome.failure.as_ref().map(AppError::code),
        Some(ErrorCode::ConfigInvalid),
        "上限为 0 是配置错误，必须说清楚"
    );
}

#[tokio::test]
async fn empty_document_list_makes_no_requests() {
    let transport = Arc::new(ScriptedTransport::new());
    let provider = provider("https://api.example.com/v1", Arc::clone(&transport));

    let outcome = embed_documents(&*provider, "embed-small", &[], 8).await;

    assert_eq!(transport.call_count(), 0);
    assert!(outcome.is_success(), "没有输入是正常状态，不是失败");
    assert!(outcome.vectors.is_empty());
}

// 5.4 的反面断言：模型返回了数量不对的向量时，宁可整批丢弃也不能错位配对
#[tokio::test]
async fn mismatched_vector_count_is_rejected() {
    let transport = Arc::new(ScriptedTransport::new().push_json(
        200,
        json_embedding(&[&[1.0]], 3), // 请求了 2 条，只回 1 条
    ));
    let provider = provider("https://api.example.com/v1", Arc::clone(&transport));

    let outcome = embed_documents(&*provider, "embed-small", &docs(2), 2).await;

    assert!(outcome.vectors.is_empty(), "数量不一致时不能瞎配对");
    let failure = outcome.failure.as_ref().expect("必须报告");
    assert_eq!(failure.code(), ErrorCode::ProviderInvalidResponse);
}

// 空向量（例如网关返回 `"embedding": []`）同样必须整批拒绝：
// 空向量写进索引会把相似度全部拉成 0，检索结果从此不可信。
#[tokio::test]
async fn empty_vector_from_provider_is_rejected() {
    let transport = Arc::new(
        ScriptedTransport::new().push_json(200, r#"{"data":[{"embedding":[],"index":0}]}"#),
    );
    let provider = provider("https://api.example.com/v1", Arc::clone(&transport));

    let outcome = embed_documents(&*provider, "embed-small", &docs(1), 2).await;

    assert!(outcome.vectors.is_empty());
    assert_eq!(
        outcome.failure.as_ref().map(AppError::code),
        Some(ErrorCode::ProviderInvalidResponse)
    );
}
