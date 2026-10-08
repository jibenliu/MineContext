//! 阶段投影与生命周期。
//!
//! 状态机本身是纯函数（`mc-domain::stage`），这一段把它接进真实数据：
//!
//! - `rebuild`：从事件（活动 + 锁屏）重放出阶段并**整份重写** `stages` 表 ——
//!   派生数据可以丢弃重算，增量维护反而会留下只有线上才暴露的脏行；
//! - `recover_interrupted`：重启时把上次没关掉的阶段补记为 `interrupted`；
//! - `flush_on_shutdown`：正常退出时立刻收尾，**不等模型** —— 关机路径必须有
//!   时间上界，否则用户会遇到「点了退出但关不掉」。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_domain::stage::{ActivitySignal, EndReason, ReplayStep, StagePolicy};
use mc_storage::projectors::stages::StageRow;
use mc_storage::projectors::summaries::NewSummary;
use mc_summary::fallback;
use mc_summary::model::{ActivityDigest, StageSummaryInput, SummaryLocale, SummaryRange};
use mc_summary::patrol::PatrolPolicy;
use mc_summary::template::SummaryTemplate;

use crate::state::ServerState;

/// 从事件重放阶段，并整份写回 `stages`。
///
/// 返回写完之后的阶段列表（调用方通常直接拿去用）。
pub fn rebuild(
    state: &ServerState,
    policy: &StagePolicy,
    now: Timestamp,
) -> Result<Vec<StageRow>, AppError> {
    let activities = mc_storage::projectors::activities::read_all(&state.db)?;
    let events = mc_storage::projectors::activities::domain_events(&state.db)?;

    // 时间线：活动的**开始**时刻作为信号；锁屏/睡眠事件按自己的时刻插入。
    // 重放规则（补 tick、先 tick 再处理）集中在 `mc_domain::stage::replay`，
    // 场景测试与这里走的是同一份实现。
    let mut steps: Vec<ReplayStep> = Vec::new();
    for activity in &activities {
        steps.push(ReplayStep::Signal(ActivitySignal {
            id: activity.id.clone(),
            title: activity.title.clone(),
            start: activity.start,
            end: activity.end,
        }));
    }
    for event in &events {
        if let mc_domain::projector::DomainEvent::ScreenLocked { at } = event {
            steps.push(ReplayStep::Locked(*at));
        }
    }

    let replay = mc_domain::stage::replay(policy, &steps, now);
    let mut rows: Vec<StageRow> = replay
        .closed
        .iter()
        .filter_map(|effect| stage_row(effect, policy))
        .collect();

    if let Some(open) = replay.open {
        rows.push(StageRow {
            id: open.id,
            start: open.start,
            end: None,
            state: "open".to_string(),
            end_reason: None,
            day: day_key(open.start, policy),
            activities: open.activities,
        });
    }

    let last_seq = state.db.last_seq()?;
    state.db.upsert_stages(&rows, last_seq)?;
    Ok(rows)
}

fn stage_row(effect: &mc_domain::stage::StageEffect, policy: &StagePolicy) -> Option<StageRow> {
    match effect {
        mc_domain::stage::StageEffect::Closed {
            id,
            start,
            end,
            reason,
            activities,
        } => Some(StageRow {
            id: id.clone(),
            start: *start,
            end: Some(*end),
            state: "closed".to_string(),
            end_reason: Some(end_reason_str(*reason).to_string()),
            day: day_key(*start, policy),
            activities: activities.clone(),
        }),
        mc_domain::stage::StageEffect::Opened { .. } => None,
    }
}

fn end_reason_str(reason: EndReason) -> &'static str {
    match reason {
        EndReason::Switched => "switched",
        EndReason::Idle => "idle",
        EndReason::Locked => "locked",
        EndReason::Suspended => "suspended",
        EndReason::MaxDuration => "max_duration",
        EndReason::DayBoundary => "day_boundary",
        EndReason::Shutdown => "shutdown",
        EndReason::Manual => "manual",
        EndReason::Interrupted => "interrupted",
    }
}

fn day_key(at: Timestamp, policy: &StagePolicy) -> String {
    at.to_local_date(&policy.timezone)
        .map(|date| date.to_string())
        .unwrap_or_else(|_| "1970-01-01".to_string())
}

