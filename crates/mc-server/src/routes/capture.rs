//! 采集与时间线接口。
//!
//! 这些是新增接口（不属于兼容面）：由 daemon 直接提供。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Response;
use axum::Json;
use base64::Engine;
use mc_capture::source::TargetKind;
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock, Timestamp};
use mc_storage::observations::ObservationQuery;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::envelope;
use crate::state::ServerState;

/// 单次导出的观测条数上限：导出是人工触发的排查动作，给足量但必须有天花板，
/// 否则一次请求会把全部截图记录读进内存。
const OBSERVATIONS_EXPORT_LIMIT: u32 = 500;

#[derive(Debug, Deserialize)]
pub struct TargetsQuery {
    visible: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DateQuery {
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PathQuery {
    path: Option<String>,
}

/// 有效时区：配置里没写就退回 UTC（配置层已经就此给过 warning）。
fn effective_timezone(state: &ServerState) -> String {
    state
        .config
        .current()
        .config
        .general
        .timezone
        .clone()
        .unwrap_or_else(|| "UTC".to_string())
}

/// 解析 `YYYY-MM-DD` 或 `YYYYMMDD`。
///
/// 两种都要接受：写入时用 `YYYY-MM-DD`，而列表默认值是 `YYYYMMDD` ——
/// 只认一种会让「截图列表永远查不到东西」。
fn parse_date(raw: &str) -> Result<(i32, u32, u32), AppError> {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 8 {
        return Err(AppError::new(
            ErrorCode::DomainInvalidTimestamp,
            format!("无法解析日期 `{raw}`，期望 YYYY-MM-DD 或 YYYYMMDD"),
        ));
    }

    let year = digits[0..4].parse::<i32>().unwrap_or(0);
    let month = digits[4..6].parse::<u32>().unwrap_or(0);
    let day = digits[6..8].parse::<u32>().unwrap_or(0);

    if chrono::NaiveDate::from_ymd_opt(year, month, day).is_none() {
        return Err(AppError::new(
            ErrorCode::DomainInvalidTimestamp,
            format!("日期不存在：{raw}"),
        ));
    }

    Ok((year, month, day))
}

pub async fn permissions(State(state): State<Arc<ServerState>>) -> Response {
    let readiness = mc_capture::platform::probe_readiness();
    let running = state
        .capture
        .as_ref()
        .map(|c| c.is_running())
        .unwrap_or(false);

    envelope::ok(json!({
        // 兼容面的老字段是一个裸 boolean；这里给出更完整的结构，
        // 并保留 `screen_recording` 这个名字，前端不必改调用点。
        "screen_recording": readiness.permission == mc_capture::source::PermissionState::Granted,
        "accessibility": false,
        "permission": permission_label(readiness.permission),
        "ready": readiness.available,
        "monitor_count": readiness.monitor_count,
        "running": running,
        "message": readiness.message,
    }))
}

fn permission_label(permission: mc_capture::source::PermissionState) -> &'static str {
    use mc_capture::source::PermissionState::*;
    match permission {
        Granted => "granted",
        Denied => "denied",
        NotRequired => "not_required",
        Unknown => "unknown",
    }
}

pub async fn targets(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<TargetsQuery>,
) -> Response {
    let Some(controls) = state.capture.as_ref() else {
        return envelope::ok(json!([]));
    };

    let targets = match controls.source.enumerate().await {
        Ok(targets) => targets,
        Err(error) => return envelope::compat_failure(&error),
    };

    let visible_only = query.visible.as_deref() == Some("1");
    // 选中状态来自配置（空 = 全部可见目标）
    let selected: Vec<String> = state.config.current().config.capture.target_ids.clone();

    // 可见性轮询（visible=1）很频繁，不抓缩略图；完整列表才给设置页预览。
    let thumbnails = if visible_only {
        std::collections::HashMap::new()
    } else {
        controls
            .source
            .preview_thumbnails(mc_capture::thumbnail::DEFAULT_PREVIEW_WIDTH)
            .await
    };

    let items: Vec<Value> = targets
        .into_iter()
        .filter(|t| !visible_only || t.is_visible)
        .map(|t| {
            let is_selected = selected.is_empty() || selected.contains(&t.id);
            let thumbnail = thumbnails
                .get(&t.id)
                .cloned()
                .map(Value::String)
                .unwrap_or(Value::Null);
            json!({
                // 形状对齐渲染层的 CaptureSource，字段名不能改
                "id": t.id,
                "name": t.name,
                "type": if t.kind == TargetKind::Screen { "screen" } else { "window" },
                "thumbnail": thumbnail,
                "appIcon": Value::Null,
                "isVisible": t.is_visible,
                "appName": t.app_name,
                "windowTitle": t.window_title,
                "windowId": t.window_id,
                "scaleFactor": t.scale_factor,
                // 新增：前端据此渲染勾选态（原先的响应里没有这个字段）
                "selected": is_selected,
            })
        })
        .collect();

    envelope::ok(json!(items))
}

pub async fn status(State(state): State<Arc<ServerState>>) -> Response {
    let readiness = mc_capture::platform::probe_readiness();
    let (running, has_controls) = state
        .capture
        .as_ref()
        .map(|c| (c.is_running(), true))
        .unwrap_or((false, false));

    let ready = has_controls && readiness.available;
    // 不能录制时必须说明原因：用户看到「不能录制」却不知道是缺屏幕录制权限还是
    // 没有可用显示器，只能去点一次「开始」才知道 —— 这里直接把原因给出来。
    let reason = if ready {
        None
    } else if !has_controls {
        Some("本实例未挂载采集控制".to_string())
    } else {
        Some(readiness.message.clone().unwrap_or_else(|| {
            "屏幕录制不可用：可能是缺少屏幕录制权限，或当前没有可用显示器".to_string()
        }))
    };

    envelope::ok(json!({
        "canRecord": ready,
        "status": if running { "running" } else { "stopped" },
        "reason": reason,
    }))
}

pub async fn start(State(state): State<Arc<ServerState>>) -> Response {
    let Some(controls) = state.capture.as_ref() else {
        return envelope::compat_failure(&AppError::new(
            ErrorCode::CaptureUnsupported,
            "本实例未挂载采集控制",
        ));
    };

    // 未就绪时**拒绝启动**，而不是谎报 running ——
    // 否则用户会看到「正在录制」却什么都采不到。
    let readiness = mc_capture::platform::probe_readiness();
    if !readiness.available {
        return envelope::compat_failure(&mc_capture::platform::readiness_error(&readiness));
    }

    // 落盘「用户要录」：daemon 重启后按 capture.enabled 决定是否自动恢复。
    // 只改内存开关的话，用户点停止再重开仍会自动开录。
    if let Err(error) = persist_capture_enabled(&state, true) {
        return envelope::compat_failure(&error);
    }

    // 推「开始录制」：渲染层的屏幕监控页靠它把状态切到 running。
    // 托盘切换录制时页面本身没有动作，不推事件页面就会一直显示旧状态。
    if !controls.is_running() {
        controls.start();
        state.publish(
            crate::events::EVENT_PUSH_SCREEN_MONITOR_STATUS,
            json!("running"),
        );
    } else {
        controls.start();
    }
    envelope::ok(json!({ "status": "running" }))
}

pub async fn stop(State(state): State<Arc<ServerState>>) -> Response {
    match state.capture.as_ref() {
        Some(controls) => {
            // 先落盘再停：重启后必须保持停止，不能因默认 enabled=true 又自动开录。
            if let Err(error) = persist_capture_enabled(&state, false) {
                return envelope::compat_failure(&error);
            }
            if controls.is_running() {
                controls.stop();
                state.publish(
                    crate::events::EVENT_PUSH_SCREEN_MONITOR_STATUS,
                    json!("stopped"),
                );
            } else {
                controls.stop();
            }
            envelope::ok(json!({ "status": "stopped" }))
        }
        None => envelope::compat_failure(&AppError::new(
            ErrorCode::CaptureUnsupported,
            "本实例未挂载采集控制",
        )),
    }
}

/// 把「是否在录」写进用户配置，供下次启动恢复。
///
/// 无写配置能力时（测试夹具）直接成功：本会话仍由 `CaptureControls` 的
/// running 位控制；持久化只对挂了用户配置层的 daemon 有意义。
fn persist_capture_enabled(state: &ServerState, enabled: bool) -> Result<(), AppError> {
    if state.config_write().is_none() {
        return Ok(());
    }
    crate::config_api::apply_patch(state, crate::config_api::enabled_patch(enabled)).map(|_| ())
}

/// `POST /api/capture/permissions/request` —— 触发系统授权并打开设置面板。
///
/// 采集跑在 `mc-daemon`：必须由它 Request，系统设置里才会出现采集进程条目；
/// 只勾外壳 MineContext.app 时 Preflight 在 daemon 里仍可能是 false。
pub async fn request_permissions(State(_state): State<Arc<ServerState>>) -> Response {
    let permission = mc_capture::platform::request_permission();
    let _ = mc_capture::platform::open_screen_recording_settings();
    let readiness = mc_capture::platform::probe_readiness();
    envelope::ok(json!({
        "screen_recording": permission == mc_capture::source::PermissionState::Granted,
        "permission": permission_label(permission),
        "ready": readiness.available,
        "monitor_count": readiness.monitor_count,
        "message": readiness.message,
    }))
}

/// `GET /api/capture/screenshots?date=YYYY-MM-DD`
pub async fn screenshots(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<DateQuery>,
) -> Response {
    let tz = effective_timezone(&state);
    let raw = match query.date.as_deref() {
        Some(raw) if !raw.is_empty() => raw.to_string(),
        _ => match SystemClock.now().to_local_date(&tz) {
            Ok(date) => date.to_string(),
            Err(error) => return envelope::compat_failure(&error),
        },
    };

    let (year, month, day) = match parse_date(&raw) {
        Ok(parts) => parts,
        Err(error) => return envelope::compat_failure(&error),
    };

    // 用配置时区算日边界（DST 安全），而不是简单地按 UTC 切
    let probe = format!("{year:04}-{month:02}-{day:02}T12:00:00Z");
    let anchor = match Timestamp::parse_rfc3339(&probe) {
        Ok(anchor) => anchor,
        Err(error) => return envelope::compat_failure(&error),
    };
    let (from, to) = match anchor.day_bounds(&tz) {
        Ok(bounds) => bounds,
        Err(error) => return envelope::compat_failure(&error),
    };

    let rows = match state.db.query_observations(&ObservationQuery {
        from: Some(from),
        to: Some(to),
        exclude_blocked: true,
        limit: Some(OBSERVATIONS_EXPORT_LIMIT),
        ..Default::default()
    }) {
        Ok(rows) => rows,
        Err(error) => return envelope::compat_failure(&error),
    };

    let date_label = format!("{year:04}-{month:02}-{day:02}");
    let items: Vec<Value> = rows
        .into_iter()
        .map(|row| {
            json!({
                // 渲染层消费的字段，名字不能改
                "id": row.id,
                "date": date_label,
                "timestamp": row.ts.as_millis(),
                "image_url": row.image_path,
                "description": Value::Null,
                "created_at": row.ts.to_rfc3339(),
                "group_id": Value::Null,
                "app_name": row.app_name,
                "change_kind": row.change_kind,
                "analysis_status": row.analysis_status,
            })
        })
        .collect();

    envelope::ok(json!(items))
}

/// `GET /api/capture/screenshots/data?path=<相对路径>`
///
/// 只接受 store 内的相对路径；响应体只有 base64 与 mime，
/// **不含任何文件系统路径**。
pub async fn screenshot_data(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<PathQuery>,
) -> Response {
    let Some(controls) = state.capture.as_ref() else {
        return envelope::compat_failure(&AppError::new(
            ErrorCode::StorageUnavailable,
            "本实例未挂载 blob 存储",
        ));
    };

    let Some(relative) = query.path.as_deref().filter(|p| !p.is_empty()) else {
        return envelope::compat_failure(&AppError::new(
            ErrorCode::StorageUnavailable,
            "缺少 path 参数",
        ));
    };

    // 路径校验在 blob store 内部完成（安全属性只允许有一处实现）
    let bytes = match controls.blobs.read_relative(relative) {
        Ok(bytes) => bytes,
        Err(error) => return envelope::compat_failure(&error),
    };

    let mime = if relative.ends_with(".png") {
        "image/png"
    } else if relative.ends_with(".jpg") || relative.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "application/octet-stream"
    };

