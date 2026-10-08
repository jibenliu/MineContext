//! SSE 事件推送。
//!
//! 前端要「总结刚生成就出现在时间线上」。轮询既慢又费，
//! 因此 daemon 把事件推到一条 SSE 连接上，渲染层按事件名分发
//! （见 `frontend/src/renderer/src/adapters/channel-map.ts` 的订阅表）。
//!
//! 这里验证三件事：
//! 1. 连接建立后先收到 `ready`（渲染层依赖它做启动握手）；
//! 2. 事件能真的穿过 SSE 传到客户端；
//! 3. **总结落库的那一刻**就会推 `summary:created` —— 而不是等前端下次轮询。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::StreamExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::projectors::stages::StageRow;
use mc_storage::Database;
use tokio::time::{timeout, Duration};
use tower::ServiceExt;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

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
        "test-token".to_string(),
        at(0),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

fn sse_request() -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri("/api/v1/stream")
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", "test-token")
        .header("accept", "text/event-stream")
        .body(Body::empty())
        .unwrap()
}

/// 从 SSE 流里读下一帧，最多等 `budget`。
async fn next_frame(
    stream: &mut (impl futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin),
    budget: Duration,
) -> Option<String> {
    let chunk = timeout(budget, stream.next()).await.ok()??.ok()?;
    Some(String::from_utf8_lossy(&chunk).to_string())
}

#[tokio::test]
async fn stream_sends_ready_then_forwards_published_events() {
    let ctx = ctx();
    let response = router(Arc::clone(&ctx.state))
        .oneshot(sse_request())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut stream = response.into_body().into_data_stream();

    let ready = next_frame(&mut stream, Duration::from_secs(2))
        .await
        .expect("首帧必须是 ready");
    assert!(ready.contains("event: ready"), "{ready}");
    assert!(ready.contains("token_ok"), "渲染层读这个字段：{ready}");

    // 第二帧是启动帧（`push:init-check-data`，见下一个用例），先消费掉它
    let init = next_frame(&mut stream, Duration::from_secs(2))
        .await
        .expect("第二帧必须是启动帧");
    assert!(init.contains("event: push:init-check-data"), "{init}");

    // 模拟「后台刚写完一条总结」
    ctx.state.publish(
        mc_server::events::EVENT_SUMMARY_CREATED,
        serde_json::json!({ "id": "sum-1", "title": "17:00–17:30 阶段总结" }),
    );

    let frame = next_frame(&mut stream, Duration::from_secs(2))
        .await
        .expect("发布的事件必须能到达订阅者");
    assert!(
        frame.contains("event: summary:created"),
        "事件名就是前端的分发键：{frame}"
    );
    assert!(frame.contains("sum-1"), "{frame}");
}

#[tokio::test]
async fn events_without_subscribers_are_not_an_error() {
    let ctx = ctx();
    // 没有连接时发布：返回 false，但不该 panic 也不该影响后续订阅
    assert!(!ctx
        .state
        .events
        .publish("summary:created", serde_json::json!({})));

    let response = router(Arc::clone(&ctx.state))
        .oneshot(sse_request())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(ctx.state.events.subscriber_count(), 1);
}

/// 手动结束阶段 → 立刻推 `summary:created`（与 4.50 的接口行为配套）
#[tokio::test]
async fn manual_close_publishes_summary_created() {
    let ctx = ctx();
    seed_activity(&ctx.state, "act-1", 0, 1800);
    ctx.state
        .db
        .upsert_stages(
            &[StageRow {
                id: "stage-1".to_string(),
                start: at(0),
                end: None,
                state: "open".to_string(),
                end_reason: None,
                day: "2026-09-30".to_string(),
                activities: vec!["act-1".to_string()],
            }],
            0,
        )
        .expect("写阶段");

    // 先订阅，再触发
    let mut stream = router(Arc::clone(&ctx.state))
        .oneshot(sse_request())
        .await
        .unwrap()
        .into_body()
        .into_data_stream();
    let _ = next_frame(&mut stream, Duration::from_secs(2)).await;

    let response = router(Arc::clone(&ctx.state))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/stages/stage-1/close")
                .header("host", "127.0.0.1:12345")
                .header("x-mc-token", "test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // 事件可能在关闭响应之前/之后到达，这里给它一点时间
    let mut found = false;
    for _ in 0..4 {
        match next_frame(&mut stream, Duration::from_secs(2)).await {
            Some(frame) if frame.contains("event: summary:created") => {
                assert!(frame.contains("sum-stage-1"), "{frame}");
                found = true;
                break;
            }
            Some(_) => continue,
            None => break,
        }
    }
    assert!(found, "手动结束阶段必须推送 summary:created");
}

fn seed_activity(state: &ServerState, id: &str, start: i64, end: i64) {
    let projection = mc_domain::projector::Projection {
        activities: vec![mc_domain::activity::ActivityView {
            id: id.to_string(),
            start: at(start),
            end: at(end),
            title: format!("活动 {id}"),
            original_title: format!("活动 {id}"),
            category: Some("开发".to_string()),
            observations: vec![mc_domain::activity::ObservationRef {
                id: format!("obs-{id}"),
                at: at(start),
            }],
            origin: mc_domain::activity::Provenance::Rule {
                rule_id: "coding".to_string(),
            },
            confidence: 1.0,
            is_user_modified: false,
        }],
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };
    mc_storage::projectors::activities::store(state.db.as_ref(), &projection, 0, at(end))
        .expect("存活动");
}

// 启动帧：渲染层靠 `push:init-check-data` 判断「模型是否已配置」，
// 从而决定进主界面还是进引导页。不发这一帧的后果很具体：
// 已经配好模型的用户每次启动都会被要求重新选一遍模型。
#[tokio::test]
async fn stream_sends_init_check_payload_after_ready() {
    let ctx = ctx();
    let response = router(Arc::clone(&ctx.state))
        .oneshot(sse_request())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let mut stream = response.into_body().into_data_stream();

    let ready = next_frame(&mut stream, Duration::from_secs(2))
        .await
        .expect("首帧仍是 ready（渲染层依赖它做握手）");
    assert!(ready.contains("event: ready"), "{ready}");

    let frame = next_frame(&mut stream, Duration::from_secs(2))
        .await
        .expect("第二帧必须是启动帧");
    assert!(
        frame.contains("event: push:init-check-data"),
        "事件名就是渲染层的分发键：{frame}"
    );

    // 负载形状：渲染层 `JSON.parse(data)` 之后读 `data.components.llm`
    let payload = frame
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .expect("启动帧必须带 data");
    let parsed: serde_json::Value = serde_json::from_str(payload).expect("负载必须是 JSON");
    assert!(
        parsed["data"]["components"]["llm"].is_object(),
        "形状不对会让渲染层判断不出是否已配置：{parsed}"
    );
    assert!(
        parsed["data"]["components"]["llm"]["status"].is_string(),
        "{parsed}"
    );
}
