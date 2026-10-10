//! 本地目录导入与文件跟踪：`/api/v1/files/import-folder`、`/api/v1/files/track*`。

use std::path::PathBuf;
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
use crate::folder_ingest;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/v1/files/import-folder", post(import_folder))
        .route(
            "/api/v1/files/track",
            get(list_tracked).post(track).delete(untrack),
        )
        .route("/api/v1/files/track/sync", post(sync_tracked))
}

#[derive(Debug, Deserialize)]
struct FolderBody {
    path: String,
    #[serde(default)]
    parent_id: Option<i64>,
    #[serde(default)]
    recursive: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct TrackBody {
    path: String,
}

#[derive(Debug, Deserialize)]
struct SyncBody {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    parent_id: Option<i64>,
}

async fn import_folder(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<FolderBody>,
) -> Response {
    let at = Clock::now(&SystemClock);
    let recursive = body.recursive.unwrap_or(true);
    match folder_ingest::import_folder(
        &state.db,
        &state.data_dir,
        PathBuf::from(body.path.trim()).as_path(),
        body.parent_id,
        recursive,
        at,
    ) {
        Ok(result) => envelope::ok(folder_json(&result)),
        Err(error) => envelope::error_response(status_for(&error), &error),
    }
}

async fn track(State(state): State<Arc<ServerState>>, Json(body): Json<TrackBody>) -> Response {
    let at = Clock::now(&SystemClock);
    match folder_ingest::track_folder(&state.db, PathBuf::from(body.path.trim()).as_path(), at) {
        Ok(folder) => envelope::ok(json!({ "path": folder.path })),
        Err(error) => envelope::error_response(status_for(&error), &error),
    }
}

async fn untrack(State(state): State<Arc<ServerState>>, Json(body): Json<TrackBody>) -> Response {
    let at = Clock::now(&SystemClock);
    match folder_ingest::untrack_folder(&state.db, PathBuf::from(body.path.trim()).as_path(), at) {
        Ok(()) => envelope::ok(json!({ "success": true })),
        Err(error) => envelope::error_response(status_for(&error), &error),
    }
}

async fn list_tracked(State(state): State<Arc<ServerState>>) -> Response {
    match folder_ingest::list_tracked(&state.db) {
        Ok(folders) => envelope::ok(json!({
            "folders": folders.iter().map(|f| json!({ "path": f.path })).collect::<Vec<_>>(),
        })),
        Err(error) => envelope::error_response(status_for(&error), &error),
    }
}

async fn sync_tracked(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<SyncBody>,
) -> Response {
    let at = Clock::now(&SystemClock);
    let only = body
        .path
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    match folder_ingest::sync_tracked(
        &state.db,
        &state.data_dir,
        only.as_deref(),
        body.parent_id,
        at,
    ) {
        Ok(result) => envelope::ok(folder_json(&result)),
        Err(error) => envelope::error_response(status_for(&error), &error),
    }
}

fn folder_json(result: &folder_ingest::FolderImportResult) -> serde_json::Value {
    json!({
        "path": result.path,
        "imported": result.imported.iter().map(|item| json!({
            "id": item.id,
            "title": item.title,
            "name": item.name,
            "kind": item.kind.as_str(),
            "file_path": item.file_path,
        })).collect::<Vec<_>>(),
        "skipped": result.skipped,
        "errors": result.errors,
    })
}

fn status_for(error: &AppError) -> StatusCode {
    match error.code() {
        ErrorCode::DomainInvalidRange => StatusCode::BAD_REQUEST,
        ErrorCode::StorageUnavailable => StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::ProviderInvalidResponse => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
