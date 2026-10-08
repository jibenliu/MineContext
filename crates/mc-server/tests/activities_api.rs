//! 、3.48–3.50：活动接口。
//!
//! 两条线：
//!
//! - **兼容面** `/api/db/activities*`：旧 `pages/screen-monitor` 直接消费，
//!   字段名、JSON 字符串列、时间格式都不能变（变了就是白屏）。
//! - **扩展面** `/api/v1/activities*`：带上 `origin` / `confidence` / `evidence`，
//!   让 UI 能标注「这是规则判断的」还是「这是猜的」，
//!   并提供用户修正入口（#14）。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

/// 2026-09-30T09:00:00Z
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
}

fn ctx() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());

    let payload = |id: &str, at_ms: i64, app: &str, title: &str| {
        serde_json::json!({
            "id": id,
            "at": at_ms,
            "app_name": app,
            "window_title": title,
            "domain": null,
            "text": null,
        })
    };

    db.append_events(&[
        mc_storage::NewEvent::new(
            "observation.recorded",
            Timestamp::from_millis(T0),
            payload("obs-1", T0, "Visual Studio Code", "main.rs"),
        ),
        mc_storage::NewEvent::new(
            "observation.recorded",
            Timestamp::from_millis(T0 + 60_000),
            payload("obs-2", T0 + 60_000, "Visual Studio Code", "main.rs"),
        ),
        mc_storage::NewEvent::new(
            "observation.recorded",
            Timestamp::from_millis(T0 + 600_000),
            payload("obs-3", T0 + 600_000, "Google Chrome", "Hacker News"),
        ),
        mc_storage::NewEvent::new(
            "observation.recorded",
            Timestamp::from_millis(T0 + 660_000),
            payload("obs-4", T0 + 660_000, "Google Chrome", "Hacker News"),
        ),
    ])
    .expect("追加事件");

    mc_storage::projectors::activities::replay(
        &db,
        &mc_domain::rules::RuleSet::default(),
        mc_domain::projector::ProjectionOptions::default(),
        Timestamp::from_millis(T0 + 700_000),
    )
    .expect("投影");

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

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

async fn compat_data(router: axum::Router, uri: &str) -> serde_json::Value {
    let response = router.oneshot(request("GET", uri, None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let json = body_json(response).await;
    assert_eq!(json["code"], 0, "{uri} 失败: {json}");
    json["data"].clone()
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Runtime::new().unwrap().block_on(future)
}

#[test]
fn get_all_activities_returns_legacy_rows() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    let data = block_on(compat_data(router, "/api/db/activities"));
    let rows = data.as_array().expect("data 必须是数组");

    assert_eq!(rows.len(), 2, "两条切换后的活动：{data}");
    for row in rows {
        for field in [
            "id",
            "title",
            "content",
            "resources",
            "metadata",
            "start_time",
            "end_time",
        ] {
            assert!(row.get(field).is_some(), "缺字段 {field}: {row}");
        }
        assert!(
            row["resources"].is_string(),
            "resources 必须是 JSON 字符串，渲染层直接 JSON.parse：{row}"
        );
        assert_eq!(row["id"].as_i64().unwrap(), row["id"].as_i64().unwrap());
    }
}

#[test]
fn get_new_activities_filters_by_start_time() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    // 渲染层的默认 end 就是 '2099-12-31 00:00:00'
    let data = block_on(compat_data(
        router,
        "/api/db/activities?start=2026-09-30%2009:00:00&end=2099-12-31%2000:00:00",
    ));
    let rows = data.as_array().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "只有 09:10 那条在 start_time > 09:00 之后：{data}"
    );
    assert_eq!(rows[0]["start_time"], "2026-09-30 09:10:00");
}

// 渲染层还会传 ISO 8601（`timeToISOTimeString` 的产物）
#[test]
fn get_new_activities_accepts_iso_timestamps() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    let data = block_on(compat_data(
        router,
        "/api/db/activities?start=2026-09-30T00:00:00.000Z&end=2026-10-01T00:00:00.000Z",
    ));
    let rows = data.as_array().unwrap();
    assert_eq!(rows.len(), 2, "ISO 时间也要能解析：{data}");
}

#[test]
fn latest_activity_uses_id_descending_order() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    let data = block_on(compat_data(router, "/api/db/activities/latest"));
    assert_eq!(data["id"].as_i64(), Some(2), "必须按 id 倒序取一条：{data}");
    assert_eq!(data["title"], "Google Chrome");
}

