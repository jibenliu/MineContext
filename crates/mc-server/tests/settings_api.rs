//! `/api/settings/{key}`（旧 `electron-store` 的 HTTP 面）。
//!
//! 渲染层用这些接口存采集设置与「已完成」标记。三条必须对齐的行为：
//!
//! - `getSettings<T>(key)` 返回**原始 JSON 值**（不是 `{value}` 包装），
//!   键不存在时返回 `null`（旧 `store.get()` 返回 `undefined`）；
//! - `setSettings` / `clearSettings` 返回 `{success: boolean}`；
//! - 值可以是任意 JSON（布尔、数组、对象），不能只支持字符串。

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
}

fn ctx() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

fn request(method: &str, uri: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN);
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

async fn call(
    state: &Arc<ServerState>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(request(method, uri, body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{method} {uri}");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("必须返回 JSON 信封")
}

fn data(envelope: &serde_json::Value) -> serde_json::Value {
    assert_eq!(envelope["code"], 0, "{envelope}");
    envelope["data"].clone()
}

#[tokio::test]
async fn missing_key_returns_null() {
    let ctx = ctx();
    let envelope = call(&ctx.state, "GET", "/api/settings/todoList-finished", None).await;
    assert!(
        data(&envelope).is_null(),
        "旧 store.get() 返回 undefined → null"
    );
}

#[tokio::test]
async fn values_round_trip_through_http() {
    let ctx = ctx();

    let values = [
        ("todoList-finished", serde_json::json!(true)),
        (
            "todoList",
            serde_json::json!([{ "id": 1, "content": "写测试", "status": 0 }]),
        ),
        (
            "capture",
            serde_json::json!({ "interval": 30, "targets": ["display-1"] }),
        ),
    ];

    for (key, value) in &values {
        let envelope = call(
            &ctx.state,
            "PUT",
            &format!("/api/settings/{key}"),
            Some(serde_json::json!({ "value": value })),
        )
        .await;
        assert_eq!(data(&envelope)["success"], true, "{envelope}");

        let envelope = call(&ctx.state, "GET", &format!("/api/settings/{key}"), None).await;
        assert_eq!(&data(&envelope), value, "键 {key} 必须原样往返");
    }
}

#[tokio::test]
async fn clear_removes_the_value() {
    let ctx = ctx();
    call(
        &ctx.state,
        "PUT",
        "/api/settings/k",
        Some(serde_json::json!({ "value": 1 })),
    )
    .await;

    let envelope = call(&ctx.state, "DELETE", "/api/settings/k", None).await;
    assert_eq!(data(&envelope)["success"], true);

    let envelope = call(&ctx.state, "GET", "/api/settings/k", None).await;
    assert!(data(&envelope).is_null());

    // 再删一次也是成功（幂等）
    let envelope = call(&ctx.state, "DELETE", "/api/settings/k", None).await;
    assert_eq!(data(&envelope)["success"], true);
}

#[tokio::test]
async fn invalid_keys_are_reported() {
    let ctx = ctx();

    // 空 key 走不到路由（路径为空），用一个超长 key 验证校验生效
    let long = "x".repeat(200);
    let envelope = call(
        &ctx.state,
        "PUT",
        &format!("/api/settings/{long}"),
        Some(serde_json::json!({ "value": 1 })),
    )
    .await;
    assert_eq!(envelope["code"], 1, "超长 key 必须被拒绝：{envelope}");
}

/// 值缺失与显式 `null` 是两件事：前者是调用方写错了，后者是合法值。
#[tokio::test]
async fn put_without_value_is_rejected_but_null_is_allowed() {
    let ctx = ctx();

    let envelope = call(
        &ctx.state,
        "PUT",
        "/api/settings/k",
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(envelope["code"], 1, "缺少 value 字段必须报错：{envelope}");

    let envelope = call(
        &ctx.state,
        "PUT",
        "/api/settings/k",
        Some(serde_json::json!({ "value": null })),
    )
    .await;
    assert_eq!(data(&envelope)["success"], true);

    let envelope = call(&ctx.state, "GET", "/api/settings/k", None).await;
    assert!(data(&envelope).is_null());
}
