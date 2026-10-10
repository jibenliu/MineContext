//! `/api/v1/vault/export` / `/api/v1/vault/import` —— 笔记树与 uploads 备份。

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock};
use serde::Deserialize;

use crate::envelope;
use crate::state::ServerState;
use crate::vault_backup::{self, Manifest};

pub fn router() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/v1/vault/export", get(export_vault))
        .route("/api/v1/vault/import", post(import_vault))
}

async fn export_vault(State(state): State<Arc<ServerState>>) -> Response {
    let at = Clock::now(&SystemClock);
    match vault_backup::build_export_zip(&state.db, &state.data_dir, at) {
        Ok((bytes, manifest)) => zip_download(bytes, &manifest),
        Err(error) => envelope::error_response(status_for(&error), &error),
    }
}

#[derive(Debug, Deserialize)]
struct ImportBody {
    /// base64 编码的备份 zip（与 `/api/files` 上传同形的字符串字段）。
    data: String,
}

async fn import_vault(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<ImportBody>,
) -> Response {
    let bytes = match vault_backup::decode_archive_base64(&body.data) {
        Ok(bytes) => bytes,
        Err(error) => return envelope::error_response(status_for(&error), &error),
    };
    let at = Clock::now(&SystemClock);
    match vault_backup::import_export_zip(&state.db, &state.data_dir, &bytes, at) {
        Ok(summary) => envelope::ok(serde_json::json!({
            "vault_count": summary.vault_count,
            "file_count": summary.file_count,
        })),
        Err(error) => envelope::error_response(status_for(&error), &error),
    }
}

fn zip_download(bytes: Vec<u8>, manifest: &Manifest) -> Response {
    let filename = "minecontext-vault-backup.zip";
    let disposition = format!("attachment; filename=\"{filename}\"");
    let mut response = (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/zip"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&disposition).unwrap_or_else(|_| {
                    HeaderValue::from_static("attachment; filename=\"minecontext-vault-backup.zip\"")
                }),
            ),
            (
                header::HeaderName::from_static("x-mc-vault-backup-schema"),
                HeaderValue::from_str(&manifest.schema_version.to_string())
                    .unwrap_or_else(|_| HeaderValue::from_static("1")),
            ),
        ],
        bytes,
    )
        .into_response();
    // 方便 curl / 调试：计数放进响应头，不污染 zip 本体
    if let Ok(value) = HeaderValue::from_str(&manifest.vault_count.to_string()) {
        response
            .headers_mut()
            .insert(header::HeaderName::from_static("x-mc-vault-count"), value);
    }
    if let Ok(value) = HeaderValue::from_str(&manifest.file_count.to_string()) {
        response
            .headers_mut()
            .insert(header::HeaderName::from_static("x-mc-file-count"), value);
    }
    response
}

fn status_for(error: &AppError) -> StatusCode {
    match error.code() {
        ErrorCode::DomainInvalidRange | ErrorCode::DomainInvariantViolated => {
            StatusCode::BAD_REQUEST
        }
        ErrorCode::StorageUnavailable => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
