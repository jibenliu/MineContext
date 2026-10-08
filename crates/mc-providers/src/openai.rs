//! OpenAI 兼容 Provider。
//!
//! **只依赖 OpenAI 的 HTTP 契约**，不依赖任何厂商 SDK。
//! 核心对策：不在客户端里硬编码厂商特有字段 —— 厂商字段一旦进客户端，
//! 换端点就会静默失败（请求被拒或参数被忽略，而错误信息指不到这里）。
//!
//! 允许出现的路径只有三条：
//!   `{base}/v1/chat/completions`、`{base}/v1/embeddings`、`{base}/v1/models`

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine;
use mc_common::error::AppError;
use mc_common::observability::{info, warn};
use serde::{Deserialize, Serialize};

use crate::error::{classify_status, classify_transport, ProviderError};
use crate::transport::{HttpRequest, HttpTransport};
use crate::{
    ChatProvider, ChatRequest, ChatResponse, EmbeddingProvider, EmbeddingResponse, Provider,
    ProviderCapabilities, ProviderHealth, Role, VisionProvider, VisionRequest, VisionResponse,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointConfig {
    pub base_url: String,
    pub model: String,
    pub api_key: Option<String>,
    pub timeout: Duration,
    /// 上传前把长边缩放到这个值以内（None = 不缩放）
    pub max_image_edge: Option<u32>,
    pub max_concurrency: u32,
}

impl EndpointConfig {
    pub fn is_configured(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.model.trim().is_empty()
    }
}

pub struct OpenAiCompatibleProvider {
    config: EndpointConfig,
    transport: Arc<dyn HttpTransport>,
    role: Role,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        config: EndpointConfig,
        transport: Arc<dyn HttpTransport>,
        role: Role,
    ) -> Result<Self, AppError> {
        // 构造时不校验「已配置」—— 允许先建后配，
        // 调用时才报 Unconfigured（并在发请求前拦截）。
        Ok(Self {
            config,
            transport,
            role,
        })
    }

    pub fn config(&self) -> &EndpointConfig {
        &self.config
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// 拼接端点 URL，保证不会出现 `/v1/v1/`。
    fn url(&self, path: &str) -> String {
        let base = self.config.base_url.trim().trim_end_matches('/');
        if base.ends_with("/v1") {
            format!("{base}/{path}")
        } else {
            format!("{base}/v1/{path}")
        }
    }

    fn build_request(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<HttpRequest, ProviderError> {
        if !self.config.is_configured() {
            return Err(ProviderError::Unconfigured);
        }

        let mut headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
        ];
        if let Some(key) = self.config.api_key.as_ref().filter(|k| !k.is_empty()) {
            headers.push(("Authorization".to_string(), format!("Bearer {key}")));
        }

        Ok(HttpRequest {
            method: "POST".to_string(),
            url: self.url(path),
            headers,
            body: Some(body.to_string()),
            timeout: self.config.timeout,
        })
    }

    /// 所有模型调用都走这里：**它只记失败**，成功由各 trait 方法记用量。
    ///
    /// 不写 URL、请求体与响应体：前者可能带查询串里的密钥，后两者是用户内容。
    async fn post_json(
        &self,
        kind: &'static str,
        path: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let request = self.build_request(path, body)?;
        let started = std::time::Instant::now();

        let response = match self.transport.send(request).await {
            Ok(response) => response,
            Err(error) => {
                warn!(
                    component = "provider",
                    event = "call_unreachable",
                    kind,
                    reason = error.kind(),
                    duration_ms = started.elapsed().as_millis() as u64,
                    "模型调用未能送达"
                );
                return Err(classify_transport(error));
            }
        };

        if response.status >= 400 {
            warn!(
                component = "provider",
                event = "call_rejected",
                kind,
                status = response.status,
                duration_ms = started.elapsed().as_millis() as u64,
                "模型服务返回错误状态"
            );
            return Err(classify_status(
                response.status,
                &response.body,
                response.retry_after,
                &self.config.model,
            ));
        }

        serde_json::from_str(&response.body).map_err(|e| ProviderError::InvalidResponse {
            detail: format!("{e}; 原文前 200 字符: {}", truncate(&response.body, 200)),
        })
    }
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

// ---------------------------------------------------------------- 请求/响应体

#[derive(Debug, Serialize)]
struct ChatCompletionBody {
    model: String,
    messages: Vec<MessagePayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<serde_json::Value>,
    /// 只有流式调用才带这个字段：不带就是一次性响应（兼容面不变）。
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct StreamFrame {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Serialize)]
struct MessagePayload {
    role: String,
    content: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChoiceMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct EmbeddingPayload {
    data: Vec<EmbeddingItem>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
    #[serde(default)]
    index: usize,
}

/// `response_format = {"type":"json_object"}` —— 标准 OpenAI 字段，
/// 不用任何厂商特有的开关。
fn json_mode_format(enabled: bool) -> Option<serde_json::Value> {
    enabled.then(|| serde_json::json!({ "type": "json_object" }))
}

/// 解析对话响应。`model` 由调用方传入（就是本次请求用的模型名）。
///
/// 解析时就填上模型名：留到调用方事后覆盖的话，多一条解析路径就会漏掉，
/// 会静默得到空 model，污染 `provider_calls` 与消息 metadata 的模型字段。
/// 参数化之后，"响应一定带模型名"是构造时就成立的。
fn parse_chat_response(raw: serde_json::Value, model: &str) -> Result<ChatResponse, ProviderError> {
    let parsed: ChatCompletionResponse =
        serde_json::from_value(raw).map_err(|e| ProviderError::InvalidResponse {
            detail: format!("缺少 choices[0].message.content: {e}"),
        })?;

    let choice =
        parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::InvalidResponse {
                detail: "choices 为空".to_string(),
            })?;

    let content = choice
        .message
        .content
        .ok_or_else(|| ProviderError::InvalidResponse {
            detail: "choices[0].message.content 为 null".to_string(),
        })?;

    Ok(ChatResponse {
        text: content,
        model: model.to_string(),
        finish_reason: choice.finish_reason,
        usage: parsed
            .usage
            .map(|u| crate::TokenUsage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
            })
            .unwrap_or_default(),
    })
}

// ---------------------------------------------------------------- traits

#[async_trait]
impl Provider for OpenAiCompatibleProvider {
    fn id(&self) -> String {
        format!("openai_compatible:{}", self.config.model)
    }

    fn role(&self) -> Role {
        self.role
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::for_role(self.role, self.config.max_concurrency)
    }

    /// 真实探活 —— 用「初始化失败也置 True」的标志位会让健康检查永远说没事，
    /// 于是 /api/health 报告 llm healthy 而客户端其实是 None。
    async fn health(&self) -> ProviderHealth {
        match self.complete(&ChatRequest::ping()).await {
            Ok(_) => ProviderHealth {
                healthy: true,
                message: None,
            },
            Err(error) => ProviderHealth {
                healthy: false,
                message: Some(error.to_app_error().user_message().to_string()),
            },
        }
    }

    fn to_app_error(&self, error: &ProviderError) -> AppError {
        error.to_app_error()
    }
}

#[async_trait]
impl ChatProvider for OpenAiCompatibleProvider {
    async fn complete(&self, request: &ChatRequest) -> Result<ChatResponse, ProviderError> {
        let messages = request
            .messages
            .iter()
            .map(|m| MessagePayload {
                role: m.role.clone(),
                content: serde_json::Value::String(m.content.clone()),
            })
            .collect();

        let body = ChatCompletionBody {
            model: self.config.model.clone(),
            messages,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            response_format: json_mode_format(request.json_mode),
            stream: None,
        };

        let raw = self
            .post_json(
                "chat",
                "chat/completions",
                serde_json::to_value(body).unwrap(),
            )
            .await?;

        let response = parse_chat_response(raw, &self.config.model)?;
        info!(
            component = "provider",
            event = "call_finished",
            kind = "chat",
            model = %response.model,
            prompt_tokens = response.usage.prompt_tokens,
            completion_tokens = response.usage.completion_tokens,
            "模型调用完成"
        );
        Ok(response)
    }

    /// 真流式：带 `stream: true` 发请求，边收 SSE 帧边回调增量。
    ///
    /// 解析失败或整段没有任何内容都算**错误**（不返回空答案）：安静的空白回答
    /// 比报错更难排查，用户只会觉得「模型坏了」。
    async fn stream(
        &self,
        request: &ChatRequest,
        on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<ChatResponse, ProviderError> {
        let messages = request
            .messages
            .iter()
            .map(|m| MessagePayload {
                role: m.role.clone(),
                content: serde_json::Value::String(m.content.clone()),
            })
            .collect();

        let body = ChatCompletionBody {
            model: self.config.model.clone(),
            messages,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            response_format: json_mode_format(request.json_mode),
            stream: Some(true),
        };

        let http_request =
            self.build_request("chat/completions", serde_json::to_value(body).unwrap())?;
        let started = std::time::Instant::now();
        let mut stream = ChatStream::new(&self.config.model);

        let response = match self
            .transport
            .send_stream(http_request, &mut |chunk: &str| {
                stream.push(chunk, on_delta)
            })
            .await
        {
            Ok(response) => response,
            Err(error) => {
                warn!(
                    component = "provider",
                    event = "call_unreachable",
                    kind = "chat",
                    reason = error.kind(),
                    duration_ms = started.elapsed().as_millis() as u64,
                    "模型调用未能送达"
                );
                return Err(classify_transport(error));
            }
        };

        if response.status >= 400 {
            warn!(
                component = "provider",
                event = "call_rejected",
                kind = "chat",
                status = response.status,
                duration_ms = started.elapsed().as_millis() as u64,
                "模型服务返回错误状态"
            );
            return Err(classify_status(
                response.status,
                &response.body,
                response.retry_after,
                &self.config.model,
            ));
        }

        let response = stream.finish(on_delta)?;
        info!(
            component = "provider",
            event = "call_finished",
            kind = "chat_stream",
            model = %response.model,
            prompt_tokens = response.usage.prompt_tokens,
            completion_tokens = response.usage.completion_tokens,
            "模型流式调用完成"
        );
        Ok(response)
    }
}

/// SSE 增量累积器：把 `data: {...}` 帧解析成增量，拼回完整答案。
struct ChatStream<'a> {
    model: String,
    text: String,
    finish_reason: Option<String>,
    usage: Option<crate::TokenUsage>,
    finished: bool,
    /// 是否见过合法的 SSE 帧：用来区分「流式但没内容」与「服务端无视 stream 返回整段」。
    saw_frame: bool,
    /// 原始响应体：给「无视 stream 的兼容实现」留一条解析退路。
    raw: String,
    fallback_model: &'a str,
}

impl<'a> ChatStream<'a> {
    fn new(fallback_model: &'a str) -> Self {
        Self {
            model: fallback_model.to_string(),
            text: String::new(),
            finish_reason: None,
            usage: None,
            finished: false,
            saw_frame: false,
            raw: String::new(),
            fallback_model,
        }
    }

    /// 处理一个**传输片段**（可能含多帧、也可能把一帧切成两半）。
    fn push(&mut self, chunk: &str, on_delta: &mut (dyn for<'b> FnMut(&'b str) + Send)) {
        self.raw.push_str(chunk);
        for line in chunk.lines() {
            let line = line.trim();
            let Some(payload) = line.strip_prefix("data:") else {
                continue;
            };
            let payload = payload.trim();
            if payload.is_empty() || payload == "[DONE]" {
                self.finished = true;
                continue;
            }
            let Ok(frame) = serde_json::from_str::<StreamFrame>(payload) else {
                // 单帧坏掉不该让整段回答丢失：跳过这一帧，其余照常累积。
                continue;
            };
            self.saw_frame = true;
            if let Some(model) = frame.model {
                if !model.is_empty() {
                    self.model = model;
                }
            }
            if let Some(usage) = frame.usage {
                self.usage = Some(crate::TokenUsage {
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                });
            }
            for choice in frame.choices {
                if let Some(reason) = choice.finish_reason {
                    self.finish_reason = Some(reason);
                }
                if let Some(delta) = choice.delta.content {
                    if !delta.is_empty() {
                        self.text.push_str(&delta);
                        on_delta(&delta);
                    }
                }
            }
        }
    }

    fn finish(
        self,
        on_delta: &mut (dyn for<'b> FnMut(&'b str) + Send),
    ) -> Result<ChatResponse, ProviderError> {
        // 兼容实现可能无视 `stream: true` 直接返回整段 JSON：那时按非流式解析，
        // 并把它作为**一个**增量回调（宁可退化，也不要让用户拿到空答案或报错）。
        if !self.saw_frame {
            let raw: serde_json::Value =
                serde_json::from_str(&self.raw).map_err(|e| ProviderError::InvalidResponse {
                    detail: format!("流式响应既不是 SSE 也不是完整 JSON: {e}"),
                })?;
            let response = parse_chat_response(raw, self.fallback_model)?;
            on_delta(&response.text.clone());
            return Ok(response);
        }
        if self.text.trim().is_empty() {
            return Err(ProviderError::InvalidResponse {
                detail: "流式响应里没有任何内容".to_string(),
            });
        }
        let usage = self.usage.unwrap_or(crate::TokenUsage {
            // 有些兼容实现不返回用量：留空账比编数字诚实，成本报表会显示缺口。
            prompt_tokens: 0,
            completion_tokens: 0,
        });
        let model = if self.model.is_empty() {
            self.fallback_model.to_string()
        } else {
            self.model
        };
        Ok(ChatResponse {
            text: self.text,
            model,
            finish_reason: self.finish_reason,
            usage,
        })
    }
}

