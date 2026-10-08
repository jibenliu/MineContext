//! `/api/monitoring/recording-stats` —— 录制统计（屏幕监控页的统计卡片）。
//!
//! 这条路径**在兼容面契约里**：
//! 计数来自 `mc-storage::monitoring`，这里只算 ETA、拼线上形状。
//!
//! 字段名逐一相同、`recent_errors` / `recent_screenshots` 最多
//! 5 条、没有活动时 ETA 从**会话起点**算且不为负。

use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use mc_common::error::AppError;
use mc_common::time::{Clock, SystemClock, Timestamp};
use serde_json::{json, Value};

use crate::envelope;
use crate::state::ServerState;

/// 失败的「采集相关」组件。
///
/// 只统计采集与视觉推断两个组件的失败：
/// 截图处理分布在这两处，因此两个都算。
const CAPTURE_COMPONENTS: &[&str] = &["capture", "vision"];

/// 最近错误与最近截图的条数上限。
const RECENT_LIMIT: usize = 5;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new().route("/api/monitoring/recording-stats", get(stats))
}

async fn stats(State(state): State<Arc<ServerState>>) -> Response {
    match build_stats(&state) {
        Ok(payload) => envelope::ok(payload),
        // 统计卡片消失好过整页报错：契约路由必须 HTTP 200 + 信封
        Err(error) => envelope::compat_failure(&error),
    }
}

/// 组装统计。全部来自库，因此重启后数字仍然正确。
pub fn build_stats(state: &ServerState) -> Result<Value, AppError> {
    let session = state.started_at;
    let counts = mc_storage::monitoring::recording_counts(
        &state.db,
        session.as_millis(),
        CAPTURE_COMPONENTS,
        RECENT_LIMIT,
    )?;

    let now = Clock::now(&SystemClock);

    // ETA：距上次活动多久了，还差多久到下一次生成。
    // 间隔取活动投影节奏（配置里的 900 秒，同一含义）。
    let interval = state
        .config
        .current()
        .config
        .activity
        .project_tick_secs
        .max(1) as i64;
    let reference_ms = counts.last_activity_ms.unwrap_or(session.as_millis());
    let elapsed_secs = (now.as_millis() - reference_ms).max(0) / 1000;
    let eta = (interval - elapsed_secs).max(0);

    let recent_screenshots = counts.recent_screenshot_paths;

    let recent_errors: Vec<Value> = counts
        .recent_errors
        .into_iter()
        .map(|error| {
            json!({
                "error_message": error.message,
                "processor_name": error.component,
                "timestamp": Timestamp::from_millis(error.at_ms).to_rfc3339(),
            })
        })
        .collect();

    Ok(json!({
        // 两个数衡量两件事，都要给：采到多少张、其中多少张分析出了结果。
        // 只给一个会让界面把「采到了」说成「处理完了」。
        "captured_screenshots": counts.captured_screenshots,
        "processed_screenshots": counts.processed_screenshots,
        "failed_screenshots": counts.failed_screenshots,
        "generated_activities": counts.generated_activities,
        "next_activity_eta_seconds": eta,
        "last_activity_time": counts
            .last_activity_ms
            .map(|ms| Timestamp::from_millis(ms).to_rfc3339()),
        "session_start_time": session.to_rfc3339(),
        "recent_errors": recent_errors,
        "recent_screenshots": recent_screenshots,
    }))
}
