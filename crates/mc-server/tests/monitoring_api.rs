//! `/api/monitoring/recording-stats`（录制统计卡片）。
//!
//! 这条路径沿用既有契约的形状，但统计全部从库里算出来，理由有两条：
//! 1. 内存计数器在 daemon 重启后归零，用户会看到「今天处理了 0 张截图」而磁盘上全是截图；
//! 2. 库里本来就是真源（观测、活动、失败、截图路径），再维护一份计数器只会多一个会漂移的数字。
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::error::ErrorCode;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::observations::NewObservation;
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";

/// daemon 启动时刻（会话起点）
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as SESSION;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
}

fn ctx_with_session(session_ms: i64) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(session_ms),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

fn ctx() -> Ctx {
    ctx_with_session(SESSION)
}

/// 越过「未同意出网 / 未配模型 / 无密钥」三道门，专门测 provider 侧失败（401/429/余额）。
fn ctx_ready_for_provider_calls() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let mut loaded = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    loaded.config.privacy.ai_upload = true;
    loaded.config.ai.enabled = true;
    loaded.config.ai.vision.base_url = "https://api.example.com/v1".to_string();
    loaded.config.ai.vision.model = "qwen3-vl".to_string();
    loaded.config.ai.vision.api_key_ref = Some("sidecar:model-keys".to_string());
    std::fs::write(
        dir.path().join("model-keys.json"),
        r#"{"api_key":"sk-live-wrong-but-present"}"#,
    )
    .unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(loaded),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(SESSION),
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

async fn call(state: &Arc<ServerState>, method: &str, uri: &str) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(request(method, uri, None))
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

fn seed_screenshot(state: &ServerState, id: &str, at_ms: i64, path: &str) {
    state
        .db
        .insert_observation(&NewObservation {
            id: id.to_string(),
            ts: Timestamp::from_millis(at_ms),
            source_id: "screen:display-1".to_string(),
            kind: "screen".to_string(),
            app_name: Some("Visual Studio Code".to_string()),
            app_bundle_id: None,
            window_title: Some("main.rs".to_string()),
            domain: None,
            display_id: Some("display-1".to_string()),
            scale_factor: Some(2.0),
            image: Some(mc_storage::observations::ImageRef {
                relative_path: path.to_string(),
                content_hash: format!("hash-{id}"),
                thumbnail_path: None,
                width: 1280,
                height: 800,
                bytes: 1024,
            }),
            text_content: None,
            text_origin: None,
            change_kind: "pixel_major".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: format!("key-{id}"),
        })
        .expect("写观测");
}

// ---------------------------------------------------------------- 形状

#[tokio::test]
async fn stats_have_every_field_the_card_reads() {
    let ctx = ctx();
    let envelope = call(&ctx.state, "GET", "/api/monitoring/recording-stats").await;
    let stats = data(&envelope);

    for field in [
        "captured_screenshots",
        "processed_screenshots",
        "failed_screenshots",
        "pending_analyses",
        "generated_activities",
        "next_activity_eta_seconds",
        "last_activity_time",
        "session_start_time",
        "recent_errors",
        "recent_screenshots",
        "analysis_blocker",
    ] {
        assert!(
            stats.get(field).is_some(),
            "缺少字段 {field}（卡片会渲染成 undefined）：{stats}"
        );
    }

    assert_eq!(stats["captured_screenshots"], 0);
    assert_eq!(stats["processed_screenshots"], 0);
    assert!(
        stats["analysis_blocker"].is_null(),
        "没采集时不应给 blocker"
    );
    assert!(stats["recent_errors"].as_array().unwrap().is_empty());
    assert!(stats["recent_screenshots"].as_array().unwrap().is_empty());
    assert!(stats["last_activity_time"].is_null(), "还没有活动时为 null");
    // 会话起点就是 daemon 启动时间（ISO）
    assert_eq!(stats["session_start_time"], "2026-09-30T09:00:00.000Z");
}

#[tokio::test]
async fn stats_count_from_the_database_not_an_in_memory_counter() {
    let ctx = ctx();
    seed_screenshot(&ctx.state, "obs-1", SESSION + MINUTE, "20260930/1.png");
    seed_screenshot(&ctx.state, "obs-2", SESSION + 2 * MINUTE, "20260930/2.png");
    // 会话开始之前的观测不算（那是上一次运行留下的）
    seed_screenshot(&ctx.state, "obs-old", SESSION - HOUR, "20260930/old.png");

    let stats = data(&call(&ctx.state, "GET", "/api/monitoring/recording-stats").await);
    assert_eq!(
        stats["captured_screenshots"], 2,
        "只统计本次会话（daemon 启动之后）的截图"
    );
    assert_eq!(
        stats["processed_screenshots"], 0,
        "只采到、还没分析出结果时为 0"
    );
}

