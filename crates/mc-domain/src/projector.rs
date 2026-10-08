//! 投影器：`events → activities`（纯函数）。
//!
//! 三条硬性约束：**确定性**（同一段事件历史重放两次逐字节一致：不读时钟、
//! 不用随机数、不依赖 HashMap 迭代顺序，时间一律由事件本身给出）；
//! **实时与重放同码**（`project()` 只是 `Projector::apply()` 的循环，
//! 不存在两套逻辑）；**活动 id 由证据决定**（取该活动最早那条观测的 id，
//! 而不是「第几个活动」—— 否则历史里插进一条新观测会让已有 id 集体漂移）。
//!
//! 规则在这里才生效，意味着「改规则 → 重放」可以追溯重算历史，
//! 而实时运行时不追溯。

use std::collections::BTreeMap;

use mc_common::time::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::activity::{
    apply_overrides, Activity, ActivityCandidate, ActivityView, AggregationPolicy, Aggregator,
    AggregatorEvent, ObservationRef, OverrideKind, Provenance,
};
use crate::observation::ObservationSummary;
use crate::rules::{AiPreference, RuleMatch, RuleSet};

// ---------------------------------------------------------------- 事件

pub const KIND_OBSERVATION_RECORDED: &str = "observation.recorded";
/// 采集管线实际写入的事件种类（`NewObservation::event_payload` 的形状）。
///
/// 两种都要认：`observation.recorded` 是领域层自己造的规整形状，
/// `observation_captured` 是采集链路已经在写的形状 ——
/// 只认其中一种，线上就会出现「有观测却永远没有活动」。
pub const KIND_OBSERVATION_CAPTURED: &str = "observation_captured";
pub const KIND_ACTIVITY_OVERRIDDEN: &str = "activity.overridden";
pub const KIND_SCREEN_LOCKED: &str = "screen.locked";
pub const KIND_PIPELINE_FLUSHED: &str = "pipeline.flushed";
/// 模型推断出的活动结论（规则没命中时段才可能有）。
///
/// 和用户修正一样是**事件**：算法升级、提示词变更之后重放，
/// 历史结论会按新的推断结果重建，而不是留一堆无法解释的旧标题。
pub const KIND_ACTIVITY_INFERRED: &str = "activity.inferred";

/// 模型对某个活动给出的结论。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InferredSuggestion {
    /// 目标活动 id（由最早一条观测决定，因此推断方可以算出来）
    pub activity_id: String,
    pub title: String,
    pub category: Option<String>,
    pub confidence: f32,
    /// provider id（含 provider 类型），出问题时必须能区分是哪个模型
    pub model: String,
}

/// 领域事件：投影器的唯一输入。
///
/// 刻意不用 serde 的 internally-tagged 形式：事件在库里本来就是
/// `kind` 列 + `payload` 列两段，`kind` 以列为准可以避免两处不一致。
#[derive(Debug, Clone, PartialEq)]
pub enum DomainEvent {
    ObservationRecorded {
        observation: ObservationSummary,
    },
    ActivityOverridden {
        override_: OverrideKind,
    },
    ActivityInferred {
        suggestion: InferredSuggestion,
    },
    ScreenLocked {
        at: Timestamp,
    },
    PipelineFlushed {
        at: Timestamp,
    },
    /// 本版本不认识的事件：跳过并计数，不中断重放
    Unknown {
        kind: String,
    },
}

impl DomainEvent {
    pub fn kind(&self) -> &str {
        match self {
            Self::ObservationRecorded { .. } => KIND_OBSERVATION_RECORDED,
            Self::ActivityOverridden { .. } => KIND_ACTIVITY_OVERRIDDEN,
            Self::ActivityInferred { .. } => KIND_ACTIVITY_INFERRED,
            Self::ScreenLocked { .. } => KIND_SCREEN_LOCKED,
            Self::PipelineFlushed { .. } => KIND_PIPELINE_FLUSHED,
            Self::Unknown { kind } => kind,
        }
    }

    pub fn payload(&self) -> Value {
        match self {
            Self::ObservationRecorded { observation } => {
                serde_json::to_value(observation).unwrap_or(Value::Null)
            }
            Self::ActivityOverridden { override_ } => {
                serde_json::to_value(override_).unwrap_or(Value::Null)
            }
            Self::ActivityInferred { suggestion } => {
                serde_json::to_value(suggestion).unwrap_or(Value::Null)
            }
            Self::ScreenLocked { at } | Self::PipelineFlushed { at } => {
                json!({ "at": at.as_millis() })
            }
            Self::Unknown { kind } => json!({ "kind": kind }),
        }
    }

