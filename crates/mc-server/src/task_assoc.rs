//! Lite 任务关联的服务端接线：读活动信号 → 推断 → 落事件 → 投影。
//!
//! Observation 只读；纠正只追加事件。

use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_domain::task_assoc::{
    default_dev_rules, extract_explicit_id, infer_associations, last_working_on,
    project_associations, AssocEvent, AssocSource, TaskProjection, WorkSignal,
    KIND_TASK_ASSOCIATED, KIND_TASK_CLEARED, KIND_TASK_CORRECTED,
};
use mc_storage::{Database, NewEvent};
use serde_json::json;

/// 从当前活动派生表收集信号。
///
/// 只读活动标题（投影产物）；**不改写 Observation**。窗口标题 / 路径线索
/// 常已折叠进活动标题（`fallback_title`），lite 关联够用。
pub fn collect_signals(db: &Database) -> Result<Vec<WorkSignal>, AppError> {
    let activities = mc_storage::projectors::activities::read_all(db)?;
    let mut signals = Vec::with_capacity(activities.len());
    for activity in activities {
        let path_hint = activity.title.split_whitespace().find_map(|token| {
            if token.starts_with('/') || token.contains(":\\") || token.contains('/') {
                Some(token.to_string())
            } else {
                None
            }
        });
        let explicit = extract_explicit_id(&activity.title);
        signals.push(WorkSignal {
            activity_id: activity.id,
            title: activity.title.clone(),
            window_title: Some(activity.title),
            path_hint,
            explicit_id: explicit,
            at: activity.end,
        });
    }
    Ok(signals)
}

/// 读取已落库的关联事件并投影。
pub fn load_events(db: &Database) -> Result<Vec<AssocEvent>, AppError> {
    let mut out = Vec::new();
    let mut from_seq = 0i64;
    loop {
        let batch = db.read_events(from_seq, 500)?;
        if batch.is_empty() {
            break;
        }
        for envelope in batch {
            from_seq = envelope.seq;
            let parsed = match envelope.kind.as_str() {
                KIND_TASK_ASSOCIATED => {
                    serde_json::from_value::<AssociatedPayload>(envelope.payload)
                        .ok()
                        .map(|p| AssocEvent::Associated {
                            activity_id: p.activity_id,
                            task_id: p.task_id,
                            label: p.label,
                            source: p.source,
                            at: envelope.at,
                        })
                }
                KIND_TASK_CORRECTED => serde_json::from_value::<CorrectedPayload>(envelope.payload)
                    .ok()
                    .map(|p| AssocEvent::Corrected {
                        activity_id: p.activity_id,
                        from_task_id: p.from_task_id,
                        to_task_id: p.to_task_id,
                        label: p.label,
                        at: envelope.at,
                    }),
                KIND_TASK_CLEARED => serde_json::from_value::<ClearedPayload>(envelope.payload)
                    .ok()
                    .map(|p| AssocEvent::Cleared {
                        activity_id: p.activity_id,
                        task_id: p.task_id,
                        at: envelope.at,
                    }),
                _ => None,
            };
            if let Some(event) = parsed {
                out.push(event);
            }
        }
    }
    Ok(out)
}

#[derive(serde::Deserialize)]
struct AssociatedPayload {
    activity_id: String,
    task_id: String,
    label: String,
    source: AssocSource,
}

#[derive(serde::Deserialize)]
struct CorrectedPayload {
    activity_id: String,
    from_task_id: Option<String>,
    to_task_id: String,
    label: String,
}

#[derive(serde::Deserialize)]
struct ClearedPayload {
    activity_id: String,
    task_id: String,
}

fn to_new_event(event: &AssocEvent) -> NewEvent {
    let payload = match event {
        AssocEvent::Associated {
            activity_id,
            task_id,
            label,
            source,
            ..
        } => json!({
            "activity_id": activity_id,
            "task_id": task_id,
            "label": label,
            "source": source,
        }),
        AssocEvent::Corrected {
            activity_id,
            from_task_id,
            to_task_id,
            label,
            ..
        } => json!({
            "activity_id": activity_id,
            "from_task_id": from_task_id,
            "to_task_id": to_task_id,
            "label": label,
        }),
        AssocEvent::Cleared {
            activity_id,
            task_id,
            ..
        } => json!({
            "activity_id": activity_id,
            "task_id": task_id,
        }),
    };
    NewEvent::new(event.event_kind(), event.at(), payload).by("system")
}

/// 对尚未关联的活动跑一遍默认规则，追加事件（幂等：已有绑定的活动跳过）。
pub fn sync_inferred(db: &Database, now: Timestamp) -> Result<usize, AppError> {
    let existing = load_events(db)?;
    let projected = project_associations(&existing);
    let bound: std::collections::HashSet<&str> = projected
        .iter()
        .flat_map(|t| t.activity_ids.iter().map(String::as_str))
        .collect();

    let signals = collect_signals(db)?;
    let mut to_infer: Vec<WorkSignal> = signals
        .into_iter()
        .filter(|s| !bound.contains(s.activity_id.as_str()))
        .collect();
    // 用调用时刻作为事件时间，便于「最近在做什么」排序稳定
    for signal in &mut to_infer {
        if signal.at.as_millis() == 0 {
            signal.at = now;
        }
    }
    let inferred = infer_associations(&to_infer, &default_dev_rules());
    if inferred.is_empty() {
        return Ok(0);
    }
    let events: Vec<NewEvent> = inferred.iter().map(to_new_event).collect();
    db.append_events(&events)?;
    Ok(events.len())
}

pub fn current_projections(db: &Database) -> Result<Vec<TaskProjection>, AppError> {
    Ok(project_associations(&load_events(db)?))
}

pub fn last_task(db: &Database) -> Result<Option<TaskProjection>, AppError> {
    let projections = current_projections(db)?;
    Ok(last_working_on(&projections).cloned())
}

/// 用户纠正：追加 Corrected 事件，不碰 Observation。
pub fn correct_association(
    db: &Database,
    activity_id: &str,
    to_task_id: &str,
    label: &str,
    at: Timestamp,
) -> Result<(), AppError> {
    let existing = load_events(db)?;
    let projected = project_associations(&existing);
    let from = projected
        .iter()
        .find(|t| t.activity_ids.iter().any(|id| id == activity_id))
        .map(|t| t.task_id.clone());
    let event = AssocEvent::Corrected {
        activity_id: activity_id.to_string(),
        from_task_id: from,
        to_task_id: to_task_id.to_string(),
        label: label.to_string(),
        at,
    };
    db.append_events(&[to_new_event(&event).by("user")])?;
    Ok(())
}
