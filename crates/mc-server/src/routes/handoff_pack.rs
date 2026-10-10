//! 本地交接包 API（确认后导出；无 SSO / 团队云）。

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock};
use serde::Deserialize;
use serde_json::json;

use crate::envelope;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/v1/handoff/candidates", get(list_candidates))
        .route("/api/v1/handoff/confirm", post(confirm))
        .route("/api/v1/handoff/export", get(export_pack))
}

pub async fn list_candidates(State(state): State<Arc<ServerState>>) -> Response {
    let cands = match crate::handoff_pack::candidates(&state.db) {
        Ok(c) => c,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let confs = match crate::handoff_pack::load_confirmations(&state.db) {
        Ok(c) => c,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let confirmed: std::collections::HashSet<_> =
        confs.iter().map(|c| c.candidate_id.as_str()).collect();
    envelope::ok(json!({
        "candidates": cands.iter().map(|c| json!({
            "id": c.id,
            "kind": c.kind,
            "title": c.title,
            "body": c.body,
            "source_id": c.source_id,
            "at": c.at.as_millis(),
            "confirmed": confirmed.contains(c.id.as_str()),
        })).collect::<Vec<_>>(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct ConfirmBody {
    pub candidate_id: String,
}

pub async fn confirm(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<ConfirmBody>,
) -> Response {
    if body.candidate_id.trim().is_empty() {
        return envelope::error_response(
            StatusCode::BAD_REQUEST,
            &AppError::new(ErrorCode::ConfigInvalid, "candidate_id 不能为空"),
        );
    }
    match crate::handoff_pack::confirm(&state.db, body.candidate_id.trim(), SystemClock.now()) {
        Ok(()) => envelope::ok(json!({ "ok": true })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

pub async fn export_pack(State(state): State<Arc<ServerState>>) -> Response {
    match crate::handoff_pack::pack(&state.db, SystemClock.now()) {
        Ok(pack) => envelope::ok(json!({
            "manifest": {
                "format": pack.manifest.format,
                "schema_version": pack.manifest.schema_version,
                "exported_at_ms": pack.manifest.exported_at_ms,
                "item_count": pack.manifest.item_count,
            },
            "items": pack.items.iter().map(|i| json!({
                "id": i.id,
                "kind": i.kind,
                "title": i.title,
                "body": i.body,
                "confirmed_at": i.confirmed_at.as_millis(),
                "confirmed_by": i.confirmed_by,
            })).collect::<Vec<_>>(),
            "markdown": pack.markdown,
        })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}