    /// 从存储的两段式记录还原。`kind` 以列值为准；payload 解析失败时
    /// 退化成 `Unknown`，**绝不让一条坏记录毁掉整次重放**。
    ///
    /// `at` 取事件的 `at_utc_ms`：采集事件自身不带时间字段，
    /// 而聚合完全依赖时间，缺了它活动会被算成一瞬间的事。
    pub fn from_stored(kind: &str, at: Timestamp, payload: &Value) -> Self {
        let decoded = match kind {
            KIND_OBSERVATION_RECORDED => {
                serde_json::from_value::<ObservationSummary>(payload.clone())
                    .ok()
                    .map(|observation| Self::ObservationRecorded { observation })
            }
            KIND_OBSERVATION_CAPTURED => Some(Self::ObservationRecorded {
                observation: ObservationSummary {
                    id: payload
                        .get("observation_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    at,
                    app_name: string_field(payload, "app_name"),
                    window_title: string_field(payload, "window_title"),
                    domain: string_field(payload, "domain"),
                    text: string_field(payload, "text"),
                },
            }),
            KIND_ACTIVITY_OVERRIDDEN => serde_json::from_value::<OverrideKind>(payload.clone())
                .ok()
                .map(|override_| Self::ActivityOverridden { override_ }),
            KIND_ACTIVITY_INFERRED => serde_json::from_value::<InferredSuggestion>(payload.clone())
                .ok()
                .map(|suggestion| Self::ActivityInferred { suggestion }),
            KIND_SCREEN_LOCKED => at_from_payload(payload).map(|at| Self::ScreenLocked { at }),
            KIND_PIPELINE_FLUSHED => {
                at_from_payload(payload).map(|at| Self::PipelineFlushed { at })
            }
            _ => None,
        };

        decoded.unwrap_or_else(|| Self::Unknown {
            kind: kind.to_string(),
        })
    }

    /// 事件本身代表的时间（用于推进 `horizon`）。
    pub fn at(&self) -> Option<Timestamp> {
        match self {
            Self::ObservationRecorded { observation } => Some(observation.at),
            Self::ActivityOverridden { override_ } => Some(override_.at()),
            // 推断事件的时间由事件本身（`at_utc_ms`）给出，不写进 payload
            Self::ActivityInferred { .. } => None,
            Self::ScreenLocked { at } | Self::PipelineFlushed { at } => Some(*at),
            Self::Unknown { .. } => None,
        }
    }
}

fn at_from_payload(payload: &Value) -> Option<Timestamp> {
    payload
        .get("at")
        .and_then(Value::as_i64)
        .map(Timestamp::from_millis)
}

// ---------------------------------------------------------------- 选项与产物

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProjectionOptions {
    pub policy: AggregationPolicy,
    /// 是否允许为「规则没命中」的观测申请模型推断。
    ///
    /// 关掉（无预算）时系统仍然产出 `Observed` 活动 —— 降级而不是停摆。
    pub allow_inference: bool,
}

impl Default for ProjectionOptions {
    fn default() -> Self {
        Self {
            policy: AggregationPolicy::default(),
            allow_inference: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Projection {
    pub activities: Vec<ActivityView>,
    /// 被判定为噪声、但必须留痕的观测 id（观测不允许成为孤儿）
    pub noise: Vec<String>,
    /// 本次投影结束后**真正需要推断**的活动数（规则未命中且用户没改过）。
    ///
    /// 按「活动」而不是按「观测」计数：实际推断路径
    /// （`mc_pipeline::activity_ai::infer_activities`）也是按活动走的，
    /// 被合并/被吸收掉的短暂切走不该产生一次请求。
    pub ai_requests: usize,
    pub unknown_event_kinds: usize,
}

// ---------------------------------------------------------------- 投影器

pub fn project(events: &[DomainEvent], rules: &RuleSet, options: ProjectionOptions) -> Projection {
    let mut projector = Projector::new(rules, options);
    for event in events {
        projector.apply(event);
    }
    projector.finish()
}

struct Builder {
    id: String,
    title: String,
    category: Option<String>,
    origin: Provenance,
    confidence: f32,
    start: Timestamp,
    end: Timestamp,
    /// 观测 id → 时间。用 BTreeMap 而不是 Vec：天然去重且顺序确定
    observations: BTreeMap<String, Timestamp>,
}

pub struct Projector<'a> {
    rules: &'a RuleSet,
    options: ProjectionOptions,
    aggregator: Aggregator,
    builders: Vec<Builder>,
    /// 聚合器内部 id → builders 下标
    index: BTreeMap<String, usize>,
    /// 观测 id → 发生时间（聚合器只给 id，时间要从事件里取）
    observation_times: BTreeMap<String, Timestamp>,
    overrides: Vec<OverrideKind>,
    inferred: Vec<InferredSuggestion>,
    unknown_event_kinds: usize,
    horizon: Option<Timestamp>,
}

impl<'a> Projector<'a> {
    pub fn new(rules: &'a RuleSet, options: ProjectionOptions) -> Self {
        Self {
            rules,
            options,
            aggregator: Aggregator::new(options.policy),
            builders: Vec::new(),
            index: BTreeMap::new(),
            observation_times: BTreeMap::new(),
            overrides: Vec::new(),
            inferred: Vec::new(),
            unknown_event_kinds: 0,
            horizon: None,
        }
    }

    pub fn apply(&mut self, event: &DomainEvent) {
        if let Some(at) = event.at() {
            self.horizon = Some(match self.horizon {
                Some(current) if current >= at => current,
                _ => at,
            });

            // 用**事件时间**推进聚合器的时钟。
            // 空闲结束与超长切分都只在 tick 里判定：重放时如果从不 tick，
            // 一个跨夜的观测序列会被算成「连续工作了 9 小时」。
            let events = self.aggregator.tick(at);
            self.absorb(events);
        }

        match event {
            DomainEvent::ObservationRecorded { observation } => {
                self.observation_times
                    .insert(observation.id.clone(), observation.at);
                let candidate = self.candidate_for(observation);
                let events = self
                    .aggregator
                    .observe(&observation.id, &candidate, observation.at);
                self.absorb(events);
            }
            DomainEvent::ActivityOverridden { override_ } => {
                self.overrides.push(override_.clone());
            }
            DomainEvent::ActivityInferred { suggestion } => {
                self.inferred.push(suggestion.clone());
            }
            DomainEvent::ScreenLocked { at } => {
                let events = self.aggregator.pause(*at);
                self.absorb(events);
            }
            DomainEvent::PipelineFlushed { at } => {
                let events = self.aggregator.flush(*at);
                self.absorb(events);
            }
            DomainEvent::Unknown { .. } => self.unknown_event_kinds += 1,
        }
    }

    pub fn finish(mut self) -> Projection {
        // 用事件自身的最后时间收尾，而不是墙上时钟 —— 否则重放结果会随「什么时候跑」变化
        let horizon = self.horizon.unwrap_or_else(|| Timestamp::from_millis(0));
        let events = self.aggregator.flush(horizon);
        self.absorb(events);

        let mut activities: Vec<Activity> = self
            .builders
            .iter()
            .filter_map(builder_to_activity)
            .collect();

        // 按开始时间排序；同一时刻用 id 兜底，保证顺序唯一
        activities.sort_by(|left, right| {
            left.start
                .cmp(&right.start)
                .then_with(|| left.id.cmp(&right.id))
        });

        // 活动 id 由最早那条观测决定，而不是「第几个」
        for activity in &mut activities {
            let anchor = activity
                .observations
                .iter()
                .min_by(|left, right| left.at.cmp(&right.at).then_with(|| left.id.cmp(&right.id)))
                .map(|observation| observation.id.clone());
            if let Some(anchor) = anchor {
                activity.id = format!("act-{anchor}");
            }
        }

        // 顺序要紧：先让模型推断升级「只有元数据」的活动，
        // 再叠加用户修正 —— 用户改过的永远压过模型猜的。
        apply_inferred(&mut activities, &self.inferred);

        let mut noise = self.aggregator.dropped_observations();
        noise.sort();
        noise.dedup();

        let views = apply_overrides(&activities, &self.overrides);
        let ai_requests = if self.options.allow_inference {
            views
                .iter()
                .filter(|view| view.origin == Provenance::Observed && !view.is_user_modified)
                .count()
        } else {
            0
        };

        Projection {
            activities: views,
            noise,
            ai_requests,
            unknown_event_kinds: self.unknown_event_kinds,
        }
    }

    // ---------------- 内部 ----------------

    /// 规则优先，否则用廉价元数据兜底（仍然不调模型）。
    fn candidate_for(&mut self, observation: &ObservationSummary) -> ActivityCandidate {
        match self.rules.classify(observation) {
            RuleMatch::Matched {
                rule_id,
                name,
                category,
                ai,
                ..
            } => ActivityCandidate {
                title: name,
                category,
                origin: Provenance::Rule { rule_id },
                // 规则是确定性的：给 1.0，避免 UI 把规则命中显示成「猜的」
                confidence: 1.0,
                ai,
            },
            RuleMatch::Conflict { .. } | RuleMatch::None => {
                let ai = if self.options.allow_inference {
                    AiPreference::Assist
                } else {
                    AiPreference::None
                };
                ActivityCandidate {
                    title: fallback_title(observation),
                    category: None,
                    // 兜底只有「看见了什么」，没有推断，因此必须是 Observed
                    origin: Provenance::Observed,
                    confidence: 1.0,
                    ai,
                }
            }
        }
    }

    fn absorb(&mut self, events: Vec<AggregatorEvent>) {
        for event in &events {
            if let AggregatorEvent::Ended { id, at, .. } = event {
                // 切换导致的结束时刻可能晚于最后一条观测（活动持续到切走那一刻）
                if let Some(index) = self.index.get(id) {
                    let builder = &mut self.builders[*index];
                    if *at > builder.end {
                        builder.end = *at;
                    }
                }
            }
        }
        self.sync_open();
    }

    /// 把聚合器当前打开的活动同步进 builder。
    ///
    /// 以 `OpenActivity` 快照为准而不是逐条解释 `AggregatorEvent`：
    /// 合并路径会复用旧 id，逐条解释极易漏挂观测。
    fn sync_open(&mut self) {
        let Some(open) = self.aggregator.open().cloned() else {
            return;
        };

        let index = match self.index.get(&open.id) {
            Some(index) => *index,
            None => {
                self.builders.push(Builder {
                    id: open.id.clone(),
                    title: open.title.clone(),
                    category: open.category.clone(),
                    origin: open.origin.clone(),
                    confidence: open.confidence,
                    start: open.start,
                    end: open.end,
                    observations: BTreeMap::new(),
                });
                self.index.insert(open.id.clone(), self.builders.len() - 1);
                self.builders.len() - 1
            }
        };

        // 聚合器只记 id；时间从事件里取，迟到观测也不该把时间抹成「当前」
        let times: Vec<(String, Timestamp)> = open
            .observations
            .iter()
            .map(|id| {
                let at = self.observation_times.get(id).copied().unwrap_or(open.end);
                (id.clone(), at)
            })
            .collect();

        let builder = &mut self.builders[index];
        builder.title = open.title.clone();
        builder.category = open.category.clone();
        builder.origin = open.origin.clone();
        builder.confidence = open.confidence;
        // 起点只往前推，终点只往后推：合并与迟到观测都不该让窗口缩水
        if open.start < builder.start {
            builder.start = open.start;
        }
        if open.end > builder.end {
            builder.end = open.end;
        }
        for (id, fallback_at) in times {
            builder.observations.entry(id).or_insert(fallback_at);
        }
    }
}

/// 把模型结论叠加到活动上。
///
/// **只升级 `Observed`**：规则命中是用户自己定的确定性结论，
/// 不能让模型的猜测把它盖掉（否则规则判定的结果会被模型猜测悄悄覆盖）。
fn apply_inferred(activities: &mut [Activity], suggestions: &[InferredSuggestion]) {
    for suggestion in suggestions {
        let Some(activity) = activities
            .iter_mut()
            .find(|activity| activity.id == suggestion.activity_id)
        else {
            continue;
        };

        if !matches!(activity.origin, Provenance::Observed) {
            continue;
        }
        if !(suggestion.confidence > 0.0 && suggestion.confidence <= 1.0) {
            continue;
        }

        activity.title = suggestion.title.clone();
        activity.category = suggestion.category.clone();
        activity.confidence = suggestion.confidence;
        activity.origin = Provenance::Inferred {
            model: suggestion.model.clone(),
        };
    }
}

fn fallback_title(observation: &ObservationSummary) -> String {
    // 用进程名而不是窗口标题：窗口标题每换一个文件就变，会把活动切碎
    match (&observation.app_name, &observation.window_title) {
        (Some(app), _) if !app.trim().is_empty() => app.clone(),
        (_, Some(title)) if !title.trim().is_empty() => title.clone(),
        _ => "未知活动".to_string(),
    }
}

fn builder_to_activity(builder: &Builder) -> Option<Activity> {
    if builder.observations.is_empty() {
        // 没有任何证据的活动等于凭空断言，宁可不写库
        return None;
    }

    let observations: Vec<ObservationRef> = builder
        .observations
        .iter()
        .map(|(id, at)| ObservationRef {
            id: id.clone(),
            at: *at,
        })
        .collect();

    let first = observations.first()?.at;
    let last = observations.last()?.at;

    let activity = Activity {
        id: builder.id.clone(),
        start: if builder.start < first {
            builder.start
        } else {
            first
        },
        end: if builder.end > last {
            builder.end
        } else {
            last
        },
        title: builder.title.clone(),
        category: builder.category.clone(),
        observations,
        origin: builder.origin.clone(),
        confidence: builder.confidence,
    };

    // 不变量不过的活动不进投影结果
    activity.validate().ok().map(|_| activity)
}

fn string_field(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
}
