//! 周报：把一周的日报收成一份总结。
//!
//! 与日报同构：**幂等**（一周一条）、**空周不产出**、**失败走兜底**。
//! 聚合的是**日报**而不是原始活动：日报已经做过「哪些算同一段工作」的判断，
//! 周报再做一遍只会引入不一致（同一份数据两处口径）。

use chrono::{Datelike, Duration as ChronoDuration, NaiveDate};
use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_storage::projectors::summaries::{NewSummary, StoredSummary};
use mc_storage::Database;

use mc_summary::model::{RenderedSummary, SummaryLocale, SummaryRange};
use mc_summary::source::SummarySource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeeklyConfig {
    pub timezone: String,
    pub locale: SummaryLocale,
}

/// 周报 id：由**周一**的日期决定（同周只有一条）。
pub fn weekly_id(monday: NaiveDate) -> String {
    format!("sum-weekly-{monday}")
}

/// 把任意日期归一到所在周的周一。
pub fn monday_of(day: NaiveDate) -> NaiveDate {
    let weekday = day.weekday().num_days_from_monday() as i64;
    day - ChronoDuration::days(weekday)
}

/// 生成某一周的周报。`None` 表示这一周没有任何日报（不是错误）。
pub async fn generate_weekly(
    db: &Database,
    source: &dyn SummarySource,
    config: &WeeklyConfig,
    week_start: NaiveDate,
    now: Timestamp,
) -> Result<Option<StoredSummary>, AppError> {
    let monday = monday_of(week_start);

    // 幂等：已有这一周的周报就复用
    if let Some(existing) = db
        .read_summaries(None, Some("weekly"))?
        .into_iter()
        .find(|summary| summary.id == weekly_id(monday))
    {
        return Ok(Some(existing));
    }

    // 收集这一周（周一 … 周日）的日报，按日期排序
    let mut days: Vec<(NaiveDate, StoredSummary)> = Vec::new();
    for offset in 0..7 {
        let day = monday + ChronoDuration::days(offset);
        let id = mc_summary::daily::daily_id(day);
        if let Some(summary) = db
            .read_summaries(None, Some("daily"))?
            .into_iter()
            .find(|summary| summary.id == id)
        {
            days.push((day, summary));
        }
    }

    if days.is_empty() {
        return Ok(None);
    }

    let (start, _) = Timestamp::day_bounds_for(monday, &config.timezone)?;
    let sunday = monday + ChronoDuration::days(6);
    let (_, end) = Timestamp::day_bounds_for(sunday, &config.timezone)?;

    // 把每天的日报拼成一份「证据」，再交给模型（或兜底）收成周报。
    // 拼接而不是重新聚合活动：日报的口径已经过判断，重来一遍会不一致。
    let mut body = String::new();
    for (index, (day, summary)) in days.iter().enumerate() {
        body.push_str(&format!(
            "## 第 {} 天（{day}）\n{}\n\n",
            index + 1,
            summary.body_markdown.trim()
        ));
    }

    let input = mc_summary::model::StageSummaryInput {
        stage_id: String::new(),
        range: SummaryRange { start, end },
        timezone: config.timezone.clone(),
        locale: config.locale,
        // 周报不再逐条列活动：证据是日报正文
        activities: Vec::new(),
        observation_count: days.len() as u32,
        blocked_observations: 0,
    };

    let outcome = source.generate(&input).await;
    let rendered: RenderedSummary = outcome.summary().clone();

    // 模型成功时用模型正文；降级时用「按天分段的拼接」——
    // 后者本身就是一份可读的周报（有日期、有内容），不是「生成失败」。
    let final_body = if rendered.quality == mc_summary::model::Quality::Model {
        format!("{}\n\n{}", rendered.body_markdown.trim(), body)
    } else {
        body
    };

    let stored_id = weekly_id(monday);
    db.insert_summary(
        &NewSummary {
            id: stored_id.clone(),
            kind: "weekly".to_string(),
            stage_id: None,
            template_id: source.template_id(),
            start,
            end,
            title: format!("{monday} 周报"),
            fields: rendered.fields.clone(),
            body_markdown: final_body,
            quality: match rendered.quality {
                mc_summary::model::Quality::Model => "model".to_string(),
                mc_summary::model::Quality::Fallback => "fallback".to_string(),
            },
            model: rendered.model.clone(),
            prompt_tokens: rendered.prompt_tokens,
            completion_tokens: rendered.completion_tokens,
            scope: Some(serde_json::json!({ "days": days.len() })),
        },
        now,
    )?;

    Ok(db
        .read_summaries(None, Some("weekly"))?
        .into_iter()
        .find(|summary| summary.id == stored_id))
}