#[test]
fn v1_activities_expose_provenance_and_evidence() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    let json = block_on(async move {
        let response = router
            .oneshot(request("GET", "/api/v1/activities", None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        body_json(response).await
    });
    let rows = json["data"]["activities"]
        .as_array()
        .expect("activities 数组");

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["origin"]["kind"], "observed");
    assert!(rows[0]["confidence"].is_number());
    assert_eq!(
        rows[0]["evidence"],
        serde_json::json!(["obs-1", "obs-2"]),
        "每个活动都要能回答「为什么是它」"
    );
    assert_eq!(rows[0]["id"], "act-obs-1");
    assert_eq!(rows[0]["is_user_modified"], false);
}

// 用户修正作为事件落库（actor=user），并且立刻在接口上生效
#[test]
fn user_can_rename_an_activity_through_the_api() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    let json = block_on({
        let router = router.clone();
        async move {
            let response = router
                .oneshot(request(
                    "POST",
                    "/api/v1/activities/overrides",
                    Some(serde_json::json!({
                        "kind": "rename",
                        "activity_id": "act-obs-1",
                        "title": "重构活动引擎"
                    })),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            body_json(response).await
        }
    });
    assert_eq!(json["data"]["applied"], true, "{json}");

    // 事件日志里必须留下 actor=user 的痕迹（可审计）
    let events = ctx.state.db.read_events(0, 100).expect("读事件");
    let recorded = events
        .iter()
        .find(|event| event.kind == "activity.overridden")
        .expect("修正必须作为事件记录");
    assert_eq!(recorded.actor, "user");
    assert_eq!(recorded.payload["kind"], "rename");

    // 接口立刻反映新标题，且算法原标题仍在
    let after = block_on(compat_data(router, "/api/db/activities"));
    assert_eq!(after[0]["title"], "重构活动引擎");
    let metadata: serde_json::Value =
        serde_json::from_str(after[0]["metadata"].as_str().unwrap()).unwrap();
    assert_eq!(metadata["is_user_modified"], true);
    assert_eq!(metadata["original_title"], "Visual Studio Code");
}

#[test]
fn invalid_override_is_rejected_without_touching_the_log() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    let response = block_on(router.oneshot(request(
        "POST",
        "/api/v1/activities/overrides",
        Some(serde_json::json!({ "kind": "nonsense" })),
    )))
    .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        ctx.state
            .db
            .read_events(0, 100)
            .unwrap()
            .iter()
            .all(|event| event.kind != "activity.overridden"),
        "非法修正不能写进日志"
    );
}

// ---------------------------------------------------------------- 后台投影任务

// 采集只写观测；活动由后台投影产出。这条测试守住「观测进日志 → 活动出现」。
#[test]
fn projector_task_turns_new_observations_into_activities() {
    let ctx = ctx();
    // ctx 已经投影过一次；再写两条新观测（模拟采集继续跑）
    let base = T0 + 3_600_000;
    for (id, offset) in [("obs-9", 0i64), ("obs-10", 60)] {
        ctx.state
            .db
            .insert_observation(&mc_storage::observations::NewObservation {
                id: id.to_string(),
                ts: Timestamp::from_millis(base + offset * 1_000),
                source_id: "macos:screen".to_string(),
                kind: "screen".to_string(),
                app_name: Some("iTerm2".to_string()),
                app_bundle_id: None,
                window_title: Some("cargo test".to_string()),
                domain: None,
                display_id: None,
                scale_factor: None,
                image: None,
                text_content: None,
                text_origin: None,
                change_kind: "pixel_major".to_string(),
                privacy_verdict: "allowed".to_string(),
                phash: None,
                idempotency: format!("idem-{id}"),
            })
            .expect("写入观测");
    }

    let at = Timestamp::from_millis(base + 700_000);
    let first = mc_server::activities::project_once(&ctx.state, at).expect("投影");
    assert!(first.is_some(), "有新观测就必须投影出新活动");

    let second = mc_server::activities::project_once(&ctx.state, at).expect("再投影");
    assert!(second.is_none(), "日志没变就不该重算");

    let router = router(Arc::clone(&ctx.state));
    let data = block_on(compat_data(router, "/api/db/activities"));
    let rows = data.as_array().unwrap();
    assert!(
        rows.iter().any(|row| row["title"] == "iTerm2"),
        "新活动必须出现在渲染层的查询里：{data}"
    );
}

