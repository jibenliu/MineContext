//! 异步总结作业。
//!
//! 长范围总结要跑好几块、可能几十秒，让 HTTP 请求一直挂着不好（客户端会超时，
//! UI 也没法显示进度）：`POST /adhoc {await:false}` 立刻返回 `job_id`，
//! 后台逐块生成并通过 SSE `summary:progress` 报进度，
//! `GET /adhoc/{job_id}` 取状态，`POST /adhoc/{job_id}/cancel` 取消
//! （已完成的块保留，可续跑）。
//!
//! **相同的并发请求只跑一个作业**：判据是「作用域指纹 + 范围」，且只在作业
//! 还在跑时合并 —— 已完成的范围应当走结果缓存。

use std::collections::HashMap;
use std::sync::Mutex;

use mc_common::observability::{self, info, warn};
use mc_common::time::Timestamp;
use serde::Serialize;

use mc_summary::adhoc::CancelFlag;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JobSnapshot {
    pub job_id: String,
    pub state: JobState,
    pub chunks_done: u32,
    pub chunks_total: u32,
    pub summary_id: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    /// 两个并发请求被合并成了一个作业（UI 可以据此提示「已在生成中」）
    pub deduped: bool,
}

struct JobEntry {
    id: String,
    key: String,
    state: JobState,
    chunks_done: u32,
    chunks_total: u32,
    summary_id: Option<String>,
    error: Option<String>,
    created_at: Timestamp,
    cancel: CancelFlag,
    deduped: bool,
}

#[derive(Default)]
struct Registry {
    jobs: HashMap<String, JobEntry>,
    /// 作用域指纹 → 仍在跑的作业 id
    in_flight: HashMap<String, String>,
    next: u64,
}

/// 作业登记表。进程内、线程安全；重启后丢失是**可接受**的 ——
/// 作业只是「正在算」，丢了重新发起即可（已完成的块还在库里）。
#[derive(Default)]
pub struct AdhocJobs {
    registry: Mutex<Registry>,
}

pub struct Submitted {
    pub job_id: String,
    pub cancel: CancelFlag,
    pub chunks_total: u32,
    pub deduped: bool,
}