#[tokio::test]
async fn recent_screenshots_are_blob_relative_and_capped_at_five() {
    let ctx = ctx();
    for index in 0..7 {
        seed_screenshot(
            &ctx.state,
            &format!("obs-{index}"),
            SESSION + (index as i64) * MINUTE,
            &format!("20260930/{index}.png"),
        );
    }

    let stats = data(&call(&ctx.state, "GET", "/api/monitoring/recording-stats").await);
    let paths = stats["recent_screenshots"].as_array().unwrap();
    assert_eq!(paths.len(), 5, "最多 5 张");
    for path in paths {
        let path = path.as_str().unwrap();
        assert!(
            path.starts_with("20260930/") && !std::path::Path::new(path).is_absolute(),
            "必须是截图读取接口接受的 blob 相对路径：{path}"
        );
    }
}

#[tokio::test]
async fn recent_errors_come_from_pipeline_failures() {
    let ctx = ctx();
    for index in 0..7 {
        let error =
            mc_common::error::AppError::new(ErrorCode::CaptureIo, format!("第 {index} 次截图失败"));
        ctx.state
            .db
            .record_failure(
                Timestamp::from_millis(SESSION + (index as i64) * MINUTE),
                "capture",
                &error,
                "warn",
            )
            .unwrap();
    }
    // 与截图无关的失败不该混进来
    ctx.state
        .db
        .record_failure(
            Timestamp::from_millis(SESSION + 8 * MINUTE),
            "summary",
            &mc_common::error::AppError::new(ErrorCode::ProviderTimeout, "总结超时"),
            "warn",
        )
        .unwrap();

    let stats = data(&call(&ctx.state, "GET", "/api/monitoring/recording-stats").await);
    let errors = stats["recent_errors"].as_array().unwrap();
    assert_eq!(errors.len(), 5, "最多 5 条");
    for entry in errors {
        assert!(entry["error_message"].is_string());
        assert!(entry["processor_name"].is_string());
        assert!(
            entry["timestamp"].as_str().unwrap().contains('T'),
            "时间戳是 ISO（卡片直接展示）：{entry}"
        );
    }
    assert_eq!(
        stats["failed_screenshots"], 7,
        "7 条采集失败全部计入，summary 的那条不算"
    );
}

// ---------------------------------------------------------------- ETA