    envelope::ok(json!({
        "data": base64::engine::general_purpose::STANDARD.encode(&bytes),
        "mime": mime,
        "bytes": bytes.len(),
    }))
}

/// `POST /api/capture/targets/selection` —— 保存「要采集哪些目标」。
///
/// 语义是**替换**：累积会让取消勾选永远不生效 —— 之前的选择必须被覆盖掉。
/// 空数组 = 不限制（全部可见目标）。
pub async fn selection(State(state): State<Arc<ServerState>>, Json(body): Json<Value>) -> Response {
    let targets = match crate::config_api::targets_from_body(&body) {
        Ok(targets) => targets,
        Err(error) => return envelope::compat_failure(&error),
    };

    let patch = crate::config_api::selection_patch(&targets);
    match crate::config_api::apply_patch(&state, patch) {
        Ok(_) => envelope::ok(json!({ "success": true, "targets": targets })),
        Err(error) => envelope::compat_failure(&error),
    }
}

/// `PATCH /api/capture/config` —— 保存采集设置（间隔、录制时段）。
///
/// 兼容 UI 的 `ScreenSettings` 形状：`recordInterval`（秒）、
/// `enableRecordingHours`、`recordingHours`（`["HH:mm:ss","HH:mm:ss"]`）、
/// `applyToDays`（`weekday` / `everyday`）。没给的字段不动。
pub async fn patch_config(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<Value>,
) -> Response {
    let patch = match crate::config_api::capture_settings_patch(&body) {
        Ok(patch) => patch,
        Err(error) => return envelope::compat_failure(&error),
    };

    match crate::config_api::apply_patch(&state, patch) {
        Ok(loaded) => envelope::ok(json!({
            "success": true,
            "interval_secs": loaded.config.capture.interval_secs,
            "enable_recording_hours": loaded.config.capture.enable_recording_hours,
        })),
        Err(error) => envelope::compat_failure(&error),
    }
}

