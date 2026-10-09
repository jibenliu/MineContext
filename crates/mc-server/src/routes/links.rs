//! `POST /api/v1/links` —— 用户提交公开链接，抓取正文写入笔记树。

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::post;
use axum::{Json, Router};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock};
use mc_providers::transport::HttpTransport;
use serde::Deserialize;
use serde_json::json;

use crate::envelope;
use crate::link_ingest::{self, default_link_transport};
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new().route("/api/v1/links", post(import_link))
}

#[derive(Debug, Deserialize)]
struct ImportBody {
    url: String,
    #[serde(default)]
    parent_id: Option<i64>,
}

async fn import_link(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<ImportBody>,
) -> Response {
    let transport = match resolve_transport(&state) {
        Ok(transport) => transport,
        Err(error) => {
            return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
        }
    };
    let blocked = state
        .config
        .current()
        .config
        .privacy
        .blocked_domains
        .clone();
    let at = Clock::now(&SystemClock);

    match link_ingest::ingest_link(
        &state.db,
        transport.as_ref(),
        &blocked,
        &body.url,
        body.parent_id,
        at,
    )
    .await
    {
        Ok(result) => envelope::ok(json!({
            "id": result.id,
            "title": result.title,
            "url": result.url,
            "source_host": result.source_host,
        })),
        Err(error) => envelope::error_response(status_for(&error), &error),
    }
}

fn resolve_transport(state: &ServerState) -> Result<Arc<dyn HttpTransport>, AppError> {
    if let Some(transport) = state.link_transport() {
        return Ok(transport);
    }
    default_link_transport()
}

fn status_for(error: &AppError) -> StatusCode {
    match error.code() {
        ErrorCode::DomainInvalidRange => StatusCode::BAD_REQUEST,
        ErrorCode::PrivacyBlocked => StatusCode::FORBIDDEN,
        ErrorCode::ProviderTimeout
        | ErrorCode::ProviderConnection
        | ErrorCode::ProviderServerError
        | ErrorCode::ProviderInvalidResponse => StatusCode::BAD_GATEWAY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
