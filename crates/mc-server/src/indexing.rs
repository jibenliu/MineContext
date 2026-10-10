//! 向量索引暂停态的查询与一键恢复。
//!
//! embedding worker 在 401/429 等上游拒绝后会停轮询；这里把暂停原因暴露给
//! 设置页 / 首页，并提供 `POST /api/indexing/resume` 让用户修好密钥后立刻重试。

use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

use crate::envelope;
use crate::state::{IndexingPause, ServerState};

pub fn router() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/indexing/status", get(status))
        .route("/api/indexing/resume", post(resume))
}

/// 把暂停态拼成前端可渲染的 JSON（status 与 recording-stats 共用）。
pub fn pause_payload(pause: Option<&IndexingPause>) -> Value {
    match pause {
        None => Value::Null,
        Some(pause) => json!({
            "paused": true,
            "code": pause.code,
            "message": pause.message,
            "component": pause.component,
            "action": {
                "target": "resume_indexing",
                "label": "恢复索引",
            },
        }),
    }
}

async fn status(State(state): State<Arc<ServerState>>) -> Response {
    let pause = state.indexing_pause();
    envelope::ok(json!({
        "paused": pause.is_some(),
        "indexing_pause": pause_payload(pause.as_ref()),
    }))
}

async fn resume(State(state): State<Arc<ServerState>>) -> Response {
    state.request_indexing_resume();
    envelope::ok(json!({
        "resumed": true,
        "paused": false,
        "indexing_pause": Value::Null,
    }))
}
