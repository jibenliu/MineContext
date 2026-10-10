//! 保留策略的定时执行。
//!
//! [`mc_storage::retention::run_retention`] 负责「删文件 + 清引用」，
//! 这一层负责让它**真的在跑**、把最近一次结果留在诊断里、并按配置取策略。
//!
//! 三条约束：只读实例（没挂载 blob 存储）不报错也不假装执行过（返回 `None`）；
//! 轮转不可逆，因此策略全部来自配置，代码里不留默认删除行为；失败写
//! `pipeline_failures` 并按时间节流 —— 磁盘故障会持续失败，逐次记录会刷爆诊断表。

use std::sync::Arc;
use std::time::Duration;

use mc_common::error::AppError;
use mc_common::observability::{error, error_summary, info};
use mc_common::time::{Clock, SystemClock, Timestamp};
use mc_storage::blob::RetentionPolicy;
use mc_storage::retention::RetentionOutcome;

use crate::state::ServerState;

/// 默认执行间隔。保留策略按天计，每小时检查一次足够。
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(3600);

/// 失败记录的节流间隔。
const FAILURE_THROTTLE_MS: i64 = 30 * 60 * 1000;

/// 最近一次轮转的快照，供 `/api/diagnostics` 展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionSnapshot {
    pub ran_at: Timestamp,
    pub outcome: RetentionOutcome,
}

/// 从配置推导保留策略。
///
/// - 天数：`capture.retention_days`（`0` = 不按天删除）
/// - 容量：`storage.max_total_gb`（`0` = 不按容量删除）
/// - 数量：`storage.max_screenshot_count`（`0` = 不按数量删除）
///
/// 三项可组合；任一项生效都会从最旧截图开始删。只动 blob 截图/缩略图，不动 vault。
pub fn policy_from(config: &mc_config::Config) -> RetentionPolicy {
    RetentionPolicy {
        screenshots_days: config.capture.retention_days,
        max_total_bytes: max_total_bytes_from_gb(config.storage.max_total_gb),
        max_blob_count: config.storage.max_screenshot_count,
    }
}

/// GiB → 字节。`<= 0` 表示不限制（与 [`RetentionPolicy::max_total_bytes`] 的 `0` 语义一致）。
pub fn max_total_bytes_from_gb(gb: f64) -> u64 {
    if !gb.is_finite() || gb <= 0.0 {
        return 0;
    }
    (gb * 1024.0 * 1024.0 * 1024.0) as u64
}

/// 立即执行一次轮转。
///
/// 返回 `None` 表示「本实例没有 blob 存储，没有东西可轮转」——
/// 这不是错误（只读实例就是这样），但**不能**谎报成功。
pub fn run_once(state: &ServerState) -> Result<Option<RetentionOutcome>, AppError> {
    let Some(controls) = state.capture.as_ref() else {
        return Ok(None);
    };

    let policy = policy_from(&state.config.current().config);
    let at = Clock::now(&SystemClock);
    let outcome = mc_storage::retention::run_retention(&state.db, &controls.blobs, &policy, at)?;
    info!(
        component = "retention",
        event = "sweep",
        deleted_files = outcome.deleted_files,
        freed_bytes = outcome.freed_bytes,
        kept_files = outcome.kept_files,
        cleared_references = outcome.cleared_references,
        "保留策略执行完成"
    );
    state.record_retention_run(RetentionSnapshot {
        ran_at: at,
        outcome,
    });
    Ok(Some(outcome))
}

/// 起定时轮转任务。没有挂载 blob 存储时立即返回（调用方不必判断）。
pub fn spawn_retention_task(
    state: Arc<ServerState>,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if state.capture.is_none() {
            return;
        }

        let mut last_failure: Option<Timestamp> = None;
        loop {
            tokio::time::sleep(interval).await;

            match run_once(&state) {
                Ok(_) => {}
                Err(error) => {
                    let at = Clock::now(&SystemClock);
                    if last_failure.is_none_or(|previous| {
                        at.saturating_diff_millis(previous) >= FAILURE_THROTTLE_MS
                    }) {
                        let _ = state.db.record_failure(at, "retention", &error, "warn");
                        error!(
                            component = "retention",
                            event = "sweep_failed",
                            detail = %error_summary(&error),
                            "保留策略执行失败"
                        );
                        last_failure = Some(at);
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(toml: &str) -> mc_config::Config {
        mc_config::load::load(&mc_config::load::LoadRequest {
            layers: vec![mc_config::load::LayerSource::Inline {
                name: "test".to_string(),
                toml: toml.to_string(),
            }],
            env: Vec::new(),
            read_process_env: false,
        })
        .unwrap()
        .config
    }

    #[test]
    fn policy_reads_days_and_capacity_from_config() {
        let config = config_with(
            "[capture]\nretention_days = 3\n[storage]\nmax_total_gb = 2.0\nmax_screenshot_count = 1000\n",
        );
        let policy = policy_from(&config);
        assert_eq!(policy.screenshots_days, 3);
        assert_eq!(policy.max_total_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(policy.max_blob_count, 1000);
    }

    #[test]
    fn zero_capacity_means_unlimited() {
        let config = config_with("[storage]\nmax_total_gb = 0\nmax_screenshot_count = 0\n");
        let policy = policy_from(&config);
        assert_eq!(policy.max_total_bytes, 0);
        assert_eq!(policy.max_blob_count, 0);
    }

    #[test]
    fn defaults_match_product_defaults() {
        let config = config_with("");
        let policy = policy_from(&config);
        assert_eq!(policy.screenshots_days, 7);
        assert_eq!(policy.max_total_bytes, 10 * 1024 * 1024 * 1024);
        assert_eq!(policy.max_blob_count, 200_000);
    }
}
