//! 内嵌提示词（编译进二进制，不依赖运行时文件；见 `check-prompt-embedded.sh`）。

use crate::model::SummaryLocale;

pub const PROMPT_VERSION: &str = "2026.09.1";

pub const STAGE_SUMMARY_ZH: &str = include_str!("../prompts/stage_summary.zh.md");
pub const STAGE_SUMMARY_EN: &str = include_str!("../prompts/stage_summary.en.md");

/// 取阶段总结提示词。**任何情况下都返回非空内容**。
pub fn stage_summary(locale: SummaryLocale) -> &'static str {
    match locale {
        SummaryLocale::ZhCn => STAGE_SUMMARY_ZH,
        SummaryLocale::EnUs => STAGE_SUMMARY_EN,
    }
}