/// 流式帧：只取用得到的字段（`delta.content` / `finish_reason` / `usage`）。

#[async_trait]
impl VisionProvider for OpenAiCompatibleProvider {
    async fn analyze(&self, request: VisionRequest) -> Result<VisionResponse, ProviderError> {
        let (bytes, mime) =
            crate::image::prepare(&request.image, &request.mime, self.config.max_image_edge)
                .map_err(|e| ProviderError::InvalidResponse {
                    detail: format!("图像预处理失败: {e}"),
                })?;

        let data_url = format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&bytes)
        );

        let body = ChatCompletionBody {
            model: self.config.model.clone(),
            messages: vec![MessagePayload {
                role: "user".to_string(),
                content: serde_json::json!([
                    { "type": "text", "text": request.prompt },
                    { "type": "image_url", "image_url": { "url": data_url } }
                ]),
            }],
            max_tokens: request.max_tokens,
            temperature: None,
            response_format: json_mode_format(request.json_mode),
            stream: None,
        };

        let raw = self
            .post_json(
                "vision",
                "chat/completions",
                serde_json::to_value(body).unwrap(),
            )
            .await?;

        let response = parse_chat_response(raw, &self.config.model)?;
        info!(
            component = "provider",
            event = "call_finished",
            kind = "vision",
            model = %response.model,
            prompt_tokens = response.usage.prompt_tokens,
            completion_tokens = response.usage.completion_tokens,
            "模型调用完成"
        );

        Ok(VisionResponse {
            text: response.text,
            model: response.model,
            usage: response.usage,
            finish_reason: response.finish_reason,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatibleProvider {
    /// 维度**从响应推断**，绝不硬编码。
    ///
    /// 维度不能写死：硬编码的维度在换模型后会与配置里的 embedding 维度不一致
    /// （例如写死 1536、实际模型给 2048），每次 upsert 都失败。
    async fn embed(&self, inputs: &[String]) -> Result<EmbeddingResponse, ProviderError> {
        let body = serde_json::json!({
            "model": self.config.model,
            "input": inputs,
        });

        let raw = self.post_json("embedding", "embeddings", body).await?;

        let parsed: EmbeddingPayload =
            serde_json::from_value(raw).map_err(|e| ProviderError::InvalidResponse {
                detail: format!("缺少 data[].embedding: {e}"),
            })?;

        if parsed.data.is_empty() {
            return Err(ProviderError::InvalidResponse {
                detail: "embeddings 响应为空".to_string(),
            });
        }

        // `index` 是服务端给的顺序：它不一定与请求顺序一致（并行分片时会乱序），
        // 而调用方需要「第 i 条输入对应第 i 条向量」。
        let mut sorted = parsed.data;
        sorted.sort_by_key(|item| item.index);

        let usage = parsed
            .usage
            .map(|u| crate::TokenUsage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: 0,
            })
            .unwrap_or_default();
        info!(
            component = "provider",
            event = "call_finished",
            kind = "embedding",
            model = %self.config.model,
            inputs = inputs.len(),
            prompt_tokens = usage.prompt_tokens,
            "模型调用完成"
        );

        Ok(EmbeddingResponse {
            vectors: sorted.into_iter().map(|item| item.embedding).collect(),
            // 自建网关常常不返回 usage：缺省按 0 记，不影响功能
            usage,
        })
    }

    /// 探活：对端点做一次最小 embedding 请求，并返回真实维度。
    async fn probe_dimensions(&self) -> Result<usize, ProviderError> {
        let response = self.embed(&["ping".to_string()]).await?;
        Ok(response.vectors[0].len())
    }
}