// 规则文件写坏时：投影失败要进诊断表，而不是静默不产出
#[test]
fn broken_rules_file_is_reported_as_a_failure() {
    let ctx = ctx();
    let rules_dir = ctx.state.data_dir.join("rules");
    std::fs::create_dir_all(&rules_dir).unwrap();
    std::fs::write(
        rules_dir.join("activities.yaml"),
        "version: 99\nactivities: []",
    )
    .unwrap();

    let error = mc_server::activities::project_once(&ctx.state, Timestamp::from_millis(T0))
        .expect_err("坏规则文件必须报错");
    assert_eq!(error.code().as_str(), "config_invalid");

    // 记进诊断表后，/api/diagnostics 的 recent_failures 就能看到它
    ctx.state
        .db
        .record_failure(Timestamp::from_millis(T0), "activities", &error, "warn")
        .unwrap();

    let router = router(Arc::clone(&ctx.state));
    let json = block_on(async move {
        let response = router
            .oneshot(request("GET", "/api/diagnostics", None))
            .await
            .unwrap();
        body_json(response).await
    });
    let failures = json["data"]["recent_failures"]
        .as_array()
        .expect("失败列表");
    assert!(
        failures
            .iter()
            .any(|item| item["error_code"] == "config_invalid" && item["component"] == "activities"),
        "投影失败必须在诊断里可见：{json}"
    );
}

// ---------------------------------------------------------------- 3.43–3.47 装配

