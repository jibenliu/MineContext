//! 场景文件：时间序列驱动的回归基线。
//!
//! 一份 YAML 同时断言多个维度：活动序列、观测归属、噪声、模型调用次数。
//! 它解决的是单测解决不了的问题 —— 活动的正确性是**时间序列上的涌现结果**，
//! 「防抖生效」这句话只有在一条完整的时间线上才有意义。
//!
//! 场景里的规则是**内联**的：改了聚合参数或规则语义，场景必须跟着变绿，
//! 而不是像快照测试那样先无脑更新期望值。

use std::path::{Path, PathBuf};

use mc_common::time::Timestamp;
use mc_domain::activity::{AggregationPolicy, Provenance};
use mc_domain::observation::ObservationSummary;
use mc_domain::projector::{
    project, DomainEvent, InferredSuggestion, Projection, ProjectionOptions,
};
use mc_domain::rules::RuleSet;
use mc_domain::stage::{ActivitySignal, ReplayStep, StageEffect, StagePolicy, StageReplay};
use serde::{Deserialize, Serialize};

/// 场景根目录（`fixtures/`）。用编译期路径，测试从任何工作目录跑都成立。
pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub fn activity_scenarios_dir() -> PathBuf {
    fixtures_root().join("scenarios/activity")
}

pub fn stage_scenarios_dir() -> PathBuf {
    fixtures_root().join("scenarios/stage")
}

pub fn summary_scenarios_dir() -> PathBuf {
    fixtures_root().join("scenarios/summary")
}

#[derive(Debug, Deserialize)]
pub struct Scenario {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub config: ScenarioConfig,
    #[serde(default)]
    pub rules: Vec<RuleSpec>,
    pub steps: Vec<Step>,
    pub expect: Expect,
}

#[derive(Debug, Default, Deserialize)]
pub struct ScenarioConfig {
    #[serde(default)]
    pub activity: ActivityConfig,
    #[serde(default)]
    pub stage: StageConfig,
}

/// 阶段策略的场景覆盖（缺省跟随 `StagePolicy::default()`）。
#[derive(Debug, Deserialize)]
pub struct StageConfig {
    #[serde(default = "default_stage_min_duration")]
    pub min_duration_secs: u64,
    #[serde(default = "default_stage_max_duration")]
    pub max_duration_secs: u64,
    #[serde(default = "default_stage_switch_grace")]
    pub switch_grace_secs: u64,
    #[serde(default = "default_stage_idle_threshold")]
    pub idle_threshold_secs: u64,
    #[serde(default = "default_stage_min_stable")]
    pub min_activity_stable_secs: u64,
    #[serde(default = "default_stage_summary_deadline")]
    pub summary_deadline_secs: u64,
    #[serde(default = "default_stage_timezone")]
    pub timezone: String,
}

impl Default for StageConfig {
    fn default() -> Self {
        let policy = StagePolicy::default();
        Self {
            min_duration_secs: policy.min_duration_secs,
            max_duration_secs: policy.max_duration_secs,
            switch_grace_secs: policy.switch_grace_secs,
            idle_threshold_secs: policy.idle_threshold_secs,
            min_activity_stable_secs: policy.min_activity_stable_secs,
            summary_deadline_secs: policy.summary_deadline_secs,
            timezone: policy.timezone,
        }
    }
}

fn default_stage_min_duration() -> u64 {
    StagePolicy::default().min_duration_secs
}
fn default_stage_max_duration() -> u64 {
    StagePolicy::default().max_duration_secs
}
fn default_stage_switch_grace() -> u64 {
    StagePolicy::default().switch_grace_secs
}
fn default_stage_idle_threshold() -> u64 {
    StagePolicy::default().idle_threshold_secs
}
fn default_stage_min_stable() -> u64 {
    StagePolicy::default().min_activity_stable_secs
}
fn default_stage_summary_deadline() -> u64 {
    StagePolicy::default().summary_deadline_secs
}
fn default_stage_timezone() -> String {
    StagePolicy::default().timezone
}

