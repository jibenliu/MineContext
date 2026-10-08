//! 总结的输入与产物。
//!
//! 输入刻意是**结构化证据**（活动标题序列、时间范围、分类、计数），
//! 不是原始截图堆 —— 最有价值的一条经验是：
//! 给模型塞得越多，总结越像流水账，token 还越贵。

use std::collections::BTreeMap;

use mc_common::locale::Locale;
use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SummaryLocale {
    ZhCn,
    EnUs,
}

impl SummaryLocale {
    /// 由 `general.locale` 推断提示词语言。
    ///
    /// **只支持 zh / en**：`en*` 走英文，其余一律回退中文 ——
    pub fn from_config(value: &str) -> Self {
        match Locale::from_config(value) {
            Locale::EnUs => Self::EnUs,
            Locale::ZhCn => Self::ZhCn,
        }
    }

    pub const fn is_chinese(self) -> bool {
        matches!(self, Self::ZhCn)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryRange {
    pub start: Timestamp,
    pub end: Timestamp,
}

impl SummaryRange {
    pub fn duration_secs(self) -> u64 {
        (self.end.saturating_diff_millis(self.start).max(0) / 1000) as u64
    }
}

/// 活动摘要（总结的输入单元）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityDigest {
    pub id: String,
    pub title: String,
    pub category: Option<String>,
    pub start: Timestamp,
    pub end: Timestamp,
    pub observations: u32,
    /// 这条活动是模型推断出来的（总结里要能标注「推测」）
    pub inferred: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageSummaryInput {
    pub stage_id: String,
    pub range: SummaryRange,
    pub timezone: String,
    pub locale: SummaryLocale,
    pub activities: Vec<ActivityDigest>,
    pub observation_count: u32,
    /// 被隐私规则拦下的观测数（总结里要如实说明「这段时间有内容被跳过」）
    pub blocked_observations: u32,
}

/// 总结质量。`Fallback` 必须能被 UI 区分（4.32）——
/// 用户有权知道这条总结是模型写的还是系统拼出来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Model,
    Fallback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedSummary {
    pub title: String,
    /// 模板字段 → 文本（前端按模板顺序渲染卡片）
    pub fields: BTreeMap<String, String>,
    pub body_markdown: String,
    pub quality: Quality,
    pub model: Option<String>,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}
