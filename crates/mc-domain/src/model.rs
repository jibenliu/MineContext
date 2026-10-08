//! 活动模型与用户修正。
//!
//! 两部分职责：
//! 1. **强制 provenance 不变量**：
//!    「AI 认为发生了什么」与「用户实际上做了什么」必须可区分。
//! 2. **用户修正作为独立的覆盖层**：
//!    修正不写进活动本身，因此**重算不会把它抹掉** ——
//!    否则算法一升级，用户手工整理的历史就全废了。

use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};

use crate::activity::Provenance;

/// 活动引用的观测（带时间，供切分判定使用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationRef {
    pub id: String,
    pub at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    pub id: String,
    pub start: Timestamp,
    pub end: Timestamp,
    pub title: String,
    pub category: Option<String>,
    pub observations: Vec<ObservationRef>,
    pub origin: Provenance,
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityError {
    /// 没有任何证据 —— 等于凭空断言
    MissingEvidence,
    InferredWithoutModel,
    InferredWithoutConfidence,
    RuleWithoutRuleId,
    EndBeforeStart,
    /// 观测落在 `[start, end]` 之外 —— 证据与窗口对不上
    EvidenceOutsideRange,
}

impl std::fmt::Display for ActivityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::MissingEvidence => "活动没有任何观测作为证据",
            Self::InferredWithoutModel => "推断出的活动必须记录是哪个模型推断的",
            Self::InferredWithoutConfidence => "推断出的活动必须带置信度",
            Self::RuleWithoutRuleId => "规则产生的活动必须记录规则 id",
            Self::EndBeforeStart => "活动结束时间早于开始时间",
            Self::EvidenceOutsideRange => "观测落在活动时间范围之外",
        };
        f.write_str(text)
    }
}

impl std::error::Error for ActivityError {}

impl Activity {
    /// 构造期不变量。**必须调用** —— 这些约束靠约定是守不住的。
    pub fn validate(&self) -> Result<(), ActivityError> {
        if self.observations.is_empty() {
            return Err(ActivityError::MissingEvidence);
        }
        if self.end < self.start {
            return Err(ActivityError::EndBeforeStart);
        }
        // 证据必须落在窗口里，否则界面上这条观测不属于任何活动
        if self
            .observations
            .iter()
            .any(|observation| observation.at < self.start || observation.at > self.end)
        {
            return Err(ActivityError::EvidenceOutsideRange);
        }

        match &self.origin {
            Provenance::Observed => Ok(()),
            Provenance::Rule { rule_id } => {
                if rule_id.trim().is_empty() {
                    Err(ActivityError::RuleWithoutRuleId)
                } else {
                    Ok(())
                }
            }
            Provenance::Inferred { model } => {
                if model.trim().is_empty() {
                    return Err(ActivityError::InferredWithoutModel);
                }
                if !(self.confidence > 0.0 && self.confidence <= 1.0) {
                    return Err(ActivityError::InferredWithoutConfidence);
                }
                Ok(())
            }
        }
    }

    pub fn is_inferred(&self) -> bool {
        self.origin.is_inferred()
    }

    pub fn observation_ids(&self) -> Vec<String> {
        self.observations.iter().map(|o| o.id.clone()).collect()
    }
}

/// 用户修正。作为**独立事件**持久化，并参与重放。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OverrideKind {
    Rename {
        activity_id: String,
        title: String,
        at: Timestamp,
    },
    SetCategory {
        activity_id: String,
        category: Option<String>,
        at: Timestamp,
    },
    Merge {
        primary: String,
        absorbed: Vec<String>,
        at: Timestamp,
    },
    Split {
        activity_id: String,
        at: Timestamp,
        tail_title: String,
    },
}

impl OverrideKind {
    pub const fn at(&self) -> Timestamp {
        match self {
            Self::Rename { at, .. }
            | Self::SetCategory { at, .. }
            | Self::Merge { at, .. }
            | Self::Split { at, .. } => *at,
        }
    }
}

/// 应用覆盖之后对外呈现的活动。
///
/// `original_title` 保留算法给出的原标题，`is_user_modified` 让 UI 能区分
/// 「这是模型说的」和「这是用户改的」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityView {
    pub id: String,
    pub start: Timestamp,
    pub end: Timestamp,
    pub title: String,
    pub original_title: String,
    pub category: Option<String>,
    pub observations: Vec<ObservationRef>,
    pub origin: Provenance,
    pub confidence: f32,
    pub is_user_modified: bool,
}

