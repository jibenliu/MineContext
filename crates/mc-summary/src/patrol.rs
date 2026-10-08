//! 巡检补偿：**兜底中的兜底**。
//!
//! 正常路径与失败路径都建立在「进程活着、任务正常调度」的前提上；崩溃、关机、
//! 任务被误删、迁移写坏数据这些情况只有巡检能兜住 —— 它定时扫描「已关闭但
//! 没有总结」的阶段并立刻补一条兜底。
//!
//! 因此它保证的是「有阶段必有总结」这条不变量**最终**成立，
//! 而不只是「大概率成立」。

use mc_common::error::AppError;
use mc_common::observability::warn;
use mc_common::time::Timestamp;
use mc_storage::projectors::stages::StageRow;
use mc_storage::projectors::summaries::{NewSummary, StoredSummary};
use mc_storage::Database;

use crate::model::{ActivityDigest, Quality, StageSummaryInput, SummaryRange};
use crate::source::SummarySource;

/// 巡检策略。
#[derive(Debug, Clone, PartialEq)]
pub struct PatrolPolicy {
    /// 阶段关闭后必须在多久内产出总结（配置项 `stage.summary_deadline_secs`）
    pub deadline_secs: u64,
    /// 短于此时长的阶段不强制总结（4.36：避免噪声总结）
    pub min_stage_duration_secs: u64,
    pub timezone: String,
}

impl Default for PatrolPolicy {
    fn default() -> Self {
        Self {
            deadline_secs: 600,
            min_stage_duration_secs: 900,
            timezone: "UTC".to_string(),
        }
    }
}

/// 单轮最多补写多少个阶段。积压再多也只做一批，剩下的下一轮继续。
pub const MAX_REPAIRS_PER_ROUND: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct PatrolReport {
    /// 本次补出的总结
    pub repaired: Vec<String>,
    /// 因为过短而**刻意不补**的阶段（不是故障，但要说清楚）
    pub skipped_too_short: Vec<String>,
    /// 出现降级（fallback）的阶段 —— 这些会同时写进 `pipeline_failures`
    pub degraded: Vec<String>,
}

impl PatrolReport {
    pub fn is_empty(&self) -> bool {
        self.repaired.is_empty() && self.skipped_too_short.is_empty() && self.degraded.is_empty()
    }
}

/// 扫一轮：给「已关闭、超过 deadline、却没有总结」的阶段补总结。
///
/// 返回的报告让调用方能回答「这一轮为什么没补/为什么降级了」。
pub async fn patrol_once(
    db: &Database,
    source: &dyn SummarySource,
    policy: &PatrolPolicy,
    now: Timestamp,
) -> Result<PatrolReport, AppError> {
    patrol_once_observed(db, source, policy, now, &mut |_| {}).await
}

/// 同上，但每写出一条总结就回调一次。
///
/// 回调存在的原因：总结落库之后要**立刻推给 UI**（SSE `summary:created`），
/// 而广播属于控制面的事，mc-summary 不该知道 HTTP。因此这里只回调，
/// 由调用方决定怎么通知。
pub async fn patrol_once_observed(
    db: &Database,
    source: &dyn SummarySource,
    policy: &PatrolPolicy,
    now: Timestamp,
    on_summary: &mut (dyn FnMut(&StoredSummary) + Send),
) -> Result<PatrolReport, AppError> {
    let missing = db.stages_missing_summary(now, policy.deadline_secs)?;
    let backlog = missing.len();
    let mut over_limit = false;

    let mut report = PatrolReport {
        repaired: Vec::new(),
        skipped_too_short: Vec::new(),
        degraded: Vec::new(),
    };

    // 单轮上限：刚配好模型时可能一次积压很多历史阶段，一轮全做完会让巡检长时间
    // 占着（调用方是后台循环），而剩下的下一轮继续补 —— 与 adhoc 分块、活动推断
    // 同一套思路。超限只记一条日志，不当作失败。
    let mut processed = 0usize;
    for stage in missing {
        if stage_span_secs(&stage) < policy.min_stage_duration_secs {
            // 过短的阶段（防抖残留）刻意不补：给它们生成总结只会稀释真正的总结
            report.skipped_too_short.push(stage.id);
            continue;
        }

        if processed >= MAX_REPAIRS_PER_ROUND {
            over_limit = true;
            break;
        }
        processed += 1;

        let input = build_input(db, &stage, policy)?;
        let outcome = source.generate(&input).await;
        let summary = outcome.summary().clone();
        let quality = summary.quality;

        let stored = NewSummary {
            id: format!("sum-{}", stage.id),
            kind: "stage".to_string(),
            stage_id: Some(stage.id.clone()),
            template_id: source.template_id(),
            start: stage.start,
            end: stage.end.unwrap_or(stage.start),
            title: summary.title.clone(),
            fields: summary.fields.clone(),
            body_markdown: summary.body_markdown.clone(),
            quality: match quality {
                Quality::Model => "model".to_string(),
                Quality::Fallback => "fallback".to_string(),
            },
            model: summary.model.clone(),
            prompt_tokens: summary.prompt_tokens,
            completion_tokens: summary.completion_tokens,
            scope: None,
        };
        db.insert_summary(&stored, now)?;
        if let Some(created) = db.stage_summaries(&stage.id)?.last() {
            on_summary(created);
        }

        // 降级必须可见：写进 pipeline_failures，
        // 诊断页会列出来，用户也能在 UI 上看到 fallback 标记
        if let crate::generator::GenerateOutcome::Fallback { reason, .. } = &outcome {
            let error = AppError::new(
                mc_common::error::ErrorCode::ProviderUnavailableFallback,
                format!("阶段 {} 的总结降级为兜底：{reason}", stage.id),
            );
            db.record_failure(now, "summary", &error, "warn")?;
            report.degraded.push(stage.id.clone());
        }

        report.repaired.push(stage.id);
    }

    if over_limit {
        // 超限不是失败：剩下的下一轮继续补。但要看得见，否则用户只会觉得「补得慢」。
        warn!(
            component = "summary",
            event = "patrol_capped",
            pending = backlog,
            this_round = processed,
            "待补写阶段较多，本轮先补一批，其余下一轮继续"
        );
    }

    Ok(report)
}

fn stage_span_secs(stage: &StageRow) -> u64 {
    let end = stage.end.unwrap_or(stage.start);
    (end.saturating_diff_millis(stage.start).max(0) / 1000) as u64
}

/// 把阶段还原成总结的输入。
///
/// 活动的标题/分类来自派生表（不是原始事件）：阶段本来就是把活动聚起来的产物，
/// 因此总结的输入应当与时间线上看到的一致。
fn build_input(
    db: &Database,
    stage: &StageRow,
    policy: &PatrolPolicy,
) -> Result<StageSummaryInput, AppError> {
    let activities = mc_storage::projectors::activities::read_by_ids(db, &stage.activities)?;

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

    Ok(StageSummaryInput {
        stage_id: stage.id.clone(),
        range: SummaryRange {
            start: stage.start,
            end: stage.end.unwrap_or(stage.start),
        },
        timezone: policy.timezone.clone(),
        locale: crate::model::SummaryLocale::ZhCn,
        activities: digests,
        observation_count,
        blocked_observations: 0,
    })
}
