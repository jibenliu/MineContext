//! 阶段与总结的接口。
//!
//! 这一层的形状直接决定前端能不能把「阶段总结卡片」画出来，
//! 因此断言都落在**前端真正要用的字段**上：
//! 时间范围、状态、结束原因（用来解释「为什么这段结束了」）、
//! 包含哪些活动、有没有总结、总结的质量标记。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::projectors::stages::StageRow;
use mc_storage::projectors::summaries::NewSummary;
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

/// 同 `ctx()`，但把配置里的 `summary.template_id` 换成指定的值。
fn ctx_with_template(template_id: &str) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let mut config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    config.config.summary.template_id = template_id.to_string();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        "test-token".to_string(),
        at(0),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

fn seed_activity(state: &ServerState, ids: &[(&str, i64, i64)]) {
    let views: Vec<mc_domain::activity::ActivityView> = ids
        .iter()
        .map(|(id, start, end)| mc_domain::activity::ActivityView {
            id: id.to_string(),
            start: at(*start),
            end: at(*end),
            title: format!("活动 {id}"),
            original_title: format!("活动 {id}"),
            category: Some("开发".to_string()),
            observations: vec![mc_domain::activity::ObservationRef {
                id: format!("obs-{id}"),
                at: at(*start),
            }],
            origin: mc_domain::activity::Provenance::Rule {
                rule_id: "coding".to_string(),
            },
            confidence: 1.0,
            is_user_modified: false,
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

fn stage(id: &str, start: i64, end: Option<i64>, activities: &[&str]) -> StageRow {
    StageRow {
        id: id.to_string(),
        start: at(start),
        end: end.map(at),
        state: if end.is_some() { "closed" } else { "open" }.to_string(),
        end_reason: end.map(|_| "idle".to_string()),
        // 2026-09-30（本地 UTC）
        day: "2026-09-30".to_string(),
        activities: activities.iter().map(|id| id.to_string()).collect(),
    }
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

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Runtime::new().unwrap().block_on(future)
}

async fn get_json(router: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = router.oneshot(request("GET", uri, None)).await.unwrap();
    let status = response.status();
    (status, body_json(response).await)
}

#[test]
fn stages_api_returns_a_day_view() {
    let ctx = ctx();
    seed_activity(&ctx.state, &[("act-1", 0, 1800), ("act-2", 3600, 5400)]);
    ctx.state
        .db
        .upsert_stages(
            &[
                stage("stage-1", 0, Some(1800), &["act-1"]),
                stage("stage-2", 3600, None, &["act-2"]),
            ],
            0,
        )
        .expect("写阶段");

    let (status, json) = block_on(get_json(
        router(Arc::clone(&ctx.state)),
        "/api/v1/stages?date=2026-09-30",
    ));

    assert_eq!(status, StatusCode::OK, "{json}");
    let stages = json["data"]["stages"].as_array().expect("stages 数组");
    assert_eq!(stages.len(), 2);
    assert_eq!(stages[0]["id"], "stage-1");
    assert_eq!(stages[0]["state"], "closed");
    assert_eq!(stages[0]["end_reason"], "idle");
    assert_eq!(stages[0]["activities"], serde_json::json!(["act-1"]));
    assert_eq!(stages[0]["has_summary"], false);
    assert!(
        stages[0]["duration_secs"].as_i64().unwrap() >= 1800,
        "时长要直接给出来，前端不该自己算：{}",
        stages[0]
    );
    assert_eq!(stages[1]["state"], "open");
    assert!(stages[1]["end"].is_null(), "进行中的阶段没有结束时间");
    assert_eq!(stages[1]["summary_id"], serde_json::Value::Null);
}

#[test]
fn stages_api_rejects_an_invalid_date() {
    let ctx = ctx();
    let (status, json) = block_on(get_json(
        router(Arc::clone(&ctx.state)),
        "/api/v1/stages?date=not-a-date",
    ));

    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error_code"], "domain_invalid_timestamp");
}

#[test]
fn stages_api_returns_a_single_stage_with_its_summary() {
    let ctx = ctx();
    seed_activity(&ctx.state, &[("act-1", 0, 1800)]);
    ctx.state
        .db
        .upsert_stages(&[stage("stage-1", 0, Some(1800), &["act-1"])], 0)
        .expect("写阶段");
    ctx.state
        .db
        .insert_summary(
            &NewSummary {
                id: "sum-stage-1".to_string(),
                kind: "stage".to_string(),
                stage_id: Some("stage-1".to_string()),
                template_id: "work_stage".to_string(),
                start: at(0),
                end: at(1800),
                title: "09:00–09:30 阶段总结".to_string(),
                fields: Default::default(),
                body_markdown: "这段时间在写代码。".to_string(),
                quality: "model".to_string(),
                model: Some("openai_compatible:qwen-max".to_string()),
                prompt_tokens: 100,
                completion_tokens: 30,
                scope: None,
            },
            at(2000),
        )
        .expect("写总结");

    let (status, json) = block_on(get_json(
        router(Arc::clone(&ctx.state)),
        "/api/v1/stages/stage-1",
    ));

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["stage"]["id"], "stage-1");
    assert_eq!(json["data"]["stage"]["has_summary"], true);
    assert_eq!(json["data"]["summaries"][0]["quality"], "model");
    assert!(
        json["data"]["summaries"][0]["body_markdown"]
            .as_str()
            .is_some_and(|body| !body.is_empty()),
        "总结正文必须给到前端：{json}"
    );
}

#[test]
fn stages_api_returns_404_for_an_unknown_stage() {
    let ctx = ctx();
    let (status, _json) = block_on(get_json(
        router(Arc::clone(&ctx.state)),
        "/api/v1/stages/stage-nope",
    ));
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[test]
fn summaries_api_filters_by_stage_and_kind() {
    let ctx = ctx();
    ctx.state
        .db
        .upsert_stages(
            &[
                stage("stage-1", 0, Some(1800), &[]),
                stage("stage-2", 3600, Some(5400), &[]),
            ],
            0,
        )
        .expect("写阶段");

    for (id, stage_id, kind, start) in [
        ("sum-1", "stage-1", "stage", 0i64),
        ("sum-2", "stage-2", "stage", 3600),
        ("sum-adhoc", "stage-1", "adhoc", 7200),
    ] {
        ctx.state
            .db
            .insert_summary(
                &NewSummary {
                    id: id.to_string(),
                    kind: kind.to_string(),
                    stage_id: Some(stage_id.to_string()),
                    template_id: "work_stage".to_string(),
                    start: at(start),
                    end: at(start + 600),
                    title: format!("总结 {id}"),
                    fields: Default::default(),
                    body_markdown: "正文".to_string(),
                    quality: "fallback".to_string(),
                    model: None,
                    prompt_tokens: 0,
                    completion_tokens: 0,
                    scope: None,
                },
                at(start + 700),
            )
            .expect("写总结");
    }

    let (_, by_stage) = block_on(get_json(
        router(Arc::clone(&ctx.state)),
        "/api/v1/summaries?stage_id=stage-1",
    ));
    let ids: Vec<&str> = by_stage["data"]["summaries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["sum-1", "sum-adhoc"], "{by_stage}");

    let (_, by_kind) = block_on(get_json(
        router(Arc::clone(&ctx.state)),
        "/api/v1/summaries?kind=adhoc",
    ));
    assert_eq!(by_kind["data"]["summaries"].as_array().unwrap().len(), 1);

    let (status, by_range) = block_on(get_json(
        router(Arc::clone(&ctx.state)),
        "/api/v1/summaries?from=2026-09-30T09:30:00Z&to=2026-09-30T10:30:00Z",
    ));
    assert_eq!(status, StatusCode::OK, "{by_range}");
    let ids: Vec<&str> = by_range["data"]["summaries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["sum-2"], "按时间范围过滤：{by_range}");
}

#[test]
fn manual_close_endpoint_closes_the_stage_and_summarizes_immediately() {
    let ctx = ctx();
    seed_activity(&ctx.state, &[("act-1", 0, 1800)]);
    ctx.state
        .db
        .upsert_stages(&[stage("stage-1", 0, None, &["act-1"])], 0)
        .expect("写阶段");
    assert_eq!(ctx.state.db.summary_count().unwrap(), 0);

    let (status, json) = block_on({
        let router = router(Arc::clone(&ctx.state));
        async move {
            let response = router
                .oneshot(request("POST", "/api/v1/stages/stage-1/close", None))
                .await
                .unwrap();
            let status = response.status();
            (status, body_json(response).await)
        }
    });
    assert_eq!(status, StatusCode::OK, "{json}");

    assert_eq!(json["data"]["stage"]["state"], "closed");
    assert_eq!(
        json["data"]["stage"]["end_reason"], "manual",
        "手动结束要能被区分出来：{json}"
    );
    assert_eq!(
        json["data"]["summary_id"], "sum-stage-1",
        "用户点了「结束」就要**立刻**拿到总结（退出标准之一）"
    );

    let summaries = ctx.state.db.read_summaries(None, None).expect("读总结");
    assert_eq!(summaries.len(), 1);
    assert!(!summaries[0].body_markdown.trim().is_empty());
    assert_eq!(
        ctx.state
            .db
            .stages_without_summary_count(0)
            .expect("哨兵指标"),
        0
    );
}

#[test]
fn manual_close_is_rejected_for_an_already_closed_stage() {
    let ctx = ctx();
    ctx.state
        .db
        .upsert_stages(&[stage("stage-1", 0, Some(1800), &[])], 0)
        .expect("写阶段");

    let response = block_on({
        let router = router(Arc::clone(&ctx.state));
        async move {
            router
                .oneshot(request("POST", "/api/v1/stages/stage-1/close", None))
                .await
                .unwrap()
        }
    });

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(ctx.state.db.summary_count().unwrap(), 0);
}

// ---------------------------------------------------------------- 任意时段总结（4.55–4.65）

fn seed_adhoc_data(state: &ServerState) {
    seed_activity(state, &[("act-1", 0, 1800)]);
    state
        .db
        .insert_observation(&mc_storage::observations::NewObservation {
            id: "obs-adhoc-1".to_string(),
            ts: at(60),
            source_id: "macos:screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some("VSCode".to_string()),
            app_bundle_id: None,
            window_title: Some("main.rs".to_string()),
            domain: None,
            display_id: None,
            scale_factor: None,
            image: None,
            text_content: None,
            text_origin: None,
            change_kind: "pixel_major".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: "idem-adhoc-1".to_string(),
        })
        .expect("写观测");
}

// 先看范围内有什么，再决定要不要生成
#[test]
fn adhoc_preview_endpoint_reports_counts() {
    let ctx = ctx();
    seed_adhoc_data(&ctx.state);

    let (status, json) = block_on(get_json(
        router(Arc::clone(&ctx.state)),
        "/api/v1/summaries/adhoc/preview?from=2026-09-30T09:00:00Z&to=2026-09-30T10:00:00Z",
    ));

    assert_eq!(status, StatusCode::OK, "{json}");
    let data = &json["data"];
    assert_eq!(data["has_data"], true);
    assert_eq!(data["counts"]["observations"], 1);
    assert_eq!(data["counts"]["activities"], 1);
    assert_eq!(data["estimated_chunks"], 1);
    // 本地时间：T0 是 UTC 09:00 → 上海 17:00（配置未设时区时按 UTC，这里只要求非空）
    assert!(data["range"]["from"].is_string(), "{data}");
}

// 前端与「今天/昨天/最近 7 天」预设发的是纯日期（`dayjs().format('YYYY-MM-DD')`），
// 不是时间戳。只认 RFC3339 时真实调用一律 400，而两层测试都看不见：
// 前端页测试用假后端，Rust 侧只用时间戳。这条按真实形状钉住。
// 配置里写了不认识的模板 id：生成必须**明确报错**，而不是静默用默认模板产出一份
// 用户没要的总结 —— 那样用户以为配置生效了，界面上看不出任何区别。
//
// 注意断言的是**生成**而不是预览：预览只统计范围内有什么，模板是在生成时才解析的。
#[test]
fn adhoc_generate_rejects_unknown_config_template() {
    let ctx = ctx_with_template("mine");
    seed_adhoc_data(&ctx.state);
    let body = serde_json::json!({
        "from": "2026-09-30T09:00:00Z",
        "to": "2026-09-30T10:00:00Z"
    });

    let (status, json) = block_on({
        let router = router(Arc::clone(&ctx.state));
        let body = body.clone();
        async move {
            let response = router
                .oneshot(request("POST", "/api/v1/summaries/adhoc", Some(body)))
                .await
                .unwrap();
            let status = response.status();
            (status, body_json(response).await)
        }
    });

    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    let detail = json["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("mine"), "错误里要指出是哪个 id：{json}");
    assert!(detail.contains("work_stage"), "错误里要列出可用 id：{json}");
}

#[test]
fn adhoc_preview_accepts_date_only_range() {
    let ctx = ctx();
    seed_adhoc_data(&ctx.state);

    let (status, json) = block_on(get_json(
        router(Arc::clone(&ctx.state)),
        "/api/v1/summaries/adhoc/preview?from=2026-09-30&to=2026-09-30",
    ));

    assert_eq!(status, StatusCode::OK, "{json}");
    let data = &json["data"];
    assert_eq!(data["has_data"], true);
    assert_eq!(data["counts"]["observations"], 1);
}

#[test]
fn adhoc_preview_endpoint_rejects_an_invalid_range() {
    let ctx = ctx();
    let (status, json) = block_on(get_json(
        router(Arc::clone(&ctx.state)),
        "/api/v1/summaries/adhoc/preview?from=2026-09-30T10:00:00Z&to=2026-09-30T09:00:00Z",
    ));

    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["error_code"], "domain_invalid_range");
}

// 正向验证：配置指定的模板**真的被用上**，并且如实写进总结行。
// 只断言"生成成功"是不够的 —— 那和用默认模板生成的结果看起来一模一样。
#[test]
fn adhoc_generate_uses_the_configured_template() {
    let ctx = ctx_with_template("work_stage_detailed");
    seed_adhoc_data(&ctx.state);
    let body = serde_json::json!({
        "from": "2026-09-30T09:00:00Z",
        "to": "2026-09-30T10:00:00Z"
    });

    let (status, json) = block_on({
        let router = router(Arc::clone(&ctx.state));
        let body = body.clone();
        async move {
            let response = router
                .oneshot(request("POST", "/api/v1/summaries/adhoc", Some(body)))
                .await
                .unwrap();
            let status = response.status();
            (status, body_json(response).await)
        }
    });

    assert_eq!(status, StatusCode::OK, "{json}");

    // 生成响应里带的是**渲染结果**（`data.summary`），看不出用了哪个模板；而且没有
    // 模型时走的是模板兜底，产出字段是固定集合（看不到详细模板独有的字段）。
    // 所以观察点选**落库记录**：列表接口返回的 `template_id` 必须等于配置里指定的
    // 那个 —— 这正是「配置生效」与「记录不说谎」两条要求的交汇处。
    let (status, list) = block_on(get_json(
        router(Arc::clone(&ctx.state)),
        "/api/v1/summaries",
    ));
    assert_eq!(status, StatusCode::OK, "{list}");
    let rows = list["data"]["summaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        rows.iter()
            .any(|row| row["template_id"] == "work_stage_detailed"),
        "落库记录必须写实际使用的模板：{list}"
    );
}

#[test]
fn adhoc_generate_endpoint_returns_a_summary_then_caches() {
    let ctx = ctx();
    seed_adhoc_data(&ctx.state);
    let body = serde_json::json!({
        "from": "2026-09-30T09:00:00Z",
        "to": "2026-09-30T10:00:00Z"
    });

    let (status, json) = block_on({
        let router = router(Arc::clone(&ctx.state));
        let body = body.clone();
        async move {
            let response = router
                .oneshot(request("POST", "/api/v1/summaries/adhoc", Some(body)))
                .await
                .unwrap();
            let status = response.status();
            (status, body_json(response).await)
        }
    });

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["cached"], false);
    assert!(json["data"]["summary"]["id"].is_string(), "{json}");
    assert!(
        json["data"]["summary"]["body_markdown"]
            .as_str()
            .is_some_and(|body| !body.is_empty()),
        "总结正文不能为空（没有模型时必须走兜底）：{json}"
    );

    // 再点一次：同一范围 + 事件没变 → 命中缓存
    let again = block_on({
        let router = router(Arc::clone(&ctx.state));
        async move {
            let response = router
                .oneshot(request("POST", "/api/v1/summaries/adhoc", Some(body)))
                .await
                .unwrap();
            body_json(response).await
        }
    });
    assert_eq!(again["data"]["cached"], true, "{again}");
    assert_eq!(ctx.state.db.summary_count().unwrap(), 1);
}

#[test]
fn adhoc_generate_endpoint_rejects_an_empty_range() {
    let ctx = ctx();

    let (status, json) = block_on({
        let router = router(Arc::clone(&ctx.state));
        async move {
            let response = router
                .oneshot(request(
                    "POST",
                    "/api/v1/summaries/adhoc",
                    Some(serde_json::json!({
                        "from": "2026-09-30T09:00:00Z",
                        "to": "2026-09-30T10:00:00Z"
                    })),
                ))
                .await
                .unwrap();
            let status = response.status();
            (status, body_json(response).await)
        }
    });

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "空范围要明确拒绝，前端据此禁用「生成」按钮：{json}"
    );
    assert_eq!(json["data"]["has_data"], false);
    assert_eq!(ctx.state.db.summary_count().unwrap(), 0);
}

#[test]
fn regenerate_endpoint_writes_a_new_summary() {
    let ctx = ctx();
    seed_adhoc_data(&ctx.state);
    ctx.state
        .db
        .insert_summary(
            &NewSummary {
                id: "sum-1".to_string(),
                kind: "adhoc".to_string(),
                stage_id: None,
                template_id: "work_stage".to_string(),
                start: at(0),
                end: at(3600),
                title: "旧总结".to_string(),
                fields: Default::default(),
                body_markdown: "旧正文".to_string(),
                quality: "fallback".to_string(),
                model: None,
                prompt_tokens: 0,
                completion_tokens: 0,
                scope: None,
            },
            at(4000),
        )
        .expect("写总结");

    let (status, json) = block_on({
        let router = router(Arc::clone(&ctx.state));
        async move {
            let response = router
                .oneshot(request("POST", "/api/v1/summaries/sum-1/regenerate", None))
                .await
                .unwrap();
            let status = response.status();
            (status, body_json(response).await)
        }
    });

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(ctx.state.db.summary_count().unwrap(), 2);
    let summaries = ctx.state.db.read_summaries(None, Some("adhoc")).unwrap();
    assert!(
        summaries.iter().any(|summary| summary.id == "sum-1"),
        "旧总结必须留着（用户可能还在看它）"
    );
}

// 长范围在接口层也必须走分块：一次请求不该把整个月塞进一次模型调用
#[test]
fn adhoc_generate_chunks_a_long_range() {
    let ctx = ctx();
    // 9 小时的范围 → 默认阈值 4 小时 → 3 块
    seed_activities_spread(&ctx.state);
    let body = serde_json::json!({
        "from": "2026-09-30T09:00:00Z",
        "to": "2026-09-30T18:00:00Z"
    });

    let (status, json) = block_on({
        let router = router(Arc::clone(&ctx.state));
        async move {
            let response = router
                .oneshot(request("POST", "/api/v1/summaries/adhoc", Some(body)))
                .await
                .unwrap();
            let status = response.status();
            (status, body_json(response).await)
        }
    });

    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(
        json["data"]["preview"]["estimated_chunks"]
            .as_u64()
            .unwrap()
            >= 2,
        "9 小时的范围应当切成多块：{json}"
    );
    let markdown = json["data"]["summary"]["body_markdown"].as_str().unwrap();
    assert!(markdown.contains("第 1 段"), "{markdown}");
    assert!(markdown.contains("第 2 段"), "{markdown}");

    // 块与归并结果都落在库里
    let chunks = ctx
        .state
        .db
        .read_summaries(None, Some("adhoc_chunk"))
        .unwrap();
    assert!(
        chunks.len() >= 2,
        "每块都要单独落库（续跑靠它）：{chunks:?}"
    );
    assert_eq!(
        ctx.state
            .db
            .read_summaries(None, Some("adhoc"))
            .unwrap()
            .len(),
        1
    );
}

fn seed_activities_spread(state: &ServerState) {
    let views: Vec<mc_domain::activity::ActivityView> = (0..18)
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

/// 同 `ctx()`，但配置里带一份用户自定义模板 YAML。
fn ctx_with_template_yaml(yaml: &str) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let mut config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    config.config.summary.template_yaml = Some(yaml.to_string());
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        "test-token".to_string(),
        at(0),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

const USER_TEMPLATE: &str = "id: mine\nname: 我的模板\nfields:\n  - id: time_range\n    label: 时间段\n    kind: time_range\n";

// 配置里给了用户模板 YAML：生成必须真的用它，且落库记录的 template_id 是 YAML 里的 id
// （不是内置模板的 id）。
#[test]
fn adhoc_generate_uses_the_user_template_yaml() {
    let ctx = ctx_with_template_yaml(USER_TEMPLATE);
    seed_adhoc_data(&ctx.state);
    let body = serde_json::json!({
        "from": "2026-09-30T09:00:00Z",
        "to": "2026-09-30T10:00:00Z"
    });

    let (status, json) = block_on({
        let router = router(Arc::clone(&ctx.state));
        let body = body.clone();
        async move {
            let response = router
                .oneshot(request("POST", "/api/v1/summaries/adhoc", Some(body)))
                .await
                .unwrap();
            let status = response.status();
            (status, body_json(response).await)
        }
    });
    assert_eq!(status, StatusCode::OK, "{json}");

    let (_, list) = block_on(get_json(
        router(Arc::clone(&ctx.state)),
        "/api/v1/summaries",
    ));
    let rows = list["data"]["summaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        rows.iter().any(|row| row["template_id"] == "mine"),
        "落库记录必须写用户模板的 id：{list}"
    );
}

// YAML 写坏时必须明确报错，而不是静默退回内置模板产出一份用户没要的总结。
#[test]
fn adhoc_generate_rejects_broken_user_template_yaml() {
    let ctx = ctx_with_template_yaml("id: [未闭合");
    seed_adhoc_data(&ctx.state);
    let body = serde_json::json!({
        "from": "2026-09-30T09:00:00Z",
        "to": "2026-09-30T10:00:00Z"
    });

    let (status, json) = block_on({
        let router = router(Arc::clone(&ctx.state));
        let body = body.clone();
        async move {
            let response = router
                .oneshot(request("POST", "/api/v1/summaries/adhoc", Some(body)))
                .await
                .unwrap();
            let status = response.status();
            (status, body_json(response).await)
        }
    });

    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    let detail = json["detail"].as_str().unwrap_or_default();
    assert!(detail.contains("模板"), "错误要说清是模板的问题：{json}");
}

// 兜底渲染是**按模板字段**渲染的（`fallback::generate` 遍历 `template.fields`），
// 所以没有模型时换模板也应当能看出来：用一份带自有字段 id 的用户模板，断言渲染
// 结果的字段集合里有它。断言产出而不是断言记录 —— 只看记录会漏掉「记录写着新模板、
// 产出仍是默认模板」这种不一致。
#[test]
fn adhoc_generate_renders_the_user_template_fields_without_a_model() {
    let yaml = "id: mine\nname: 我的模板\nfields:\n  - id: my_captured\n    label: 采集量\n    kind: observation_count\n";
    let ctx = ctx_with_template_yaml(yaml);
    seed_adhoc_data(&ctx.state);
    let body = serde_json::json!({
        "from": "2026-09-30T09:00:00Z",
        "to": "2026-09-30T10:00:00Z"
    });

    let (status, json) = block_on({
        let router = router(Arc::clone(&ctx.state));
        let body = body.clone();
        async move {
            let response = router
                .oneshot(request("POST", "/api/v1/summaries/adhoc", Some(body)))
                .await
                .unwrap();
            let status = response.status();
            (status, body_json(response).await)
        }
    });

    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(
        json["data"]["summary"]["fields"]
            .get("my_captured")
            .is_some(),
        "兜底渲染必须按模板字段渲染：{json}"
    );
}
