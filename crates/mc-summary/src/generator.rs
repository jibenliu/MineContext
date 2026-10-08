//! 阶段总结生成器：正常路径 + 降级链。
//!
//! 关键设计是**返回类型里没有 `Err`**：`GenerateOutcome` 只有
//! `Model { .. }` 与 `Fallback { .. }` 两个分支，于是调用方**没办法
//! 「忘记处理失败」** —— 模型不可用只可能降级，不可能出现「这个阶段没有总结」。
//!
//! 发给模型的是**结构化证据**（时间段、活动序列、分类、计数）而不是原始截图：
//! 总结要的是「这段时间在做什么」，塞图只会更贵更差。

use std::sync::Arc;

use mc_common::time::Timestamp;
use mc_providers::{ChatMessage, ChatProvider, ChatRequest};

use crate::fallback;
use crate::model::{Quality, RenderedSummary, StageSummaryInput, SummaryLocale};
use crate::template::SummaryTemplate;
use mc_common::observability::{info, warn};

pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum GenerateOutcome {
    Model {
        summary: RenderedSummary,
    },
    /// 降级路径：`reason` 要写进 `pipeline_failures`，让失败可见
    Fallback {
        summary: RenderedSummary,
        reason: String,
    },
}

impl GenerateOutcome {
    pub fn summary(&self) -> &RenderedSummary {
        match self {
            Self::Model { summary } | Self::Fallback { summary, .. } => summary,
        }
    }

    pub fn into_summary(self) -> RenderedSummary {
        match self {
            Self::Model { summary } | Self::Fallback { summary, .. } => summary,
        }
    }

    pub const fn is_fallback(&self) -> bool {
        matches!(self, Self::Fallback { .. })
    }
}

pub struct SummaryGenerator {
    provider: Arc<dyn ChatProvider>,
    template: SummaryTemplate,
    locale: SummaryLocale,
    max_attempts: u32,
    /// 出网前的文本脱敏（`privacy.redact_patterns`）。
    ///
    /// 放在这一层而不是调用方：提示词是在这里拼出来的，
    /// 脱敏必须贴着拼装点，才能保证「发了什么就一定脱敏了什么」。
    redactor: mc_common::redact::Redactor,
    /// 累计命中次数（诊断可见）。
    redactions: Arc<std::sync::atomic::AtomicU64>,
}

