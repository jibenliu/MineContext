//! `/api/privacy` —— 隐私出网开关（`privacy.ai_upload`）与 AI 总开关。
//!
//! 默认不出网；设置页与录制统计「去开启」入口都写这里，避免用户去改 config.toml。

use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use mc_common::error::{AppError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::envelope;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new().route("/api/privacy", get(read).put(write))
}

async fn read(State(state): State<Arc<ServerState>>) -> Response {
    let config = state.config.current();
    envelope::ok(json!({
        "ai_upload": config.config.privacy.ai_upload,
        "ai_enabled": config.config.ai.enabled,
    }))
}

#[derive(Debug, Deserialize)]
struct WriteBody {
    #[serde(default)]
    ai_upload: Option<bool>,
    #[serde(default)]
    ai_enabled: Option<bool>,
}

async fn write(State(state): State<Arc<ServerState>>, Json(body): Json<WriteBody>) -> Response {
    if body.ai_upload.is_none() && body.ai_enabled.is_none() {
        return envelope::compat_failure(&AppError::new(
            ErrorCode::ConfigInvalid,
            "请求体至少包含 ai_upload 或 ai_enabled",
        ));
    }

    let mut patch = serde_json::Map::new();
    if let Some(ai_upload) = body.ai_upload {
        patch.insert("privacy".to_string(), json!({ "ai_upload": ai_upload }));
    }
    if let Some(ai_enabled) = body.ai_enabled {
        patch.insert("ai".to_string(), json!({ "enabled": ai_enabled }));
    }

    match crate::config_api::apply_patch(&state, Value::Object(patch)) {
        Ok(loaded) => envelope::ok(json!({
            "ai_upload": loaded.config.privacy.ai_upload,
            "ai_enabled": loaded.config.ai.enabled,
        })),
        Err(error) => envelope::compat_failure(&error),
    }
}
