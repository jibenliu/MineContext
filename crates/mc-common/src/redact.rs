//! 文本脱敏：把命中模式的内容在**出网之前**替换掉。
//!
//! 与「拦截」的区别很重要：**拦截**（`blocked_apps` 等）让内容根本不落盘；
//! **脱敏**（本模块）让内容照常记录（本地仍然有用），只在出网时替换。
//! 因此脱敏是上传前的最后一道关卡，必须做在构造提示词的位置 ——
//! 做在采集或存储位置会**损失本地信息**。
//!
//! 模式按**正则**解释（例如 `\d{16}` 匹配 16 位数字）。正则写错时
//! `Redactor::new` 返回错误而不是忽略：静默忽略一条写错的正则，等于用户
//! 以为自己脱敏了、实际没有。替换文本固定为 [`PLACEHOLDER`]。

use regex::Regex;

use crate::error::{AppError, ErrorCode};

/// 替换后的占位文本。
pub const PLACEHOLDER: &str = "[已脱敏]";

/// 一次脱敏的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redaction {
    /// 脱敏后的文本
    pub text: String,
    /// 命中的模式（去重、保序）
    pub hits: Vec<String>,
}

impl Redaction {
    /// 是否发生了脱敏。
    pub fn is_clean(&self) -> bool {
        self.hits.is_empty()
    }
}

/// 一组脱敏规则。
#[derive(Debug, Clone, Default)]
pub struct Redactor {
    patterns: Vec<Regex>,
    sources: Vec<String>,
}

impl Redactor {
    /// 编译模式列表。非法正则**报错**（不静默跳过）。
    pub fn new(patterns: &[String]) -> Result<Self, AppError> {
        let mut compiled = Vec::with_capacity(patterns.len());
        let mut sources = Vec::with_capacity(patterns.len());

        for pattern in patterns {
            if pattern.is_empty() {
                continue;
            }
            let regex = Regex::new(pattern).map_err(|error| {
                AppError::new(
                    ErrorCode::ConfigInvalid,
                    format!("隐私脱敏模式 {pattern:?} 不是合法正则：{error}"),
                )
                .with_context("path", "privacy.redact_patterns")
            })?;
            compiled.push(regex);
            sources.push(pattern.clone());
        }

        Ok(Self {
            patterns: compiled,
            sources,
        })
    }

    /// 没有任何规则时直接返回原文（避免无谓的分配与替换）。
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// 对文本执行脱敏。
    pub fn redact(&self, text: &str) -> Redaction {
        if self.patterns.is_empty() || text.is_empty() {
            return Redaction {
                text: text.to_string(),
                hits: Vec::new(),
            };
        }

        let mut result = text.to_string();
        let mut hits: Vec<String> = Vec::new();

        for (regex, source) in self.patterns.iter().zip(&self.sources) {
            if regex.is_match(&result) {
                result = regex.replace_all(&result, PLACEHOLDER).to_string();
                if !hits.contains(source) {
                    hits.push(source.clone());
                }
            }
        }

        Redaction { text: result, hits }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor(patterns: &[&str]) -> Redactor {
        Redactor::new(&patterns.iter().map(|p| p.to_string()).collect::<Vec<_>>())
            .expect("模式合法")
    }

    #[test]
    fn replaces_matching_content() {
        let redactor = redactor(&[r"\d{16}"]);
        let redaction = redactor.redact("卡号 6222021234567890 请勿外传");

        assert!(!redaction.text.contains("6222021234567890"));
        assert!(redaction.text.contains(PLACEHOLDER));
        assert_eq!(redaction.hits, vec![r"\d{16}".to_string()]);
    }

    #[test]
    fn keeps_text_when_nothing_matches() {
        let redactor = redactor(&[r"\d{16}"]);
        let redaction = redactor.redact("今天在写 Rust");
        assert!(redaction.is_clean());
        assert_eq!(redaction.text, "今天在写 Rust");
    }

    #[test]
    fn reports_each_pattern_once() {
        let redactor = redactor(&[r"\d{4}", r"密码"]);
        let redaction = redactor.redact("密码 1234 与 5678");
        assert_eq!(redaction.hits.len(), 2);
        assert!(!redaction.text.contains("1234"));
        assert!(!redaction.text.contains("密码"));
    }

    #[test]
    fn empty_pattern_list_is_a_no_op() {
        let redactor = redactor(&[]);
        assert!(redactor.is_empty());
        assert_eq!(redactor.redact("原样").text, "原样");
    }

    #[test]
    fn invalid_pattern_is_rejected() {
        let error = Redactor::new(&["[未闭合".to_string()]).expect_err("非法正则必须报错");
        assert_eq!(error.code(), ErrorCode::ConfigInvalid);
        assert!(error.detail().contains("未闭合"));
    }
}
