//! AI 辅助识别。
//!
//! 存在理由只有一个：**规则判不了的时候，才花钱问模型**。
//! 因此默认行为是「不调用」，四道闸门依次拦：规则已给出结论 → 预算闸门关闭
//! → 同一活动窗口已有结论（缓存复用，不消耗额度所以先查）→ 滑动窗口限流。
//!
//! 模型输出一律当作**不可信输入**：解析失败、缺字段、置信度越界都降级到
//! Observed 并保留原始输出，而不是让坏数据进入活动时间线。

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use mc_common::error::AppError;
use mc_common::observability::{debug, info, warn};
use mc_common::time::Timestamp;
use mc_domain::activity::Provenance;
use mc_domain::observation::ObservationSummary;
use mc_providers::ProviderError;
use mc_providers::{TokenUsage, VisionProvider, VisionRequest};

/// 一条活动窗口的推断请求。
#[derive(Debug, Clone)]
pub struct ActivityRequest<'a> {
    pub observation: &'a ObservationSummary,
    /// 活动窗口标识（同一个活动里的多条观测共用一个 key）
    pub window_key: &'a str,
    pub image: &'a [u8],
    pub mime: &'a str,
    pub at: Timestamp,
    /// 规则已经判出结论 —— 不再问模型
    pub rule_resolved: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActivityAiPolicy {
    /// 预算闸门的总开关。关掉时**一次都不调用**，但仍然产出活动
    pub ai_enabled: bool,
    /// 滑动窗口内允许的调用次数
    pub max_calls_per_window: u32,
    pub window_secs: u64,
    /// 同一活动窗口在这段时间内不重复分析（0 = 每次都重新分析）
    pub reanalyze_after_secs: u64,
}

impl Default for ActivityAiPolicy {
    fn default() -> Self {
        Self {
            ai_enabled: true,
            // 与 mc-config 的 ai.budget.max_vlm_calls_per_hour 一致
            max_calls_per_window: 240,
            window_secs: 3600,
            // 活动的最长时长是 2 小时，超过就该重新判断了
            reanalyze_after_secs: 7200,
        }
    }
}

/// 一次模型调用的记账信息。字段与 `provider_calls` 表一一对应。
///
/// 「钱花了但看不见」是这个产品最贵的一类问题，因此**被闸门拦下的调用也记账**
/// （token 为 0，原因写进 `error_code`）——报表上要能回答
/// 「这段时间为什么没有推断」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiCall {
    pub model: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub latency_ms: u64,
    pub result: AiCallResult,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiCallResult {
    Ok,
    Error,
    RateLimited,
    Timeout,
    InvalidResponse,
}

