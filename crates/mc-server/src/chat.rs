//! 对话引擎。
//!
//! 答案由 `ChatEngine` 产出；**帧与落库由路由负责** ——
//! 因为契约是「服务端负责落库、调用方只消费帧」，
//! 把落库放进引擎会让契约分散在两处。
//!
//! 关于「流式」：provider 调用是一次性的（OpenAI 兼容的非流式接口），
//! 拿到完整答案后按小段切分推送：打字机效果与既有契约都不变，
//! 代价是**首字延迟等于整段生成时间**（当前是整段生成后分片推送，不是 token 级流式）。

use std::sync::Arc;

use async_trait::async_trait;
use mc_common::error::AppError;

/// 一次问答的生成上限：答案要能说清一件事，又不能长到把界面刷满 ——
/// 问答与流式两条路径共用同一个值，避免两条路径的行为悄悄分叉。
const ANSWER_MAX_TOKENS: u32 = 800;

/// 对话里的一轮历史（只带角色与文本，够模型理解指代即可）。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

/// 引擎的输入。
#[derive(Debug, Clone, PartialEq)]
pub struct ChatInput {
    pub query: String,
    /// 检索到的上下文（已带来源），引擎应当用它并**在回答里引用**
    pub citations: Vec<Citation>,
    /// 同一会话里最近的几轮消息（由调用方截断并保证顺序：旧 → 新）。
    /// 没有它，「那第二点呢」这类追问拿不到上下文。
    pub history: Vec<ChatTurn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Citation {
    pub document_id: String,
    pub title: String,
    pub kind: String,
    /// 文档时间（毫秒）。前端跳活动时间线时用来切到对应日期。
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThinkingNote {
    pub stage: String,
    pub content: String,
    pub progress: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatAnswer {
    pub text: String,
    pub thinking: Vec<ThinkingNote>,
    pub citations: Vec<Citation>,
    pub model: Option<String>,
}

/// 流式回答过程中推给调用方的事件。
///
/// 分成「思考」与「增量」两类，而不是只留文本：渲染层按 `type` 分支，
/// 丢掉思考事件会让界面少一段可解释的信息。
pub enum ChatEvent<'a> {
    Delta(&'a str),
    Thinking(&'a ThinkingNote),
}

#[async_trait]
pub trait ChatEngine: Send + Sync {
    async fn answer(&self, input: &ChatInput) -> Result<ChatAnswer, AppError>;

    /// 流式回答：增量与思考过程按发生顺序回调，返回值与 `answer` 同形。
    ///
    /// 默认实现退化为「先算完，再回放」——不支持流式的引擎仍然可用，
    /// 但**首个增量等于整段答案**（首字延迟没有改善）。
    /// 调用方因此只能依赖「拼接结果」，不能依赖回调次数。
    async fn answer_stream(
        &self,
        input: &ChatInput,
        on_event: &mut (dyn for<'a> FnMut(ChatEvent<'a>) + Send),
    ) -> Result<ChatAnswer, AppError> {
        let answer = self.answer(input).await?;
        for note in &answer.thinking {
            on_event(ChatEvent::Thinking(note));
        }
        let text = answer.text.clone();
        on_event(ChatEvent::Delta(&text));
        Ok(answer)
    }
}

/// 本地列表引擎的原因：决定用户看到的开场白，避免把「断网」说成「没配模型」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LocalListingReason {
    /// 未配置模型 / 未同意出网 / 密钥组装失败
    #[default]
    Unconfigured,
    /// 配了模型，但本次调用因网络/服务端失败而不可用
    ProviderUnavailable,
}

/// 本地列表引擎：**不假装在线生成**，只把检索到的内容如实摘出来。
///
/// 比「模型不可用时什么都不给」更诚实：用户至少能看到「系统找到了这些」，
/// 也符合「答案永远有出处」。
#[derive(Debug, Default)]
pub struct ExtractOnlyEngine {
    reason: LocalListingReason,
}

impl ExtractOnlyEngine {
    pub fn provider_unavailable() -> Self {
        Self {
            reason: LocalListingReason::ProviderUnavailable,
        }
    }

    fn lead_in(&self, empty: bool) -> &'static str {
        match (self.reason, empty) {
            (LocalListingReason::Unconfigured, true) => {
                "没有配置模型，而且在你的记录里没有找到相关内容。"
            }
            (LocalListingReason::Unconfigured, false) => {
                "没有配置模型，先把你记录里的相关内容列出来：\n"
            }
            (LocalListingReason::ProviderUnavailable, true) => {
                "模型暂时不可用，而且在你的记录里没有找到相关内容。"
            }
            (LocalListingReason::ProviderUnavailable, false) => {
                "模型暂时不可用，先列出本地相关记录：\n"
            }
        }
    }
}

#[async_trait]
impl ChatEngine for ExtractOnlyEngine {
    async fn answer(&self, input: &ChatInput) -> Result<ChatAnswer, AppError> {
        let text = if input.citations.is_empty() {
            self.lead_in(true).to_string()
        } else {
            let mut text = String::from(self.lead_in(false));
            for (index, citation) in input.citations.iter().enumerate() {
                text.push_str(&format!(
                    "{}. {}（{}）\n",
                    index + 1,
                    citation.title,
                    citation.kind
                ));
            }
            text
        };

        Ok(ChatAnswer {
            text,
            thinking: vec![ThinkingNote {
                stage: "retrieval".to_string(),
                content: format!("检索到 {} 条相关内容", input.citations.len()),
                progress: 1.0,
            }],
            citations: input.citations.clone(),
            model: None,
        })
    }
}

/// 用 chat provider 回答（把检索到的内容作为上下文）。
///
/// 持有 [`mc_common::redact::Redactor`]：**外发之前**把命中隐私模式的内容
/// 替换掉。脱敏放在这里而不是采集/存储处，因为本地记录仍然有用，
/// 只有出网才需要改写。
pub struct ProviderChatEngine {
    provider: Arc<dyn mc_providers::ChatProvider>,
    locale: mc_summary::model::SummaryLocale,
    redactor: mc_common::redact::Redactor,
    redactions: Arc<std::sync::atomic::AtomicU64>,
}

impl ProviderChatEngine {
    pub fn new(
        provider: Arc<dyn mc_providers::ChatProvider>,
        locale: mc_summary::model::SummaryLocale,
    ) -> Self {
        Self::with_redactor(provider, locale, mc_common::redact::Redactor::default())
    }

