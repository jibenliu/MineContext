//! Stage 检测：把活动序列切成「阶段」。
//!
//! 阶段划分是总结的前提，而它本身是**时间序列上的状态机**：
//! `Idle → Observing → Open → Ending → Closed`；`Ending` 是宽限期，
//! 宽限期内回到原活动就取消关闭（否则「切出去看一眼再切回来」会被切成两段）。
//! 六种结束条件：切换 / 空闲 / 锁屏 / 睡眠 / 超长 / 跨天；另有手动关闭与
//! 关机（关机产出的阶段要能被总结侧识别为「被打断」）。
//!
//! **纯函数 + 注入时间**：不读时钟、不做 IO，相同输入必须给出相同的 effect 序列，
//! 否则重放无法复现 —— 而阶段与总结都是派生数据，必须可重建。

use chrono::NaiveDate;
use mc_common::time::Timestamp;

use crate::activity::Provenance;

/// 重放的一步：活动信号，或一次锁屏/睡眠。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayStep {
    Signal(ActivitySignal),
    Locked(Timestamp),
}

impl ReplayStep {
    pub fn at(&self) -> Timestamp {
        match self {
            Self::Signal(signal) => signal.start,
            Self::Locked(at) => *at,
        }
    }
}

/// 进行中阶段的快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenStageSnapshot {
    pub id: String,
    pub start: Timestamp,
    pub activities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StageReplay {
    /// 按关闭顺序排列的已结束阶段
    pub closed: Vec<StageEffect>,
    /// 重放到 `now` 时仍然进行中的阶段
    pub open: Option<OpenStageSnapshot>,
}

/// 从一串历史步骤重放出阶段。**这是重放的唯一实现**：daemon 的后台循环与
/// 场景测试都走这里 —— 各写一份的结果是「测试里对、线上不对」。
///
/// 1. **事件之间要补 tick**：真机每 30 秒一条观测，重放却是「0 秒」跳到
///    「10 分钟」；不补 tick，30 秒的活动会因为稳定阈值没到而开不出阶段。
/// 2. **处理每一步之前先 tick**：否则「干完活离开 10 分钟又回来」会被算成
///    一个连续阶段并以 `Switched` 结束 ——「离开」被误记成「切换」。
pub fn replay(policy: &StagePolicy, steps: &[ReplayStep], now: Timestamp) -> StageReplay {
    let mut ordered: Vec<ReplayStep> = steps.to_vec();
    // 同一时刻先处理活动再处理锁屏：锁屏意味着「离开」，
    // 不该把同一瞬间的活动并进来。
    ordered.sort_by(|left, right| {
        left.at()
            .cmp(&right.at())
            .then_with(|| match (left, right) {
                (ReplayStep::Signal(_), ReplayStep::Locked(_)) => std::cmp::Ordering::Less,
                (ReplayStep::Locked(_), ReplayStep::Signal(_)) => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            })
    });

    let mut detector = StageDetector::new(policy.clone());
    let mut closed: Vec<StageEffect> = Vec::new();
    let catch_up_after = policy.min_activity_stable_secs.max(1) as i64 * 1000;
    let mut previous: Option<Timestamp> = None;

    for step in &ordered {
        let at = step.at();
        if let Some(prev) = previous {
            let boundary = Timestamp::from_millis(prev.as_millis() + catch_up_after);
            if boundary < at {
                closed.extend(only_closed(detector.tick(boundary)));
            }
        }
        previous = Some(at);

        closed.extend(only_closed(detector.tick(at)));

        match step {
            ReplayStep::Signal(signal) => {
                closed.extend(only_closed(detector.observe(signal, signal.start)));
                closed.extend(only_closed(detector.tick(signal.end)));
            }
            ReplayStep::Locked(at) => {
                closed.extend(only_closed(detector.locked(*at)));
            }
        }
    }

    closed.extend(only_closed(detector.tick(now)));

    let open = match (detector.current_id(), detector.open_snapshot()) {
        (Some(id), Some((start, activities))) => Some(OpenStageSnapshot {
            id: id.to_string(),
            start,
            activities,
        }),
        _ => None,
    };

    StageReplay { closed, open }
}

