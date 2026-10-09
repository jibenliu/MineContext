//! `/api/privacy`：设置页「允许 AI 出网」读写。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
    config_path: std::path::PathBuf,
}

fn ctx() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, "[privacy]\nai_upload = false\n").unwrap();

    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let request = mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::File(config_path.clone())],
        env: Vec::new(),
        read_process_env: false,
    };
    let loaded = mc_config::load::load(&request).unwrap();
    let state = Arc::new(
        ServerState::new(
            mc_config::ConfigHandle::new(loaded),
            db,
            TOKEN.to_string(),
            Timestamp::from_millis(T0),
            dir.path().to_path_buf(),
        )
        .with_config_write(request, config_path.clone()),
    );
    Ctx {
        _dir: dir,
        state,
        config_path,
    }
}

async fn call(
    state: &Arc<ServerState>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN);
    let request = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            builder.body(Body::from(value.to_string())).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router(Arc::clone(state)).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

#[tokio::test]
async fn get_reports_default_ai_upload_off() {
    let ctx = ctx();
    let (status, json) = call(&ctx.state, "GET", "/api/privacy", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 0);
    assert_eq!(json["data"]["ai_upload"], false);
}

#[tokio::test]
async fn put_enables_ai_upload_and_persists() {
    let ctx = ctx();
    let (status, json) = call(
        &ctx.state,
        "PUT",
        "/api/privacy",
        Some(serde_json::json!({ "ai_upload": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["ai_upload"], true);

    let written = std::fs::read_to_string(&ctx.config_path).unwrap();
    assert!(
        written.contains("ai_upload = true") || written.contains("ai_upload=true"),
        "应写入用户配置：{written}"
    );

    let (status, json) = call(&ctx.state, "GET", "/api/privacy", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["ai_upload"], true);
}
