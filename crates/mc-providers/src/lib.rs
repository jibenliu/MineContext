//! `mc-providers` — 模型 Provider 抽象。
//!
//! 设计约束：
//! **只依赖 OpenAI 的 HTTP 契约**，不依赖任何厂商 SDK。
//! 允许出现的路径只有 `/v1/chat/completions`、`/v1/embeddings`、`/v1/models`，
//! CI 里有脚本强制这一点（`scripts/check-provider-purity.sh`）。

pub mod credentials;
pub mod error;
pub mod image;
pub mod openai;
pub mod transport;

pub use error::ProviderError;

use async_trait::async_trait;
use mc_common::error::AppError;

/// Provider 扮演的角色。同一个 OpenAI 兼容实现按角色实例化三次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Vision,
    Chat,
    Embedding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub supports_images: bool,
    pub supports_embeddings: bool,
    pub supports_json_mode: bool,
    pub max_concurrency: u32,
}

impl ProviderCapabilities {
    pub const fn for_role(role: Role, max_concurrency: u32) -> Self {
        Self {
            supports_images: matches!(role, Role::Vision),
            supports_embeddings: matches!(role, Role::Embedding),
            supports_json_mode: !matches!(role, Role::Embedding),
            max_concurrency: if max_concurrency == 0 {
                1
            } else {
                max_concurrency
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

impl TokenUsage {
    pub const fn total(&self) -> u32 {
        self.prompt_tokens + self.completion_tokens
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderHealth {
    pub healthy: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }

    /// 历史里的助手发言（多轮上下文用）。
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub json_mode: bool,
}

impl ChatRequest {
    /// 最小探活请求。
    pub fn ping() -> Self {
        Self {
            messages: vec![ChatMessage::user("ping")],
            max_tokens: Some(1),
            temperature: None,
            json_mode: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatResponse {
    pub text: String,
    pub model: String,
    pub finish_reason: Option<String>,
    pub usage: TokenUsage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionRequest {
    pub image: Vec<u8>,
    pub mime: String,
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub json_mode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisionResponse {
    pub text: String,
    pub model: String,
    pub finish_reason: Option<String>,
    pub usage: TokenUsage,
}

/// 所有 Provider 的公共契约。
///
/// 把 `id` / `capabilities` / `health` **只声明一次**是刻意的：
/// 若把它们分别放进三个角色 trait，同一个类型同时实现多个 trait 时
/// 方法解析会歧义（E0034），调用方被迫写 UFCS —— 那是设计问题，不是调用问题。
#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> String;
    fn role(&self) -> Role;
    fn capabilities(&self) -> ProviderCapabilities;
    /// **真实探活**，不是读一个标志位。
    async fn health(&self) -> ProviderHealth;
    fn to_app_error(&self, error: &ProviderError) -> AppError;
}

#[async_trait]
pub trait ChatProvider: Provider {
    async fn complete(&self, request: &ChatRequest) -> Result<ChatResponse, ProviderError>;

    /// 流式回答：把增量按到达顺序交给回调，返回值与 `complete` 同形（完整答案 + 用量）。
    ///
    /// 默认实现退化为「先 `complete`，再一次性回调整段」——
    /// 不支持流式的 provider 仍然可用，但**首字延迟等于整段生成时间**；
    /// 调用方拿到的回调次数因此不是契约的一部分，**只有拼接结果是**。
    async fn stream(
        &self,
        request: &ChatRequest,
        on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ChatResponse, ProviderError> {
        let response = self.complete(request).await?;
        // 先取出一份再回调：回调持有的是本次调用的借用，不能同时把 response 交出去
        let text = response.text.clone();
        on_delta(&text);
        Ok(response)
    }
}

#[async_trait]
pub trait VisionProvider: Provider {
    async fn analyze(&self, request: VisionRequest) -> Result<VisionResponse, ProviderError>;
}

/// Embedding 的返回：向量 + 用量。
///
/// 之所以不直接返回 `Vec<Vec<f32>>`：**embedding 也要计入成本** ——
/// 成本报表里缺了它，索引重建花掉的 token 就完全不可见。
#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingResponse {
    pub vectors: Vec<Vec<f32>>,
    pub usage: TokenUsage,
}

#[async_trait]
pub trait EmbeddingProvider: Provider {
    async fn embed(&self, inputs: &[String]) -> Result<EmbeddingResponse, ProviderError>;
    /// 探测真实维度（用于建索引与「换模型后维度变了」的检测）。
    async fn probe_dimensions(&self) -> Result<usize, ProviderError>;
}