impl ActivityView {
    pub fn observation_ids(&self) -> Vec<String> {
        self.observations.iter().map(|o| o.id.clone()).collect()
    }
}

/// 把用户修正叠加到算法产出的活动上。
///
/// **纯函数**：不改动输入，同样输入给同样输出，因此可以在重放后重复调用。
pub fn apply_overrides(activities: &[Activity], overrides: &[OverrideKind]) -> Vec<ActivityView> {
    let mut views: Vec<ActivityView> = activities
        .iter()
        .map(|activity| ActivityView {
            id: activity.id.clone(),
            start: activity.start,
            end: activity.end,
            title: activity.title.clone(),
            original_title: activity.title.clone(),
            category: activity.category.clone(),
            observations: activity.observations.clone(),
            origin: activity.origin.clone(),
            confidence: activity.confidence,
            is_user_modified: false,
        })
        .collect();

    // 按发生顺序应用；后发生的覆盖先前的
    let mut ordered: Vec<&OverrideKind> = overrides.iter().collect();
    ordered.sort_by_key(|item| item.at().as_millis());

    for item in ordered {
        match item {
            OverrideKind::Rename {
                activity_id, title, ..
            } => {
                if let Some(view) = views.iter_mut().find(|v| &v.id == activity_id) {
                    view.title = title.clone();
                    view.is_user_modified = true;
                }
            }
            OverrideKind::SetCategory {
                activity_id,
                category,
                ..
            } => {
                if let Some(view) = views.iter_mut().find(|v| &v.id == activity_id) {
                    view.category = category.clone();
                    view.is_user_modified = true;
                }
            }
            OverrideKind::Merge {
                primary, absorbed, ..
            } => {
                // 主活动不存在时整条修正忽略，避免吸收方被误删
                if !views.iter().any(|v| &v.id == primary) {
                    continue;
                }

                for id in absorbed {
                    if id == primary {
                        continue;
                    }
                    let Some(index) = views.iter().position(|v| &v.id == id) else {
                        continue;
                    };

                    let taken = views.remove(index);
                    // 每次移除都会让下标失效，因此按 id 重新定位主活动
                    let Some(target) = views.iter().position(|v| &v.id == primary) else {
                        // 主活动不在列表里，先前的判定会拦住；放回去避免丢数据
                        views.insert(index, taken);
                        continue;
                    };

                    let view = &mut views[target];
                    view.observations.extend(taken.observations);
                    view.observations.sort_by_key(|o| o.at.as_millis());
                    view.start = view.start.min(taken.start);
                    view.end = view.end.max(taken.end);
                    view.is_user_modified = true;
                }
            }
            OverrideKind::Split {
                activity_id,
                at,
                tail_title,
            } => {
                let Some(index) = views.iter().position(|v| &v.id == activity_id) else {
                    continue;
                };

                let original = views[index].clone();
                let mut head_observations = Vec::new();
                let mut tail_observations = Vec::new();
                for observation in &original.observations {
                    if observation.at < *at {
                        head_observations.push(observation.clone());
                    } else {
                        tail_observations.push(observation.clone());
                    }
                }

                // 切分点落在观测之间才有意义；切不出两段就忽略这次修正
                if head_observations.is_empty() || tail_observations.is_empty() {
                    continue;
                }

                let head_end = head_observations
                    .last()
                    .map(|o| o.at)
                    .unwrap_or(original.end);
                let tail_start = tail_observations.first().map(|o| o.at).unwrap_or(*at);

                let mut head = original.clone();
                head.end = head_end;
                head.observations = head_observations;
                head.is_user_modified = true;

                let tail = ActivityView {
                    id: format!("{}-split-{}", original.id, at.as_millis()),
                    start: tail_start,
                    end: original.end,
                    title: tail_title.clone(),
                    original_title: original.original_title.clone(),
                    category: original.category.clone(),
                    observations: tail_observations,
                    origin: original.origin.clone(),
                    confidence: original.confidence,
                    is_user_modified: true,
                };

                views[index] = head;
                views.insert(index + 1, tail);
            }
        }
    }

    views
}