/// `GET /api/capture/config` —— 读当前采集设置。
///
/// 与 `PATCH` 成对：前端要能在没有本地缓存时把表单填成「现在的值」，
/// 而不是靠猜默认值。字段名跟兼容 UI 的 `ScreenSettings` 一致。
pub async fn get_config(State(state): State<Arc<ServerState>>) -> Response {
    let config = state.config.current();
    let capture = &config.config.capture;

    envelope::ok(json!({
        "enabled": capture.enabled,
        "interval_secs": capture.interval_secs,
        "recordInterval": capture.interval_secs,
        "target_ids": capture.target_ids,
        "region": capture.region,
        "enableRecordingHours": capture.enable_recording_hours,
        "enable_recording_hours": capture.enable_recording_hours,
        "recordingHours": capture.recording_hours,
        "recording_hours": capture.recording_hours,
        "applyToDays": match capture.apply_to_days {
            mc_config::model::ApplyToDays::Everyday => "everyday",
            mc_config::model::ApplyToDays::Weekday => "weekday",
        },
        "apply_to_days": match capture.apply_to_days {
            mc_config::model::ApplyToDays::Everyday => "everyday",
            mc_config::model::ApplyToDays::Weekday => "weekday",
        },
        "retention_days": capture.retention_days,
    }))
}

