//! `/api/settings/{key}` —— 通用设置 KV。
//!
//! 渲染层用它存采集设置与「已完成」标记。三条与既有行为对齐：
//! `getSettings<T>(key)` 返回**原始 JSON 值**而不是 `{value}` 包装，
//! 键不存在时 `data` 为 `null`；`setSettings` / `clearSettings` 返回
//! `{success: boolean}`；值可以是任意 JSON。
//!
//! `PUT` 的请求体用 `{value: ...}` 包装，其中 `value: null` 是**合法值**，
//! 而「没有 value 字段」是调用方写错了 —— 两者必须区分。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock};
use serde_json::{json, Value};

use crate::envelope;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new().route("/api/settings/{key}", get(read).put(write).delete(clear))
}

async fn read(State(state): State<Arc<ServerState>>, Path(key): Path<String>) -> Response {
    match state.db.get_setting(&key) {
        Ok(value) => envelope::ok(value.unwrap_or(Value::Null)),
        Err(error) => envelope::compat_failure(&error),
    }
}

/// `{value: ...}`；字段缺失与显式 `null` 必须区分。
#[derive(Debug, serde::Deserialize)]
struct WriteBody {
    #[serde(default, deserialize_with = "present_value")]
    value: Option<Value>,
}

fn present_value<'de, D>(deserializer: D) -> Result<Option<Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    // 用 `Value::deserialize` 而不是 `Option<Value>`：后者会把 `null`
    // 折叠成 `None`，于是「设为 null」与「没给 value」就分不出来了。
    Ok(Some(Value::deserialize(deserializer)?))
}

async fn write(
    State(state): State<Arc<ServerState>>,
    Path(key): Path<String>,
    Json(body): Json<WriteBody>,
) -> Response {
    let Some(value) = body.value else {
        return envelope::compat_failure(&AppError::new(
            ErrorCode::DomainInvalidRange,
            "请求体缺少 value 字段",
        ));
    };

    let at = Clock::now(&SystemClock);
    match state.db.set_setting(&key, &value, at) {
        Ok(()) => envelope::ok(json!({ "success": true })),
        Err(error) => envelope::compat_failure(&error),
    }
}

async fn clear(State(state): State<Arc<ServerState>>, Path(key): Path<String>) -> Response {
    match state.db.clear_setting(&key) {
        // 旧 `clearSetting` 不返回行数，只表示成功；重复删除也算成功（幂等）
        Ok(_) => envelope::ok(json!({ "success": true })),
        Err(error) => envelope::compat_failure(&error),
    }
}
