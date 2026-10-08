//! 首页「最新活动」推送（由前端的 running/stopped 开关驱动）。
//!
//! 前端 `latest-activity-card` 挂载时发 `'running'`、卸载时发 `'stopped'`，
//! 真正的数据走 SSE 的 `push:latest-activity` —— 所以这是**开关**而不是取一次。
//!
//! 三条约束：`stopped` 必须真的停（否则一次挂载就把轮询永久留在进程里）；
//! 同一个活动只推一次（卡片每收到一次就重渲染）；推送负载与
//! `/api/db/activities/latest` 完全一致（前端直接当 `Activity` 渲染）。

use std::sync::Arc;
use std::time::Duration;

use mc_common::time::{Clock, SystemClock};
use serde_json::Value;

use crate::events::EVENT_PUSH_LATEST_ACTIVITY;
use crate::state::ServerState;

/// 默认轮询间隔：5 秒，与渲染层的既有节奏一致。
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// 开始推送。重复调用是空操作（不会堆出第二个任务）。
pub fn start(state: &Arc<ServerState>, interval: Duration) {
    if is_running(state) {
        return;
    }

    let task_state = Arc::clone(state);
    let handle = tokio::spawn(async move {
        // 记住上一次推过的活动，避免重复推同一个
        let mut last_pushed: Option<String> = None;
        loop {
            tokio::time::sleep(interval).await;

            let Ok(Some(row)) = mc_storage::projectors::activities::latest_legacy(&task_state.db)
            else {
                continue;
            };

            let payload = match serde_json::to_value(&row) {
                Ok(payload) => payload,
                Err(_) => continue,
            };

            if !has_changed(&payload, &mut last_pushed) {
                continue;
            }

            // 没有订阅者时返回 false，这不是错误
            task_state
                .events
                .publish(EVENT_PUSH_LATEST_ACTIVITY, payload);
        }
    });

    if let Ok(mut slot) = state.latest_activity_task.lock() {
        *slot = Some(handle);
    }
}

/// 停止推送。返回「之前是否在跑」。
pub fn stop(state: &ServerState) -> bool {
    let Ok(mut slot) = state.latest_activity_task.lock() else {
        return false;
    };
    match slot.take() {
        Some(handle) => {
            handle.abort();
            true
        }
        None => false,
    }
}

pub fn is_running(state: &ServerState) -> bool {
    let guard = match state.latest_activity_task.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    match guard.as_ref() {
        // `abort()` 之后 handle 仍在，因此这里看的是任务状态而不是「有没有 handle」
        Some(handle) => !handle.is_finished(),
        None => false,
    }
}

/// 与上次推送的比对：看活动身份 + 结束时间。
///
/// 只比 id 会漏掉「同一个活动被延长了」；只比内容会漏掉「标题被用户改了」。
fn has_changed(payload: &Value, last_pushed: &mut Option<String>) -> bool {
    let fingerprint = format!(
        "{}|{}|{}",
        payload["id"].as_i64().unwrap_or_default(),
        payload["end_time"].as_str().unwrap_or_default(),
        payload["title"].as_str().unwrap_or_default(),
    );

    if last_pushed.as_deref() == Some(fingerprint.as_str()) {
        return false;
    }
    *last_pushed = Some(fingerprint);
    true
}

/// 供测试与诊断：当前时间（避免测试直接依赖系统时钟）。
pub fn now_ms() -> i64 {
    Clock::now(&SystemClock).as_millis()
}