impl AiCallResult {
    /// 与 `mc_storage::provider_calls::CallResult::as_str()` 同名，
    /// 落库时按这个字符串映射。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::InvalidResponse => "invalid_response",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActivitySuggestion {
    pub title: String,
    pub category: Option<String>,
    pub confidence: f32,
    pub origin: Provenance,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActivityAiOutcome {
    Inferred {
        suggestion: ActivitySuggestion,
        usage: TokenUsage,
    },
    /// 同一活动窗口内复用上次结论
    Cached { suggestion: ActivitySuggestion },
    /// 没有结论：调用方应当退回 `Observed`（用进程名兜底）
    Degraded { reason: String, raw: Option<String> },
    /// 本来就不需要调用
    Skipped { reason: SkipReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    RuleResolved,
}

pub struct ActivityAiWorker {
    provider: Arc<dyn VisionProvider>,
    locale: crate::prompts::Locale,
    policy: ActivityAiPolicy,
    /// 滑动窗口内的调用时刻
    calls: VecDeque<Timestamp>,
    /// 活动窗口 → （分析时刻、结论）
    analyzed: HashMap<String, (Timestamp, ActivitySuggestion)>,
    model: String,
    /// 本轮累积的调用记账，由 [`Self::take_calls`] 取走
    accounting: Vec<AiCall>,
}

impl ActivityAiWorker {
    pub fn new(provider: Arc<dyn VisionProvider>, policy: ActivityAiPolicy) -> Self {
        let model = provider.id();
        Self {
            provider,
            locale: crate::prompts::Locale::ZhCn,
            policy,
            calls: VecDeque::new(),
            analyzed: HashMap::new(),
            model,
            accounting: Vec::new(),
        }
    }

    /// 取走累积的调用记账（取走即清空）。
    pub fn take_calls(&mut self) -> Vec<AiCall> {
        std::mem::take(&mut self.accounting)
    }

    /// 提示词语言跟随 `general.locale`。
    pub fn with_locale(mut self, locale: crate::prompts::Locale) -> Self {
        self.locale = locale;
        self
    }

    pub fn policy(&self) -> &ActivityAiPolicy {
        &self.policy
    }

    /// 打开/关掉模型调用（预算闸门用它）。
    ///
    /// 关掉之后仍然产出活动（走廉价元数据），只是不再花钱 —— 降级不是停摆。
    pub fn set_ai_enabled(&mut self, enabled: bool) {
        self.policy.ai_enabled = enabled;
    }

    /// 窗口内的已用调用次数（供诊断与测试观察）。
    pub fn calls_in_window(&mut self, at: Timestamp) -> usize {
        self.evict(at);
        self.calls.len()
    }

    pub async fn resolve(
        &mut self,
        request: ActivityRequest<'_>,
    ) -> Result<ActivityAiOutcome, AppError> {
        // 闸门 1：规则已经判出来了
        if request.rule_resolved {
            debug!(
                component = "pipeline",
                event = "ai_skipped",
                reason = "rule_resolved",
                "规则已给出结论，不调用模型"
            );
            return Ok(ActivityAiOutcome::Skipped {
                reason: SkipReason::RuleResolved,
            });
        }

        // 闸门 2：预算总开关
        if !self.policy.ai_enabled {
            warn!(
                component = "pipeline",
                event = "ai_degraded",
                reason = "budget_exhausted",
                "预算已用尽，本时段不再调用模型"
            );
            self.accounting.push(AiCall {
                model: self.model.clone(),
                prompt_tokens: 0,
                completion_tokens: 0,
                latency_ms: 0,
                result: AiCallResult::RateLimited,
                error_code: Some("budget_exhausted".to_string()),
            });
            return Ok(ActivityAiOutcome::Degraded {
                reason: "预算已用尽，本时段不再调用模型（budget exhausted）".to_string(),
                raw: None,
            });
        }

        // 闸门 4：同一活动窗口复用结论。
        // 放在限流之前 —— 复用不消耗额度，先拦下来才能真的省下调用。
        if self.policy.reanalyze_after_secs > 0 {
            if let Some((analyzed_at, suggestion)) = self.analyzed.get(request.window_key) {
                let elapsed = request.at.saturating_diff_millis(*analyzed_at).max(0) as u64 / 1000;
                if elapsed < self.policy.reanalyze_after_secs {
                    debug!(
                        component = "pipeline",
                        event = "ai_cached",
                        elapsed_secs = elapsed,
                        "同一活动窗口复用上次结论"
                    );
                    return Ok(ActivityAiOutcome::Cached {
                        suggestion: suggestion.clone(),
                    });
                }
            }
        }

        // 闸门 3：滑动窗口限流
        self.evict(request.at);
        if self.calls.len() as u32 >= self.policy.max_calls_per_window {
            warn!(
                component = "pipeline",
                event = "ai_degraded",
                reason = "rate_limited",
                calls = self.calls.len(),
                window_secs = self.policy.window_secs,
                "滑动窗口限流，本时段不再调用模型"
            );
            self.accounting.push(AiCall {
                model: self.model.clone(),
                prompt_tokens: 0,
                completion_tokens: 0,
                latency_ms: 0,
                result: AiCallResult::RateLimited,
                error_code: Some("rate_limited".to_string()),
            });
            return Ok(ActivityAiOutcome::Degraded {
                reason: format!(
                    "滑动窗口限流：{} 秒内已调用 {} 次（rate limited）",
                    self.policy.window_secs,
                    self.calls.len()
                ),
                raw: None,
            });
        }

        let prompt = build_prompt(request.observation, self.locale);
        let started = std::time::Instant::now();
        let response = match self
            .provider
            .analyze(VisionRequest {
                image: request.image.to_vec(),
                mime: request.mime.to_string(),
                prompt,
                max_tokens: Some(256),
                json_mode: true,
            })
            .await
        {
            Ok(response) => response,
            Err(error) => {
                // 失败的调用也要记账：重试与超时同样烧时间（有时也烧 token）
                self.accounting.push(AiCall {
                    model: self.model.clone(),
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    latency_ms: started.elapsed().as_millis() as u64,
                    result: call_result_of(&error),
                    error_code: Some(error.to_app_error().code().as_str().to_string()),
                });
                return Err(error.to_app_error());
            }
        };
        let latency_ms = started.elapsed().as_millis() as u64;

        // 只有真的发出去了才记账 —— 失败重试不算额度会变成无限放大
        self.calls.push_back(request.at);

        match parse_suggestion(&response.text, &self.model) {
            Ok(suggestion) => {
                info!(
                    component = "pipeline",
                    event = "ai_inferred",
                    model = %suggestion.model,
                    confidence = suggestion.confidence,
                    "活动推断完成"
                );
                self.accounting.push(AiCall {
                    model: self.model.clone(),
                    prompt_tokens: response.usage.prompt_tokens,
                    completion_tokens: response.usage.completion_tokens,
                    latency_ms,
                    result: AiCallResult::Ok,
                    error_code: None,
                });
                self.analyzed.insert(
                    request.window_key.to_string(),
                    (request.at, suggestion.clone()),
                );
                Ok(ActivityAiOutcome::Inferred {
                    suggestion,
                    usage: response.usage,
                })
            }
            Err(reason) => {
                self.accounting.push(AiCall {
                    model: self.model.clone(),
                    prompt_tokens: response.usage.prompt_tokens,
                    completion_tokens: response.usage.completion_tokens,
                    latency_ms,
                    result: AiCallResult::InvalidResponse,
                    error_code: Some("invalid_response".to_string()),
                });
                Ok(ActivityAiOutcome::Degraded {
                    reason,
                    // 原始输出必须留着：事后要能回答「模型到底吐了什么」
                    raw: Some(response.text),
                })
            }
        }
    }

    fn evict(&mut self, now: Timestamp) {
        let window_ms = (self.policy.window_secs as i64) * 1_000;
        while let Some(front) = self.calls.front() {
            if now.saturating_diff_millis(*front) > window_ms {
                self.calls.pop_front();
            } else {
                break;
            }
        }
    }
}

/// 把 provider 的错误分类映射成记账结果码。
fn call_result_of(error: &ProviderError) -> AiCallResult {
    match error {
        ProviderError::RateLimited { .. } => AiCallResult::RateLimited,
        ProviderError::Timeout => AiCallResult::Timeout,
        ProviderError::InvalidResponse { .. } => AiCallResult::InvalidResponse,
        _ => AiCallResult::Error,
    }
}

/// 请求里要带上**廉价元数据**：只看一张图，模型无从知道这是哪个应用。
fn build_prompt(observation: &ObservationSummary, locale: crate::prompts::Locale) -> String {
    let app = observation.app_name.as_deref().unwrap_or("未知");
    let title = observation.window_title.as_deref().unwrap_or("未知");
    let domain = observation.domain.as_deref().unwrap_or("无");

    format!(
        "{}\n\n进程：{app}\n窗口标题：{title}\n域名：{domain}",
        crate::prompts::activity_infer(locale)
    )
}

/// 一批活动的推断结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InferenceBatch {
    pub suggestions: Vec<mc_domain::projector::InferredSuggestion>,
    /// 不需要推断（规则已判 / 用户已改 / 没有像素）
    pub skipped: usize,
    /// 想推断但没成（无预算 / 限流 / 输出不可用）
    pub degraded: usize,
    /// 本轮每一笔模型开销（含被闸门拦下的），供调用方落 `provider_calls`
    pub calls: Vec<AiCall>,
}

/// 对一批活动做推断，只挑「规则没判出来」的那些。
///
/// `image_for` 由调用方注入（测试给假数据，生产给 blob 存储）——
/// 这样这一段逻辑不需要磁盘、不需要网络就能测。
pub async fn infer_activities(
    worker: &mut ActivityAiWorker,
    activities: &[mc_domain::activity::ActivityView],
    mut image_for: impl FnMut(&str) -> Option<(Vec<u8>, String)>,
    at: Timestamp,
) -> Result<InferenceBatch, AppError> {
    let mut batch = InferenceBatch::default();

    for activity in activities {
        // 规则命中 = 用户自己定的结论；用户改过 = 用户已经给了更好的答案。
        // 两种都不该再花钱问模型。
        if !matches!(activity.origin, Provenance::Observed) || activity.is_user_modified {
            batch.skipped += 1;
            continue;
        }

        // 拿第一条**有图**的观测：没有像素就没有推断依据，
        // 宁可用进程名兜底，也不要让模型凭元数据编一个标题。
        let Some((observation_id, image, mime)) =
            activity.observations.iter().find_map(|observation| {
                image_for(&observation.id)
                    .map(|(image, mime)| (observation.id.clone(), image, mime))
            })
        else {
            batch.skipped += 1;
            continue;
        };

        let observation = ObservationSummary {
            id: observation_id,
            at: activity.start,
            app_name: None,
            window_title: Some(activity.original_title.clone()),
            domain: None,
            text: None,
        };

        match worker
            .resolve(ActivityRequest {
                observation: &observation,
                window_key: &activity.id,
                image: &image,
                mime: &mime,
                at,
                rule_resolved: false,
            })
            .await?
        {
            ActivityAiOutcome::Inferred { suggestion, .. }
            | ActivityAiOutcome::Cached { suggestion } => {
                batch
                    .suggestions
                    .push(mc_domain::projector::InferredSuggestion {
                        activity_id: activity.id.clone(),
                        title: suggestion.title,
                        category: suggestion.category,
                        confidence: suggestion.confidence,
                        model: suggestion.model,
                    });
            }
            ActivityAiOutcome::Degraded { .. } | ActivityAiOutcome::Skipped { .. } => {
                batch.degraded += 1;
            }
        }
    }

    // 记账在最后统一收口：中间任何一条 return 都不会漏掉已经发生的调用。
    batch.calls = worker.take_calls();
    Ok(batch)
}

/// 标题上限（字符数）。提示词要求 12 个汉字 / 6 个英文词，这里给一定余量：
/// 目的是拦住「整段话当标题」，不是精确复刻提示词。
const MAX_TITLE_CHARS: usize = 24;

/// 分类的封闭集合（中英两份提示词的并集）。与规则引擎产出的中文分类对齐。
const KNOWN_CATEGORIES: &[&str] = &[
    "开发",
    "需求",
    "文档",
    "研究",
    "协作",
    "其他",
    "development",
    "requirements",
    "documentation",
    "research",
    "collaboration",
    "other",
];

fn truncate_title(title: &str) -> String {
    let trimmed = title.trim();
    let mut chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= MAX_TITLE_CHARS {
        return trimmed.to_string();
    }
    chars.truncate(MAX_TITLE_CHARS);
    format!("{}…", chars.into_iter().collect::<String>())
}

/// 未知分类归一到「其他」；英文分类映射回中文，保证两种 locale 下同一类活动同标签。
fn normalize_category(category: &str) -> String {
    let lowered = category.trim().to_ascii_lowercase();
    let mapped = match lowered.as_str() {
        "development" => "开发",
        "requirements" => "需求",
        "documentation" | "writing" => "文档",
        "research" => "研究",
        "collaboration" => "协作",
        "other" => "其他",
        other => other,
    };
    if KNOWN_CATEGORIES.contains(&mapped) {
        mapped.to_string()
    } else {
        "其他".to_string()
    }
}

/// 校验模型输出。**任何一项不满足都降级**，不做「尽力猜」——
/// 猜错的活动会带着 `Inferred` 标签写进用户的历史，比没有结论更糟。
fn parse_suggestion(raw: &str, model: &str) -> Result<ActivitySuggestion, String> {
    let value = crate::extract::parse::json_object(raw)
        .ok_or_else(|| "模型输出不是 JSON 对象".to_string())?;

    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or_else(|| "模型输出缺少非空 title".to_string())?;
    // 提示词要求「不超过 12 个字」，但约束不能只交给模型自觉：超长标题会污染
    // 时间线与检索展示。这里按字符数截断（中日韩按字计，英文单词会被切开，故留 24 的余量）。
    let title = truncate_title(title);

    let confidence = value
        .get("confidence")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| "模型输出缺少 confidence".to_string())?;
    if !(confidence > 0.0 && confidence <= 1.0) {
        return Err(format!("confidence {confidence} 不在 (0, 1] 区间内"));
    }

    // 分类是**封闭集合**：模型返回 "coding" / "会议" 这类近义词时不能直接入库，
    // 否则分类标签会随时间发散、按 category 聚合时碎成一堆近义标签。
    let category = value
        .get("category")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|category| !category.is_empty())
        .map(normalize_category);

    Ok(ActivitySuggestion {
        title,
        category,
        confidence: confidence as f32,
        origin: Provenance::Inferred {
            model: model.to_string(),
        },
        model: model.to_string(),
    })
}