#[derive(Debug, Deserialize)]
pub struct ActivityConfig {
    #[serde(default = "default_debounce")]
    pub debounce_secs: u64,
    #[serde(default = "default_min_duration")]
    pub min_duration_secs: u64,
    #[serde(default = "default_merge_gap")]
    pub merge_gap_secs: u64,
    #[serde(default = "default_max_duration")]
    pub max_duration_secs: u64,
    #[serde(default = "default_idle_gap")]
    pub idle_gap_secs: u64,
}

impl Default for ActivityConfig {
    fn default() -> Self {
        let policy = AggregationPolicy::default();
        Self {
            debounce_secs: policy.debounce_secs,
            min_duration_secs: policy.min_duration_secs,
            merge_gap_secs: policy.merge_gap_secs,
            max_duration_secs: policy.max_duration_secs,
            idle_gap_secs: policy.idle_gap_secs,
        }
    }
}

fn default_debounce() -> u64 {
    AggregationPolicy::default().debounce_secs
}
fn default_min_duration() -> u64 {
    AggregationPolicy::default().min_duration_secs
}
fn default_merge_gap() -> u64 {
    AggregationPolicy::default().merge_gap_secs
}
fn default_max_duration() -> u64 {
    AggregationPolicy::default().max_duration_secs
}
fn default_idle_gap() -> u64 {
    AggregationPolicy::default().idle_gap_secs
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RuleSpec {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub triggers: TriggerSpec,
    #[serde(default)]
    pub exclude: TriggerSpec,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TriggerSpec {
    #[serde(default)]
    pub apps: Vec<String>,
    #[serde(default)]
    pub window_patterns: Vec<String>,
    #[serde(default)]
    pub domains: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// 时间推进的两种写法：绝对时刻（`at`）或相对上一步（`advance`）。
///
/// 两者都支持是因为场景的可读性需求不同：
/// 「09:00 打开 VSCode」用绝对时刻一眼就懂，
/// 「60 秒后」在长序列里更省心。
#[derive(Debug, Deserialize)]
pub struct Step {
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default)]
    pub advance: Option<String>,
    #[serde(default)]
    pub observe: Option<ObserveSpec>,
    #[serde(default)]
    pub override_: Option<OverrideSpec>,
    #[serde(default)]
    pub inferred: Option<InferredSpec>,
    #[serde(default)]
    pub lock: bool,
}

#[derive(Debug, Deserialize)]
pub struct ObserveSpec {
    pub app: String,
    pub window: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OverrideSpec {
    pub kind: String,
    pub activity_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct InferredSpec {
    pub activity_id: String,
    pub title: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    #[serde(default = "default_model")]
    pub model: String,
}

fn default_confidence() -> f32 {
    0.8
}
fn default_model() -> String {
    "stub-vlm".to_string()
}

#[derive(Debug, Default, Deserialize)]
pub struct Expect {
    #[serde(default)]
    pub activities: Vec<ActivityExpect>,
    #[serde(default)]
    pub count_total: Option<usize>,
    #[serde(default)]
    pub noise: Option<Vec<String>>,
    #[serde(default)]
    pub provider_calls: ProviderExpect,
    /// 阶段断言。给了才会跑阶段重放。
    #[serde(default)]
    pub stages: Option<StageExpectations>,
    /// 总结断言（走确定性兜底，因此是纯函数、可重复）
    #[serde(default)]
    pub summaries: Option<SummaryExpectations>,
}

#[derive(Debug, Default, Deserialize)]
pub struct StageExpectations {
    #[serde(default)]
    pub count_total: Option<usize>,
    #[serde(default)]
    pub items: Vec<StageExpect>,
    /// 重放到场景末尾时是否应当有「进行中」的阶段
    #[serde(default)]
    pub open: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct StageExpect {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub end_reason: Option<String>,
    #[serde(default)]
    pub activities: Option<Vec<String>>,
    #[serde(default)]
    pub min_duration_secs: Option<u64>,
    #[serde(default)]
    pub max_duration_secs: Option<u64>,
}

/// 总结断言。
///
/// **不调用模型**：用确定性兜底渲染，因此断言可以精确到内容，
/// 也不需要在场景里模拟 provider。这条兜底正是「有阶段必有总结」的第二道防线。
#[derive(Debug, Default, Deserialize)]
pub struct SummaryExpectations {
    /// 断言「每个够长的已关闭阶段都有非空总结」（不变量本身）
    #[serde(default)]
    pub every_long_stage_has_one: Option<bool>,
    #[serde(default)]
    pub quality: Option<String>,
    /// 每份总结正文里必须出现的片段
    #[serde(default)]
    pub body_contains: Vec<String>,
    /// 断言「够长的阶段一个都不该被跳过」
    #[serde(default)]
    pub long_stage_count: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ActivityExpect {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub title_contains: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub observations: Option<Vec<String>>,
    #[serde(default)]
    pub min_duration_secs: Option<u64>,
    #[serde(default)]
    pub max_duration_secs: Option<u64>,
    #[serde(default)]
    pub is_user_modified: Option<bool>,
}

/// `vision` 是**精确**次数，`vision_at_least` 是下界。
///
/// 精确断言只用在「规则命中 ⇒ 必须 0 次」这类硬要求上；
/// 其余场景用下界，避免换个模型就红一片。
#[derive(Debug, Default, Deserialize)]
pub struct ProviderExpect {
    #[serde(default)]
    pub vision: Option<usize>,
    #[serde(default)]
    pub vision_at_least: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioOutcome {
    pub projection: Projection,
    /// 阶段重放结果（场景声明了 `expect.stages`/`summaries` 时才有意义）
    pub stages: StageReplay,
    /// 重放终点（= 最后一条事件的时刻，保证确定性）
    pub horizon: Timestamp,
}

/// 加载目录下的全部场景（按文件名排序，失败信息可复现）。
pub fn load_all(directory: &Path) -> Result<Vec<(PathBuf, Scenario)>, String> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(directory)
        .map_err(|error| format!("无法读取场景目录 {}: {error}", directory.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "yaml"))
        .collect();
    paths.sort();

    let mut out = Vec::new();
    for path in paths {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("无法读取 {}: {error}", path.display()))?;
        let scenario: Scenario =
            serde_yaml::from_str(&text).map_err(|error| format!("场景文件无法解析：{error}"))?;
        out.push((path, scenario));
    }
    Ok(out)
}

/// 跑一条场景，得到投影结果。
pub fn run(scenario: &Scenario) -> Result<ScenarioOutcome, String> {
    let policy = AggregationPolicy {
        debounce_secs: scenario.config.activity.debounce_secs,
        min_duration_secs: scenario.config.activity.min_duration_secs,
        merge_gap_secs: scenario.config.activity.merge_gap_secs,
        max_duration_secs: scenario.config.activity.max_duration_secs,
        idle_gap_secs: scenario.config.activity.idle_gap_secs,
    };

    let rules = parse_rules(&scenario.rules)?;

    // 时间从场景第一条 `at` 的「当天 09:00 UTC」起算：绝对时刻写起来好看，
    // 但聚合只关心相对间隔。
    let base = Timestamp::parse_rfc3339("2026-09-30T09:00:00Z")
        .map_err(|error| format!("基准时间不合法：{}", error.detail()))?;

    let mut events: Vec<DomainEvent> = Vec::new();
    let mut cursor = base;
    let mut counter = 0usize;

    for step in &scenario.steps {
        if let Some(advance) = &step.advance {
            cursor = Timestamp::from_millis(cursor.as_millis() + parse_duration(advance)? * 1_000);
        }
        if let Some(at) = &step.at {
            cursor = Timestamp::parse_rfc3339(at)
                .map_err(|error| format!("步骤时间 `{at}` 不合法：{}", error.detail()))?;
        }

        if let Some(observe) = &step.observe {
            counter += 1;
            let id = observe
                .id
                .clone()
                .unwrap_or_else(|| format!("obs-{counter}"));
            events.push(DomainEvent::ObservationRecorded {
                observation: ObservationSummary {
                    id,
                    at: cursor,
                    app_name: Some(observe.app.clone()),
                    window_title: Some(observe.window.clone()),
                    domain: observe.domain.clone(),
                    text: observe.text.clone(),
                },
            });
        }

        if step.lock {
            events.push(DomainEvent::ScreenLocked { at: cursor });
        }

        if let Some(spec) = &step.override_ {
            let override_ = match spec.kind.as_str() {
                "rename" => mc_domain::activity::OverrideKind::Rename {
                    activity_id: spec.activity_id.clone(),
                    title: spec
                        .title
                        .clone()
                        .ok_or_else(|| "rename 覆盖缺少 title".to_string())?,
                    at: cursor,
                },
                "set_category" => mc_domain::activity::OverrideKind::SetCategory {
                    activity_id: spec.activity_id.clone(),
                    category: spec.category.clone(),
                    at: cursor,
                },
                other => return Err(format!("不支持的覆盖类型 `{other}`")),
            };
            events.push(DomainEvent::ActivityOverridden { override_ });
        }

        if let Some(spec) = &step.inferred {
            events.push(DomainEvent::ActivityInferred {
                suggestion: InferredSuggestion {
                    activity_id: spec.activity_id.clone(),
                    title: spec.title.clone(),
                    category: spec.category.clone(),
                    confidence: spec.confidence,
                    model: spec.model.clone(),
                },
            });
        }
    }

    let options = ProjectionOptions {
        policy,
        allow_inference: true,
    };
    let projection = project(&events, &rules, options);

    // 阶段重放：与 daemon 走**同一份实现**（`mc_domain::stage::replay`）。
    // 重放终点取最后一条事件的时刻，而不是墙上时钟 —— 否则同一条场景
    // 在不同时间运行会得到不同结果。
    let horizon = events
        .iter()
        .filter_map(|event| event.at())
        .max()
        .unwrap_or(base);

    let mut steps: Vec<ReplayStep> = projection
        .activities
        .iter()
        .map(|activity| {
            ReplayStep::Signal(ActivitySignal {
                id: activity.id.clone(),
                title: activity.title.clone(),
                start: activity.start,
                end: activity.end,
            })
        })
        .collect();
    for event in &events {
        if let DomainEvent::ScreenLocked { at } = event {
            steps.push(ReplayStep::Locked(*at));
        }
    }

    let stages = mc_domain::stage::replay(&stage_policy(&scenario.config.stage), &steps, horizon);

    Ok(ScenarioOutcome {
        projection,
        stages,
        horizon,
    })
}

fn stage_policy(config: &StageConfig) -> StagePolicy {
    StagePolicy {
        min_duration_secs: config.min_duration_secs,
        max_duration_secs: config.max_duration_secs,
        switch_grace_secs: config.switch_grace_secs,
        idle_threshold_secs: config.idle_threshold_secs,
        min_activity_stable_secs: config.min_activity_stable_secs,
        summary_deadline_secs: config.summary_deadline_secs,
        timezone: config.timezone.clone(),
    }
}

/// 断言。失败信息带上场景名与路径，避免「第 3 条断言挂了」这种谜题。
pub fn verify(scenario: &Scenario, outcome: &ScenarioOutcome) -> Result<(), String> {
    let projection = &outcome.projection;

    if let Some(total) = scenario.expect.count_total {
        if projection.activities.len() != total {
            return Err(format!(
                "期望 {total} 个活动，实际 {}（{}）",
                projection.activities.len(),
                describe(&projection.activities)
            ));
        }
    }

    // 只有**声明了活动断言**的场景才要求逐条对应：
    // stage/summary 场景关心的是阶段与总结，不该被迫把活动列表也写一遍。
    if !scenario.expect.activities.is_empty()
        && scenario.expect.activities.len() != projection.activities.len()
    {
        return Err(format!(
            "期望断言 {} 个活动，实际 {} 个（{}）",
            scenario.expect.activities.len(),
            projection.activities.len(),
            describe(&projection.activities)
        ));
    }

    for (index, expect) in scenario.expect.activities.iter().enumerate() {
        let activity = &projection.activities[index];
        let where_ = format!("第 {} 个活动", index + 1);

        if let Some(title) = &expect.title {
            if &activity.title != title {
                return Err(format!(
                    "{where_} 标题期望 `{title}`，实际 `{}`",
                    activity.title
                ));
            }
        }
        if let Some(fragment) = &expect.title_contains {
            if !activity.title.contains(fragment) {
                return Err(format!(
                    "{where_} 标题 `{}` 不包含 `{fragment}`",
                    activity.title
                ));
            }
        }
        if let Some(kind) = &expect.origin {
            let actual = origin_kind(&activity.origin);
            if actual != kind {
                return Err(format!("{where_} origin 期望 `{kind}`，实际 `{actual}`"));
            }
        }
        if let Some(observations) = &expect.observations {
            let mut actual = activity.observation_ids();
            actual.sort();
            let mut wanted = observations.clone();
            wanted.sort();
            if actual != wanted {
                return Err(format!(
                    "{where_} 观测归属期望 {wanted:?}，实际 {actual:?}（观测不能丢也不能错挂）"
                ));
            }
        }
        if let Some(min) = expect.min_duration_secs {
            let span = duration_secs(activity.start, activity.end);
            if span < min {
                return Err(format!("{where_} 时长 {span}s 短于期望下界 {min}s"));
            }
        }
        if let Some(max) = expect.max_duration_secs {
            let span = duration_secs(activity.start, activity.end);
            if span > max {
                return Err(format!("{where_} 时长 {span}s 超过期望上界 {max}s"));
            }
        }
        if let Some(modified) = expect.is_user_modified {
            if activity.is_user_modified != modified {
                return Err(format!(
                    "{where_} is_user_modified 期望 {modified}，实际 {}",
                    activity.is_user_modified
                ));
            }
        }
    }

    if let Some(noise) = &scenario.expect.noise {
        let mut actual = projection.noise.clone();
        actual.sort();
        let mut wanted = noise.clone();
        wanted.sort();
        if actual != wanted {
            return Err(format!("噪声观测期望 {wanted:?}，实际 {actual:?}"));
        }
    }

    if let Some(exact) = scenario.expect.provider_calls.vision {
        if projection.ai_requests != exact {
            return Err(format!(
                "VLM 调用次数期望恰好 {exact}，实际 {}（规则命中就该一次都不调）",
                projection.ai_requests
            ));
        }
    }
    if let Some(at_least) = scenario.expect.provider_calls.vision_at_least {
        if projection.ai_requests < at_least {
            return Err(format!(
                "VLM 调用次数期望至少 {at_least}，实际 {}",
                projection.ai_requests
            ));
        }
    }

    if let Some(stages) = &scenario.expect.stages {
        verify_stages(scenario, outcome, stages)?;
    }
    if let Some(summaries) = &scenario.expect.summaries {
        verify_summaries(scenario, outcome, summaries)?;
    }

    Ok(())
}

fn verify_stages(
    scenario: &Scenario,
    outcome: &ScenarioOutcome,
    expect: &StageExpectations,
) -> Result<(), String> {
    let closed = &outcome.stages.closed;
    let total = closed.len() + usize::from(outcome.stages.open.is_some());

    if let Some(expected) = expect.count_total {
        if total != expected {
            return Err(format!(
                "阶段数期望 {expected}，实际 {total}（阶段：{}｜活动：{}）",
                describe_stages(&outcome.stages),
                describe_activities(&outcome.projection)
            ));
        }
    }

    if expect.items.len() > closed.len() {
        return Err(format!(
            "期望断言 {} 个已结束阶段，实际只有 {}（{}）",
            expect.items.len(),
            closed.len(),
            describe_stages(&outcome.stages)
        ));
    }

    for (index, item) in expect.items.iter().enumerate() {
        let StageEffect::Closed {
            id,
            start,
            end,
            reason,
            activities,
        } = &closed[index]
        else {
            continue;
        };
        let where_ = format!("第 {} 个已结束阶段", index + 1);

        if let Some(expected) = &item.id {
            if id != expected {
                return Err(format!("{where_} id 期望 `{expected}`，实际 `{id}`"));
            }
        }
        if let Some(expected) = &item.end_reason {
            let actual = reason_str(*reason);
            if actual != expected {
                return Err(format!(
                    "{where_} 结束原因期望 `{expected}`，实际 `{actual}`"
                ));
            }
        }
        if let Some(expected) = &item.activities {
            let mut actual = activities.clone();
            actual.sort();
            let mut wanted = expected.clone();
            wanted.sort();
            if actual != wanted {
                return Err(format!("{where_} 活动归属期望 {wanted:?}，实际 {actual:?}"));
            }
        }

        let span = (end.saturating_diff_millis(*start).max(0) / 1000) as u64;
        if let Some(min) = item.min_duration_secs {
            if span < min {
                return Err(format!("{where_} 时长 {span}s 短于期望下界 {min}s"));
            }
        }
        if let Some(max) = item.max_duration_secs {
            if span > max {
                return Err(format!("{where_} 时长 {span}s 超过期望上界 {max}s"));
            }
        }
    }

    if let Some(expected_open) = expect.open {
        if expected_open != outcome.stages.open.is_some() {
            return Err(format!(
                "期望{}进行中的阶段，实际{}（{}）",
                if expected_open { "有" } else { "没有" },
                if outcome.stages.open.is_some() {
                    "有"
                } else {
                    "没有"
                },
                describe_stages(&outcome.stages)
            ));
        }
    }

    // 阶段区间不得重叠（不变量，任何场景都要满足）
    let mut intervals: Vec<(Timestamp, Timestamp)> = closed
        .iter()
        .filter_map(|effect| match effect {
            StageEffect::Closed { start, end, .. } => Some((*start, *end)),
            StageEffect::Opened { .. } => None,
        })
        .collect();
    intervals.sort();
    for pair in intervals.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(format!("阶段区间重叠：{:?} 与 {:?}", pair[0], pair[1]));
        }
    }

    let _ = scenario;
    Ok(())
}

/// 总结断言：用**确定性兜底**渲染，不调用模型。
fn verify_summaries(
    scenario: &Scenario,
    outcome: &ScenarioOutcome,
    expect: &SummaryExpectations,
) -> Result<(), String> {
    let policy = stage_policy(&scenario.config.stage);
    let template = mc_summary::template::SummaryTemplate::default_work_stage();

    let mut long_stages = 0usize;
    let mut rendered = 0usize;

    for effect in &outcome.stages.closed {
        let StageEffect::Closed {
            start,
            end,
            activities,
            ..
        } = effect
        else {
            continue;
        };

        let span = (end.saturating_diff_millis(*start).max(0) / 1000) as u64;
        if span < policy.min_duration_secs {
            continue;
        }
        long_stages += 1;

        // 只有落到实际活动上的阶段才有内容可写；空阶段在这里被如实跳过
        let digests: Vec<mc_summary::model::ActivityDigest> = outcome
            .projection
            .activities
            .iter()
            .filter(|activity| activities.contains(&activity.id))
            .map(|activity| mc_summary::model::ActivityDigest {
                id: activity.id.clone(),
                title: activity.title.clone(),
                category: activity.category.clone(),
                start: activity.start,
                end: activity.end,
                observations: activity.observations.len() as u32,
                inferred: activity.origin.is_inferred(),
            })
            .collect();

        let input = mc_summary::model::StageSummaryInput {
            stage_id: String::new(),
            range: mc_summary::model::SummaryRange {
                start: *start,
                end: *end,
            },
            timezone: policy.timezone.clone(),
            locale: mc_summary::model::SummaryLocale::ZhCn,
            activities: digests,
            observation_count: 0,
            blocked_observations: 0,
        };

        let summary = mc_summary::fallback::generate(&input, &template);
        if summary.body_markdown.trim().is_empty() {
            return Err(format!(
                "阶段 {start}–{end} 的兜底总结是空的 —— 「有阶段必有总结」被破坏"
            ));
        }
        if let Some(quality) = &expect.quality {
            let actual = match summary.quality {
                mc_summary::model::Quality::Model => "model",
                mc_summary::model::Quality::Fallback => "fallback",
            };
            if actual != quality {
                return Err(format!("总结质量期望 `{quality}`，实际 `{actual}`"));
            }
        }
        for fragment in &expect.body_contains {
            if !summary.body_markdown.contains(fragment) {
                return Err(format!(
                    "总结正文里缺少 `{fragment}`：{}",
                    summary.body_markdown
                ));
            }
        }
        rendered += 1;
    }

    if expect.every_long_stage_has_one == Some(true) && rendered != long_stages {
        return Err(format!(
            "有 {long_stages} 个够长的阶段，却只渲染出 {rendered} 份总结（活动：{}）",
            describe_activities(&outcome.projection)
        ));
    }
    if let Some(expected) = expect.long_stage_count {
        if long_stages != expected {
            return Err(format!(
                "够长的阶段数期望 {expected}，实际 {long_stages}（阶段：{}｜活动：{}）",
                describe_stages(&outcome.stages),
                describe_activities(&outcome.projection)
            ));
        }
    }

    Ok(())
}

fn describe_activities(projection: &Projection) -> String {
    projection
        .activities
        .iter()
        .map(|activity| {
            format!(
                "{} {}–{} {:?}",
                activity.title,
                activity.start.to_rfc3339(),
                activity.end.to_rfc3339(),
                activity.observation_ids()
            )
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn describe_stages(replay: &StageReplay) -> String {
    let mut parts: Vec<String> = replay
        .closed
        .iter()
        .filter_map(|effect| match effect {
            StageEffect::Closed {
                start,
                end,
                reason,
                activities,
                ..
            } => Some(format!(
                "{start:?}–{end:?} {} {activities:?}",
                reason_str(*reason)
            )),
            StageEffect::Opened { .. } => None,
        })
        .collect();
    if let Some(open) = &replay.open {
        parts.push(format!("进行中 {} {:?}", open.start, open.activities));
    }
    parts.join(" | ")
}

fn reason_str(reason: mc_domain::stage::EndReason) -> &'static str {
    use mc_domain::stage::EndReason;
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

fn describe(activities: &[mc_domain::activity::ActivityView]) -> String {
    activities
        .iter()
        .map(|activity| format!("{}[{:?}]", activity.title, activity.observation_ids()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn origin_kind(origin: &Provenance) -> &'static str {
    match origin {
        Provenance::Observed => "observed",
        Provenance::Rule { .. } => "rule",
        Provenance::Inferred { .. } => "inferred",
    }
}

fn duration_secs(start: Timestamp, end: Timestamp) -> u64 {
    (end.saturating_diff_millis(start).max(0) / 1000) as u64
}

/// `60s` / `5min` / `2h`。写错单位要**报错**，不能当成 0 ——
/// 把 `600` 当成 0 秒会让场景静默通过，而没有比这更糟的测试问题。
fn parse_duration(raw: &str) -> Result<i64, String> {
    let trimmed = raw.trim();
    let (digits, unit) = trimmed.split_at(
        trimmed
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(trimmed.len()),
    );
    let value: i64 = digits
        .parse()
        .map_err(|_| format!("无法解析时长 `{raw}`"))?;

    let multiplier = match unit.trim() {
        "s" | "sec" | "secs" | "秒" => 1,
        "m" | "min" | "mins" | "分" => 60,
        "h" | "hour" | "hours" | "小时" => 3600,
        other => return Err(format!("无法识别的时间单位 `{other}`（支持 s/min/h）")),
    };
    Ok(value * multiplier)
}

/// 把场景里内联的规则转成 `RuleSet`。
///
/// 走一次「序列化回 YAML 再交给 `RuleSet::parse_yaml`」：
/// 规则的匹配语义只能有一份实现，测试与生产必须共用。
fn parse_rules(specs: &[RuleSpec]) -> Result<RuleSet, String> {
    if specs.is_empty() {
        return Ok(RuleSet::default());
    }

    let mut activities = Vec::new();
    for spec in specs {
        activities.push(serde_yaml::to_value(spec).map_err(|error| error.to_string())?);
    }
    let document = serde_yaml::to_string(&serde_yaml::Value::Mapping(
        [
            (
                serde_yaml::Value::String("version".to_string()),
                serde_yaml::Value::Number(1.into()),
            ),
            (
                serde_yaml::Value::String("activities".to_string()),
                serde_yaml::Value::Sequence(activities),
            ),
        ]
        .into_iter()
        .collect(),
    ))
    .map_err(|error| error.to_string())?;

    RuleSet::parse_yaml(&document)
}
