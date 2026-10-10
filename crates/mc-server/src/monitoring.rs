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

    let pending_analyses = state.db.pending_analysis_count().unwrap_or(0);
    let analysis_blocker = analysis_blocker(state, &counts, pending_analyses);
    let indexing_pause = crate::indexing::pause_payload(state.indexing_pause().as_ref());

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
        "pending_analyses": pending_analyses,
        "generated_activities": counts.generated_activities,
        "next_activity_eta_seconds": eta,
        "last_activity_time": counts
            .last_activity_ms
            .map(|ms| Timestamp::from_millis(ms).to_rfc3339()),
        "session_start_time": session.to_rfc3339(),
        "recent_errors": recent_errors,
        "recent_screenshots": recent_screenshots,
        "analysis_blocker": analysis_blocker,
        // 与 analysis_blocker 独立：分析可能已通，向量索引仍会因 401/429 暂停。
        "indexing_pause": indexing_pause,
    }))
}

/// 采到了但「已分析」为 0 时，给前端一句可行动的原因（配置 / 隐私 / 管道 / 鉴权）。
fn analysis_blocker(
    state: &ServerState,
    counts: &mc_storage::monitoring::RecordingCounts,
    pending_analyses: i64,
) -> Value {
    if counts.processed_screenshots > 0 || counts.captured_screenshots == 0 {
        return Value::Null;
    }

    let config = state.config.current();
    let vision = &config.config.ai.vision;
    let vision_ok = !vision.base_url.trim().is_empty() && !vision.model.trim().is_empty();
    let key_readable = crate::routes::read_stored_model_api_key_for_diagnostics(state);

    if !config.config.privacy.ai_upload {
        return json!({
            "code": "ai_upload_disabled",
            "message": "尚未允许 AI 出网（默认只在本机工作），截图不会送模型分析。",
            "action": { "target": "settings_ai_upload", "label": "去设置开启" },
        });
    }
    if !config.config.ai.enabled {
        return json!({
            "code": "ai_disabled",
            "message": "AI 分析已关闭，截图不会送模型。",
            "action": { "target": "settings_ai_upload", "label": "去设置开启" },
        });
    }
    if !vision_ok {
        return json!({
            "code": "vision_unconfigured",
            "message": "未配置视觉模型。请到设置页填写模型平台与模型 ID。",
            "action": { "target": "settings_model", "label": "去设置模型" },
        });
    }
    if !key_readable {
        return json!({
            "code": "api_key_missing",
            "message": "视觉模型已配但读不到 API Key。请到设置页重新保存 API Key。",
            "action": { "target": "settings_model", "label": "去设置密钥" },
        });
    }
    // 密钥能读到但仍失败：401/403、429、余额不足 —— 比「pending 未接线」更可行动。
    if let Ok(Some(hint)) =
        mc_storage::monitoring::latest_provider_failure(&state.db, state.started_at.as_millis())
    {
        if let Some(blocker) = provider_failure_blocker(&hint) {
            return blocker;
        }
    }
    if counts.failed_screenshots > 0 {
        let detail = counts
            .recent_errors
            .first()
            .map(|error| error.message.as_str())
            .unwrap_or("见 recent_errors");
        return json!({
            "code": "analysis_failed",
            "message": format!(
                "已有 {n} 次采集/视觉失败。最近一条：{detail}",
                n = counts.failed_screenshots
            ),
        });
    }
    if pending_analyses > 0 {
        return json!({
            "code": "analyses_pending_unwired",
            "message": format!(
                "有 {pending_analyses} 条截图分析仍为 pending：截图视觉分析作业尚未接线（队列消费者目前只处理补偿推断），因此「已分析」会一直为 0。配置本身看起来可用。"
            ),
        });
    }
    json!({
        "code": "no_completed_analyses",
        "message": "本会话有采集但没有任何 done/degraded 分析记录。请检查诊断页 /api/diagnostics 与 pipeline_failures。",
    })
}

fn provider_failure_blocker(hint: &mc_storage::monitoring::ProviderFailureHint) -> Option<Value> {
    let blob = format!(
        "{} {} {}",
        hint.message,
        hint.remediation.as_deref().unwrap_or(""),
        hint.context.as_deref().unwrap_or("")
    );
    let looks_like_quota = looks_like_quota_exhaustion(&blob);

    match hint.error_code.as_str() {
        "provider_auth_failed" => Some(json!({
            "code": "api_key_invalid",
            "message": format!(
                "{}请到设置页更新 API Key 后重试。{}",
                hint.message,
                hint.remediation
                    .as_deref()
                    .map(|text| format!(" {text}"))
                    .unwrap_or_default()
            ),
            "action": { "target": "settings_model", "label": "去设置密钥" },
        })),
        "provider_rate_limited" => Some(json!({
            "code": "provider_rate_limited",
            "message": format!(
                "{}{}",
                hint.message,
                hint.remediation
                    .as_deref()
                    .map(|text| format!(" {text}"))
                    .unwrap_or_default()
            ),
            "action": { "target": "resume_indexing", "label": "恢复索引" },
        })),
        "budget_exceeded" => Some(json!({
            "code": "budget_exceeded",
            "message": format!(
                "{}{}",
                hint.message,
                hint.remediation
                    .as_deref()
                    .map(|text| format!(" {text}"))
                    .unwrap_or_default()
            ),
        })),
        "provider_not_found" => Some(json!({
            "code": "model_not_found",
            "message": format!(
                "{}{}",
                hint.message,
                hint.remediation
                    .as_deref()
                    .map(|text| format!(" {text}"))
                    .unwrap_or_default()
            ),
        })),
        "provider_invalid_response" | "provider_server_error" if looks_like_quota => Some(json!({
            "code": "quota_exhausted",
            "message": format!(
                "模型服务返回额度/余额不足（{}）。请充值或更换可用的 API Key 后再观察「已分析」计数。",
                hint.context
                    .as_deref()
                    .filter(|text| !text.is_empty())
                    .unwrap_or(hint.message.as_str())
            ),
        })),
        _ => None,
    }
}

fn looks_like_quota_exhaustion(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("余额")
        || lower.contains("欠费")
        || lower.contains("额度")
        || lower.contains("充值")
        || lower.contains("insufficient")
        || lower.contains("quota")
        || lower.contains("billing")
        || lower.contains("credit")
}
