//! 补偿作业：入队 → 消费 → 查状态。
//!
//! 队列表、`JobKind`、幂等键与租约此前就有，但**没有任何生产者与消费者**。
//! 这一组测试钉住「补上之后」的三条契约：
//! 同一范围重复入队只有一条作业；没有模型时**如实跳过**而不是假装成功；
//! 队列里没接线的类型会被明确跳过（不留悬空状态）。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::jobs::JobState;
use mc_storage::Database;
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

fn post(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", "test-token")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", "test-token")
        .body(Body::empty())
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Runtime::new().unwrap().block_on(future)
}

#[test]
fn same_range_is_enqueued_once() {
    let ctx = ctx();
    let body = serde_json::json!({ "from": "2026-09-30T09:00:00Z", "to": "2026-09-30T18:00:00Z" });

    let first = block_on(async {
        let response = router(Arc::clone(&ctx.state))
            .oneshot(post("/api/v1/jobs/backfill", body.clone()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response).await
    });
    assert_eq!(first["code"], 0, "{first}");
    assert_eq!(first["data"]["deduped"], false, "首次入队不是去重：{first}");

    let second = block_on(async {
        let response = router(Arc::clone(&ctx.state))
            .oneshot(post("/api/v1/jobs/backfill", body))
            .await
            .unwrap();
        body_json(response).await
    });
    assert_eq!(
        second["data"]["job_id"], first["data"]["job_id"],
        "同一范围必须命中同一条作业（幂等键）：{second}"
    );
    assert_eq!(second["data"]["deduped"], true);
}

#[test]
fn bad_range_is_rejected_before_enqueue() {
    let ctx = ctx();
    let response = block_on(router(Arc::clone(&ctx.state)).oneshot(post(
        "/api/v1/jobs/backfill",
        serde_json::json!({ "from": "2026-09-30T18:00:00Z", "to": "2026-09-30T09:00:00Z" }),
    )))
    .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = block_on(body_json(response));
    assert_eq!(json["error_code"], "domain_invalid_range", "{json}");
}

// 没有视觉模型时：作业**明确跳过**，不是默默「成功」
#[tokio::test]
async fn backfill_without_a_model_is_skipped_not_faked() {
    let ctx = ctx();
    let (job_id, _) = mc_server::jobs_worker::enqueue_backfill(
        &ctx.state,
        at(0),
        at(3600),
        mc_common::time::Clock::now(&mc_common::time::SystemClock),
    )
    .expect("入队");

    // 消费者跑一轮：未配置模型 → 跳过
    let mut worker = None;
    let handled = mc_server::jobs_worker::run_once(&ctx.state, &mut worker)
        .await
        .expect("run_once 不该失败");
    assert!(handled, "队列里有作业就该被取走");

    let job = ctx.state.db.job(job_id).unwrap().expect("作业还在");
    assert_eq!(job.state, JobState::Skipped, "未配置模型必须如实跳过");
    assert!(
        job.skip_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("未配置视觉模型")),
        "跳过要留下原因：{:?}",
        job.skip_reason
    );
}

// 未接线的类型不留悬空状态
#[tokio::test]
async fn unwired_job_kinds_are_skipped_with_a_reason() {
    let ctx = ctx();
    let outcome = ctx
        .state
        .db
        .enqueue_job(
            &mc_storage::jobs::NewJob {
                kind: mc_storage::jobs::JobKind::SummaryStage,
                priority: mc_storage::jobs::Priority::StageSummary,
                observation_id: None,
                stage_id: Some("stage-1".to_string()),
                adhoc_id: None,
                idempotency: "stage-1".to_string(),
                weight_bytes: 0,
            },
            mc_common::time::Clock::now(&mc_common::time::SystemClock),
        )
        .expect("入队");
    let job_id = match outcome {
        mc_storage::jobs::EnqueueOutcome::Enqueued { job_id } => job_id,
        other => panic!("应当新建：{other:?}"),
    };

    let mut worker = None;
    mc_server::jobs_worker::run_once(&ctx.state, &mut worker)
        .await
        .expect("run_once");

    let job = ctx.state.db.job(job_id).unwrap().expect("作业还在");
    assert_eq!(job.state, JobState::Skipped);
    assert!(
        job.skip_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("尚未接线")),
        "未接线的类型要说明原因：{:?}",
        job.skip_reason
    );
}

// 状态接口
#[test]
fn job_status_reports_state_and_404_for_unknown() {
    let ctx = ctx();
    let json = block_on(async {
        let response = router(Arc::clone(&ctx.state))
            .oneshot(post(
                "/api/v1/jobs/backfill",
                serde_json::json!({ "from": "2026-09-30T09:00:00Z", "to": "2026-09-30T18:00:00Z" }),
            ))
            .await
            .unwrap();
        body_json(response).await
    });
    let job_id = json["data"]["job_id"].as_i64().expect("job_id");

    let json = block_on(async {
        let response = router(Arc::clone(&ctx.state))
            .oneshot(get(&format!("/api/v1/jobs/{job_id}")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response).await
    });
    assert_eq!(json["data"]["kind"], "backfill");
    assert_eq!(json["data"]["state"], "queued");

    let response =
        block_on(router(Arc::clone(&ctx.state)).oneshot(get("/api/v1/jobs/999999"))).unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
