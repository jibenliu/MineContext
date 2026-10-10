//! Lite 任务关联：规则 / 路径 / 窗口标题 / 显式 ID。
//!
//! **不改写 Observation**。关联以事件形式追加，投影可重放；用户纠正也是事件。
//! 目标只回答「我上次在做什么」，不是完整 Task OS。

use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

/// 关联事件 kind（落 `events.kind`）。
pub const KIND_TASK_ASSOCIATED: &str = "task.associated";
pub const KIND_TASK_CORRECTED: &str = "task.corrected";
pub const KIND_TASK_CLEARED: &str = "task.cleared";

/// 关联来源（可解释、可纠正）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssocSource {
    Rule,
    Path,
    WindowTitle,
    ExplicitId,
    Correction,
}

/// 一条活动上可用于打分的信号（来自派生活动 / 观测摘要，只读）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkSignal {
    pub activity_id: String,
    pub title: String,
    pub window_title: Option<String>,
    pub path_hint: Option<String>,
    pub explicit_id: Option<String>,
    pub at: Timestamp,
}

/// 用户或内置的匹配规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssocRule {
    pub task_id: String,
    pub label: String,
    /// 子串匹配（小写比较）：命中 title / window_title / path_hint 之一即可。
    pub match_any: Vec<String>,
    pub source: AssocSource,
}

/// 可持久化、可重放的关联事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssocEvent {
    Associated {
        activity_id: String,
        task_id: String,
        label: String,
        source: AssocSource,
        at: Timestamp,
    },
    Corrected {
        activity_id: String,
        from_task_id: Option<String>,
        to_task_id: String,
        label: String,
        at: Timestamp,
    },
    Cleared {
        activity_id: String,
        task_id: String,
        at: Timestamp,
    },
}

impl AssocEvent {
    pub fn at(&self) -> Timestamp {
        match self {
            Self::Associated { at, .. } | Self::Corrected { at, .. } | Self::Cleared { at, .. } => {
                *at
            }
        }
    }

    pub fn activity_id(&self) -> &str {
        match self {
            Self::Associated { activity_id, .. }
            | Self::Corrected { activity_id, .. }
            | Self::Cleared { activity_id, .. } => activity_id,
        }
    }

    pub fn event_kind(&self) -> &'static str {
        match self {
            Self::Associated { .. } => KIND_TASK_ASSOCIATED,
            Self::Corrected { .. } => KIND_TASK_CORRECTED,
            Self::Cleared { .. } => KIND_TASK_CLEARED,
        }
    }
}

/// 投影后的任务视图。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskProjection {
    pub task_id: String,
    pub label: String,
    pub activity_ids: Vec<String>,
    pub last_at: Timestamp,
    pub source: AssocSource,
}

/// 按规则从信号推断关联事件（纯函数；不写库）。
///
/// 同一活动多规则命中时取**第一条**（调用方应把更具体的规则放前面）。
pub fn infer_associations(signals: &[WorkSignal], rules: &[AssocRule]) -> Vec<AssocEvent> {
    let mut out = Vec::new();
    for signal in signals {
        if let Some(event) = infer_one(signal, rules) {
            out.push(event);
        }
    }
    out
}

fn infer_one(signal: &WorkSignal, rules: &[AssocRule]) -> Option<AssocEvent> {
    if let Some(id) = signal
        .explicit_id
        .as_deref()
        .filter(|s| !s.trim().is_empty())
    {
        return Some(AssocEvent::Associated {
            activity_id: signal.activity_id.clone(),
            task_id: normalize_task_id(id),
            label: id.trim().to_string(),
            source: AssocSource::ExplicitId,
            at: signal.at,
        });
    }

    let haystacks = [
        signal.title.as_str(),
        signal.window_title.as_deref().unwrap_or(""),
        signal.path_hint.as_deref().unwrap_or(""),
    ];
    for rule in rules {
        if rule.match_any.is_empty() {
            continue;
        }
        let matched = rule.match_any.iter().any(|needle| {
            let needle = needle.to_lowercase();
            haystacks
                .iter()
                .any(|h| !h.is_empty() && h.to_lowercase().contains(&needle))
        });
        if matched {
            return Some(AssocEvent::Associated {
                activity_id: signal.activity_id.clone(),
                task_id: rule.task_id.clone(),
                label: rule.label.clone(),
                source: rule.source,
                at: signal.at,
            });
        }
    }
    None
}

/// 从路径 / 窗口标题里抽出常见工单 ID（如 `APEX-389`、`#42`）。
pub fn extract_explicit_id(text: &str) -> Option<String> {
    // 简单扫描：LETTER+-?DIGITS 或 #DIGITS
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let start = i;
            i += 1;
            let digit_start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i > digit_start {
                return Some(text[start..i].to_string());
            }
            continue;
        }
        if bytes[i].is_ascii_alphabetic() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'-' {
                let after_dash = i + 1;
                let mut j = after_dash;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > after_dash {
                    return Some(text[start..j].to_string());
                }
            }
            continue;
        }
        i += 1;
    }
    None
}

fn normalize_task_id(raw: &str) -> String {
    raw.trim().trim_start_matches('#').to_lowercase()
}

