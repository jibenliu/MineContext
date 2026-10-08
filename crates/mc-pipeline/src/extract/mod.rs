//! 结构化抽取器：调用模型 → 解析 → 失败重试 → 降级。
//!
//! 第五级降级链在这里实现（前四级在 [`parse`]）：
//! 解析失败时把**校验错误回给模型**再试一次；仍失败则产出
//! [`ExtractionOutcome::Degraded`]，**完整保留原始输出**。
//!
//! 关键性质：**任何模型输出都不能让管线崩溃或丢数据**。

pub mod parse;

pub use parse::{parse, StructuredUnderstanding};

use std::sync::Arc;

use mc_common::error::AppError;
use mc_providers::{ChatMessage, ChatProvider, ChatRequest};

use crate::prompts::{screenshot_analyze, Locale};

#[derive(Debug, Clone, PartialEq)]
pub enum ExtractionOutcome {
    Parsed {
        understanding: StructuredUnderstanding,
        attempts: u8,
    },
    /// 两次都没解析出来。**原始输出完整保留**，观测不丢。
    Degraded { raw: String, reason: String },
}

impl ExtractionOutcome {
    pub fn is_parsed(&self) -> bool {
        matches!(self, Self::Parsed { .. })
    }

    pub fn understanding(&self) -> Option<&StructuredUnderstanding> {
        match self {
            Self::Parsed { understanding, .. } => Some(understanding),
            Self::Degraded { .. } => None,
        }
    }

    pub fn raw_text(&self) -> Option<&str> {
        match self {
            Self::Degraded { raw, .. } => Some(raw),
            Self::Parsed { .. } => None,
        }
    }
}

pub struct StructuredExtractor {
    provider: Arc<dyn ChatProvider>,
    locale: Locale,
}

impl StructuredExtractor {
    pub fn new(provider: Arc<dyn ChatProvider>, locale: Locale) -> Self {
        Self { provider, locale }
    }

    /// 从文本输入抽取结构化理解。
    ///
    /// 提示词由「内嵌模板 + 本次输入」构成，**不携带任何跨调用的历史状态** ——
    /// 把历史结果无条件拼进 prompt 会让提示词随运行时间无界增长，
    /// 这是「2 小时烧掉 300 万 token」的直接成因。
    pub async fn extract(&self, input: &str) -> Result<ExtractionOutcome, AppError> {
        let system = screenshot_analyze(self.locale);
        let request = self.build_request(system, input, None);

        let response = self
            .provider
            .complete(&request)
            .await
            .map_err(|e| e.to_app_error())?;

        match parse(&response.text) {
            Ok(understanding) => Ok(ExtractionOutcome::Parsed {
                understanding,
                attempts: 1,
            }),
            Err(first_error) => {
                // 第五级：把校验错误回给模型，再试一次
                let retry = self.build_request(system, input, Some(&first_error));
                let second = self
                    .provider
                    .complete(&retry)
                    .await
                    .map_err(|e| e.to_app_error())?;

                match parse(&second.text) {
                    Ok(understanding) => Ok(ExtractionOutcome::Parsed {
                        understanding,
                        attempts: 2,
                    }),
                    Err(second_error) => Ok(ExtractionOutcome::Degraded {
                        raw: second.text,
                        reason: format!("首次失败：{first_error}；重试失败：{second_error}"),
                    }),
                }
            }
        }
    }

    fn build_request(
        &self,
        system: &str,
        input: &str,
        previous_error: Option<&str>,
    ) -> ChatRequest {
        let mut messages = vec![ChatMessage::system(system), ChatMessage::user(input)];

        if let Some(error) = previous_error {
            messages.push(ChatMessage::user(format!(
                "上一次输出无法使用：{error}\n\
                 请只输出一个符合要求的 JSON 对象，不要任何解释或 markdown 围栏。"
            )));
        }

        ChatRequest {
            messages,
            max_tokens: Some(512),
            temperature: Some(0.0),
            json_mode: true,
        }
    }
}
