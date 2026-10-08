//! 确定性兜底总结。
//!
//! **模型不可用不是「什么都不给」的理由**：
//! 三次重试都失败、断网、没配模型，
//! 用户依然要看到「这段时间我做了什么」。
//!
//! 因此兜底总结由**结构化证据**直接渲染：
//! 时间段 + 活动序列 + 涉及分类 + 采集量 + 隐私说明。
//! 全部确定性（同输入同输出），因此可以进快照测试，
//! 也能在重放时逐字节复现。

use crate::model::{ActivityDigest, Quality, RenderedSummary, StageSummaryInput, SummaryLocale};
use crate::template::{FieldKind, SummaryTemplate};

/// 生成兜底总结。**任何输入都会返回非空正文**（4.35 属性测试守门）。
pub fn generate(input: &StageSummaryInput, template: &SummaryTemplate) -> RenderedSummary {
    let mut fields = std::collections::BTreeMap::new();
    let mut sections: Vec<String> = Vec::new();

    for field in &template.fields {
        let value = render_field(field.kind, input);
        if value.trim().is_empty() {
            continue;
        }
        fields.insert(field.id.clone(), value.clone());
        sections.push(format!("### {}\n{}", field.label, value));
    }

    let title = format_title(input);

    // 模板把字段裁到只剩「不可能有内容」的组合时，正文仍要有话说：
    // 时间段永远可渲染，因此这里补一段最小信息，保证正文非空。
    if sections.is_empty() {
        sections.push(format!(
            "### {}\n{}",
            FieldKind::TimeRange.label(input.locale),
            render_time_range(input)
        ));
    }

    RenderedSummary {
        title,
        fields,
        body_markdown: sections.join("\n\n"),
        quality: Quality::Fallback,
        model: None,
        prompt_tokens: 0,
        completion_tokens: 0,
    }
}

/// 渲染单个字段。模型路径也会复用它来填结构化字段 ——
/// 「哪些字段、怎么渲染」只能有一份实现。
pub fn render_field(kind: FieldKind, input: &StageSummaryInput) -> String {
    match kind {
        FieldKind::TimeRange => render_time_range(input),
        FieldKind::ActivityList => render_activities(input),
        FieldKind::AppList => render_categories(input),
        FieldKind::HighlightList => render_highlights(input),
        FieldKind::ObservationCount => render_captured(input),
        FieldKind::BlockedNotice => render_privacy(input),
    }
}

/// `17:00–17:30（Asia/Shanghai）` —— 用户看到的是**本地时间**。
///
/// 时区解析失败时退回 UTC 并在文案里说明：宁可标注「时间可能有偏差」，
/// 也不要显示一个假装是本地的错误时间。
fn render_time_range(input: &StageSummaryInput) -> String {
    let (start, suffix) = local_hm(input.range.start, &input.timezone);
    let (end, _) = local_hm(input.range.end, &input.timezone);
    format!("{start}–{end}（{}{suffix}）", input.timezone)
}

fn local_hm(at: mc_common::time::Timestamp, timezone: &str) -> (String, &'static str) {
    match at.format_in_tz(timezone, "%H:%M") {
        Ok(text) => (text, ""),
        Err(_) => (
            at.to_rfc3339()[11..16].to_string(),
            "，时区无法识别，按 UTC 显示",
        ),
    }
}

fn render_activities(input: &StageSummaryInput) -> String {
    if input.activities.is_empty() {
        return if input.locale.is_chinese() {
            "这段时间没有识别出活动（只有采集记录）".to_string()
        } else {
            "No activities were recognized (captures only)".to_string()
        };
    }

    input
        .activities
        .iter()
        .map(|activity| render_one_activity(activity, input.locale, &input.timezone))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_one_activity(activity: &ActivityDigest, locale: SummaryLocale, timezone: &str) -> String {
    let title = if activity.title.trim().is_empty() {
        if locale.is_chinese() {
            "未命名活动"
        } else {
            "Untitled activity"
        }
    } else {
        activity.title.trim()
    };

    let span = format!(
        "{start}–{end}",
        start = local_hm(activity.start, timezone).0,
        end = local_hm(activity.end, timezone).0
    );
    let mut line = format!("- {span} {title}");

    if let Some(category) = activity
        .category
        .as_deref()
        .filter(|c| !c.trim().is_empty())
    {
        line.push_str(&format!("（{category}）"));
    }
    if activity.inferred {
        line.push_str(if locale.is_chinese() {
            " · 推测"
        } else {
            " · inferred"
        });
    }
    line
}

fn render_categories(input: &StageSummaryInput) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for activity in &input.activities {
        let Some(category) = activity
            .category
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        else {
            continue;
        };
        match counts.iter_mut().find(|(name, _)| name == category) {
            Some((_, count)) => *count += 1,
            None => counts.push((category.to_string(), 1)),
        }
    }

    if counts.is_empty() {
        return String::new();
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts
        .iter()
        .map(|(name, count)| format!("{name}×{count}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn render_highlights(input: &StageSummaryInput) -> String {
    // 兜底总结不做「提炼」——那是模型的活。这里只如实列出时长最长的几段，
    // 让人一眼看到「主要时间花在哪」。
    let mut activities: Vec<&ActivityDigest> = input.activities.iter().collect();
    activities.sort_by_key(|activity| std::cmp::Reverse(duration_secs(activity)));

    activities
        .iter()
        .take(3)
        .map(|activity| {
            format!(
                "- {}（{} 分钟）",
                activity.title.trim(),
                duration_secs(activity) / 60
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_captured(input: &StageSummaryInput) -> String {
    if input.locale.is_chinese() {
        format!(
            "{} 条观测 · {} 个活动",
            input.observation_count,
            input.activities.len()
        )
    } else {
        format!(
            "{} captures · {} activities",
            input.observation_count,
            input.activities.len()
        )
    }
}

fn render_privacy(input: &StageSummaryInput) -> String {
    if input.blocked_observations == 0 {
        return String::new();
    }
    if input.locale.is_chinese() {
        format!(
            "有 {} 条观测命中隐私规则，未参与本总结。",
            input.blocked_observations
        )
    } else {
        format!(
            "{} captures matched privacy rules and are not included.",
            input.blocked_observations
        )
    }
}

/// 标题用**本地时间**：用户看到的是自己的时间，不是 UTC。
fn format_title(input: &StageSummaryInput) -> String {
    let start = local_hm(input.range.start, &input.timezone).0;
    let end = local_hm(input.range.end, &input.timezone).0;
    if input.locale.is_chinese() {
        format!("{start}–{end} 阶段总结")
    } else {
        format!("{start}–{end} stage summary")
    }
}

fn duration_secs(activity: &ActivityDigest) -> u64 {
    (activity.end.saturating_diff_millis(activity.start).max(0) / 1000) as u64
}

/// 极短阶段是否值得总结（4.36）。
///
/// 防抖残留会产生几十秒的「阶段」；给它们生成总结只会稀释真正的总结，
/// 反而让用户不再相信这些卡片。
pub fn should_summarize(duration_secs: u64, min_duration_secs: u64) -> bool {
    duration_secs >= min_duration_secs
}
