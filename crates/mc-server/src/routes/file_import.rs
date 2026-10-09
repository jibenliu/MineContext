//! `POST /api/v1/files/import` —— 用户上传本地文件，抽取正文写入笔记树。

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock};
use serde::Deserialize;
use serde_json::json;

use crate::envelope;
use crate::file_ingest;
use crate::routes::files::decode_upload_payload;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new().route("/api/v1/files/import", post(import_file))
}

#[derive(Debug, Deserialize)]
struct ImportBody {
    name: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(default)]
    parent_id: Option<i64>,
}

async fn import_file(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<ImportBody>,
) -> Response {
    let bytes = match decode_upload_payload(body.data.as_ref()) {
        Ok(bytes) => bytes,
        Err(error) => return envelope::error_response(status_for(&error), &error),
    };
    let at = Clock::now(&SystemClock);
    match file_ingest::ingest_file(
        &state.db,
        &state.data_dir,
        &body.name,
        &bytes,
        body.parent_id,
        at,
    ) {
        Ok(result) => envelope::ok(json!({
            "id": result.id,
            "title": result.title,
            "name": result.name,
            "kind": result.kind.as_str(),
            "file_path": result.file_path,
        })),
        Err(error) => envelope::error_response(status_for(&error), &error),
    }
}

fn status_for(error: &AppError) -> StatusCode {
    match error.code() {
        ErrorCode::DomainInvalidRange => StatusCode::BAD_REQUEST,
        ErrorCode::ProviderInvalidResponse => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::StorageUnavailable => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