/// 重启时把上次留下的 `open` 阶段补记为中断（4.39）。
///
/// 返回本次补记的条数（0 表示没有需要补记的）。补记之后它们就会进入巡检的视野 ——
/// 崩溃路径同样要满足「有阶段必有总结」。
pub fn recover_interrupted(state: &ServerState, at: Timestamp) -> Result<usize, AppError> {
    let stages = state.db.read_stages()?;
    let open: Vec<StageRow> = stages
        .into_iter()
        .filter(|stage| !stage.is_closed())
        .collect();
    if open.is_empty() {
        return Ok(0);
    }

    let last_seq = state.db.last_seq()?;
    for mut stage in open {
        // 结束时刻取「最后一条活动的结束」与「重启时刻」中较晚者：
        // 关机期间的时间不该算进阶段，但也不能早于已有的证据。
        let end = stage.end.unwrap_or(at);
        let end = if end > at { end } else { at };
        stage.end = Some(end);
        stage.state = "closed".to_string();
        stage.end_reason = Some("interrupted".to_string());
        state.db.upsert_stage(&stage, last_seq)?;
    }

    Ok(1)
}

/// 关机路径：立即关闭当前阶段，并用**确定性兜底**写出总结。
///
/// 刻意不调用模型：关机不能被一次网络请求拖住（4.43）。
/// 兜底内容是确定性的、亚毫秒级，且带 `quality=fallback` 标记；
/// 想要模型版本的话，下次启动的重生成/巡检会补上。
pub async fn flush_on_shutdown(
    state: &ServerState,
    policy: &StagePolicy,
    at: Timestamp,
) -> Result<Option<String>, AppError> {
    let stages = state.db.read_stages()?;
    let Some(open) = stages.into_iter().find(|stage| !stage.is_closed()) else {
        return Ok(None);
    };

    let end = open.end.unwrap_or(at);
    let end = if end > at { end } else { at };
    let closed = StageRow {
        id: open.id.clone(),
        start: open.start,
        end: Some(end),
        state: "closed".to_string(),
        end_reason: Some("shutdown".to_string()),
        day: open.day.clone(),
        activities: open.activities.clone(),
    };
    let last_seq = state.db.last_seq()?;
    state.db.upsert_stage(&closed, last_seq)?;

    let template = SummaryTemplate::default_work_stage();
    let input = build_input(state, &closed, policy, &template)?;
    let summary = fallback::generate(&input, &template);
    let id = format!("sum-{}", closed.id);

    state.db.insert_summary(
        &NewSummary {
            id: id.clone(),
            kind: "stage".to_string(),
            stage_id: Some(closed.id.clone()),
            template_id: template.id.clone(),
            start: closed.start,
            end,
            title: summary.title.clone(),
            fields: summary.fields.clone(),
            body_markdown: summary.body_markdown.clone(),
            quality: "fallback".to_string(),
            model: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            scope: None,
        },
        at,
    )?;

    if let Some(stored) = state.db.stage_summaries(&closed.id)?.last() {
        publish_summary(state, stored);
    }

    Ok(Some(id))
}

/// 没有模型时的总结来源：直接走确定性兜底。
///
/// 这是一条**真实生产路径**（用户没配模型 / 关了 AI），
/// 不是测试专用：巡检必须在这种情况下依然能把总结补出来。
pub fn fallback_generator() -> mc_summary::source::FallbackOnly {
    fallback_generator_with(SummaryTemplate::default_work_stage())
}

/// 兜底生成器也要按模板渲染：用户换了模板，没配模型时同样应当看得出差别。
pub fn fallback_generator_with(template: SummaryTemplate) -> mc_summary::source::FallbackOnly {
    mc_summary::source::FallbackOnly::new(template)
}

fn build_input(
    state: &ServerState,
    stage: &StageRow,
    policy: &StagePolicy,
    _template: &SummaryTemplate,
) -> Result<StageSummaryInput, AppError> {
    let activities = mc_storage::projectors::activities::read_by_ids(&state.db, &stage.activities)?;

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
        locale: SummaryLocale::ZhCn,
        activities: digests,
        observation_count,
        blocked_observations: 0,
    })
}

/// `summary:created` 的事件负载。
///
/// 带上正文：前端收到就能直接渲染卡片，不必再发一次请求。
/// 但**不放** `fields`/tokens 这类诊断字段 —— SSE 帧越大越容易丢。
pub fn summary_created_payload(
    summary: &mc_storage::projectors::summaries::StoredSummary,
) -> serde_json::Value {
    serde_json::json!({
        "id": summary.id,
        "kind": summary.kind,
        "stage_id": summary.stage_id,
        "title": summary.title,
        "body_markdown": summary.body_markdown,
        "quality": summary.quality,
        "model": summary.model,
        "start": summary.start.to_rfc3339(),
        "end": summary.end.to_rfc3339(),
    })
}

pub fn publish_summary(
    state: &ServerState,
    summary: &mc_storage::projectors::summaries::StoredSummary,
) {
    state.publish(
        crate::events::EVENT_SUMMARY_CREATED,
        summary_created_payload(summary),
    );
}

/// 有效时区：配置没写就退回 UTC（配置层已经就此给过 warning）。
fn effective_timezone(state: &ServerState) -> String {
    state
        .config
        .current()
        .config
        .general
        .timezone
        .clone()
        .unwrap_or_else(|| "UTC".to_string())
}