/// `DELETE /api/capture/screenshots?path=<相对路径>` —— 主动删除单张截图。
///
/// 两条要求（形状不变）：
/// 1. 删除后**同步清掉数据库引用** —— 只 unlink 文件的话，那条观测还留着路径，
///    界面继续显示破图；
/// 2. **幂等** —— 「文件已不存在」也返回成功，否则界面会弹一个没有意义的错误提示。
pub async fn delete_screenshot(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<PathQuery>,
) -> Response {
    let Some(controls) = state.capture.as_ref() else {
        return envelope::compat_failure(&AppError::new(
            ErrorCode::StorageUnavailable,
            "本实例未挂载 blob 存储",
        ));
    };

    let Some(relative) = query.path.as_deref().filter(|path| !path.is_empty()) else {
        return envelope::compat_failure(&AppError::new(
            ErrorCode::StorageUnavailable,
            "缺少 path 参数",
        ));
    };

    match mc_storage::retention::delete_screenshot(&state.db, &controls.blobs, relative) {
        // 返回 `{success, error?}`：错误放在载荷里，不抛异常
        Ok(_outcome) => envelope::ok(json!({ "success": true })),
        Err(error) => envelope::compat_failure(&error),
    }
}

/// `POST /api/capture/now` —— 按需截一张（前端「手动截图」按钮）。
///
/// 前端调用契约（`takeScreenshot`）：
/// `{ success, screenshotInfo: { url, date, timestamp }, error? }`。
/// `url` 会被原样交给 `readImageAsBase64(url)` →
/// `GET /api/capture/screenshots/data?path=<url>`，因此它必须是
/// **blob store 内的相对路径**；返回绝对路径的话第二步会被路径校验拒绝。
///
/// `group_interval` 参数被忽略：截图分组是**前端**的事（渲染层自己算
/// `groupTimestamp` 并写进记录），服务端不需要第二个分组实现。
#[derive(Debug, Deserialize)]
pub struct NowBody {
    #[serde(default)]
    pub group_interval: Option<String>,
    #[serde(default)]
    pub target_id: Option<String>,
}

