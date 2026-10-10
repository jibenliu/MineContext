//! Lite 任务关联：可纠正、可重放，能回答「上次在做什么」。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::Database;
use tower::ServiceExt;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn ctx() -> (tempfile::TempDir, Arc<ServerState>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        "test-token".to_string(),
        at(0),
        dir.path().to_path_buf(),
    ));
    (dir, state)
}

fn seed_all(state: &ServerState, rows: &[(&str, &str, i64)]) {
    let activities = rows
        .iter()
        .map(
            |(id, title, end_offset)| mc_domain::activity::ActivityView {
                id: (*id).to_string(),
                start: at(end_offset - 60),
                end: at(*end_offset),
                title: (*title).to_string(),
                original_title: (*title).to_string(),
                category: Some("开发".into()),
                observations: Vec::new(),
                origin: mc_domain::activity::Provenance::Observed,
                confidence: 1.0,
                is_user_modified: false,
            },
        )
        .collect();
    mc_storage::projectors::activities::store(
        state.db.as_ref(),
        &mc_domain::projector::Projection {
            activities,
            noise: Vec::new(),
            ai_requests: 0,
            unknown_event_kinds: 0,
        },
        0,
        at(rows.last().map(|r| r.2).unwrap_or(0)),
    )
    .unwrap();
}

async fn get(state: &Arc<ServerState>, uri: &str) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header("host", "127.0.0.1:12345")
                .header("x-mc-token", "test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

async fn post(state: &Arc<ServerState>, uri: &str, body: serde_json::Value) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("host", "127.0.0.1:12345")
                .header("x-mc-token", "test-token")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn answers_what_was_i_last_working_on_and_accepts_correction() {
    let (_dir, state) = ctx();
    seed_all(
        &state,
        &[
            ("act-old", "Browsing MineContext README", 100),
            ("act-new", "Fix APEX-389 acceptance in mc-server", 200),
        ],
    );

    let json = get(&state, "/api/v1/tasks/last").await;
    let task = &json["data"]["task"];
    assert_eq!(task["task_id"], "apex-389", "{json}");
    assert!(
        task["activity_ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == "act-new"),
        "{json}"
    );

    // 纠正：不改 Observation，只追加事件；重放后仍是纠正结果。
    let _ = post(
        &state,
        "/api/v1/tasks/correct",
        serde_json::json!({
            "activity_id": "act-new",
            "to_task_id": "release-1.0.8",
            "label": "Release 1.0.8"
        }),
    )
    .await;

    let again = get(&state, "/api/v1/tasks/last").await;
    assert_eq!(again["data"]["task"]["task_id"], "release-1.0.8", "{again}");
    assert_eq!(again["data"]["task"]["source"], "correction");

    // 再 sync 不应把纠正冲掉（已绑定活动跳过推断）。
    let _ = post(&state, "/api/v1/tasks/sync", serde_json::json!({})).await;
    let stable = get(&state, "/api/v1/tasks/last").await;
    assert_eq!(stable["data"]["task"]["task_id"], "release-1.0.8");
}