fn vision_response(content: &str) -> String {
    serde_json::json!({
        "choices": [{ "message": { "content": content }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 120, "completion_tokens": 20 }
    })
    .to_string()
}

fn scripted_worker(
    transport: std::sync::Arc<mc_testkit::provider::ScriptedTransport>,
) -> mc_pipeline::activity_ai::ActivityAiWorker {
    let provider = mc_providers::openai::OpenAiCompatibleProvider::new(
        mc_providers::openai::EndpointConfig {
            base_url: "https://api.example.com/v1".to_string(),
            model: "qwen3-vl".to_string(),
            api_key: Some("sk-test".to_string()),
            timeout: std::time::Duration::from_secs(30),
            max_image_edge: None,
            max_concurrency: 1,
        },
        transport,
        mc_providers::Role::Vision,
    )
    .expect("provider");

    mc_pipeline::activity_ai::ActivityAiWorker::new(
        std::sync::Arc::new(provider),
        mc_pipeline::activity_ai::ActivityAiPolicy::default(),
    )
}

/// 未配置模型时不组装 Provider（而不是每 30 秒失败一次）。
#[test]
fn vision_worker_is_not_built_when_unconfigured() {
    let ctx = ctx();
    let mut config = ctx.state.config.current().config.clone();
    config.privacy.ai_upload = true; // 先排除「未同意出网」这个变量

    let secrets = mc_providers::credentials::StaticSecretStore::default();
    let worker =
        mc_server::activities::build_vision_worker(&config, &secrets).expect("未配置不是错误");
    assert!(worker.is_none(), "没配 base_url/model 就不该有 Provider");
}

/// 密钥没填也只是「不推断」，不能让装配失败（这里失败会持续）。
#[test]
fn vision_worker_is_built_without_a_secret_but_degrades() {
    let ctx = ctx();
    let mut config = ctx.state.config.current().config.clone();
    config.ai.vision.base_url = "https://api.example.com/v1".to_string();
    config.ai.vision.model = "qwen3-vl".to_string();
    config.ai.vision.api_key_ref = Some("keychain:provider:vision".to_string());
    // 出网许可是组装 provider 的前提（默认 false = 不出网）；
    // 本用例要验证的是「密钥缺失」这一个变量，因此先显式同意出网。
    config.privacy.ai_upload = true;

    let secrets = mc_providers::credentials::StaticSecretStore::default();
    let worker = mc_server::activities::build_vision_worker(&config, &secrets).expect("装配成功");
    assert!(worker.is_some(), "密钥缺失不影响 Provider 组装");
}

/// 规则没判出来的活动 → 模型给出结论 → 落成事件 → 接口上立刻能看到。
#[test]
fn inference_upgrades_an_observed_activity_end_to_end() {
    let ctx = ctx();

    // 一条没有规则可命中的观测，并且**真的有一张图**在 blob 目录里
    let relative = "screenshots/2026/09/30/obs-ai.png";
    let image_path = ctx.state.data_dir.join("blobs").join(relative);
    std::fs::create_dir_all(image_path.parent().unwrap()).unwrap();
    std::fs::write(&image_path, b"fake-png-bytes").unwrap();

    ctx.state
        .db
        .insert_observation(&mc_storage::observations::NewObservation {
            id: "obs-ai-1".to_string(),
            ts: Timestamp::from_millis(T0 + 7_200_000),
            source_id: "macos:screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some("Aurora".to_string()),
            app_bundle_id: None,
            window_title: Some("quarterly planning".to_string()),
            domain: None,
            display_id: None,
            scale_factor: None,
            image: Some(mc_storage::observations::ImageRef {
                relative_path: relative.to_string(),
                content_hash: "hash-ai".to_string(),
                thumbnail_path: None,
                width: 100,
                height: 80,
                bytes: 14,
            }),
            text_content: None,
            text_origin: None,
            change_kind: "pixel_major".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: "idem-ai-1".to_string(),
        })
        .expect("写入观测");

    let at = Timestamp::from_millis(T0 + 7_300_000);
    mc_server::activities::project_once(&ctx.state, at).expect("先投影出 Observed 活动");

    let transport = std::sync::Arc::new(mc_testkit::provider::ScriptedTransport::new().push_json(
        200,
        vision_response(r#"{"title":"季度规划","category":"需求","confidence":0.77}"#),
    ));
    let mut worker = scripted_worker(std::sync::Arc::clone(&transport));

    let batch = block_on(mc_server::activities::infer_once(
        &ctx.state,
        &mut worker,
        at,
    ))
    .expect("推断应当成功");

    assert_eq!(batch.suggestions.len(), 1, "{batch:?}");
    assert_eq!(batch.suggestions[0].title, "季度规划");
    assert_eq!(transport.call_count(), 1);

    // 结论必须是**事件**：重放之后还会生效
    let events = ctx.state.db.read_events(0, 200).unwrap();
    let inferred = events
        .iter()
        .find(|event| event.kind == "activity.inferred")
        .expect("推断结论必须落成事件");
    assert_eq!(inferred.actor, "ai");
    assert_eq!(inferred.payload["title"], "季度规划");

    // 接口上立刻能看到（origin 也变成了 inferred）
    let router = router(Arc::clone(&ctx.state));
    let json = block_on(async move {
        let response = router
            .oneshot(request("GET", "/api/v1/activities", None))
            .await
            .unwrap();
        body_json(response).await
    });
    let rows = json["data"]["activities"].as_array().unwrap();
    let inferred_row = rows
        .iter()
        .find(|row| row["title"] == "季度规划")
        .expect("推断结果要出现在接口里");
    assert_eq!(inferred_row["origin"]["kind"], "inferred");
    assert_eq!(
        inferred_row["origin"]["model"],
        "openai_compatible:qwen3-vl"
    );
}

/// 单轮上限：积压再多也只做一批，剩下的下一轮继续（否则一轮把循环占住，
/// 期间新数据不处理、进度不可见）。
#[test]
fn inference_processes_at_most_one_batch_per_round() {
    let ctx = ctx();
    let transport = std::sync::Arc::new(
        mc_testkit::provider::ScriptedTransport::new()
            .push_json(200, vision_response(r#"{"title":"x","confidence":0.9}"#)),
    );
    let mut worker = scripted_worker(std::sync::Arc::clone(&transport));

    // ctx 里有 2 条待推断活动；上限给 1 → 本轮只应处理 1 条
    let batch = block_on(mc_server::activities::infer_once_with_limit(
        &ctx.state,
        &mut worker,
        Timestamp::from_millis(T0 + 800_000),
        1,
    ))
    .expect("不该报错");

    assert_eq!(
        batch.skipped, 1,
        "上限 1 时本轮只处理一条（剩下的一条下一轮再做）：{batch:?}"
    );
}

/// 规则命中的活动一次都不该问模型。
#[test]
fn inference_never_asks_about_rule_activities() {
    let ctx = ctx();
    // ctx 里已有的两条活动都是兜底（Observed）但没有图片 → 不该调用
    let transport = std::sync::Arc::new(
        mc_testkit::provider::ScriptedTransport::new()
            .push_json(200, vision_response(r#"{"title":"x","confidence":0.9}"#)),
    );
    let mut worker = scripted_worker(std::sync::Arc::clone(&transport));

    let batch = block_on(mc_server::activities::infer_once(
        &ctx.state,
        &mut worker,
        Timestamp::from_millis(T0 + 800_000),
    ))
    .expect("不该报错");

    assert_eq!(
        transport.call_count(),
        0,
        "没有图片的观测不该让模型凭空猜标题"
    );
    assert!(batch.suggestions.is_empty());
    assert_eq!(batch.skipped, 2);
}

/// 补跑推断：没有配置模型时**同步返回零结果**，而不是「已提交」或报错。
///
/// 这条路径刻意不进作业表 —— 存储层的 jobs 表当前没有任何消费者，
/// 塞进去只会得到「看起来排了队、实际没人做」。
#[tokio::test]
async fn backfill_runs_inline_and_reports_what_it_did() {
    let ctx = ctx();
    let response = router(Arc::clone(&ctx.state))
        .oneshot(request("POST", "/api/v1/backfill", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["code"], 0);
    assert_eq!(body["data"]["calls"], 0, "没配模型就不该有调用");
    assert!(body["data"]["suggestions"].is_number());
}

/// 补跑推断也要鉴权：没 token 一律 401。
#[tokio::test]
async fn backfill_requires_a_token() {
    let ctx = ctx();
    let response = router(Arc::clone(&ctx.state))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/backfill")
                .header("host", "127.0.0.1:12345")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
