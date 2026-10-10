//! 销售与客户跟进记忆 API（本地优先，无 CRM OAuth）。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use mc_common::time::{Clock, SystemClock};
use serde::Deserialize;
use serde_json::json;

use crate::envelope;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/v1/sales/timeline", get(timeline))
        .route("/api/v1/sales/follow-ups", get(follow_ups))
        .route("/api/v1/sales/visit-prep", get(visit_prep))
}

pub async fn timeline(State(state): State<Arc<ServerState>>) -> Response {
    match crate::sales_memory::timeline(&state.db) {
        Ok(events) => envelope::ok(json!({
            "events": events.iter().map(|e| json!({
                "contact_id": e.contact_id,
                "display_name": e.display_name,
                "summary": e.summary,
                "source_id": e.source_id,
                "at": e.at.as_millis(),
            })).collect::<Vec<_>>(),
        })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

pub async fn follow_ups(State(state): State<Arc<ServerState>>) -> Response {
    match crate::sales_memory::follow_ups(&state.db, SystemClock.now()) {
        Ok(hints) => envelope::ok(json!({
            "follow_ups": hints.iter().map(|h| json!({
                "contact_id": h.contact_id,
                "hint": h.hint,
                "reason": h.reason,
                "at": h.at.as_millis(),
            })).collect::<Vec<_>>(),
        })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

#[derive(Debug, Deserialize)]
pub struct VisitPrepQuery {
    pub contact_id: String,
}

pub async fn visit_prep(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<VisitPrepQuery>,
) -> Response {
    match crate::sales_memory::visit_prep(&state.db, query.contact_id.trim(), SystemClock.now()) {
        Ok(Some(pack)) => envelope::ok(json!({
            "pack": {
                "contact_id": pack.contact_id,
                "display_name": pack.display_name,
                "prep_notes": pack.prep_notes,
                "recent": pack.recent.iter().map(|e| json!({
                    "summary": e.summary,
                    "at": e.at.as_millis(),
                })).collect::<Vec<_>>(),
                "commitments": pack.commitments.iter().map(|c| json!({
                    "text": c.text,
                    "at": c.at.as_millis(),
                })).collect::<Vec<_>>(),
                "follow_ups": pack.follow_ups.iter().map(|h| json!({
                    "hint": h.hint,
                    "reason": h.reason,
                })).collect::<Vec<_>>(),
            }
        })),
        Ok(None) => envelope::ok(json!({ "pack": null })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}
