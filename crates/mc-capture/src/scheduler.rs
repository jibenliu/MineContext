//! 采集调度：什么时候抓、抓哪些目标、下游忙不过来时怎么办。
//!
//! 全部是纯状态机：时间由调用方注入（`tick(now, signals)`），不读时钟、不 sleep、
//! 不做 IO，因此「漂移修正」「空闲降频」「队列满降频」都能确定性测试。
//!
//! 三条硬规则：**同一目标串行**（在途时不再抓同一个目标）、
//! **队列满时降频而不丢图**（已抓到的图必须留在队列里）、
//! **节拍按名义时刻推进**（下游再慢也不能让采集漂移）。

use mc_common::time::Timestamp;

#[derive(Debug, Clone, PartialEq)]
pub struct CapturePolicy {
    /// 正常采集间隔
    pub interval_secs: u64,
    /// 用户空闲时的采集间隔（降频，省电省磁盘）
    pub idle_interval_secs: u64,
    /// 连续空闲多久算「用户离开」
    pub idle_threshold_secs: u64,
    /// 待落盘/待分析队列的容量上限
    pub queue_capacity: usize,
    /// 单轮最多并行采集多少个目标
    pub max_parallel_targets: usize,
}

impl Default for CapturePolicy {
    fn default() -> Self {
        Self {
            // 与 mc-config 的 capture 默认值保持一致
            interval_secs: 15,
            idle_interval_secs: 60,
            idle_threshold_secs: 300,
            queue_capacity: 32,
            max_parallel_targets: 4,
        }
    }
}

/// 外部世界给调度器的信号。
///
/// `idle_for_secs` 是**已空闲时长**而不是布尔值：macOS 的
/// `CGEventSourceSecondsSinceLastEventType` 本来就直接给时长。
/// 用布尔值会丢掉「刚刚空闲」与「已经空闲一小时」的区别，
/// 于是调度器无法在第一次收到「空闲」信号时就正确降频。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureSignals {
    pub locked: bool,
    pub suspended: bool,
    /// 用户已经空闲了多少秒（0 = 有输入）
    pub idle_for_secs: u64,
}

impl CaptureSignals {
    pub const RUNNING: Self = Self {
        locked: false,
        suspended: false,
        idle_for_secs: 0,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    NotDue,
    Locked,
    Suspended,
    QueueFull,
    NoTargets,
    AllInflight,
}

/// 状态变化：供 Stage 引擎决定是否强制结束阶段。
///
/// 隔夜/跨睡眠不能被算进同一个阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateChange {
    /// 从锁屏/睡眠恢复，`away_for_secs` 是离开时长
    Resumed { away_for_secs: u64 },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TickOutcome {
    /// 本 tick 启动采集的目标
    pub started: Vec<String>,
    pub skipped: Option<SkipReason>,
    /// 因为下游队列满而降频
    pub throttled: bool,
    pub state_change: Option<StateChange>,
    /// 本次生效的采集间隔（空闲时会变大）
    pub interval_secs: u64,
}

#[derive(Debug, Clone)]
pub struct CaptureScheduler {
    policy: CapturePolicy,
    queue_capacity: usize,
    targets: Vec<String>,
    next_due: Option<Timestamp>,
    inflight: Vec<String>,
    pending: usize,
    last_idle_secs: u64,
    last_seen: Option<Timestamp>,
    away_since: Option<Timestamp>,
    throttled_total: u64,
    dropped_total: u64,
}

impl CaptureScheduler {
    pub fn new(policy: CapturePolicy) -> Self {
        let queue_capacity = policy.queue_capacity;
        Self {
            policy,
            queue_capacity,
            targets: Vec::new(),
            next_due: None,
            inflight: Vec::new(),
            pending: 0,
            last_idle_secs: 0,
            last_seen: None,
            away_since: None,
            throttled_total: 0,
            dropped_total: 0,
        }
    }

    pub fn set_targets(&mut self, targets: Vec<String>) {
        self.targets = targets;
        self.inflight.retain(|id| self.targets.contains(id));
    }

    pub fn set_queue_capacity(&mut self, capacity: usize) {
        self.queue_capacity = capacity.max(1);
    }

    pub fn next_due(&self) -> Option<Timestamp> {
        self.next_due
    }

    pub fn pending_count(&self) -> usize {
        self.pending
    }

    pub fn throttled_total(&self) -> u64 {
        self.throttled_total
    }

    /// 只要大于 0 就说明有观测被丢弃 —— 这是不允许发生的事，专门暴露出来做哨兵。
    pub fn dropped_total(&self) -> u64 {
        self.dropped_total
    }

    pub fn is_idle(&self) -> bool {
        self.last_idle_secs >= self.policy.idle_threshold_secs
    }

    /// 最近一次观测到的空闲时长。
    pub fn idle_for_secs(&self) -> u64 {
        self.last_idle_secs
    }