#[tokio::test]
async fn eta_counts_down_from_the_last_activity() {
    let ctx = ctx();
    // 造一个 10 分钟前结束的活动
    let ended = SESSION + HOUR - 10 * MINUTE;
    let projection = mc_domain::projector::Projection {
        activities: vec![mc_domain::activity::ActivityView {
            id: "act-1".to_string(),
            start: Timestamp::from_millis(ended - 30 * MINUTE),
            end: Timestamp::from_millis(ended),
            title: "写测试".to_string(),
            original_title: "写测试".to_string(),
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
        ctx.state.db.as_ref(),
        &projection,
        0,
        Timestamp::from_millis(SESSION + HOUR),
    )
    .unwrap();

    let stats = data(&call(&ctx.state, "GET", "/api/monitoring/recording-stats").await);
    assert_eq!(
        stats["last_activity_time"], "2026-09-30T09:50:00.000Z",
        "最近一次活动的时间（ISO）"
    );
    assert_eq!(stats["generated_activities"], 1, "本次会话内产出的活动数");

    // ETA = 生成间隔 - 距上次活动的时长；间隔取活动投影节奏（默认 900 秒档），
    // 因此刚做完活动时 ETA 应当明显小于间隔且非负
    let eta = stats["next_activity_eta_seconds"].as_i64().unwrap();
    assert!(eta >= 0, "ETA 不能是负数：{eta}");
    assert!(eta <= 900, "ETA 不该超过一个生成间隔：{eta}");
}

/// 还没有任何活动时，ETA 从**会话起点**算起（沿用同一约定）
#[tokio::test]
async fn eta_falls_back_to_the_session_start() {
    let state_session = Timestamp::from_millis(0);
    let _ = state_session;
    // 会话起点设在很久以前 → ETA 归零，而不是负数
    let ctx = ctx_with_session(SESSION - 10 * HOUR);
    let stats = data(&call(&ctx.state, "GET", "/api/monitoring/recording-stats").await);
    assert_eq!(stats["next_activity_eta_seconds"], 0);
}

/// 统计接口是契约路由：出现失败也不能 500（卡片会整块消失）
#[tokio::test]
async fn stats_never_fail_loudly() {
    let ctx = ctx();
    let response = router(Arc::clone(&ctx.state))
        .oneshot(request("GET", "/api/monitoring/recording-stats", None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn captures_and_analyses_are_counted_separately() {
    let ctx = ctx();
    seed_screenshot(&ctx.state, "obs-1", SESSION + MINUTE, "20260930/1.png");
    seed_screenshot(&ctx.state, "obs-2", SESSION + 2 * MINUTE, "20260930/2.png");

    // 观测量入库时 `analyses` 是 `pending`：采到了 ≠ 分析完了
    let stats = data(&call(&ctx.state, "GET", "/api/monitoring/recording-stats").await);
    assert_eq!(stats["captured_screenshots"], 2);
    assert_eq!(stats["processed_screenshots"], 0, "排队中不算处理完");
    assert_eq!(stats["pending_analyses"], 2);
    let blocker = &stats["analysis_blocker"];
    assert!(blocker.is_object(), "采到但未分析时应给出原因：{blocker}");
    // 默认 privacy.ai_upload = false，这是「采到但已分析为 0」最常见的可行动原因。
    assert_eq!(blocker["code"], "ai_upload_disabled");
    assert!(
        blocker["message"]
            .as_str()
            .unwrap_or_default()
            .contains("ai_upload"),
        "blocker 必须点名配置项：{blocker}"
    );

    // 只把第一张标成分析完成
    ctx.state
        .db
        .with_write(|conn| {
            conn.execute(
                "UPDATE analyses SET status = 'done' WHERE observation_id = 'obs-1'",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let stats = data(&call(&ctx.state, "GET", "/api/monitoring/recording-stats").await);
    assert_eq!(stats["captured_screenshots"], 2, "采集数不受分析影响");
    assert_eq!(
        stats["processed_screenshots"], 1,
        "只有 done/degraded 才算分析出结果"
    );
    assert!(
        stats["analysis_blocker"].is_null(),
        "已有分析完成时不再展示 blocker：{}",
        stats["analysis_blocker"]
    );
}

#[tokio::test]
async fn analysis_blocker_names_invalid_api_key_from_401() {
    let ctx = ctx_ready_for_provider_calls();
    seed_screenshot(
        &ctx.state,
        "obs-auth",
        SESSION + MINUTE,
        "20260930/auth.png",
    );
    ctx.state
        .db
        .record_failure(
            Timestamp::from_millis(SESSION + 2 * MINUTE),
            "activities",
            &mc_common::error::AppError::new(
                ErrorCode::ProviderAuthFailed,
                "HTTP 401: invalid api key",
            ),
            "warn",
        )
        .unwrap();

    let stats = data(&call(&ctx.state, "GET", "/api/monitoring/recording-stats").await);
    assert_eq!(stats["captured_screenshots"], 1);
    assert_eq!(stats["processed_screenshots"], 0);
    let blocker = &stats["analysis_blocker"];
    assert_eq!(blocker["code"], "api_key_invalid", "{blocker}");
    let message = blocker["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("API Key") || message.contains("密钥"),
        "必须点名密钥问题：{blocker}"
    );
}

#[tokio::test]
async fn analysis_blocker_names_rate_limit_from_429() {
    let ctx = ctx_ready_for_provider_calls();
    seed_screenshot(&ctx.state, "obs-429", SESSION + MINUTE, "20260930/429.png");
    ctx.state
        .db
        .record_failure(
            Timestamp::from_millis(SESSION + 2 * MINUTE),
            "embedding",
            &mc_common::error::AppError::new(ErrorCode::ProviderRateLimited, "HTTP 429"),
            "warn",
        )
        .unwrap();

    let stats = data(&call(&ctx.state, "GET", "/api/monitoring/recording-stats").await);
    let blocker = &stats["analysis_blocker"];
    assert_eq!(blocker["code"], "provider_rate_limited", "{blocker}");
    assert!(
        blocker["message"]
            .as_str()
            .unwrap_or_default()
            .contains("限流"),
        "{blocker}"
    );
}

#[tokio::test]
async fn analysis_blocker_names_quota_or_balance_exhaustion() {
    let ctx = ctx_ready_for_provider_calls();
    seed_screenshot(
        &ctx.state,
        "obs-quota",
        SESSION + MINUTE,
        "20260930/quota.png",
    );
    ctx.state
        .db
        .record_failure(
            Timestamp::from_millis(SESSION + 2 * MINUTE),
            "activities",
            &mc_common::error::AppError::new(
                ErrorCode::ProviderInvalidResponse,
                "HTTP 400: 余额不足，请充值后重试",
            ),
            "warn",
        )
        .unwrap();

    let stats = data(&call(&ctx.state, "GET", "/api/monitoring/recording-stats").await);
    let blocker = &stats["analysis_blocker"];
    assert_eq!(blocker["code"], "quota_exhausted", "{blocker}");
    assert!(
        blocker["message"]
            .as_str()
            .unwrap_or_default()
            .contains("余额")
            || blocker["message"]
                .as_str()
                .unwrap_or_default()
                .contains("额度"),
        "{blocker}"
    );
}
