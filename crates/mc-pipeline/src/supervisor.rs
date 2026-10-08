//! 池调度器的执行层：把调度决策变成真实的异步任务。
//!
//! `pools.rs` 只回答「该放行谁」，这一段负责「真的放行」：`submit` 入队，
//! `PoolScheduler::admit` 放行，任务完成后 `complete(id)` 归还容量。
//! 不让调用方各自 `tokio::spawn` 再自己数并发，是因为那样「并发上限」就只是
//! 各处的约定，迟早有人漏掉一处。
//!
//! **单持有者**模型（`submit` 需要 `&mut self`）：调用方在自己的循环里
//! 「先提交、再 `run_until_idle`」。跨任务提交（HTTP 交互式总结）需要另设同步，
//! 不能直接复用这条签名 —— 多一层为它准备的结构只会变成没人走的代码。

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use mc_common::time::{Clock, SystemClock};
use tokio::task::JoinSet;

use crate::pools::{PoolConfig, PoolScheduler, PoolSnapshot, PoolTask, Priority, TaskClass};

type JobFuture = BoxFuture<'static, ()>;

pub struct PoolSupervisor {
    scheduler: PoolScheduler,
    /// 待放行作业的**执行体**（调度器只记元数据）
    pending: HashMap<String, JobFuture>,
    running: JoinSet<String>,
    clock: Arc<dyn Clock>,
}

impl PoolSupervisor {
    pub fn new(config: PoolConfig) -> Self {
        Self {
            scheduler: PoolScheduler::new(config),
            pending: HashMap::new(),
            running: JoinSet::new(),
            clock: Arc::new(SystemClock),
        }
    }

    /// 注入时钟（测试用假时钟推进 aging）。
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// 提交一个作业。`work` 只有在调度器放行后才会开始执行。
    pub fn submit<F, Fut>(
        &mut self,
        id: impl Into<String>,
        class: TaskClass,
        priority: Priority,
        weight: u32,
        work: F,
    ) where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let id = id.into();
        self.scheduler.submit(PoolTask {
            id: id.clone(),
            class,
            priority,
            enqueued_at: self.clock.now(),
            weight: weight.max(1),
        });
        self.pending.insert(id, work().boxed());
    }

    pub fn snapshot(&self) -> PoolSnapshot {
        self.scheduler.snapshot()
    }

    /// 跑到「队列空 + 没有在跑的任务」为止。
    pub async fn run_until_idle(&mut self) {
        loop {
            self.pump();
            if self.running.is_empty() {
                return;
            }
            self.step().await;
        }
    }

    /// 放行尽可能多的作业，并把它们变成真实任务。
    fn pump(&mut self) {
        let now = self.clock.now();
        for task in self.scheduler.admit(now) {
            let Some(work) = self.pending.remove(&task.id) else {
                // 没有执行体说明调用方提交时漏了；直接标记完成，避免队列卡死
                self.scheduler.complete(&task.id);
                continue;
            };
            let id = task.id.clone();
            self.running.spawn(async move {
                work.await;
                id
            });
        }
    }

    /// 等一个作业完成，然后让调度器回收容量。
    async fn step(&mut self) {
        if let Some(Ok(id)) = self.running.join_next().await {
            self.scheduler.complete(&id);
        }
    }
}