impl AdhocJobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个新作业；同 key 的作业还在跑时直接复用。
    pub fn submit(&self, key: impl Into<String>, chunks_total: u32, at: Timestamp) -> Submitted {
        let key = key.into();
        let mut registry = mc_common::lock::lock_or_recover(&self.registry);

        if let Some(existing) = registry.in_flight.get(&key) {
            if let Some(entry) = registry.jobs.get(existing) {
                return Submitted {
                    job_id: entry.id.clone(),
                    cancel: entry.cancel.clone(),
                    chunks_total: entry.chunks_total,
                    deduped: true,
                };
            }
        }

        registry.next += 1;
        let id = format!("job-{}", registry.next);
        let cancel = CancelFlag::new();
        registry.in_flight.insert(key.clone(), id.clone());
        registry.jobs.insert(
            id.clone(),
            JobEntry {
                id: id.clone(),
                key,
                state: JobState::Queued,
                chunks_done: 0,
                chunks_total,
                summary_id: None,
                error: None,
                created_at: at,
                cancel: cancel.clone(),
                deduped: false,
            },
        );

        Submitted {
            job_id: id,
            cancel,
            chunks_total,
            deduped: false,
        }
    }

    pub fn mark_running(&self, job_id: &str, chunks_total: u32) {
        self.update(job_id, |entry| {
            entry.state = JobState::Running;
            entry.chunks_total = chunks_total.max(1);
        });
    }

    pub fn mark_progress(&self, job_id: &str, done: u32, total: u32) {
        self.update(job_id, |entry| {
            entry.state = JobState::Running;
            entry.chunks_done = done;
            entry.chunks_total = total.max(1);
        });
    }

    pub fn mark_done(&self, job_id: &str, summary_id: impl Into<String>) {
        info!(
            component = "jobs",
            event = "job_done",
            id = %observability::redact_id(job_id),
            "总结作业完成"
        );
        let summary_id = summary_id.into();
        let mut registry = mc_common::lock::lock_or_recover(&self.registry);
        if let Some(entry) = registry.jobs.get_mut(job_id) {
            entry.state = JobState::Done;
            entry.summary_id = Some(summary_id);
            let key = entry.key.clone();
            registry.in_flight.remove(&key);
        }
    }

    pub fn mark_failed(&self, job_id: &str, reason: impl Into<String>) {
        let reason = reason.into();
        warn!(
            component = "jobs",
            event = "job_failed",
            id = %observability::redact_id(job_id),
            reason = %observability::redact_text(&reason),
            "总结作业失败"
        );
        let mut registry = mc_common::lock::lock_or_recover(&self.registry);
        if let Some(entry) = registry.jobs.get_mut(job_id) {
            entry.state = JobState::Failed;
            entry.error = Some(reason);
            let key = entry.key.clone();
            registry.in_flight.remove(&key);
        }
    }

    pub fn mark_cancelled(&self, job_id: &str) {
        let mut registry = mc_common::lock::lock_or_recover(&self.registry);
        if let Some(entry) = registry.jobs.get_mut(job_id) {
            entry.state = JobState::Cancelled;
            let key = entry.key.clone();
            registry.in_flight.remove(&key);
        }
    }

    /// 请求取消。返回是否确实发出了取消（未知作业返回 false）。
    pub fn cancel(&self, job_id: &str) -> bool {
        let registry = mc_common::lock::lock_or_recover(&self.registry);
        match registry.jobs.get(job_id) {
            Some(entry) if !entry.state.is_terminal() => {
                entry.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    pub fn snapshot(&self, job_id: &str) -> Option<JobSnapshot> {
        let registry = mc_common::lock::lock_or_recover(&self.registry);
        registry.jobs.get(job_id).map(JobSnapshot::from)
    }

    pub fn in_flight_count(&self) -> usize {
        mc_common::lock::lock_or_recover(&self.registry)
            .in_flight
            .len()
    }

    fn update(&self, job_id: &str, mutate: impl FnOnce(&mut JobEntry)) {
        let mut registry = mc_common::lock::lock_or_recover(&self.registry);
        if let Some(entry) = registry.jobs.get_mut(job_id) {
            mutate(entry);
        }
    }
}

impl From<&JobEntry> for JobSnapshot {
    fn from(entry: &JobEntry) -> Self {
        Self {
            job_id: entry.id.clone(),
            state: entry.state,
            chunks_done: entry.chunks_done,
            chunks_total: entry.chunks_total,
            summary_id: entry.summary_id.clone(),
            error: entry.error.clone(),
            created_at: entry.created_at.to_rfc3339(),
            deduped: entry.deduped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 固定时间基准（`mc_testkit::fixtures` 是唯一来源，这里只做秒级偏移）
    fn at(seconds: i64) -> Timestamp {
        mc_testkit::fixtures::fixture_at(seconds * 1_000)
    }

    #[test]
    fn identical_in_flight_requests_share_one_job() {
        let jobs = AdhocJobs::new();

        let first = jobs.submit("range:0-3600", 3, at(0));
        let second = jobs.submit("range:0-3600", 3, at(1));

        assert_eq!(first.job_id, second.job_id, "并发同请求只跑一个作业");
        assert!(second.deduped);
        assert!(!first.deduped);
        assert_eq!(jobs.in_flight_count(), 1);
    }

    #[test]
    fn different_scopes_get_different_jobs() {
        let jobs = AdhocJobs::new();
        let a = jobs.submit("range:0-3600", 1, at(0));
        let b = jobs.submit("range:0-7200", 1, at(0));
        assert_ne!(a.job_id, b.job_id);
        assert_eq!(jobs.in_flight_count(), 2);
    }

    #[test]
    fn a_finished_job_no_longer_dedupes() {
        let jobs = AdhocJobs::new();
        let first = jobs.submit("range:0-3600", 1, at(0));
        jobs.mark_done(&first.job_id, "sum-1");

        let second = jobs.submit("range:0-3600", 1, at(1));
        assert_ne!(
            first.job_id, second.job_id,
            "已完成的请求应当走结果缓存，而不是复用旧作业"
        );
        assert_eq!(jobs.in_flight_count(), 1);
    }

    #[test]
    fn cancel_only_applies_to_running_jobs() {
        let jobs = AdhocJobs::new();
        let job = jobs.submit("range:0-3600", 3, at(0));
        jobs.mark_progress(&job.job_id, 1, 3);

        assert!(jobs.cancel(&job.job_id));
        assert!(job.cancel.is_cancelled());

        jobs.mark_cancelled(&job.job_id);
        assert!(!jobs.cancel(&job.job_id), "已结束的作业不能再取消");
        assert!(!jobs.cancel("job-does-not-exist"));
    }

    #[test]
    fn progress_is_visible_in_the_snapshot() {
        let jobs = AdhocJobs::new();
        let job = jobs.submit("range:0-3600", 3, at(0));
        jobs.mark_progress(&job.job_id, 2, 3);

        let snapshot = jobs.snapshot(&job.job_id).expect("作业存在");
        assert_eq!(snapshot.state, JobState::Running);
        assert_eq!(snapshot.chunks_done, 2);
        assert_eq!(snapshot.chunks_total, 3);
        assert!(snapshot.summary_id.is_none());

        jobs.mark_failed(&job.job_id, "模型超时");
        let snapshot = jobs.snapshot(&job.job_id).unwrap();
        assert_eq!(snapshot.state, JobState::Failed);
        assert_eq!(snapshot.error.as_deref(), Some("模型超时"));
    }
}