impl SummaryGenerator {
    pub fn new(
        provider: Arc<dyn ChatProvider>,
        template: SummaryTemplate,
        locale: SummaryLocale,
    ) -> Self {
        Self {
            provider,
            template,
            locale,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            redactor: mc_common::redact::Redactor::default(),
            redactions: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    /// 设置脱敏规则（未设置时不做任何改写）。
    pub fn with_redactor(mut self, redactor: mc_common::redact::Redactor) -> Self {
        self.redactor = redactor;
        self
    }

    /// 累计脱敏命中次数。
    pub fn redaction_count(&self) -> u64 {
        self.redactions.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn template(&self) -> &SummaryTemplate {
        &self.template
    }

    /// 换一份模板：调用方在运行期按配置或请求解析出模板后用它覆盖默认值。
    pub fn with_template(mut self, template: SummaryTemplate) -> Self {
        self.template = template;
        self
    }

    pub fn with_max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts.max(1);
        self
    }

    /// 生成总结。**永远返回一份非空总结**（可能带 `Fallback` 标记）。
    pub async fn generate(&self, input: &StageSummaryInput) -> GenerateOutcome {
        let mut last_error = String::from("未发起调用");

        for attempt in 1..=self.max_attempts {
            let request = self.build_request(input);
            match self.provider.complete(&request).await {
                Ok(response) => {
                    let body = response.text.trim().to_string();
                    if body.is_empty() {
                        last_error = "模型返回空内容".to_string();
                        continue;
                    }

                    // 结构化字段与模型正文分开取：字段由模板确定，
                    // 因此换模型、换提示词都不会让卡片结构变形。
                    let fallback = fallback::generate(input, &self.template);
                    info!(
                        component = "summary",
                        event = "generated",
                        quality = "model",
                        model = %self.provider.id(),
                        prompt_tokens = response.usage.prompt_tokens,
                        completion_tokens = response.usage.completion_tokens,
                        "总结已生成"
                    );
                    return GenerateOutcome::Model {
                        summary: RenderedSummary {
                            title: fallback.title.clone(),
                            fields: fallback.fields,
                            body_markdown: body,
                            quality: Quality::Model,
                            model: Some(self.provider.id()),
                            prompt_tokens: response.usage.prompt_tokens,
                            completion_tokens: response.usage.completion_tokens,
                        },
                    };
                }
                Err(error) => {
                    last_error =
                        format!("第 {attempt} 次调用失败：{}", error.to_app_error().detail());
                }
            }
        }

        warn!(
            component = "summary",
            event = "fallback",
            reason = %mc_common::observability::redact_text(&last_error),
            "模型不可用，改用确定性兜底"
        );
        GenerateOutcome::Fallback {
            summary: fallback::generate(input, &self.template),
            reason: last_error,
        }
    }

    fn build_request(&self, input: &StageSummaryInput) -> ChatRequest {
        let mut user = String::new();
        user.push_str(&render_evidence(input, self.locale));

        // 字段清单也要跟随 locale：模板内置字段的 label 是中文，英文 locale 下
        // 直接用稳定的字段 id（time_range / activities / …），避免「英文提示词 + 中文标签」。
        // （用户自定义模板的中文 label 同样按此处理；模板级别的多语言属于产品决定。）
        let field_list: Vec<String> = match self.locale {
            SummaryLocale::ZhCn => self
                .template
                .fields
                .iter()
                .map(|field| format!("- {}（{}）", field.label, field.id))
                .collect(),
            SummaryLocale::EnUs => self
                .template
                .fields
                .iter()
                .map(|field| format!("- {}", field.id))
                .collect(),
        };
        if !field_list.is_empty() {
            match self.locale {
                SummaryLocale::ZhCn => user.push_str("\n总结里需要覆盖这些信息：\n"),
                SummaryLocale::EnUs => user.push_str("\nThe summary must cover these fields:\n"),
            }
            user.push_str(&field_list.join("\n"));
        }

        // 脱敏发生在最后一步：对**实际要发出去的正文**处理，
        // 而不是对中间变量处理（否则容易漏掉后拼上去的部分）。
        let redacted = self.redactor.redact(&user);
        self.redactions.fetch_add(
            redacted.hits.len() as u64,
            std::sync::atomic::Ordering::SeqCst,
        );

        ChatRequest {
            messages: vec![
                ChatMessage::system(crate::prompts::stage_summary(self.locale)),
                ChatMessage::user(redacted.text),
            ],
            max_tokens: Some(512),
            temperature: Some(0.2),
            // 总结是散文，强制 JSON 只会让模型把内容塞进转义字符串里
            json_mode: false,
        }
    }
}

/// 证据文案。提示词有 zh/en 两套，证据也必须跟着走 ——
/// 「英文提示词 + 中文证据」会让模型跨语言理解，质量不稳定。
struct EvidenceLabels {
    range: &'static str,
    volume: &'static str,
    observations: &'static str,
    blocked: &'static str,
    blocked_suffix: &'static str,
    activities_header: &'static str,
    no_activities: &'static str,
    inferred: &'static str,
}

impl EvidenceLabels {
    fn for_locale(locale: SummaryLocale) -> Self {
        match locale {
            SummaryLocale::ZhCn => Self {
                range: "时间范围",
                volume: "采集量",
                observations: "条观测",
                blocked: "其中",
                blocked_suffix: "条命中隐私规则，未参与总结",
                activities_header: "活动（按时间顺序）：",
                no_activities: "（没有识别出活动，只有采集记录）",
                inferred: " · 推测",
            },
            SummaryLocale::EnUs => Self {
                range: "Time range",
                volume: "Volume",
                observations: "observations",
                blocked: "excluded",
                blocked_suffix: "observations matched privacy rules",
                activities_header: "Activities (chronological):",
                no_activities: "(no activity detected, only raw captures)",
                inferred: " · inferred",
            },
        }
    }
}

/// 结构化证据。**不放原始截图、不放 OCR 全文** ——
/// 总结要的是「做了什么」，不是「屏幕上有哪些字」。
fn render_evidence(input: &StageSummaryInput, locale: SummaryLocale) -> String {
    // 证据文案跟随 locale：英文提示词 + 中文证据会让模型跨语言理解，
    // 质量不稳定（与对话提示词的语言问题是同一个根因）。
    let labels = EvidenceLabels::for_locale(locale);
    let mut out = String::new();
    out.push_str(&format!(
        "{range}：{} – {}（{}）\n",
        local(input.range.start, &input.timezone),
        local(input.range.end, &input.timezone),
        input.timezone,
        range = labels.range
    ));
    out.push_str(&format!(
        "{volume}：{} {}\n",
        input.observation_count,
        labels.observations,
        volume = labels.volume
    ));
    if input.blocked_observations > 0 {
        out.push_str(&format!(
            "{blocked} {} {}\n",
            input.blocked_observations,
            labels.blocked_suffix,
            blocked = labels.blocked
        ));
    }

    out.push_str(&format!("\n{}\n", labels.activities_header));
    if input.activities.is_empty() {
        out.push_str(&format!("- {}\n", labels.no_activities));
    }
    for activity in &input.activities {
        out.push_str(&format!(
            "- {start}–{end} {title}{category}{inferred}\n",
            start = local(activity.start, &input.timezone),
            end = local(activity.end, &input.timezone),
            title = activity.title,
            category = activity
                .category
                .as_deref()
                .filter(|c| !c.trim().is_empty())
                .map(|c| format!("（{c}）"))
                .unwrap_or_default(),
            inferred = if activity.inferred {
                labels.inferred
            } else {
                ""
            }
        ));
    }

    out
}

fn local(at: Timestamp, timezone: &str) -> String {
    at.format_in_tz(timezone, "%H:%M")
        .unwrap_or_else(|_| at.to_rfc3339()[11..16].to_string())
}
