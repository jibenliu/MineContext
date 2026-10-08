//! 任意时段总结的异步作业接口。
//! 长范围总结要跑几十秒，同步请求会把客户端挂死、UI 也没法显示进度。
//! 作业模型把「等」与「拿结果」拆开：
//!
//! ```text
//! POST /summaries/adhoc/jobs ─► 202 {job_id} 立刻返回
//! 后台逐块生成 ─► SSE summary:progress 进度实时可见
//! GET /summaries/adhoc/jobs/{id} ─► 状态 + 结果
//! POST /summaries/adhoc/jobs/{id}/cancel ─► 已完成块保留
//! ```

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
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

fn request(method: &str, uri: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", "test-token");
    let body = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            Body::from(serde_json::to_vec(&value).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
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

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Runtime::new().unwrap().block_on(future)
}

fn seed_activities(state: &ServerState, count: i64) {
    let views: Vec<mc_domain::activity::ActivityView> = (0..count)
        .map(|index| {
            let start = index * 1800;
            mc_domain::activity::ActivityView {
                id: format!("act-{index}"),
                start: at(start),
                end: at(start + 900),
                title: format!("活动 {index}"),
                original_title: format!("活动 {index}"),
                category: Some("开发".to_string()),
                observations: vec![mc_domain::activity::ObservationRef {
                    id: format!("obs-{index}"),
                    at: at(start),
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
        activities: views,
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };
    mc_storage::projectors::activities::store(state.db.as_ref(), &projection, 0, at(0))
        .expect("存活动");
}

#[tokio::test]
async fn async_job_runs_and_reports_progress() {
    let ctx = ctx();
    seed_activities(&ctx.state, 18);

    // 先连上 SSE，验证进度确实推给了前端
    let mut stream = router(Arc::clone(&ctx.state))
        .oneshot(sse_request())
        .await
        .unwrap()
        .into_body()
        .into_data_stream();
    let _ready = timeout(Duration::from_secs(2), stream.next()).await;

    let response = router(Arc::clone(&ctx.state))
        .oneshot(request(
            "POST",
            "/api/v1/summaries/adhoc/jobs",
            Some(serde_json::json!({
                "from": "2026-09-30T09:00:00Z",
                "to": "2026-09-30T18:00:00Z"
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let job_id = json["data"]["job_id"].as_str().expect("job_id").to_string();
    assert_eq!(json["data"]["deduped"], false);

    // 轮询直到完成
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut final_state = None;
    while std::time::Instant::now() < deadline {
        let response = router(Arc::clone(&ctx.state))
            .oneshot(request(
                "GET",
                &format!("/api/v1/summaries/adhoc/jobs/{job_id}"),
                None,
            ))
            .await
            .unwrap();
        let json = body_json(response).await;
        let state = json["data"]["job"]["state"]
            .as_str()
            .unwrap_or("")
            .to_string();
        if state == "done" {
            assert!(
                json["data"]["job"]["summary_id"].is_string(),
                "完成后要给出 summary_id：{json}"
            );
            assert!(
                json["data"]["summary"]["body_markdown"]
                    .as_str()
                    .is_some_and(|body| !body.is_empty()),
                "完成后要能直接拿到正文：{json}"
            );
            final_state = Some(state);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        final_state.as_deref(),
        Some("done"),
        "作业应当在 10 秒内完成"
    );

    // 进度帧（可能有多帧，至少一帧）
    let mut saw_progress = false;
    for _ in 0..8 {
        match timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                let frame = String::from_utf8_lossy(&chunk).to_string();
                if frame.contains("event: summary:progress") {
                    assert!(frame.contains("chunks_total"), "{frame}");
                    saw_progress = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(saw_progress, "应当推送 summary:progress 帧");
}

#[tokio::test]
async fn unknown_job_is_not_found() {
    let ctx = ctx();
    let response = router(Arc::clone(&ctx.state))
        .oneshot(request("GET", "/api/v1/summaries/adhoc/jobs/job-999", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cancel_reports_that_completed_chunks_are_kept() {
    let ctx = ctx();
    seed_activities(&ctx.state, 18);

    let response = router(Arc::clone(&ctx.state))
        .oneshot(request(
            "POST",
            "/api/v1/summaries/adhoc/jobs",
            Some(serde_json::json!({
                "from": "2026-09-30T09:00:00Z",
                "to": "2026-09-30T18:00:00Z"
            })),
        ))
        .await
        .unwrap();
    let job_id = body_json(response).await["data"]["job_id"]
        .as_str()
        .unwrap()
        .to_string();

    let response = router(Arc::clone(&ctx.state))
        .oneshot(request(
            "POST",
            &format!("/api/v1/summaries/adhoc/jobs/{job_id}/cancel"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert!(
        json["data"]["cancel_requested"].is_boolean(),
        "取消要如实回答是否接受：{json}"
    );
    assert!(
        json["data"]["note"]
            .as_str()
            .is_some_and(|note| note.contains("保留")),
        "要告诉调用方已完成块不会被丢掉：{json}"
    );
}

#[tokio::test]
async fn empty_range_is_rejected_before_a_job_is_created() {
    let ctx = ctx();
    let response = router(Arc::clone(&ctx.state))
        .oneshot(request(
            "POST",
            "/api/v1/summaries/adhoc/jobs",
            Some(serde_json::json!({
                "from": "2026-09-30T09:00:00Z",
                "to": "2026-09-30T10:00:00Z"
            })),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        ctx.state.jobs.in_flight_count(),
        0,
        "空范围不该占用一个作业"
    );
}

#[tokio::test]
async fn second_request_after_completion_reuses_the_summary() {
    let ctx = ctx();
    seed_activities(&ctx.state, 18);
    let body = serde_json::json!({
        "from": "2026-09-30T09:00:00Z",
        "to": "2026-09-30T18:00:00Z"
    });

    // 第一次：同步接口生成（会写块 + 归并结果）
    let response = router(Arc::clone(&ctx.state))
        .oneshot(request(
            "POST",
            "/api/v1/summaries/adhoc",
            Some(body.clone()),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let first = body_json(response).await;
    assert_eq!(first["data"]["cached"], false);
    let total_after_first = ctx.state.db.summary_count().unwrap();

    // 第二次：同一范围 + 数据没变 → 命中缓存，不新增任何行
    let response = router(Arc::clone(&ctx.state))
        .oneshot(request("POST", "/api/v1/summaries/adhoc", Some(body)))
        .await
        .unwrap();
    let second = body_json(response).await;
    assert_eq!(second["data"]["cached"], true, "{second}");
    assert_eq!(
        ctx.state.db.summary_count().unwrap(),
        total_after_first,
        "命中缓存不该再写任何行"
    );
}

#[test]
fn registry_dedupes_identical_in_flight_jobs() {
    // 接口层的并发去重：两个相同请求在第一个还没跑完时只启动一个作业
    let ctx = ctx();
    seed_activities(&ctx.state, 18);
    let key = "same-scope";

    let first = ctx.state.jobs.submit(key, 3, at(0));
    let second = ctx.state.jobs.submit(key, 3, at(1));

    assert_eq!(first.job_id, second.job_id);
    assert!(second.deduped);
    assert_eq!(ctx.state.jobs.in_flight_count(), 1);
    let _ = block_on(async { 0 });
}

// 25 —— 配置里的模板必须真的生效，而不是只被记录。
// 两条：配置了自定义 YAML → 产出用它的 id 记录；配了未知 id → 明确报错。
fn ctx_with_template(template_id: &str, template_yaml: Option<&str>) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let mut loaded = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    loaded.config.summary.template_id = template_id.to_string();
    loaded.config.summary.template_yaml = template_yaml.map(str::to_string);
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(loaded),
        db,
        "test-token".to_string(),
        at(0),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

fn run_job_until_done(state: &Arc<ServerState>, body: serde_json::Value) -> serde_json::Value {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt
        .block_on(router(Arc::clone(state)).oneshot(request(
            "POST",
            "/api/v1/summaries/adhoc/jobs",
            Some(body),
        )))
        .unwrap();
    let json = rt.block_on(body_json(response));
    assert_eq!(json["code"], 0, "提交应当成功：{json}");
    let job_id = json["data"]["job_id"].as_str().expect("job_id").to_string();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        let response = rt
            .block_on(router(Arc::clone(state)).oneshot(request(
                "GET",
                &format!("/api/v1/summaries/adhoc/jobs/{job_id}"),
                None,
            )))
            .unwrap();
        let json = rt.block_on(body_json(response));
        match json["data"]["job"]["state"].as_str().unwrap_or("") {
            "done" => return json,
            "failed" => panic!("作业失败：{json}"),
            _ => std::thread::sleep(std::time::Duration::from_millis(50)),
        }
    }
    panic!("作业没有在 10 秒内完成");
}

#[tokio::test]
async fn configured_template_yaml_is_really_used_and_recorded() {
    let ctx = ctx_with_template(
        "work_stage",
        Some("id: my_custom\nname: 我的模板\nfields:\n  - id: time_range\n    label: 时间段\n    kind: time_range\n"),
    );
    seed_activities(&ctx.state, 18);

    let json = tokio::task::spawn_blocking({
        let state = Arc::clone(&ctx.state);
        move || {
            run_job_until_done(
                &state,
                serde_json::json!({
                    "from": "2026-09-30T09:00:00Z",
                    "to": "2026-09-30T18:00:00Z"
                }),
            )
        }
    })
    .await
    .unwrap();

    let summary_id = json["data"]["job"]["summary_id"]
        .as_str()
        .expect("summary_id")
        .to_string();
    let stored = ctx
        .state
        .db
        .read_summaries(None, None)
        .expect("读总结")
        .into_iter()
        .find(|row| row.id == summary_id)
        .expect("总结要落库");
    assert_eq!(
        stored.template_id, "my_custom",
        "配置里的自定义模板必须真的被用上（记录写实际 id），而不是只记请求值：{json}"
    );
}

#[test]
fn unknown_template_id_in_config_is_rejected_explicitly() {
    let ctx = ctx_with_template("does_not_exist", None);
    seed_activities(&ctx.state, 18);

    let response = block_on(router(Arc::clone(&ctx.state)).oneshot(request(
        "POST",
        "/api/v1/summaries/adhoc/jobs",
        Some(serde_json::json!({
            "from": "2026-09-30T09:00:00Z",
            "to": "2026-09-30T18:00:00Z"
        })),
    )))
    .unwrap();
    let status = response.status();
    let json = block_on(body_json(response));
    assert_eq!(ctx.state.jobs.in_flight_count(), 0);
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "未知模板要明确拒绝：{json}"
    );
    assert_eq!(
        json["error_code"], "config_invalid",
        "模板 id 来自配置，应当报「配置有误」而不是「内部状态不一致」：{json}"
    );
    let message = json["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("配置"),
        "用户看到的 message 要说清是自己配错了：{json}"
    );
    let detail = json["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("does_not_exist") && detail.contains("work_stage"),
        "detail 要指出未知 id 并列出可用 id：{json}"
    );
}