/// 重放关联事件 → 任务投影（纯函数）。
///
/// 后发生的 Corrected / Cleared 覆盖先前 Associated。
pub fn project_associations(events: &[AssocEvent]) -> Vec<TaskProjection> {
    let mut ordered: Vec<&AssocEvent> = events.iter().collect();
    ordered.sort_by_key(|e| e.at().as_millis());

    // activity_id → current binding
    let mut by_activity: std::collections::BTreeMap<
        String,
        (String, String, AssocSource, Timestamp),
    > = std::collections::BTreeMap::new();

    for event in ordered {
        match event {
            AssocEvent::Associated {
                activity_id,
                task_id,
                label,
                source,
                at,
            } => {
                by_activity.insert(
                    activity_id.clone(),
                    (task_id.clone(), label.clone(), *source, *at),
                );
            }
            AssocEvent::Corrected {
                activity_id,
                to_task_id,
                label,
                at,
                ..
            } => {
                by_activity.insert(
                    activity_id.clone(),
                    (
                        to_task_id.clone(),
                        label.clone(),
                        AssocSource::Correction,
                        *at,
                    ),
                );
            }
            AssocEvent::Cleared { activity_id, .. } => {
                by_activity.remove(activity_id);
            }
        }
    }

    let mut tasks: std::collections::BTreeMap<String, TaskProjection> =
        std::collections::BTreeMap::new();
    for (activity_id, (task_id, label, source, at)) in by_activity {
        let entry = tasks
            .entry(task_id.clone())
            .or_insert_with(|| TaskProjection {
                task_id: task_id.clone(),
                label: label.clone(),
                activity_ids: Vec::new(),
                last_at: at,
                source,
            });
        entry.activity_ids.push(activity_id);
        if at.as_millis() >= entry.last_at.as_millis() {
            entry.last_at = at;
            entry.label = label;
            entry.source = source;
        }
    }

    let mut out: Vec<TaskProjection> = tasks.into_values().collect();
    out.sort_by(|a, b| {
        b.last_at
            .as_millis()
            .cmp(&a.last_at.as_millis())
            .then_with(|| a.task_id.cmp(&b.task_id))
    });
    out
}

/// 「我上次在做什么」—— 取最近有关联的任务。
pub fn last_working_on(projections: &[TaskProjection]) -> Option<&TaskProjection> {
    projections.first()
}

/// 内置开发者规则：常见 IDE / 仓库路径片段。
pub fn default_dev_rules() -> Vec<AssocRule> {
    vec![
        AssocRule {
            task_id: "minecontext".into(),
            label: "MineContext".into(),
            match_any: vec!["minecontext".into(), "mc-server".into(), "mc-daemon".into()],
            source: AssocSource::Path,
        },
        AssocRule {
            task_id: "vscode-edit".into(),
            label: "Editor work".into(),
            match_any: vec![
                "visual studio code".into(),
                " — code".into(),
                ".rs —".into(),
                ".tsx —".into(),
            ],
            source: AssocSource::WindowTitle,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(
        id: &str,
        title: &str,
        window: Option<&str>,
        path: Option<&str>,
        at_ms: i64,
    ) -> WorkSignal {
        WorkSignal {
            activity_id: id.into(),
            title: title.into(),
            window_title: window.map(str::to_string),
            path_hint: path.map(str::to_string),
            explicit_id: extract_explicit_id(title)
                .or_else(|| window.and_then(extract_explicit_id)),
            at: Timestamp::from_millis(at_ms),
        }
    }

    #[test]
    fn associates_by_path_and_window_title_and_answers_last_working_on() {
        let rules = default_dev_rules();
        let signals = vec![
            signal(
                "a1",
                "browsing docs",
                Some("README — MineContext"),
                Some("/workspace/MineContext/README.md"),
                1_000,
            ),
            signal("a2", "standup", Some("Slack"), None, 2_000),
            signal(
                "a3",
                "Fix APEX-389 acceptance",
                Some("main.rs — MineContext"),
                Some("/workspace/MineContext/crates/mc-server/src/main.rs"),
                3_000,
            ),
        ];
        let inferred = infer_associations(&signals, &rules);
        assert!(inferred.iter().any(|e| matches!(
            e,
            AssocEvent::Associated { task_id, .. } if task_id == "apex-389" || task_id == "minecontext"
        )));
        // explicit id 优先
        let a3 = inferred
            .iter()
            .find(|e| e.activity_id() == "a3")
            .expect("a3 associated");
        assert!(matches!(
            a3,
            AssocEvent::Associated {
                task_id,
                source: AssocSource::ExplicitId,
                ..
            } if task_id == "apex-389"
        ));

        let projected = project_associations(&inferred);
        let last = last_working_on(&projected).expect("last task");
        assert_eq!(last.task_id, "apex-389");
        assert!(last.activity_ids.contains(&"a3".to_string()));
    }

    #[test]
    fn correction_is_replay_safe_and_does_not_need_observation_rewrite() {
        let events = vec![
            AssocEvent::Associated {
                activity_id: "a1".into(),
                task_id: "wrong".into(),
                label: "Wrong".into(),
                source: AssocSource::Rule,
                at: Timestamp::from_millis(1),
            },
            AssocEvent::Corrected {
                activity_id: "a1".into(),
                from_task_id: Some("wrong".into()),
                to_task_id: "right".into(),
                label: "Right task".into(),
                at: Timestamp::from_millis(2),
            },
        ];
        let once = project_associations(&events);
        let twice = project_associations(&events);
        assert_eq!(once, twice);
        assert_eq!(once.len(), 1);
        assert_eq!(once[0].task_id, "right");
        assert_eq!(once[0].source, AssocSource::Correction);
        // Cleared removes binding
        let mut cleared = events;
        cleared.push(AssocEvent::Cleared {
            activity_id: "a1".into(),
            task_id: "right".into(),
            at: Timestamp::from_millis(3),
        });
        assert!(project_associations(&cleared).is_empty());
    }
}
