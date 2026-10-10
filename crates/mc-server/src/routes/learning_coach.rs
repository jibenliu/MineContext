//! 学习轨迹与间隔复习 API。

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use mc_common::time::{Clock, SystemClock};
use serde_json::json;

use crate::envelope;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/v1/learning/topics", get(topics))
        .route("/api/v1/learning/stuck", get(stuck))
        .route("/api/v1/learning/review-plan", get(review_plan))
}

pub async fn topics(State(state): State<Arc<ServerState>>) -> Response {
    match crate::learning_coach::topics(&state.db) {
        Ok(topics) => envelope::ok(json!({
            "topics": topics.iter().map(|t| json!({
                "topic_id": t.topic_id,
                "label": t.label,
                "hit_count": t.hit_count,
                "last_at": t.last_at.as_millis(),
                "source_ids": t.source_ids,
            })).collect::<Vec<_>>(),
        })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

pub async fn stuck(State(state): State<Arc<ServerState>>) -> Response {
    match crate::learning_coach::stuck(&state.db) {
        Ok(patterns) => envelope::ok(json!({
            "patterns": patterns.iter().map(|p| json!({
                "pattern_id": p.pattern_id,
                "label": p.label,
                "repeats": p.repeats,
                "last_at": p.last_at.as_millis(),
                "hint": p.hint,
            })).collect::<Vec<_>>(),
        })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

pub async fn review_plan(State(state): State<Arc<ServerState>>) -> Response {
    match crate::learning_coach::review_plan(&state.db, SystemClock.now()) {
        Ok(plan) => envelope::ok(json!({
            "generated_at": plan.generated_at.as_millis(),
            "summary": plan.summary,
            "items": plan.items.iter().map(|i| json!({
                "topic_id": i.topic_id,
                "label": i.label,
                "due_at": i.due_at.as_millis(),
                "interval_days": i.interval_days,
                "reason": i.reason,
            })).collect::<Vec<_>>(),
        })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}
