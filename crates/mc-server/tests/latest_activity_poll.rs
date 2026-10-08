//! `POST /api/v1/latest-activity/poll`（首页「最新活动」推送开关）。
//! 前端的用法（`latest-activity-card`）：挂载时发 `'running'`，卸载时发
//! `'stopped'`，真正的数据走 SSE 的 `push:latest-activity`。
//! 也就是说这个接口是**开关**，不是「取一次最新活动」。
//!
//! 两个必须钉住的行为：
//!
//! 1. `'stopped'` 之后**不再推送**（否则一次挂载就把轮询永久留在进程里，
//!    daemon 会一直为没人看的页面查库）；
//! 2. 同一个活动不重复推（卡片每次 `setLatestActivity` 都会重渲染）。

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::events::EVENT_PUSH_LATEST_ACTIVITY;
use mc_server::{latest_activity, router, ServerState};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as SESSION;
const MINUTE: i64 = 60_000;

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
        Timestamp::from_millis(SESSION),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

fn request(body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/latest-activity/poll")
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn call(state: &Arc<ServerState>, body: serde_json::Value) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(request(body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("必须返回 JSON 信封")
}

fn add_activity(state: &ServerState, id: &str, at_ms: i64) {
    let projection = mc_domain::projector::Projection {
        activities: vec![mc_domain::activity::ActivityView {
            id: id.to_string(),
            start: Timestamp::from_millis(at_ms - 5 * MINUTE),
            end: Timestamp::from_millis(at_ms),
            title: format!("活动 {id}"),
            original_title: format!("活动 {id}"),
            category: Some("开发".to_string()),
            observations: Vec::new(),
            origin: mc_domain::activity::Provenance::Observed,
            confidence: 1.0,
            is_user_modified: false,
        }],
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };
    mc_storage::projectors::activities::store(
        state.db.as_ref(),
        &projection,
        0,
        Timestamp::from_millis(at_ms),
    )
    .expect("存活动");
}

// ---------------------------------------------------------------- 开关

#[tokio::test]
async fn running_and_stopped_toggle_the_poller() {
    let ctx = ctx();

    assert!(!latest_activity::is_running(&ctx.state), "初始应当是关的");

    let envelope = call(&ctx.state, serde_json::json!({ "state": "running" })).await;
    assert_eq!(envelope["code"], 0, "{envelope}");
    assert!(
        latest_activity::is_running(&ctx.state),
        "running 之后必须处于开启状态"
    );

    let envelope = call(&ctx.state, serde_json::json!({ "state": "stopped" })).await;
    assert_eq!(envelope["code"], 0, "{envelope}");
    assert!(
        !latest_activity::is_running(&ctx.state),
        "stopped 之后必须关闭"
    );
}

#[tokio::test]
async fn unknown_state_is_rejected() {
    let ctx = ctx();
    let envelope = call(&ctx.state, serde_json::json!({ "state": "maybe" })).await;
    assert_eq!(envelope["code"], 1, "未知状态必须报错：{envelope}");
    assert!(!latest_activity::is_running(&ctx.state));
}

/// 重复 `running` 不该起第二个任务（否则 `stopped` 只能停掉其中一个）
#[tokio::test]
async fn repeated_running_does_not_stack_tasks() {
    let ctx = ctx();
    call(&ctx.state, serde_json::json!({ "state": "running" })).await;
    call(&ctx.state, serde_json::json!({ "state": "running" })).await;
    assert!(latest_activity::is_running(&ctx.state));

    call(&ctx.state, serde_json::json!({ "state": "stopped" })).await;
    assert!(
        !latest_activity::is_running(&ctx.state),
        "一次 stopped 必须把轮询彻底停掉"
    );
}

// ---------------------------------------------------------------- 推送

#[tokio::test]
async fn new_activity_is_pushed_once() {
    let ctx = ctx();
    let mut rx = ctx.state.events.subscribe();

    latest_activity::start(&ctx.state, Duration::from_millis(20));

    add_activity(&ctx.state, "act-1", SESSION + MINUTE);

    let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("必须收到推送")
        .expect("总线不该关闭");
    assert_eq!(event.kind, EVENT_PUSH_LATEST_ACTIVITY);
    // 形状与 `/api/db/activities/latest` 完全一致：`id` 是旧库的整数主键，
    // `metadata` / `resources` 是 **JSON 字符串**（前端直接当 Activity 渲染）。
    assert!(
        event.data["id"].as_i64().unwrap_or(0) > 0,
        "id 是旧库的整数主键：{:?}",
        event.data
    );
    assert_eq!(event.data["title"], "活动 act-1");
    assert!(event.data["start_time"].is_string(), "{:?}", event.data);
    assert!(
        event.data["metadata"].is_string(),
        "metadata 是 JSON 字符串"
    );

    // 同一个活动不重复推
    let second = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
    assert!(second.is_err(), "没有新活动时不该重复推送");

    latest_activity::stop(&ctx.state);
}

#[tokio::test]
async fn stopped_poller_pushes_nothing() {
    let ctx = ctx();
    let mut rx = ctx.state.events.subscribe();

    latest_activity::start(&ctx.state, Duration::from_millis(20));
    latest_activity::stop(&ctx.state);

    add_activity(&ctx.state, "act-1", SESSION + MINUTE);

    let result = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
    assert!(result.is_err(), "停掉之后不该再推送");
}

/// 没有订阅者时推送失败不算错误（`EventBus` 的语义），开关状态也不受影响
#[tokio::test]
async fn polling_survives_without_subscribers() {
    let ctx = ctx();
    latest_activity::start(&ctx.state, Duration::from_millis(10));
    add_activity(&ctx.state, "act-1", SESSION + MINUTE);
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(
        latest_activity::is_running(&ctx.state),
        "没人订阅也要继续跑（首页可能刚打开）"
    );
    latest_activity::stop(&ctx.state);
}