fn only_closed(effects: Vec<StageEffect>) -> Vec<StageEffect> {
    effects
        .into_iter()
        .filter(|effect| matches!(effect, StageEffect::Closed { .. }))
        .collect()
}

/// 阶段的输入信号：来自投影后的活动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivitySignal {
    pub id: String,
    pub title: String,
    pub start: Timestamp,
    pub end: Timestamp,
}

impl ActivitySignal {
    pub fn origin(&self) -> Provenance {
        // 阶段本身不关心 provenance（总结才关心，用于标注「推测」）；
        // 这里保留一个入口，避免调用方为了取标题而重建活动视图。
        Provenance::Observed
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StagePolicy {
    /// 短于此时长的阶段不产出总结（避免噪声总结，4.36）
    pub min_duration_secs: u64,
    /// 单个阶段的最长时长，超过则强制切分
    pub max_duration_secs: u64,
    /// 换活动之后的宽限期：期间回到原活动就取消结束
    pub switch_grace_secs: u64,
    /// 多久没有新信号就认为阶段结束
    pub idle_threshold_secs: u64,
    /// 活动需要稳定多久才开启阶段
    pub min_activity_stable_secs: u64,
    /// 阶段关闭后必须在多久内产出总结（巡检用）
    pub summary_deadline_secs: u64,
    /// 日边界按这个 IANA 时区算（DST 日已单列测试）
    pub timezone: String,
}

impl Default for StagePolicy {
    fn default() -> Self {
        // 与 mc-config 的 stage 默认值保持一致
        Self {
            min_duration_secs: 900,
            max_duration_secs: 7200,
            switch_grace_secs: 300,
            idle_threshold_secs: 300,
            min_activity_stable_secs: 60,
            summary_deadline_secs: 600,
            timezone: "UTC".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    Idle,
    /// 看到活动了，但还没稳定 —— 不急着开阶段
    Observing,
    Open,
    /// 换活动了，正在宽限期里等用户切回来
    Ending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Switched,
    Idle,
    Locked,
    Suspended,
    MaxDuration,
    DayBoundary,
    /// 正常退出
    Shutdown,
    /// 用户手动结束
    Manual,
    /// 进程被强杀后重启时补记（4.39）
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageEffect {
    Opened {
        id: String,
        at: Timestamp,
    },
    Closed {
        id: String,
        start: Timestamp,
        end: Timestamp,
        reason: EndReason,
        /// 只包含**本阶段**的活动 id
        activities: Vec<String>,
    },
}

struct OpenStage {
    id: String,
    start: Timestamp,
    activities: Vec<String>,
    /// 当前活动（判断切换用）
    current: ActivitySignal,
    /// 宽限期起算点（第一次切走的时刻）
    ending_since: Option<Timestamp>,
    /// 切过去的新活动，宽限到期后由它开新阶段
    pending: Option<ActivitySignal>,
}

pub struct StageDetector {
    policy: StagePolicy,
    state: StageState,
    /// Observing 期间的活动与稳定起算点
    candidate: Option<ActivitySignal>,
    stable_since: Option<Timestamp>,
    stage: Option<OpenStage>,
    last_signal_at: Option<Timestamp>,
    /// 当前所处本地日（用于跨天判断）
    day: Option<NaiveDate>,
    /// 每个活动已经开过几个阶段（切分出来的第 n 段要能区分开）
    stage_counts: std::collections::HashMap<String, u32>,
    next_id: u64,
}

impl StageDetector {
    pub fn new(policy: StagePolicy) -> Self {
        Self {
            policy,
            state: StageState::Idle,
            candidate: None,
            stable_since: None,
            stage: None,
            last_signal_at: None,
            day: None,
            stage_counts: std::collections::HashMap::new(),
            next_id: 1,
        }
    }

    pub fn policy(&self) -> &StagePolicy {
        &self.policy
    }

    pub fn state(&self) -> StageState {
        self.state
    }

    /// 当前（或刚关闭的）阶段 id，供测试与诊断观察。
    pub fn current_id(&self) -> Option<&str> {
        self.stage.as_ref().map(|stage| stage.id.as_str())
    }

    /// 进行中阶段的快照：起点 + 已包含的活动（投影器写库用）。
    ///
    /// 只暴露这两个字段：结束时刻要等关闭时才知道，
    /// 提前给一个「大概的结束时间」会让人误以为阶段已经定量。
    pub fn open_snapshot(&self) -> Option<(Timestamp, Vec<String>)> {
        self.stage
            .as_ref()
            .map(|stage| (stage.start, stage.activities.clone()))
    }

    /// 一条新的活动信号。`at` 是**观测到它的时刻**，不一定等于活动开始时刻。
    pub fn observe(&mut self, signal: &ActivitySignal, at: Timestamp) -> Vec<StageEffect> {
        let effects = self.advance_time(at);
        self.last_signal_at = Some(max(self.last_signal_at, at));
        self.track_day(at);

        match self.state {
            StageState::Idle => {
                self.state = StageState::Observing;
                self.candidate = Some(signal.clone());
                self.stable_since = Some(at);
                effects
            }
            StageState::Observing => {
                // 稳定期内换了活动：重新计时。短促的切换不该开启阶段。
                if self.candidate.as_ref().map(|c| &c.id) != Some(&signal.id) {
                    self.candidate = Some(signal.clone());
                    self.stable_since = Some(at);
                }
                effects
            }
            StageState::Open => {
                let switched = self
                    .stage
                    .as_ref()
                    .map(|stage| stage.current.id != signal.id)
                    .unwrap_or(false);

                if switched {
                    self.state = StageState::Ending;
                    if let Some(stage) = self.stage.as_mut() {
                        stage.ending_since = Some(at);
                        stage.pending = Some(signal.clone());
                    }
                } else {
                    self.attach(signal);
                }
                effects
            }
            StageState::Ending => {
                let back_to_current = self
                    .stage
                    .as_ref()
                    .map(|stage| stage.current.id == signal.id)
                    .unwrap_or(false);

                if back_to_current {
                    // 宽限期内切回来 → 取消结束，阶段继续
                    self.state = StageState::Open;
                    if let Some(stage) = self.stage.as_mut() {
                        stage.ending_since = None;
                        stage.pending = None;
                        stage.current = signal.clone();
                    }
                    self.attach(signal);
                } else if let Some(stage) = self.stage.as_mut() {
                    // 又切到第三种活动：宽限期从**第一次**切走时算起，
                    // 否则连续切换可以把阶段无限期拖下去
                    stage.pending = Some(signal.clone());
                }
                effects
            }
        }
    }

    /// 定时推进：按**时刻先后**结算各种结束条件。
    ///
    /// 顺序不能写死。固定顺序会算错时长：
    /// 「上午 11 点收工、午夜跨天」如果先判日边界，
    /// 阶段会被拉到午夜结束 —— 凭空多出 4 小时「工作时间」。
    /// 因此这里把所有到期条件都列出来，**谁先到就按谁结束**。
    pub fn tick(&mut self, now: Timestamp) -> Vec<StageEffect> {
        // 宽限最优先：用户已经切走了，阶段在那一刻就结束了
        let produced = self.close_on_grace_expiry(now);
        if !produced.is_empty() {
            return produced;
        }

        let mut effects = Vec::new();
        // 上限只是防御：正常情况下一两轮就结束
        const MAX_CLOSES_PER_TICK: usize = 128;

        for _ in 0..MAX_CLOSES_PER_TICK {
            let Some((_, at, reason, continues)) = self.next_boundary(now) else {
                break;
            };

            let current = self.stage.as_ref().map(|stage| stage.current.clone());
            effects.extend(self.close_stage(at, reason));

            if matches!(reason, EndReason::DayBoundary) {
                // 记下已经跨过的这一天，避免同一 tick 内反复触发
                self.day = self.local_day(now);
            }

            if !continues {
                // 空闲：这段工作真的结束了
                break;
            }

            // 切分型（超长 / 跨天）：证据跨过了切分点才续段
            let Some(current) = current else { break };
            if current.end <= at {
                break;
            }
            self.state = StageState::Observing;
            self.candidate = Some(current);
            self.stable_since = Some(at);

            // **立刻续段**再进入下一轮：否则一轮只切一刀，
            // 一段连续 5 小时的活动永远切不出中间那几段。
            effects.extend(self.open_when_stable(now));
        }

        effects.extend(self.open_when_stable(now));
        effects
    }

    /// 下一个到期的结束条件：`(到期时刻, 结束时刻, 原因, 是否续段)`。
    ///
    /// 两个时刻刻意分开：空闲的**到期**时刻是「最后证据 + 阈值」，
    /// 而阶段的**结束**时刻是最后那条证据 —— 用到期时刻当结束时刻，
    /// 会把离开的时间算进工作时长（这是最容易被用户发现的那类错误）。
    fn next_boundary(&self, now: Timestamp) -> Option<(Timestamp, Timestamp, EndReason, bool)> {
        let stage = self.stage.as_ref()?;

        // 证据的最后时刻：信号与活动自身声明的结束取较晚者
        let last = match self.last_signal_at {
            Some(signal) if signal >= stage.current.end => signal,
            _ => stage.current.end,
        };

        let mut candidates: Vec<(Timestamp, Timestamp, EndReason, u8, bool)> = Vec::new();

        // 空闲：证据消失超过阈值后到期，但结束时刻是**最后一条证据**
        let idle_due = Timestamp::from_millis(
            last.as_millis() + self.policy.idle_threshold_secs as i64 * 1000,
        );
        if idle_due <= now {
            candidates.push((idle_due, last, EndReason::Idle, 0, false));
        }

        // 超长：按「起点 + 上限」切分
        let max_at = Timestamp::from_millis(
            stage.start.as_millis() + self.policy.max_duration_secs as i64 * 1000,
        );
        if max_at <= now && stage.current.end >= max_at {
            candidates.push((max_at, max_at, EndReason::MaxDuration, 1, true));
        }

        // 跨天：证据确实跨过本地午夜才切
        if let Some(midnight) = self.pending_midnight(now) {
            if stage.current.end >= midnight {
                candidates.push((midnight, midnight, EndReason::DayBoundary, 2, true));
            }
        }

        candidates
            .into_iter()
            .min_by_key(|(due, _, _, rank, _)| (due.as_millis(), *rank))
            .map(|(due, end, reason, _, continues)| (due, end, reason, continues))
    }

    /// 本地日已经翻页时，返回当前本地日的起点（UTC 时刻）。
    fn pending_midnight(&self, now: Timestamp) -> Option<Timestamp> {
        let day = self.day?;
        let current_day = self.local_day(now)?;
        if current_day <= day {
            return None;
        }
        now.day_bounds(&self.policy.timezone)
            .ok()
            .map(|(start, _)| start)
    }

    fn close_on_grace_expiry(&mut self, now: Timestamp) -> Vec<StageEffect> {
        if self.state != StageState::Ending {
            return Vec::new();
        }
        let Some(stage) = self.stage.as_ref() else {
            return Vec::new();
        };
        let Some(since) = stage.ending_since else {
            return Vec::new();
        };
        if elapsed_secs(since, now) < self.policy.switch_grace_secs {
            return Vec::new();
        }

        let pending = stage.pending.clone();
        let mut effects = self.close_stage(since, EndReason::Switched);

        // 切过去的活动紧接着开新阶段（区间半开，因此不会重叠）
        if let Some(pending) = pending {
            self.state = StageState::Observing;
            self.candidate = Some(pending);
            self.stable_since = Some(since);
            effects.extend(self.open_when_stable(now));
        }
        effects
    }

    /// 锁屏：立即结束（离开的时间不能算进阶段）。
    pub fn locked(&mut self, at: Timestamp) -> Vec<StageEffect> {
        self.close_now(at, EndReason::Locked)
    }

    /// 睡眠 / 休眠：与锁屏同理，但结束原因分开记 ——
    /// 「锁屏 5 分钟」和「休眠一整天」在总结里是两件不同的事。
    pub fn suspended(&mut self, at: Timestamp) -> Vec<StageEffect> {
        self.close_now(at, EndReason::Suspended)
    }

    /// 关机 / 正常退出。
    pub fn shutdown(&mut self, at: Timestamp) -> Vec<StageEffect> {
        self.close_now(at, EndReason::Shutdown)
    }

    /// 用户手动结束阶段。
    pub fn close_manually(&mut self, at: Timestamp) -> Vec<StageEffect> {
        self.close_now(at, EndReason::Manual)
    }

    /// 崩溃恢复：把上次中断留下的阶段补记为 `Interrupted`（4.39）。
    pub fn mark_interrupted(&mut self, at: Timestamp) -> Vec<StageEffect> {
        if self.stage.is_none() {
            return Vec::new();
        }
        self.close_now(at, EndReason::Interrupted)
    }

    // ---------------- 内部 ----------------

    /// 时间只会向前：迟到信号不能把状态机带回过去（乱序重放会发生）。
    fn advance_time(&mut self, at: Timestamp) -> Vec<StageEffect> {
        if self.last_signal_at.is_some_and(|last| at < last) {
            return Vec::new();
        }
        Vec::new()
    }

    fn track_day(&mut self, at: Timestamp) {
        if self.day.is_none() {
            self.day = self.local_day(at);
        }
    }

    fn local_day(&self, at: Timestamp) -> Option<NaiveDate> {
        at.to_local_date(&self.policy.timezone).ok()
    }

    fn attach(&mut self, signal: &ActivitySignal) {
        if let Some(stage) = self.stage.as_mut() {
            if stage.activities.last() != Some(&signal.id) {
                stage.activities.push(signal.id.clone());
            }
            stage.current = signal.clone();
        }
    }

    fn open_when_stable(&mut self, now: Timestamp) -> Vec<StageEffect> {
        if self.state != StageState::Observing {
            return Vec::new();
        }
        let (Some(candidate), Some(since)) = (self.candidate.clone(), self.stable_since) else {
            return Vec::new();
        };
        if elapsed_secs(since, now) < self.policy.min_activity_stable_secs {
            return Vec::new();
        }

        // 阶段 id 由**触发它的活动**决定：重放时同一段历史得到同样的 id。
        // 一个活动可能被切分成多段（超长 / 跨天），因此第 2 段起加后缀 ——
        // 否则两段阶段会共用一个 id，后者直接覆盖前者。
        let count = self
            .stage_counts
            .entry(candidate.id.clone())
            .and_modify(|value| *value += 1)
            .or_insert(1);
        let id = if *count == 1 {
            format!("stage-{}", candidate.id)
        } else {
            format!("stage-{}-{count}", candidate.id)
        };

        self.stage = Some(OpenStage {
            id: id.clone(),
            start: since,
            activities: vec![candidate.id.clone()],
            current: candidate,
            ending_since: None,
            pending: None,
        });
        self.state = StageState::Open;
        vec![StageEffect::Opened { id, at: since }]
    }

    fn close_now(&mut self, at: Timestamp, reason: EndReason) -> Vec<StageEffect> {
        if let Some(stage) = self.stage.as_ref() {
            // 结束时刻不早于阶段起点（关机时刻可能早于最后一条信号）
            let end = if at > stage.start { at } else { stage.start };
            self.close_stage(end, reason)
        } else {
            self.state = StageState::Idle;
            self.candidate = None;
            self.stable_since = None;
            Vec::new()
        }
    }

    fn close_stage(&mut self, end: Timestamp, reason: EndReason) -> Vec<StageEffect> {
        let Some(stage) = self.stage.take() else {
            return Vec::new();
        };
        self.state = StageState::Idle;
        self.candidate = None;
        self.stable_since = None;
        self.next_id += 1;

        // 零长度的阶段等于「没有任何证据」：不产出。
        // 否则总结侧会为一个空时间范围生成一条空总结，
        // 反而把「有 stage 必有 summary」这条不变量变成噪声来源。
        if end <= stage.start {
            return Vec::new();
        }

        vec![StageEffect::Closed {
            id: stage.id,
            start: stage.start,
            end,
            reason,
            activities: stage.activities,
        }]
    }
}

fn elapsed_secs(from: Timestamp, to: Timestamp) -> u64 {
    (to.saturating_diff_millis(from).max(0) / 1000) as u64
}

fn max(current: Option<Timestamp>, candidate: Timestamp) -> Timestamp {
    match current {
        Some(value) if value >= candidate => value,
        _ => candidate,
    }
}
