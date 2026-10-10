//! `/api/indexing/status` 与 `/api/indexing/resume`：索引暂停要对设置/首页可见，且可一键恢复。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::state::IndexingPause;
use mc_server::{router, ServerState};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";

fn state() -> (tempfile::TempDir, Arc<ServerState>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(0),
        dir.path().to_path_buf(),
    ));
    (dir, state)
}

async fn call(state: &Arc<ServerState>, method: &str, uri: &str) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("host", "127.0.0.1:12345")
                .header("x-mc-token", TOKEN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{method} {uri}");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn status_is_clear_when_not_paused() {
    let (_dir, state) = state();
    let envelope = call(&state, "GET", "/api/indexing/status").await;
    assert_eq!(envelope["code"], 0);
    assert_eq!(envelope["data"]["paused"], false);
    assert!(envelope["data"]["indexing_pause"].is_null());
}

#[tokio::test]
async fn status_names_pause_reason_and_resume_clears_it() {
    let (_dir, state) = state();
    state.set_indexing_pause(IndexingPause {
        code: "api_key_invalid".into(),
        message: "向量索引已暂停：API Key 无效或已过期。".into(),
        component: "embedding".into(),
    });

    let status = call(&state, "GET", "/api/indexing/status").await;
    assert_eq!(status["data"]["paused"], true);
    let pause = &status["data"]["indexing_pause"];
    assert_eq!(pause["code"], "api_key_invalid");
    assert!(
        pause["message"]
            .as_str()
            .unwrap_or_default()
            .contains("暂停"),
        "{pause}"
    );
    assert_eq!(pause["action"]["target"], "resume_indexing");
    assert_eq!(pause["action"]["label"], "恢复索引");

    let before = state.indexing_resume_epoch();
    let resumed = call(&state, "POST", "/api/indexing/resume").await;
    assert_eq!(resumed["data"]["resumed"], true);
    assert_eq!(resumed["data"]["paused"], false);
    assert!(state.indexing_pause().is_none());
    assert!(
        state.indexing_resume_epoch() > before,
        "resume 必须推进代数，worker 才能在不改配置时重试"
    );
}
