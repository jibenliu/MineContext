//! Embedding 批量执行。
//!
//! 三条决定，都是为了不重演「采集被 AI 拖死」：
//!
//! 1. **没有 `Result`**：失败只落在 `EmbeddingOutcome::failure` 里，
//!    调用方不可能顺手把它传播成采集链路的错误；
//! 2. **失败即停**：限流/超时之后继续打只会烧光配额，剩余批次记
//!    `skipped_batches`，下一轮幂等覆盖；
//! 3. **宁可整批丢弃，也不错位配对**：向量与文档按下标配对，数量不符就整批
//!    拒绝 —— 检索结果错了，用户没有任何办法看出来。

use std::time::Instant;

use mc_common::error::{AppError, ErrorCode};
use mc_providers::error::ProviderError;
use mc_providers::EmbeddingProvider;
use mc_search::plan_batches;
use mc_storage::provider_calls::CallResult;

/// 一条待向量化的文档。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingDocument {
    /// `activity` / `summary` / `document`
    pub kind: String,
    pub doc_id: String,
    pub text: String,
}

/// 一条已算好的向量。
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddedVector {
    pub kind: String,
    pub doc_id: String,
    pub model: String,
    pub values: Vec<f32>,
}

/// 一次请求的记账信息。字段与 `provider_calls` 一一对应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingCall {
    pub model: String,
    pub prompt_tokens: u32,
    pub latency_ms: u64,
    pub result: CallResult,
    pub error_code: Option<String>,
}

/// 一轮 embedding 的结果。**刻意不是 `Result`**（见模块说明）。
#[derive(Debug, Clone, Default)]
pub struct EmbeddingOutcome {
    pub vectors: Vec<EmbeddedVector>,
    pub calls: Vec<EmbeddingCall>,
    /// 第一个失败。有它就意味着这一轮只索引了一部分。
    pub failure: Option<AppError>,
    pub batches: usize,
    pub skipped_batches: usize,
}

impl EmbeddingOutcome {
    /// 是否整轮成功（没有失败）。
    pub fn is_success(&self) -> bool {
        self.failure.is_none()
    }

    pub fn total_prompt_tokens(&self) -> u64 {
        self.calls.iter().map(|c| c.prompt_tokens as u64).sum()
    }
}

fn classify(error: &ProviderError) -> CallResult {
    match error {
        ProviderError::RateLimited { .. } => CallResult::RateLimited,
        ProviderError::Timeout => CallResult::Timeout,
        ProviderError::InvalidResponse { .. } => CallResult::InvalidResponse,
        _ => CallResult::Error,
    }
}

/// 按 `batch_limit` 切批调用 provider。
pub async fn embed_documents(
    provider: &dyn EmbeddingProvider,
    model: &str,
    documents: &[EmbeddingDocument],
    batch_limit: usize,
) -> EmbeddingOutcome {
    let mut outcome = EmbeddingOutcome::default();
    if documents.is_empty() {
        return outcome;
    }

    let plan = match plan_batches(documents.len(), batch_limit) {
        Ok(plan) => plan,
        Err(message) => {
            outcome.failure = Some(AppError::new(ErrorCode::ConfigInvalid, message));
            return outcome;
        }
    };
    outcome.batches = plan.len();

    for (index, batch) in plan.iter().enumerate() {
        let inputs: Vec<String> = documents[batch.clone()]
            .iter()
            .map(|document| document.text.clone())
            .collect();

        let started = Instant::now();
        let response = provider.embed(&inputs).await;
        let latency_ms = started.elapsed().as_millis() as u64;

        match response {
            Ok(response) => {
                let error = validate_batch(&response.vectors, inputs.len());
                outcome.calls.push(EmbeddingCall {
                    model: model.to_string(),
                    prompt_tokens: response.usage.prompt_tokens,
                    latency_ms,
                    result: if error.is_some() {
                        CallResult::InvalidResponse
                    } else {
                        CallResult::Ok
                    },
                    error_code: error
                        .as_ref()
                        .map(|error| error.code().as_str().to_string()),
                });

                if let Some(error) = error {
                    outcome.failure = Some(error);
                    outcome.skipped_batches = plan.len() - index - 1;
                    return outcome;
                }

                for (offset, values) in response.vectors.into_iter().enumerate() {
                    let document = &documents[batch.start + offset];
                    outcome.vectors.push(EmbeddedVector {
                        kind: document.kind.clone(),
                        doc_id: document.doc_id.clone(),
                        model: model.to_string(),
                        values,
                    });
                }
            }
            Err(error) => {
                let app_error = error.to_app_error();
                outcome.calls.push(EmbeddingCall {
                    model: model.to_string(),
                    prompt_tokens: 0,
                    latency_ms,
                    result: classify(&error),
                    error_code: Some(app_error.code().as_str().to_string()),
                });
                outcome.failure = Some(app_error);
                outcome.skipped_batches = plan.len() - index - 1;
                return outcome;
            }
        }
    }

    outcome
}

/// 校验一批返回值：数量必须一致，且每条都不能是空向量。
///
/// 这两种情况如果放过，产生的不是「少几条索引」，而是**错位的索引** ——
/// 用户会搜到本来无关的内容，且没有任何迹象表明出了问题。
fn validate_batch(vectors: &[Vec<f32>], expected: usize) -> Option<AppError> {
    if vectors.len() != expected {
        return Some(AppError::new(
            ErrorCode::ProviderInvalidResponse,
            format!(
                "embedding 返回 {} 条向量，但请求了 {expected} 条",
                vectors.len()
            ),
        ));
    }

    if let Some(index) = vectors.iter().position(|values| values.is_empty()) {
        return Some(AppError::new(
            ErrorCode::ProviderInvalidResponse,
            format!("embedding 第 {index} 条向量为空，无法用于相似度计算"),
        ));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_batch_is_detected() {
        let error = validate_batch(&[vec![1.0], Vec::new()], 2).expect("空向量必须被拒绝");
        assert_eq!(error.code(), ErrorCode::ProviderInvalidResponse);
    }

    #[test]
    fn count_mismatch_is_detected() {
        assert!(validate_batch(&[vec![1.0]], 2).is_some());
        assert!(validate_batch(&[vec![1.0]], 1).is_none());
    }
}