pub async fn now(State(state): State<Arc<ServerState>>, body: Option<Json<NowBody>>) -> Response {
    let Some(controls) = state.capture.clone() else {
        return envelope::compat_failure(&AppError::new(
            ErrorCode::StorageUnavailable,
            "本实例未挂载采集控制",
        ));
    };

    let body = body.map(|Json(body)| body).unwrap_or(NowBody {
        group_interval: None,
        target_id: None,
    });
    let _ = body.group_interval;

    // 选目标：指定了就找它，没指定就用第一个可见目标
    let targets = match controls.source.enumerate().await {
        Ok(targets) => targets,
        Err(error) => {
            return envelope::ok(json!({ "success": false, "error": error.detail() }));
        }
    };

    let target = match body.target_id.as_deref() {
        Some(wanted) => match targets.iter().find(|target| target.id == wanted) {
            Some(target) => target.clone(),
            None => {
                return envelope::ok(json!({
                    "success": false,
                    "error": format!("没有找到采集目标 {wanted}"),
                }));
            }
        },
        None => match targets
            .iter()
            .find(|target| target.is_visible)
            .or(targets.first())
        {
            Some(target) => target.clone(),
            None => {
                return envelope::ok(json!({
                    "success": false,
                    "error": "当前没有可用的采集目标",
                }));
            }
        },
    };

    let at = Clock::now(&SystemClock);
    let ctx = mc_capture::source::CaptureContext {
        targets: vec![target.id.clone()],
        now: at,
        idle: false,
    };

    let captures = match controls.source.poll(&ctx).await {
        Ok(captures) => captures,
        Err(error) => {
            // 失败走 `success:false`，不抛异常
            return envelope::ok(json!({ "success": false, "error": error.detail() }));
        }
    };

    let Some(frame) = captures.into_iter().find(|frame| frame.image.is_some()) else {
        return envelope::ok(json!({
            "success": false,
            "error": "采集源没有返回图像（可能是锁屏或权限未生效）",
        }));
    };
    let Some(image) = frame.image else {
        return envelope::ok(json!({ "success": false, "error": "采集源没有返回图像" }));
    };

    // 落盘 + 落库：与采集环走同一套实现，避免两条路径各写一份
    use mc_pipeline::pump::{FrameStore, ObservationSink};

    let frame_store = crate::capture_loop::BlobFrameStore::new(Arc::clone(&controls.blobs), None);
    let stored = match frame_store.store_frame(&image, frame.captured_at) {
        Ok(stored) => stored,
        Err(error) => {
            return envelope::ok(json!({ "success": false, "error": error.detail() }));
        }
    };

    let change_kind = mc_capture::change::ChangeKind::New;
    let observation = mc_pipeline::pump::CapturedObservation {
        // 手动截图也用稳定 id：`manual-{目标}-{毫秒时间戳}`，便于排查
        id: format!(
            "manual-{}-{}",
            frame.target.id,
            frame.captured_at.as_millis()
        ),
        captured_at: frame.captured_at,
        target: frame.target.clone(),
        source_id: controls.source.id().to_string(),
        source_kind: controls.source.kind(),
        change_kind,
        privacy_verdict: mc_pipeline::pump::PrivacyVerdict::Allowed,
        image: Some(stored.clone()),
        phash: 0,
        idempotency: format!(
            "{}|{}|{}|manual",
            controls.source.id(),
            frame.target.id,
            stored.content_hash
        ),
    };

    let sink = crate::capture_loop::DatabaseSink::new(Arc::clone(&state.db));
    if let Err(error) = sink.persist(&observation) {
        return envelope::ok(json!({ "success": false, "error": error.detail() }));
    }

    let timezone = effective_timezone(&state);
    let date = at
        .format_in_tz(&timezone, "%Y-%m-%d")
        .unwrap_or_else(|_| at.to_legacy_datetime()[..10].to_string());

    envelope::ok(json!({
        "success": true,
        "screenshotInfo": {
            "url": stored.relative_path,
            "date": date,
            "timestamp": frame.captured_at.as_millis(),
        }
    }))
}

