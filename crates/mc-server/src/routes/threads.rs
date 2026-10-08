//! 线索（Thread）接口。
//!
//! 回答「APEX-389 到哪了」：同一个实体的活动按天串起来，
//! 并给出**确定性**的跨天进展（不调模型 —— 用户要的是事实清单）。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use crate::envelope;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new().route("/api/v1/threads", get(list_threads))
}

#[derive(Debug, Deserialize)]
pub struct ThreadsQuery {
    limit: Option<usize>,
}

pub async fn list_threads(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<ThreadsQuery>,
) -> Response {
    let timezone = state
        .config
        .current()
        .config
        .general
        .timezone
        .clone()
        .unwrap_or_else(|| "UTC".to_string());

    match crate::retrieval::threads(&state.db, &timezone, query.limit.unwrap_or(20)) {
        Ok(threads) => envelope::ok(json!({
            "timezone": timezone,
            "threads": threads
                .iter()
                .map(|thread| json!({
                    "entity": {
                        "kind": thread.entity.kind,
                        "canonical": thread.entity.canonical,
                        "display": thread.entity.display,
                    },
                    "days": thread.days.iter().map(|day| day.to_string()).collect::<Vec<_>>(),
                    "activities": thread.activities.iter().map(|activity| json!({
                        "id": activity.id,
                        "title": activity.title,
                        "start": activity.start.to_rfc3339(),
                        "end": activity.end.to_rfc3339(),
                        "origin": activity.provenance,
                    })).collect::<Vec<_>>(),
                    "observed_count": thread.observed_count(),
                    "inferred_count": thread.inferred_count(),
                    "brief": mc_memory::thread::thread_brief(thread),
                }))
                .collect::<Vec<_>>(),
        })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}
