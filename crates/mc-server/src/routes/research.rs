//! `POST /api/v1/research` —— 主题 + 公开 URL 汇编成一篇研究笔记。

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
use crate::research_ingest;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new().route("/api/v1/research", post(import_research))
}

#[derive(Debug, Deserialize)]
struct ImportBody {
    topic: String,
    urls: Vec<String>,
    #[serde(default)]
    parent_id: Option<i64>,
}

async fn import_research(
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

    match research_ingest::ingest_research(
        &state.db,
        transport.as_ref(),
        &blocked,
        &body.topic,
        &body.urls,
        body.parent_id,
        at,
    )
    .await
    {
        Ok(result) => envelope::ok(json!({
            "id": result.id,
            "title": result.title,
            "sources": result.sources,
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
