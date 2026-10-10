//! 只读 MCP Server：经检索/记忆 API 暴露工具；fail-closed + ai_upload。

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

fn state_with(toml: &str) -> (tempfile::TempDir, Arc<ServerState>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let loaded = mc_config::load::load(&mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::Inline {
            name: "test".into(),
            toml: toml.into(),
        }],
        env: Vec::new(),
        read_process_env: false,
    })
    .expect("config");
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(loaded),
        db,
        "test-token".to_string(),
        at(0),
        dir.path().to_path_buf(),
    ));
    (dir, state)
}

fn seed_activity(state: &ServerState, id: &str, title: &str) {
    let projection = mc_domain::projector::Projection {
        activities: vec![mc_domain::activity::ActivityView {
            id: id.into(),
            start: at(0),
            end: at(60),
            title: title.into(),
            original_title: title.into(),
            category: Some("开发".into()),
            observations: Vec::new(),
            origin: mc_domain::activity::Provenance::Observed,
            confidence: 1.0,
            is_user_modified: false,
        }],
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };
    mc_storage::projectors::activities::store(&state.db, &projection, 0, at(60)).unwrap();
}

async fn rpc(state: &Arc<ServerState>, body: serde_json::Value) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/mcp/serve/rpc")
                .header("host", "127.0.0.1:12345")
                .header("x-mc-token", "test-token")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn serve_tools_fail_closed_without_ai_upload() {
    let (_dir, state) = state_with(
        r#"
[mcp]
enabled = true
[mcp.serve]
enabled = true
allowed_tools = ["search", "activity", "memory", "context_pack"]
[privacy]
ai_upload = false
"#,
    );
    let resp = rpc(
        &state,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "search", "arguments": { "q": "auth" } }
        }),
    )
    .await;
    assert!(resp.get("error").is_some(), "{resp}");
    assert!(
        resp["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("ai_upload"),
        "{resp}"
    );
}

#[tokio::test]
async fn serve_exposes_search_activity_and_context_pack_when_authorized() {
    let (_dir, state) = state_with(
        r#"
[mcp]
enabled = true
[mcp.serve]
enabled = true
allowed_tools = ["search", "activity", "memory", "context_pack"]
[privacy]
ai_upload = true
"#,
    );
    seed_activity(
        &state,
        "act-1",
        "Fix auth token refresh\nTouched src/auth.rs",
    );

    let listed = rpc(
        &state,
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    )
    .await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"search"));
    assert!(names.contains(&"context_pack"));
    assert!(names.contains(&"activity"));
    assert!(names.contains(&"memory"));

    let search = rpc(
        &state,
        serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"tools/call",
            "params":{"name":"search","arguments":{"q":"auth"}}
        }),
    )
    .await;
    assert_eq!(search["result"]["isError"], false, "{search}");
    let text = search["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("auth") || text.contains("act-1"), "{text}");

    let pack = rpc(
        &state,
        serde_json::json!({
            "jsonrpc":"2.0","id":3,"method":"tools/call",
            "params":{"name":"context_pack","arguments":{"q":"auth","budget":400}}
        }),
    )
    .await;
    assert_eq!(pack["result"]["isError"], false, "{pack}");
    let pack_text = pack["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        pack_text.contains("auth") || pack_text.contains("refresh"),
        "{pack_text}"
    );

    let activity = rpc(
        &state,
        serde_json::json!({
            "jsonrpc":"2.0","id":4,"method":"tools/call",
            "params":{"name":"activity","arguments":{"limit":5}}
        }),
    )
    .await;
    assert_eq!(activity["result"]["isError"], false, "{activity}");
}
