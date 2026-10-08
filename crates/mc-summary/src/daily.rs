//! 跨天日报。
//!
//! 「今天做了什么」是用户每天都会问一次的问题，因此它值得自动产出：
//! 本地日翻页时，把昨天那一段收成一份日报。
//!
//! 与阶段总结共用同一套引擎与兜底（`SummarySource`），因此不变量相同：
//! **只要那天有内容，就一定拿得到一份非空总结**。
//!
//! 三条刻意的约束：范围是**本地日**边界（DST 日不是 24 小时，`now - 24h`
//! 会漏掉或重复一小时）；同一天重复生成**幂等**；**空白天不出日报**。

use chrono::NaiveDate;
use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_storage::projectors::summaries::{NewSummary, StoredSummary};
use mc_storage::Database;

use crate::model::{ActivityDigest, StageSummaryInput, SummaryLocale, SummaryRange};
use crate::source::SummarySource;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyConfig {
    pub timezone: String,
    pub locale: SummaryLocale,
}

/// 生成某一天的日报。`None` 表示那天没有内容（不是错误）。
pub async fn generate_daily(
    db: &Database,
    source: &dyn SummarySource,
    config: &DailyConfig,
    day: NaiveDate,
    now: Timestamp,
) -> Result<Option<StoredSummary>, AppError> {
    // 幂等：已经有这一天的日报就直接复用（不重复调模型）
    let existing_id = daily_id(day);
    let existing = db
        .read_summaries(None, Some("daily"))?
        .into_iter()
        .find(|summary| summary.id == existing_id);
    if let Some(existing) = existing {
        return Ok(Some(existing));
    }

    let (start, end) = Timestamp::day_bounds_for(day, &config.timezone)?;

    let activities: Vec<mc_storage::projectors::activities::StoredActivity> =
        mc_storage::projectors::activities::read_all(db)?
            .into_iter()
            .filter(|activity| activity.start < end && activity.end >= start)
            .collect();

    let stages = db
        .read_stages()?
        .into_iter()
        .filter(|stage| {
            let stage_end = stage.end.unwrap_or(stage.start);
            stage.start < end && stage_end >= start
        })
        .count();

    if activities.is_empty() && stages == 0 {
        return Ok(None);
    }

    let digests: Vec<ActivityDigest> = activities
        .iter()
        .map(|activity| ActivityDigest {
            id: activity.id.clone(),
            title: activity.title.clone(),
            category: activity.category.clone(),
            start: activity.start,
            end: activity.end,
            observations: activity.evidence.len() as u32,
            inferred: matches!(
                activity.origin,
                mc_domain::activity::Provenance::Inferred { .. }
            ),
        })
        .collect();
    let observation_count = digests.iter().map(|digest| digest.observations).sum();

    let input = StageSummaryInput {
        stage_id: String::new(),
        range: SummaryRange { start, end },
        timezone: config.timezone.clone(),
        locale: config.locale,
        activities: digests,
        observation_count,
        blocked_observations: 0,
    };

    let outcome = source.generate(&input).await;
    let summary = outcome.summary().clone();

    db.insert_summary(
        &NewSummary {
            id: existing_id,
            kind: "daily".to_string(),
            stage_id: None,
            template_id: source.template_id(),
            start,
            end,
            title: format!("{} 日报", day),
            fields: summary.fields.clone(),
            body_markdown: summary.body_markdown.clone(),
            quality: match summary.quality {
                crate::model::Quality::Model => "model".to_string(),
                crate::model::Quality::Fallback => "fallback".to_string(),
            },
            model: summary.model.clone(),
            prompt_tokens: summary.prompt_tokens,
            completion_tokens: summary.completion_tokens,
            scope: None,
        },
        now,
    )?;

    Ok(db
        .read_summaries(None, Some("daily"))?
        .into_iter()
        .find(|summary| summary.id == daily_id(day)))
}

/// 日报 id 由日期决定：同一天只能有一条，重算也不会产生第二条。
pub fn daily_id(day: NaiveDate) -> String {
    format!("sum-daily-{day}")
}
