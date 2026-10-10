//! Lite 任务关联 API。

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
        .route("/api/v1/tasks/last", get(last_working_on))
        .route("/api/v1/tasks", get(list_tasks))
        .route("/api/v1/tasks/sync", post(sync_tasks))
        .route("/api/v1/tasks/correct", post(correct_task))
}

/// `GET /api/v1/tasks/last` —— 「我上次在做什么」
pub async fn last_working_on(State(state): State<Arc<ServerState>>) -> Response {
    // 先补一轮推断，再回答；推断幂等，不改 Observation。
    let now = SystemClock.now();
    if let Err(error) = crate::task_assoc::sync_inferred(&state.db, now) {
        return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    match crate::task_assoc::last_task(&state.db) {
        Ok(Some(task)) => envelope::ok(json!({
            "task": {
                "task_id": task.task_id,
                "label": task.label,
                "activity_ids": task.activity_ids,
                "last_at": task.last_at.as_millis(),
                "source": task.source,
            }
        })),
        Ok(None) => envelope::ok(json!({ "task": null })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

pub async fn list_tasks(State(state): State<Arc<ServerState>>) -> Response {
    let now = SystemClock.now();
    if let Err(error) = crate::task_assoc::sync_inferred(&state.db, now) {
        return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    match crate::task_assoc::current_projections(&state.db) {
        Ok(tasks) => envelope::ok(json!({
            "tasks": tasks.iter().map(|task| json!({
                "task_id": task.task_id,
                "label": task.label,
                "activity_ids": task.activity_ids,
                "last_at": task.last_at.as_millis(),
                "source": task.source,
            })).collect::<Vec<_>>(),
        })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

pub async fn sync_tasks(State(state): State<Arc<ServerState>>) -> Response {
    match crate::task_assoc::sync_inferred(&state.db, SystemClock.now()) {
        Ok(count) => envelope::ok(json!({ "associated": count })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

#[derive(Debug, Deserialize)]
pub struct CorrectBody {
    pub activity_id: String,
    pub to_task_id: String,
    pub label: Option<String>,
}

pub async fn correct_task(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<CorrectBody>,
) -> Response {
    if body.activity_id.trim().is_empty() || body.to_task_id.trim().is_empty() {
        return envelope::error_response(
            StatusCode::BAD_REQUEST,
            &AppError::new(
                ErrorCode::ConfigInvalid,
                "activity_id 与 to_task_id 不能为空",
            ),
        );
    }
    let label = body.label.unwrap_or_else(|| body.to_task_id.clone());
    match crate::task_assoc::correct_association(
        &state.db,
        body.activity_id.trim(),
        body.to_task_id.trim(),
        label.trim(),
        SystemClock.now(),
    ) {
        Ok(()) => envelope::ok(json!({ "ok": true })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}
