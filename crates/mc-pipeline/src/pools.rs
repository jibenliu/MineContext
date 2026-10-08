//! Vision / Summary 双池与全局 Provider 闸门。
//!
//! **按职责物理分池**：图片积压不得让总结排队 —— 单池设计下，用户点
//! 「总结最近 2 小时」会排在几百张图之后。vision 只能使用
//! `limit - reserved` 的额度，summary 有专属名额，vision 永不借用。
//!
//! 这里只有调度逻辑、没有真实任务与 IO，因此「图片积压 500 项时总结仍在
//! SLA 内开始」这条不变量可以确定性验证，而不是靠压测碰运气。

use std::collections::VecDeque;

use mc_common::time::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskClass {
    Vision,
    Summary,
}

/// 优先级。数值越小越先执行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// 用户当场点的「总结这段时间」
    Interactive,
    /// 阶段关闭后自动产出的总结
    StageSummary,
    /// 补算历史（可以等）
    Backfill,
}

impl Priority {
    const fn rank(self) -> i64 {
        match self {
            Self::Interactive => 0,
            Self::StageSummary => 1,
            Self::Backfill => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolConfig {
    pub vision_capacity: usize,
    pub summary_capacity: usize,
    /// Provider 侧的总并发上限（vision + summary 之和）
    pub global_provider_limit: usize,
    /// 全局闸门里给 summary 保留的名额：vision 永远吃不掉它
    pub global_reserved_for_summary: usize,
    /// summary 池里给交互式任务保留的名额
    pub interactive_reserved_summary_slots: usize,
    /// 等待超过这个时长就提升一级优先级（防饿死）
    pub aging_after_secs: u64,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            // 与 mc-config 的默认值保持一致
            vision_capacity: 2,
            summary_capacity: 2,
            global_provider_limit: 4,
            global_reserved_for_summary: 1,
            interactive_reserved_summary_slots: 1,
            aging_after_secs: 30,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolTask {
    pub id: String,
    pub class: TaskClass,
    pub priority: Priority,
    pub enqueued_at: Timestamp,
    /// 权重：一次请求可能占多个名额（例如长总结的归并阶段）
    pub weight: u32,
}

impl PoolTask {
    pub fn new(
        id: impl Into<String>,
        class: TaskClass,
        priority: Priority,
        enqueued_at: Timestamp,
    ) -> Self {
        Self {
            id: id.into(),
            class,
            priority,
            enqueued_at,
            weight: 1,
        }
    }

    pub fn with_weight(mut self, weight: u32) -> Self {
        self.weight = weight.max(1);
        self
    }

    /// 计入 aging 之后的实际优先级（越小越先）。
    fn effective_rank(&self, now: Timestamp, aging_after_secs: u64) -> i64 {
        if aging_after_secs == 0 {
            return self.priority.rank();
        }
        let waited = (now.saturating_diff_millis(self.enqueued_at).max(0) / 1000) as u64;
        let promotions = (waited / aging_after_secs) as i64;
        // 最多提升到最前面，避免「等得越久越无限优先」把新任务全挡住
        (self.priority.rank() - promotions).max(0)
    }
}

pub struct PoolScheduler {
    config: PoolConfig,
    waiting: VecDeque<PoolTask>,
    running: Vec<PoolTask>,
}

impl PoolScheduler {
    pub fn new(config: PoolConfig) -> Self {
        Self {
            config,
            waiting: VecDeque::new(),
            running: Vec::new(),
        }
    }

    pub fn config(&self) -> &PoolConfig {
        &self.config
    }

    pub fn submit(&mut self, task: PoolTask) {
        self.waiting.push_back(task);
    }

    /// 尽量放行等待中的任务，返回本次新放行的任务。
    ///
    /// 每次只从「当前可放行的任务」里挑**最优**的一个，直到再也放不进 ——
    /// 顺序确定，因此测试不会出现「有时通过有时不通过」。
    pub fn admit(&mut self, now: Timestamp) -> Vec<PoolTask> {
        let mut admitted = Vec::new();

        while let Some(index) = self.next_admissible(now) {
            let task = self
                .waiting
                .remove(index)
                .expect("next_admissible 返回的下标一定有效");
            self.running.push(task.clone());
            admitted.push(task);
        }

        admitted
    }

    /// 在等待队列里挑下一个可放行的任务（按 aging 后的优先级 + 入队顺序）。
    fn next_admissible(&self, now: Timestamp) -> Option<usize> {
        let mut best: Option<(usize, i64, i64)> = None;

        for (index, task) in self.waiting.iter().enumerate() {
            if !self.can_admit(task) {
                continue;
            }
            let rank = task.effective_rank(now, self.config.aging_after_secs);
            let key = (rank, task.enqueued_at.as_millis());
            match best {
                Some((_, best_rank, best_at)) if (best_rank, best_at) <= key => {}
                _ => best = Some((index, key.0, key.1)),
            }
        }

        best.map(|(index, _, _)| index)
    }

    /// 某个池**按权重**计的在途量。
    ///
    /// 容量必须按权重比较：一个 weight=2 的作业占两个名额。
    /// 按任务个数比较的话，两个 weight=2 的作业会一起挤进容量为 4 的池 ——
    /// 看起来「没超」，按权重算其实已经超出容量。
    fn class_weight_inflight(&self, class: TaskClass) -> usize {
        self.running
            .iter()
            .filter(|running| running.class == class)
            .map(|running| running.weight.max(1) as usize)
            .sum()
    }

    fn can_admit(&self, task: &PoolTask) -> bool {
        let weight = task.weight.max(1) as usize;

        // 全局闸门
        if self.global_weight_inflight() + weight > self.config.global_provider_limit {
            return false;
        }

        match task.class {
            TaskClass::Vision => {
                if self.class_weight_inflight(TaskClass::Vision) + weight
                    > self.config.vision_capacity
                {
                    return false;
                }
                // vision 只能用「全局名额 - 给 summary 保留的名额」。
                // 这条规则是「图片再积压也吃不掉摘要名额」的落点。
                let vision_global_limit = self
                    .config
                    .global_provider_limit
                    .saturating_sub(self.config.global_reserved_for_summary);
                if self.class_weight_inflight(TaskClass::Vision) + weight > vision_global_limit {
                    return false;
                }
                true
            }
            TaskClass::Summary => {
                let capacity = if task.priority == Priority::Interactive {
                    self.config.summary_capacity
                } else {
                    // 非交互任务不能占用给交互式保留的名额
                    self.config
                        .summary_capacity
                        .saturating_sub(self.config.interactive_reserved_summary_slots)
                        .max(1)
                };
                self.class_weight_inflight(TaskClass::Summary) + weight <= capacity
            }
        }
    }

    pub fn complete(&mut self, id: &str) {
        if let Some(index) = self.running.iter().position(|task| task.id == id) {
            self.running.remove(index);
        }
    }

    pub fn inflight(&self, class: TaskClass) -> usize {
        self.running
            .iter()
            .filter(|task| task.class == class)
            .count()
    }

    pub fn global_inflight(&self) -> usize {
        self.running.len()
    }

    /// 按**权重**计的全局在途量（provider 侧真正关心的数字）。
    pub fn global_weight_inflight(&self) -> usize {
        self.running
            .iter()
            .map(|task| task.weight.max(1) as usize)
            .sum()
    }

    pub fn queue_depth(&self, class: TaskClass) -> usize {
        self.waiting
            .iter()
            .filter(|task| task.class == class)
            .count()
    }

    pub fn running_ids(&self, class: TaskClass) -> Vec<String> {
        self.running
            .iter()
            .filter(|task| task.class == class)
            .map(|task| task.id.clone())
            .collect()
    }

    /// 诊断快照：给 `/api/diagnostics` 的队列水位用。
    pub fn snapshot(&self) -> PoolSnapshot {
        PoolSnapshot {
            vision_depth: self.queue_depth(TaskClass::Vision),
            vision_inflight: self.inflight(TaskClass::Vision),
            summary_depth: self.queue_depth(TaskClass::Summary),
            summary_inflight: self.inflight(TaskClass::Summary),
            global_inflight: self.global_inflight(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolSnapshot {
    pub vision_depth: usize,
    pub vision_inflight: usize,
    pub summary_depth: usize,
    pub summary_inflight: usize,
    pub global_inflight: usize,
}