    /// 该目标采集完成（图像已拿到），释放「在途」标记。
    ///
    /// 注意：**不会**释放队列槽位 —— 槽位在启动采集时就已预留，
    /// 由 [`Self::drain_pending`] 在落盘完成后释放。
    pub fn mark_captured(&mut self, target_id: &str) {
        self.inflight.retain(|id| id != target_id);
    }

    /// 下游消费完成（已落盘/已处理），释放队列槽位。
    pub fn drain_pending(&mut self, count: usize) {
        self.pending = self.pending.saturating_sub(count);
    }

    pub fn tick(&mut self, now: Timestamp, signals: &CaptureSignals) -> TickOutcome {
        let mut outcome = TickOutcome::default();
        self.last_seen = Some(now);
        self.last_idle_secs = signals.idle_for_secs;

        // ---- 锁屏 / 睡眠：不采集，并且复位调度，避免恢复时补采 ----
        if signals.locked || signals.suspended {
            if self.away_since.is_none() {
                self.away_since = Some(now);
            }
            self.next_due = None;
            outcome.skipped = Some(if signals.locked {
                SkipReason::Locked
            } else {
                SkipReason::Suspended
            });
            outcome.interval_secs = self.current_interval_secs();
            return outcome;
        }

        // ---- 恢复 ----
        if let Some(away_since) = self.away_since.take() {
            let away_for_secs = (now.saturating_diff_millis(away_since).max(0) / 1000) as u64;
            outcome.state_change = Some(StateChange::Resumed { away_for_secs });
        }

        // ---- 到期判定 ----
        let due = self.next_due.unwrap_or(now);
        if now < due {
            outcome.skipped = Some(SkipReason::NotDue);
            outcome.interval_secs = self.current_interval_secs();
            return outcome;
        }

        if self.targets.is_empty() {
            outcome.skipped = Some(SkipReason::NoTargets);
            self.advance_schedule(now, due);
            outcome.interval_secs = self.current_interval_secs();
            return outcome;
        }

        // ---- 背压：满了就不启动新的抓取（不丢已抓到的） ----
        let free = self.queue_capacity.saturating_sub(self.pending);
        if free == 0 {
            self.throttled_total += 1;
            outcome.throttled = true;
            outcome.skipped = Some(SkipReason::QueueFull);
            self.advance_schedule(now, due);
            outcome.interval_secs = self.current_interval_secs();
            return outcome;
        }

        // ---- 选目标：跳过仍在途的 ----
        let available: Vec<String> = self
            .targets
            .iter()
            .filter(|id| !self.inflight.contains(id))
            .cloned()
            .collect();

        if available.is_empty() {
            outcome.skipped = Some(SkipReason::AllInflight);
            self.advance_schedule(now, due);
            outcome.interval_secs = self.current_interval_secs();
            return outcome;
        }

        let budget = free.min(self.policy.max_parallel_targets);
        for id in available.into_iter().take(budget) {
            self.inflight.push(id.clone());
            // 槽位在「启动采集」时就预留：因为图已经要抓了，不能因为队列满而丢
            self.pending += 1;
            outcome.started.push(id);
        }

        self.advance_schedule(now, due);
        outcome.interval_secs = self.current_interval_secs();
        outcome
    }

    fn current_interval_secs(&self) -> u64 {
        if self.is_idle() {
            self.policy.idle_interval_secs
        } else {
            self.policy.interval_secs
        }
    }

    /// 按**名义时刻**推进下次到期时间，从而不累积漂移。
    ///
    /// 若已经落后超过一个完整间隔（例如进程被长时间挂起），则重新对齐到 `now`，
    /// 避免解锁瞬间的「追赶风暴」把队列打爆。
    fn advance_schedule(&mut self, now: Timestamp, due: Timestamp) {
        let interval_ms = (self.current_interval_secs() * 1000) as i64;
        let mut next = due.plus_millis(interval_ms);

        if next <= now {
            let behind = now.saturating_diff_millis(next);
            if behind >= interval_ms * 2 {
                next = now.plus_millis(interval_ms);
            } else {
                while next <= now {
                    next = next.plus_millis(interval_ms);
                }
            }
        }

        self.next_due = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_matches_config_defaults() {
        let policy = CapturePolicy::default();
        assert_eq!(policy.interval_secs, 15);
        assert_eq!(policy.idle_threshold_secs, 300);
        assert_eq!(policy.queue_capacity, 32);
    }

    #[test]
    fn queue_capacity_is_never_zero() {
        let mut scheduler = CaptureScheduler::new(CapturePolicy::default());
        scheduler.set_queue_capacity(0);
        scheduler.set_targets(vec!["a".to_string()]);

        let outcome = scheduler.tick(Timestamp::from_millis(0), &CaptureSignals::RUNNING);
        // 容量被抬到 1，因此仍能抓一帧（而不是永久停摆）
        assert_eq!(outcome.started.len(), 1);
    }
}
