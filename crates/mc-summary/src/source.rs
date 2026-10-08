//! 总结来源：把「谁生成总结」抽象出来。
//!
//! 存在的理由是一条**真实生产路径**：用户没配模型、或者关了 AI。
//! 此时巡检仍然必须把「已关闭但没有总结」的阶段补上，
//! 因此它不能要求一个可用的 Provider —— 兜底本身就是一种来源。
//!
//! 这样 `patrol_once` 的实现里也不会出现「没有 Provider 就跳过」的分支：
//! 跳过就等于打破了「有阶段必有总结」。

use async_trait::async_trait;

use crate::fallback;
use crate::generator::{GenerateOutcome, SummaryGenerator};
use crate::model::StageSummaryInput;
use crate::template::SummaryTemplate;

#[async_trait]
pub trait SummarySource: Send + Sync {
    async fn generate(&self, input: &StageSummaryInput) -> GenerateOutcome;
    /// 记录进 `summaries.template_id`，便于回答「这条总结是用哪个模板产出的」
    fn template_id(&self) -> String;
}

#[async_trait]
impl SummarySource for SummaryGenerator {
    async fn generate(&self, input: &StageSummaryInput) -> GenerateOutcome {
        SummaryGenerator::generate(self, input).await
    }

    fn template_id(&self) -> String {
        self.template().id.clone()
    }
}

/// 只走确定性兜底：不调用任何模型。
pub struct FallbackOnly {
    template: SummaryTemplate,
    reason: String,
}

impl FallbackOnly {
    pub fn new(template: SummaryTemplate) -> Self {
        Self {
            template,
            reason: "未配置模型，直接使用确定性兜底".to_string(),
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = reason.into();
        self
    }
}

#[async_trait]
impl SummarySource for FallbackOnly {
    async fn generate(&self, input: &StageSummaryInput) -> GenerateOutcome {
        GenerateOutcome::Fallback {
            summary: fallback::generate(input, &self.template),
            reason: self.reason.clone(),
        }
    }

    fn template_id(&self) -> String {
        self.template.id.clone()
    }
}