/// 从配置取阶段策略（时区跟随用户设置）。
pub fn policy_for(state: &ServerState) -> StagePolicy {
    let config = state.config.current();
    config.config.stage.stage_policy(&effective_timezone(state))
}

/// 一次完整的阶段推进：重建阶段 + 巡检补总结。
///
/// **前置条件**：活动已经投影过（阶段读的是派生表里的活动）。
/// daemon 的顺序是「投影活动 → 推断 → 阶段」，推断放在中间是为了
/// 让阶段与总结用上推断更新过的标题。
///
/// 合成一个入口是为了让调用方不可能只做一半 ——
/// 「重建了阶段却没跑巡检」出现一次，就意味着一个阶段永远没有总结。
pub async fn tick(
    state: &ServerState,
    source: &dyn mc_summary::source::SummarySource,
    at: Timestamp,
) -> Result<StageTickReport, AppError> {
    let policy = policy_for(state);
    let rows = rebuild(state, &policy, at)?;
    let patrol =
        mc_summary::patrol::patrol_once(&state.db, source, &patrol_policy(&policy), at).await?;

    Ok(StageTickReport {
        stages: rows.len(),
        repaired: patrol.repaired.len(),
        skipped_too_short: patrol.skipped_too_short.len(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageTickReport {
    pub stages: usize,
    pub repaired: usize,
    pub skipped_too_short: usize,
}

/// 本地日翻页时补上「昨天」的日报（4.42）。
///
/// 刻意放在这里而不是 daemon 自己判断：日边界、时区、日志与广播
/// 都在这条链路上，散出去只会多一处不一致。
///
/// 返回生成的日报 id（没有内容时是 `None`）。
pub async fn roll_over_day(
    state: &ServerState,
    source: &dyn mc_summary::source::SummarySource,
    now: Timestamp,
    previous_day: chrono::NaiveDate,
) -> Result<Option<String>, AppError> {
    let config = crate::stages::policy_for(state);
    let daily = mc_summary::daily::DailyConfig {
        timezone: config.timezone.clone(),
        locale: mc_summary::model::SummaryLocale::from_config(
            &state.config.current().config.general.locale,
        ),
    };

    let Some(stored) =
        mc_summary::daily::generate_daily(&state.db, source, &daily, previous_day, now).await?
    else {
        // 空白天：不产出日报，也**不写空文档** —— 占位文案会被当成
        // 真实日报展示，用户看到的就是一条假总结。
        if let Some(week) = weekly_rollover(previous_day) {
            archive_week(state, source, now, week).await?;
        }
        return Ok(None);
    };

    publish_summary(state, &stored);
    // 归档进笔记树：笔记树直接读 vaults 表，不归档就等于日报没生成
    if let Err(error) = mc_memory::vault_writer::archive_daily_report(&state.db, &stored, now) {
        // 归档失败不该让日报本身消失：日报已经在库里，下次翻页会重试
        crate::activities::record_failure(state, &error, now)?;
    }

    // 周日翻页时，把上一周收成周报
    if let Some(week) = weekly_rollover(previous_day) {
        archive_week(state, source, now, week).await?;
    }

    Ok(Some(stored.id))
}

/// 如果刚翻过去的那天是周日，返回它所在周的周一（该生成周报了）。
fn weekly_rollover(previous_day: chrono::NaiveDate) -> Option<chrono::NaiveDate> {
    use chrono::Datelike;
    (previous_day.weekday() == chrono::Weekday::Sun)
        .then(|| mc_memory::weekly::monday_of(previous_day))
}

async fn archive_week(
    state: &ServerState,
    source: &dyn mc_summary::source::SummarySource,
    now: Timestamp,
    monday: chrono::NaiveDate,
) -> Result<(), AppError> {
    let config = policy_for(state);
    let weekly = mc_memory::weekly::WeeklyConfig {
        timezone: config.timezone.clone(),
        locale: mc_summary::model::SummaryLocale::from_config(
            &state.config.current().config.general.locale,
        ),
    };

    let Some(stored) =
        mc_memory::weekly::generate_weekly(&state.db, source, &weekly, monday, now).await?
    else {
        return Ok(());
    };

    publish_summary(state, &stored);
    if let Err(error) = mc_memory::vault_writer::archive_weekly_report(&state.db, &stored, now) {
        crate::activities::record_failure(state, &error, now)?;
    }
    Ok(())
}

/// 巡检策略：与阶段策略保持一致，避免「关闭后多久必须总结」两处各写一个数。
pub fn patrol_policy(policy: &StagePolicy) -> PatrolPolicy {
    PatrolPolicy {
        deadline_secs: policy.summary_deadline_secs,
        min_stage_duration_secs: policy.min_duration_secs,
        timezone: policy.timezone.clone(),
    }
}
