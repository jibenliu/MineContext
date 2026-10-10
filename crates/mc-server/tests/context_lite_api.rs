//! context-lite：从检索 + 引用打出可用的上下文包。

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

fn seed_activities(state: &ServerState, rows: &[(&str, &str)]) {
    let activities = rows
        .iter()
        .enumerate()
        .map(|(index, (id, title))| {
            state
                .db
                .insert_observation(&mc_storage::observations::NewObservation {
                    id: format!("obs-{id}"),
                    ts: at(index as i64),
                    source_id: "macos:screen".to_string(),
                    kind: "screen".to_string(),
                    app_name: Some("Code".to_string()),
                    app_bundle_id: None,
                    window_title: Some((*title).lines().next().unwrap_or(title).to_string()),
                    domain: None,
                    display_id: None,
                    scale_factor: None,
                    image: None,
                    text_content: None,
                    text_origin: None,
                    change_kind: "pixel_major".to_string(),
                    privacy_verdict: "allow".to_string(),
                    phash: None,
                    idempotency: format!("idem-{id}"),
                })
                .unwrap();
            mc_domain::activity::ActivityView {
                id: (*id).to_string(),
                start: at(index as i64),
                end: at(index as i64 + 1800),
                title: (*title).to_string(),
                original_title: (*title).to_string(),
                category: Some("开发".to_string()),
                observations: vec![mc_domain::activity::ObservationRef {
                    id: format!("obs-{id}"),
                    at: at(index as i64),
                }],
                origin: mc_domain::activity::Provenance::Rule {
                    rule_id: "coding".to_string(),
                },
                confidence: 1.0,
                is_user_modified: false,
            }
        })
        .collect();
    let projection = mc_domain::projector::Projection {
        activities,
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };
    mc_storage::projectors::activities::store(state.db.as_ref(), &projection, 0, at(1800))
        .expect("存活动");
}

#[tokio::test]
async fn builds_usable_context_pack_from_retrieval_and_citations() {
    let (_dir, state) = ctx();
    seed_activities(
        &state,
        &[
            (
                "act-auth",
                "Fix auth token refresh\nEdited src/auth/refresh.rs and verified cookie rotation.",
            ),
            (
                "act-other",
                "Standup notes\nDiscussed sprint goals unrelated to tokens.",
            ),
        ],
    );

    let app = router(Arc::clone(&state));
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/context/pack?q=auth%20refresh&budget=500")
                .header("host", "127.0.0.1:12345")
                .header("x-mc-token", "test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let data = &json["data"];
    assert_eq!(data["query"], "auth refresh");
    let items = data["items"].as_array().expect("items");
    assert!(!items.is_empty(), "应至少打出一条证据");
    assert!(
        items
            .iter()
            .any(|item| item["title"].as_str().unwrap_or("").contains("auth")),
        "包内应含与查询相关的活动标题: {data}"
    );
    let citations = data["citations"].as_array().expect("citations");
    assert_eq!(citations.len(), items.len());
    assert!(data["total_tokens"].as_u64().unwrap() <= data["budget"].as_u64().unwrap());
    let text = data["text"].as_str().unwrap();
    assert!(
        text.contains("auth") || text.contains("refresh"),
        "渲染文本应可用: {text}"
    );
}
