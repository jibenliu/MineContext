//! 项目风险与遗漏 API。

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
        .route("/api/v1/risks", get(list_risks))
        .route("/api/v1/risks/report", get(risk_report))
        .route("/api/v1/risks/dismiss", post(dismiss_risk))
}

pub async fn list_risks(State(state): State<Arc<ServerState>>) -> Response {
    match crate::risk_scan::scan(&state.db) {
        Ok(findings) => envelope::ok(json!({
            "findings": findings.iter().map(|f| json!({
                "id": f.id,
                "kind": f.kind,
                "text": f.text,
                "source_id": f.source_id,
                "source_kind": f.source_kind,
                "at": f.at.as_millis(),
                "score": f.score,
            })).collect::<Vec<_>>(),
        })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

pub async fn risk_report(State(state): State<Arc<ServerState>>) -> Response {
    match crate::risk_scan::report(&state.db) {
        Ok(markdown) => envelope::ok(json!({ "markdown": markdown })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

#[derive(Debug, Deserialize)]
pub struct DismissBody {
    pub finding_id: String,
}

pub async fn dismiss_risk(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<DismissBody>,
) -> Response {
    if body.finding_id.trim().is_empty() {
        return envelope::error_response(
            StatusCode::BAD_REQUEST,
            &AppError::new(ErrorCode::ConfigInvalid, "finding_id 不能为空"),
        );
    }
    match crate::risk_scan::dismiss(&state.db, body.finding_id.trim(), SystemClock.now()) {
        Ok(()) => envelope::ok(json!({ "ok": true })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}