pub fn router() -> axum::Router<Arc<ServerState>> {
    use axum::routing::{get, post};

    axum::Router::new()
        .route("/api/capture/permissions", get(permissions))
        .route(
            "/api/capture/permissions/request",
            post(request_permissions),
        )
        .route("/api/capture/targets", get(targets))
        .route("/api/capture/now", post(now))
        .route(
            "/api/capture/config",
            axum::routing::get(get_config).patch(patch_config),
        )
        .route("/api/capture/targets/selection", post(selection))
        .route("/api/capture/status", get(status))
        .route("/api/capture/start", post(start))
        .route("/api/capture/stop", post(stop))
        .route(
            "/api/capture/screenshots",
            get(screenshots).delete(delete_screenshot),
        )
        .route("/api/capture/screenshots/data", get(screenshot_data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_parsing_accepts_both_formats() {
        assert_eq!(parse_date("2026-09-30").unwrap(), (2026, 9, 30));
        assert_eq!(parse_date("20260930").unwrap(), (2026, 9, 30));
    }

    #[test]
    fn date_parsing_rejects_garbage() {
        for bad in ["", "not-a-date", "2026-13-01", "20260230", "26-09-30"] {
            assert!(parse_date(bad).is_err(), "{bad} 应当被拒绝");
        }
    }

    #[test]
    fn permission_labels_are_snake_case() {
        assert_eq!(
            permission_label(mc_capture::source::PermissionState::Denied),
            "denied"
        );
        assert_eq!(
            permission_label(mc_capture::source::PermissionState::Granted),
            "granted"
        );
    }
}