    /// 带脱敏规则的构造。`redactions` 用于把命中次数汇总到诊断。
    pub fn with_redactor(
        provider: Arc<dyn mc_providers::ChatProvider>,
        locale: mc_summary::model::SummaryLocale,
        redactor: mc_common::redact::Redactor,
    ) -> Self {
        Self {
            provider,
            locale,
            redactor,
            redactions: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// 累计命中次数（诊断页展示「本次会话脱敏了几次」）。
    pub fn redaction_count(&self) -> u64 {
        self.redactions.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl ProviderChatEngine {
    /// 构造提示词（含脱敏），返回 `(system, user)` 与本次命中的脱敏次数。
    ///
    /// 流式与非流式共用：两条路径的提示词必须一模一样，
    /// 否则「同一句话问两次，答案依据不同」。
    fn prepare(&self, input: &ChatInput) -> (String, String) {
        // 提示词语言跟随 `general.locale`：设置成英文的用户不该被要求「用中文回答」
        let language_rule = match self.locale {
            mc_summary::model::SummaryLocale::EnUs => "Answer in English, concisely.",
            mc_summary::model::SummaryLocale::ZhCn => "回答用中文，简洁。",
        };
        let mut system = String::from(
            "你是用户的本地记忆助手。只根据下面提供的「检索到的记录」回答；\
             找不到依据就直说没有找到，不要编造。",
        );
        system.push_str(language_rule);
        // 脱敏：检索到的标题与用户提问都可能在出网前命中隐私模式。
        // 放在这里（构造提示词时）而不是存储处 —— 本地记录不受影响。
        let mut hits = 0usize;
        if !input.citations.is_empty() {
            system.push_str("\n\n检索到的记录：\n");
            for (index, citation) in input.citations.iter().enumerate() {
                let redacted = self.redactor.redact(&citation.title);
                hits += redacted.hits.len();
                system.push_str(&format!(
                    "[{}] {}（{}）\n",
                    index + 1,
                    redacted.text,
                    citation.kind
                ));
            }
        }

        let query = self.redactor.redact(&input.query);
        hits += query.hits.len();
        self.redactions
            .fetch_add(hits as u64, std::sync::atomic::Ordering::SeqCst);
        (system, query.text)
    }

    /// 历史消息转成 provider 的 message 列表（**逐条脱敏**：历史同样要出网）。
    fn history_messages(&self, input: &ChatInput) -> Vec<mc_providers::ChatMessage> {
        let mut messages = Vec::new();
        let mut hits = 0usize;
        for turn in &input.history {
            let redacted = self.redactor.redact(&turn.content);
            hits += redacted.hits.len();
            if redacted.text.trim().is_empty() {
                continue;
            }
            messages.push(if turn.role == "assistant" {
                mc_providers::ChatMessage::assistant(redacted.text)
            } else {
                mc_providers::ChatMessage::user(redacted.text)
            });
        }
        self.redactions
            .fetch_add(hits as u64, std::sync::atomic::Ordering::SeqCst);
        messages
    }

    fn thinking_note(&self, input: &ChatInput) -> ThinkingNote {
        ThinkingNote {
            stage: "retrieval".to_string(),
            content: format!("检索到 {} 条相关内容，已交给模型", input.citations.len()),
            progress: 1.0,
        }
    }
}

#[async_trait]
impl ChatEngine for ProviderChatEngine {
    async fn answer(&self, input: &ChatInput) -> Result<ChatAnswer, AppError> {
        let (system, query) = self.prepare(input);
        let mut messages = vec![mc_providers::ChatMessage::system(system)];
        messages.extend(self.history_messages(input));
        messages.push(mc_providers::ChatMessage::user(query));
        let response = self
            .provider
            .complete(&mc_providers::ChatRequest {
                messages,
                max_tokens: Some(ANSWER_MAX_TOKENS),
                temperature: Some(0.3),
                json_mode: false,
            })
            .await
            .map_err(|error| error.to_app_error())?;

        Ok(ChatAnswer {
            text: response.text.trim().to_string(),
            thinking: vec![self.thinking_note(input)],
            citations: input.citations.clone(),
            model: Some(response.model),
        })
    }

    /// 真流式：检索说明先推，随后是 provider 的逐 token 增量。
    async fn answer_stream(
        &self,
        input: &ChatInput,
        on_event: &mut (dyn for<'a> FnMut(ChatEvent<'a>) + Send),
    ) -> Result<ChatAnswer, AppError> {
        let (system, query) = self.prepare(input);
        let note = self.thinking_note(input);
        on_event(ChatEvent::Thinking(&note));

        let mut messages = vec![mc_providers::ChatMessage::system(system)];
        messages.extend(self.history_messages(input));
        messages.push(mc_providers::ChatMessage::user(query));
        let response = self
            .provider
            .stream(
                &mc_providers::ChatRequest {
                    messages,
                    max_tokens: Some(ANSWER_MAX_TOKENS),
                    temperature: Some(0.3),
                    json_mode: false,
                },
                &mut |delta: &str| on_event(ChatEvent::Delta(delta)),
            )
            .await
            .map_err(|error| error.to_app_error())?;

        Ok(ChatAnswer {
            text: response.text.trim().to_string(),
            thinking: vec![note],
            citations: input.citations.clone(),
            model: Some(response.model),
        })
    }
}

/// 「配了 chat 模型没」的**轻量**判断：只读配置，不解析密钥、不组装 provider。
///
/// 门槛与 `build_summary_generator` 保持一致（出网同意 / `ai.enabled` / 端点非空），
/// 否则会出现「判断说配了、实际组装不出来」的分裂。
pub fn chat_model_configured(config: &mc_config::Config) -> bool {
    crate::activities::upload_allowed(config)
        && config.ai.enabled
        && !config.ai.chat.base_url.trim().is_empty()
        && !config.ai.chat.model.trim().is_empty()
}

/// 选择对话引擎：配了 chat 模型就用模型，否则用「只列检索结果」的兜底。
///
/// **没有模型也要能回话**（哪怕是如实说「我找到了这些」）——
/// 这是「答案永远有出处」的落点。
pub fn engine_for(state: &crate::state::ServerState) -> Box<dyn ChatEngine> {
    let secrets = mc_providers::credentials::KeychainCommand::default();
    let config = state.config.current();
    let locale = mc_summary::model::SummaryLocale::from_config(&config.config.general.locale);

    // 未同意出网 / 没配模型：用本地引擎把检索结果如实列出来，不发任何请求
    if !chat_model_configured(&config.config) {
        return Box::new(ExtractOnlyEngine::default());
    }

    // 复用 chat 端点的配置构造 provider（总结与对话用的是同一个端点）
    let endpoint = &config.config.ai.chat;
    if let Ok(api_key) =
        mc_providers::credentials::resolve_secret(&secrets, endpoint.api_key_ref.as_deref())
    {
        if let Ok(provider) = mc_providers::openai::OpenAiCompatibleProvider::new(
            mc_providers::openai::EndpointConfig {
                base_url: endpoint.base_url.clone(),
                model: endpoint.model.clone(),
                api_key,
                timeout: std::time::Duration::from_secs(endpoint.timeout_secs),
                max_image_edge: None,
                max_concurrency: endpoint.max_concurrency,
            },
            std::sync::Arc::new(mc_providers::transport::ReqwestTransport::default()),
            mc_providers::Role::Chat,
        ) {
            // 脱敏规则非法时**不组装 provider**：静默放行等于用户以为自己脱敏了、
            // 实际把原文发了出去。
            let redactor =
                match mc_common::redact::Redactor::new(&config.config.privacy.redact_patterns) {
                    Ok(redactor) => redactor,
                    Err(_) => return Box::new(ExtractOnlyEngine::default()),
                };

            return Box::new(ProviderChatEngine::with_redactor(
                std::sync::Arc::new(provider) as std::sync::Arc<dyn mc_providers::ChatProvider>,
                locale,
                redactor,
            ));
        }
    }

    // 密钥解析或 provider 组装失败：仍然是「有出处但没模型」的本地引擎
    Box::new(ExtractOnlyEngine::default())
}

/// 对话侧「可诚实降级为本地列表」的 provider 失败：断网 / 超时 / 5xx / 限流。
///
/// 鉴权失败、模型不存在等**不**降级——那些需要用户改配置，假装本地成功会误导。
pub fn provider_failure_allows_local_fallback(error: &AppError) -> bool {
    use mc_common::error::Component;
    error.code().component() == Component::Provider && error.code().retryable()
}
