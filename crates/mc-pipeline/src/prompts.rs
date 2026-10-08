//! 内嵌的提示词。
//!
//! **编译进二进制**，不是运行时读文件。
//!
//! 升级后新版本若依赖用户配置里存在某个模板，老配置里没有就会让截图分析
//! 持续失败。内嵌之后，缺文件、配置损坏、用户改坏模板都不可能让分析整体停摆。
//!
//! `scripts/check-prompt-embedded.sh` 会遍历 `prompts/` 下的每个文件，
//! 断言它确实被 `include_str!` 引用 —— 防止有人新增模板却忘了接线。

pub use mc_common::locale::Locale;

/// 提示词版本。每次改动都要递增，并记录进 `provider_calls`，
/// 便于回答「输出变差是不是换了提示词」。
pub const PROMPT_VERSION: &str = "2026.09.1";

pub const SCREENSHOT_ANALYZE_ZH: &str = include_str!("../prompts/screenshot_analyze.zh.md");
pub const SCREENSHOT_ANALYZE_EN: &str = include_str!("../prompts/screenshot_analyze.en.md");
pub const ACTIVITY_INFER_ZH: &str = include_str!("../prompts/activity_infer.zh.md");
pub const ACTIVITY_INFER_EN: &str = include_str!("../prompts/activity_infer.en.md");

/// 取「截图分析」提示词。**任何情况下都返回非空内容**。
pub fn screenshot_analyze(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhCn => SCREENSHOT_ANALYZE_ZH,
        Locale::EnUs => SCREENSHOT_ANALYZE_EN,
    }
}

/// 取「活动推断」提示词（规则未命中时才用）。
pub fn activity_infer(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhCn => ACTIVITY_INFER_ZH,
        Locale::EnUs => ACTIVITY_INFER_EN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_are_embedded_and_non_empty() {
        for locale in [Locale::ZhCn, Locale::EnUs] {
            let body = screenshot_analyze(locale);
            assert!(body.len() > 100, "{locale:?} 提示词过短，可能是空文件");
            assert!(body.contains("JSON"), "{locale:?} 提示词必须要求 JSON 输出");
        }
    }

    #[test]
    fn prompt_version_is_set() {
        assert!(!PROMPT_VERSION.is_empty());
        assert!(PROMPT_VERSION.contains('.'), "版本号应当是可读的语义化形式");
    }

    #[test]
    fn locale_detection_handles_common_values() {
        assert_eq!(Locale::from_config("zh-CN"), Locale::ZhCn);
        assert_eq!(Locale::from_config("en-US"), Locale::EnUs);
        assert_eq!(Locale::from_config("EN"), Locale::EnUs);
        assert_eq!(Locale::from_config(""), Locale::ZhCn);
        assert_eq!(Locale::from_config("fr"), Locale::ZhCn, "未知语言退回默认");
    }
}
