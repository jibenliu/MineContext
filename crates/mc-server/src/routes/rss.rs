//! `POST /api/v1/rss` —— 用户提交公开 RSS/Atom URL，条目写入笔记树。

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
use crate::link_ingest::default_link_transport;
use crate::rss_ingest;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new().route("/api/v1/rss", post(import_rss))
}

#[derive(Debug, Deserialize)]
struct ImportBody {
    url: String,
    #[serde(default)]
    parent_id: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn import_rss(
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

    match rss_ingest::ingest_rss(
        &state.db,
        transport.as_ref(),
        &blocked,
        &body.url,
        body.parent_id,
        body.limit,
        at,
    )
    .await
    {
        Ok(result) => envelope::ok(json!({
            "feed_title": result.feed_title,
            "feed_url": result.feed_url,
            "source_host": result.source_host,
            "imported": result.imported.iter().map(|item| json!({
                "id": item.id,
                "title": item.title,
                "link": item.link,
            })).collect::<Vec<_>>(),
            "skipped": result.skipped,
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
