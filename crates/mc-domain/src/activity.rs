//! 活动聚合器：把逐条观测变成「一段时间里在做什么」。
//!
//! 经典用例：`VSCode → Chrome(3s) → VSCode` 应当聚成**一个**活动。
//! 没有这层，时间线就是「10:01 Chrome / 10:02 VSCode / 10:03 Terminal」
//! 这样的碎片噪声列表。
//!
//! 纯状态机：时间由调用方注入，不读时钟、不做 IO，因此可确定性测试。

use mc_common::time::Timestamp;

use crate::rules::AiPreference;

pub use crate::model::{
    apply_overrides, Activity, ActivityError, ActivityView, ObservationRef, OverrideKind,
};

/// 判断来源。规则判定与模型推测必须可区分，否则用户无法判断可信度。
///
/// 用 internally-tagged 序列化成 `{"kind": "rule", "rule_id": "..."}`，
/// 这样前端可以直接据此渲染「推测」标记，不需要自己拼字符串。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Provenance {
    /// 直接来自廉价元数据（窗口标题/进程名）
    Observed,
    /// 用户规则命中，确定性
    Rule { rule_id: String },
    /// 模型推断
    Inferred { model: String },
}

impl Provenance {
    pub fn is_inferred(&self) -> bool {
        matches!(self, Self::Inferred { .. })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActivityCandidate {
    pub title: String,
    pub category: Option<String>,
    pub origin: Provenance,
    pub confidence: f32,
    pub ai: AiPreference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AggregationPolicy {
    /// 短暂切走再回来不产生新活动
    pub debounce_secs: u64,
    /// 短于此长度的候选活动视为噪声
    pub min_duration_secs: u64,
    /// 结束之后多久内回到同一标题就合并回原活动
    pub merge_gap_secs: u64,
    /// 单个活动的最长时长，超过则强制切分
    pub max_duration_secs: u64,
    /// 多久没有观测就认为活动结束
    pub idle_gap_secs: u64,
}

impl Default for AggregationPolicy {
    fn default() -> Self {
        Self {
            // 与 mc-config 的 activity 默认值保持一致
            debounce_secs: 45,
            min_duration_secs: 60,
            merge_gap_secs: 120,
            max_duration_secs: 7200,
            idle_gap_secs: 300,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Switched,
    Idle,
    MaxDuration,
    Locked,
    Flushed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AggregatorEvent {
    Started {
        id: String,
        title: String,
        category: Option<String>,
        at: Timestamp,
        origin: Provenance,
        confidence: f32,
    },
    Extended {
        id: String,
        observation_id: String,
        at: Timestamp,
    },
    Ended {
        id: String,
        at: Timestamp,
        reason: EndReason,
    },
    /// 过短且未持续的活动被丢弃，但**必须记录**，否则观测就成了孤儿。
    NoiseDropped {
        observation_ids: Vec<String>,
        title: String,
        reason: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct OpenActivity {
    pub id: String,
    pub title: String,
    pub category: Option<String>,
    pub origin: Provenance,
    pub confidence: f32,
    pub start: Timestamp,
    /// 最近一次观测的时间（**不会**被 tick 或迟到观测改动）
    pub end: Timestamp,
    pub observations: Vec<String>,
}

#[derive(Debug, Clone)]
struct Pending {
    candidate: ActivityCandidate,
    first_seen: Timestamp,
    last_seen: Timestamp,
    observations: Vec<String>,
}

#[derive(Debug, Clone)]
struct ClosedActivity {
    id: String,
    title: String,
    observations: Vec<String>,
    ended_at: Timestamp,
}

/// 已关闭活动的保留上限。真正持久化由投影器负责，这里只用于
/// 「观测归属核对」与合并判断，所以有界即可。
const CLOSED_HISTORY: usize = 256;

pub struct Aggregator {
    policy: AggregationPolicy,
    open: Option<OpenActivity>,
    pending: Option<Pending>,
    closed: Vec<ClosedActivity>,
    dropped: Vec<String>,
    next_id: u64,
}

impl Aggregator {
    pub fn new(policy: AggregationPolicy) -> Self {
        Self {
            policy,
            open: None,
            pending: None,
            closed: Vec::new(),
            dropped: Vec::new(),
            next_id: 1,
        }
    }

    pub fn policy(&self) -> &AggregationPolicy {
        &self.policy
    }

    pub fn open(&self) -> Option<&OpenActivity> {
        self.open.as_ref()
    }

    /// 所有归属过活动的观测（含已关闭的）。
    pub fn all_observations(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .closed
            .iter()
            .flat_map(|activity| activity.observations.clone())
            .collect();
        if let Some(open) = &self.open {
            out.extend(open.observations.clone());
        }
        out
    }

    pub fn dropped_observations(&self) -> Vec<String> {
        self.dropped.clone()
    }

    pub fn observe(
        &mut self,
        observation_id: &str,
        candidate: &ActivityCandidate,
        at: Timestamp,
    ) -> Vec<AggregatorEvent> {
        let mut events = Vec::new();

        // 先把到期的切换提交掉，保证「被替换的 pending 一定短于防抖阈值」这个前提成立
        events.extend(self.commit_if_due(at));

        // 当前没有任何活动 → 立即开启。
        // 若这里也走防抖，时间线会凭空滞后一个防抖窗口，用户会觉得「刚打开就没了」。
        if self.open.is_none() && self.pending.is_none() {
            let started = self.start_activity(candidate, at);
            if let Some(open) = &mut self.open {
                open.observations.push(observation_id.to_string());
            }
            return started;
        }

        // 与当前活动同标题 → 延续，并取消任何待定切换
        if let Some(open) = &mut self.open {
            if open.title == candidate.title {
                if let Some(pending) = self.pending.take() {
                    // 中断期间的观测属于原活动，不能丢
                    open.observations.extend(pending.observations);
                }
                // 迟到观测不把结束时间往回拨
                if at >= open.end {
                    open.end = at;
                }
                open.observations.push(observation_id.to_string());
                events.push(AggregatorEvent::Extended {
                    id: open.id.clone(),
                    observation_id: observation_id.to_string(),
                    at,
                });
                return events;
            }
        }

        // 与待定切换同标题 → 延长待定
        if let Some(pending) = &mut self.pending {
            if pending.candidate.title == candidate.title {
                pending.last_seen = at;
                pending.observations.push(observation_id.to_string());
                events.extend(self.commit_if_due(at));
                return events;
            }
        }

        // 换成第三种标题：被替换掉的待定必然短于防抖阈值（见上文），因此是噪声
        if let Some(replaced) = self.pending.take() {
            events.push(AggregatorEvent::NoiseDropped {
                observation_ids: replaced.observations.clone(),
                title: replaced.candidate.title.clone(),
                reason: "too_short",
            });
            self.dropped.extend(replaced.observations);

            // 切换到新标题的起点沿用第一次离开原活动的时刻
            self.pending = Some(Pending {
                candidate: candidate.clone(),
                first_seen: replaced.first_seen,
                last_seen: at,
                observations: vec![observation_id.to_string()],
            });
        } else {
            self.pending = Some(Pending {
                candidate: candidate.clone(),
                first_seen: at,
                last_seen: at,
                observations: vec![observation_id.to_string()],
            });
        }

        events.extend(self.commit_if_due(at));
        events
    }

    /// 定时驱动：提交到期切换、处理空闲与超长。
    pub fn tick(&mut self, now: Timestamp) -> Vec<AggregatorEvent> {
        let mut events = self.commit_if_due(now);

        if let Some(pending) = &self.pending {
            if self.elapsed_secs(pending.first_seen, now) >= self.policy.debounce_secs {
                events.extend(self.commit_switch());
            }
        }

        // 超长与空闲按**时刻先后**结算，且结束时刻取边界而不是「现在」：
        // 否则「干到 11:02 就离开、14 小时后才又有人 tick」会被记成一个
        // 持续到 01:00 的活动 —— 凭空多出 14 小时，用户一眼能看出是错的。
        const MAX_SPLITS_PER_TICK: usize = 64;
        for _ in 0..MAX_SPLITS_PER_TICK {
            let Some(open) = self.open.as_ref() else {
                break;
            };
            let max_due = Timestamp::from_millis(
                open.start.as_millis() + self.policy.max_duration_secs as i64 * 1000,
            );
            let idle_due = Timestamp::from_millis(
                open.end.as_millis() + self.policy.idle_gap_secs as i64 * 1000,
            );

            // 超长切分：只有「更早到期」且「切分点之后还有证据」才切
            if max_due <= now && max_due <= idle_due && open.end >= max_due {
                let (title, category, origin, confidence) = (
                    open.title.clone(),
                    open.category.clone(),
                    open.origin.clone(),
                    open.confidence,
                );
                events.extend(self.close_open(max_due, EndReason::MaxDuration));
                events.extend(self.start_activity(
                    &ActivityCandidate {
                        title,
                        category,
                        origin,
                        confidence,
                        ai: AiPreference::None,
                    },
                    max_due,
                ));
                continue;
            }

            // 空闲：结束时刻是**最后一条观测**，不是「现在」
            if idle_due <= now {
                events.extend(self.close_open(open.end, EndReason::Idle));
            }
            break;
        }

        events
    }

    /// 锁屏/睡眠：立即结束当前活动，避免把离开的时间算进去。
    pub fn pause(&mut self, at: Timestamp) -> Vec<AggregatorEvent> {
        let mut events = Vec::new();
        if let Some(pending) = self.pending.take() {
            events.push(AggregatorEvent::NoiseDropped {
                observation_ids: pending.observations.clone(),
                title: pending.candidate.title,
                reason: "interrupted",
            });
            self.dropped.extend(pending.observations);
        }
        if let Some(open) = self.open.as_ref() {
            // 活动在最后一次观测时刻结束 —— 锁屏本身不该被算进活动时长
            let end = open.end;
            let _ = at;
            events.extend(self.close_open(end, EndReason::Locked));
        }
        events
    }

    /// 关机/强制 flush。
    pub fn flush(&mut self, at: Timestamp) -> Vec<AggregatorEvent> {
        let mut events = Vec::new();
        if let Some(pending) = self.pending.take() {
            events.push(AggregatorEvent::NoiseDropped {
                observation_ids: pending.observations.clone(),
                title: pending.candidate.title,
                reason: "flushed",
            });
            self.dropped.extend(pending.observations);
        }
        if let Some(open) = self.open.as_ref() {
            let end = if at > open.end { at } else { open.end };
            events.extend(self.close_open(end, EndReason::Flushed));
        }
        events
    }

    // ---------------- 内部 ----------------

    fn elapsed_secs(&self, from: Timestamp, to: Timestamp) -> u64 {
        (to.saturating_diff_millis(from).max(0) / 1000) as u64
    }

    fn commit_if_due(&mut self, now: Timestamp) -> Vec<AggregatorEvent> {
        let due = self
            .pending
            .as_ref()
            .map(|pending| self.elapsed_secs(pending.first_seen, now) >= self.policy.debounce_secs)
            .unwrap_or(false);

        if due {
            self.commit_switch()
        } else {
            Vec::new()
        }
    }

    fn commit_switch(&mut self) -> Vec<AggregatorEvent> {
        let Some(pending) = self.pending.take() else {
            return Vec::new();
        };

        let mut events = Vec::new();

        // 原活动在「第一次离开」的时刻结束
        if self.open.is_some() {
            events.extend(self.close_open(pending.first_seen, EndReason::Switched));
        }

        // 短时间内回到同一标题 → 合并回上一个已关闭的活动
        if let Some(index) = self.merge_target(&pending.candidate.title, pending.first_seen) {
            let mut merged = self.closed.remove(index);
            merged
                .observations
                .extend(pending.observations.iter().cloned());
            merged.ended_at = pending.last_seen;
            let activity = OpenActivity {
                id: merged.id.clone(),
                title: merged.title.clone(),
                category: pending.candidate.category.clone(),
                origin: pending.candidate.origin.clone(),
                confidence: pending.candidate.confidence,
                start: pending.first_seen,
                end: pending.last_seen,
                observations: merged.observations.clone(),
            };
            // 合并后不重新发 Started —— 调用方通过 Extended 事件把观测挂回原活动
            for observation in &pending.observations {
                events.push(AggregatorEvent::Extended {
                    id: activity.id.clone(),
                    observation_id: observation.clone(),
                    at: pending.last_seen,
                });
            }
            self.open = Some(activity);
            return events;
        }

        events.extend(self.start_activity(&pending.candidate, pending.first_seen));

        if let Some(open) = &mut self.open {
            open.end = pending.last_seen;
            open.observations = pending.observations;
        }

        events
    }

    fn merge_target(&self, title: &str, first_seen: Timestamp) -> Option<usize> {
        self.closed
            .iter()
            .enumerate()
            .rev()
            .find(|(_, activity)| {
                activity.title == title
                    && self.elapsed_secs(activity.ended_at, first_seen)
                        <= self.policy.merge_gap_secs
            })
            .map(|(index, _)| index)
    }

    fn start_activity(
        &mut self,
        candidate: &ActivityCandidate,
        at: Timestamp,
    ) -> Vec<AggregatorEvent> {
        let id = format!("act-{}", self.next_id);
        self.next_id += 1;

        self.open = Some(OpenActivity {
            id: id.clone(),
            title: candidate.title.clone(),
            category: candidate.category.clone(),
            origin: candidate.origin.clone(),
            confidence: candidate.confidence,
            start: at,
            end: at,
            observations: Vec::new(),
        });

        vec![AggregatorEvent::Started {
            id,
            title: candidate.title.clone(),
            category: candidate.category.clone(),
            at,
            origin: candidate.origin.clone(),
            confidence: candidate.confidence,
        }]
    }

    fn close_open(&mut self, at: Timestamp, reason: EndReason) -> Vec<AggregatorEvent> {
        let Some(open) = self.open.take() else {
            return Vec::new();
        };

        self.closed.push(ClosedActivity {
            id: open.id.clone(),
            title: open.title.clone(),
            observations: open.observations.clone(),
            ended_at: at,
        });
        if self.closed.len() > CLOSED_HISTORY {
            self.closed.remove(0);
        }

        vec![AggregatorEvent::Ended {
            id: open.id,
            at,
            reason,
        }]
    }
}
