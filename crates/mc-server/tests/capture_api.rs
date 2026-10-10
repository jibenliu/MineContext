//! 采集与时间线的 HTTP 接口。
//!
//! 这些接口形状**必须与旧主进程的 IPC 返回一致** ——
//! `pages/screen-monitor` 直接消费它们。任何字段缺失都会让时间线白屏。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, CaptureControls, ServerState};
use mc_storage::blob::{BlobStore, FileSystemBlobStore, ImageFormat};
use mc_storage::observations::{ImageRef, NewObservation};
use mc_storage::Database;
use mc_testkit::capture::FakeCaptureSource;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

/// 2026-09-30T09:00:00Z
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as DAY_MS;

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
    blobs: Arc<FileSystemBlobStore>,
}

fn ctx() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let blobs =
        Arc::new(FileSystemBlobStore::new(dir.path().join("blobs"), ImageFormat::Png).unwrap());

    let source: Arc<dyn mc_capture::source::CaptureSource> = Arc::new(
        FakeCaptureSource::builder()
            .screen("display-1", "Built-in Retina Display", 2.0)
            .screen("display-2", "DELL U2720Q", 1.0)
            .window("win-1", "main.rs — VSCode", "Visual Studio Code")
            .image_size(320, 200)
            .build()
            .unwrap(),
    );

    let controls = Arc::new(CaptureControls::new(source, Arc::clone(&blobs)));

    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(
        ServerState::new(
            mc_config::ConfigHandle::new(config),
            db,
            TOKEN.to_string(),
            Timestamp::from_millis(DAY_MS),
            dir.path().to_path_buf(),
        )
        .with_capture(controls),
    );

    Ctx {
        _dir: dir,
        state,
        blobs,
    }
}

fn get(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "127.0.0.1:12345");
    if let Some(t) = token {
        builder = builder.header("x-mc-token", t);
    }
    builder.body(Body::empty()).unwrap()
}

fn post(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "127.0.0.1:12345");
    if let Some(t) = token {
        builder = builder.header("x-mc-token", t);
    }
    builder.body(Body::empty()).unwrap()
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// 兼容面响应是 `{code, status, message, data}` 信封，这里取 data。
async fn get_data(router: axum::Router, uri: &str) -> serde_json::Value {
    let response = router.oneshot(get(uri, Some(TOKEN))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let json = body_json(response).await;
    assert_eq!(json["code"], 0, "{uri} 失败: {json}");
    json["data"].clone()
}

fn seed_observation(db: &Database, id: &str, at_ms: i64, image: Option<ImageRef>) {
    let mut obs = NewObservation {
        id: id.to_string(),
        ts: Timestamp::from_millis(at_ms),
        source_id: "macos:screen".to_string(),
        kind: "screen".to_string(),
        app_name: Some("Visual Studio Code".to_string()),
        app_bundle_id: None,
        window_title: Some("main.rs".to_string()),
        domain: None,
        display_id: Some("display-1".to_string()),
        scale_factor: Some(2.0),
        image: None,
        text_content: None,
        text_origin: None,
        change_kind: "pixel_major".to_string(),
        privacy_verdict: "allowed".to_string(),
        phash: Some(123),
        idempotency: format!("idem-{id}"),
    };
    obs.image = image;
    db.insert_observation(&obs).unwrap();
}

// ---------------------------------------------------------------- 权限与目标

#[test]
fn permissions_endpoint_reports_readiness() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    let data = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(get_data(router, "/api/capture/permissions"));

    assert!(data["permission"].is_string(), "{data}");
    assert!(data["ready"].is_boolean(), "{data}");
    assert!(
        data["monitor_count"].as_u64().is_some(),
        "必须报告显示器数量：{data}"
    );
    assert!(
        data["screen_recording_tcc"].is_boolean(),
        "必须区分 TCC 原值与经验证后的 screen_recording：{data}"
    );
    assert!(
        data["enabled"].is_boolean(),
        "必须报告 capture.enabled，否则前端无法说明「录制尚未开始」：{data}"
    );
    assert!(
        data.get("windows_reason").is_some(),
        "必须带 windows_reason（可为 null）：{data}"
    );
    assert!(
        data["capture_supported"].is_boolean(),
        "必须报告 capture_supported，前端据此区分「缺权限」与「平台未实现」：{data}"
    );
    assert_eq!(
        data["capture_supported"],
        mc_capture::platform::capture_supported(),
        "capture_supported 必须与平台探测一致：{data}"
    );
}

#[test]
fn status_endpoint_reports_tcc_and_window_reason() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    let data = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(get_data(router, "/api/capture/status"));

    assert!(
        data["screen_recording_tcc"].is_boolean(),
        "status 必须带 screen_recording_tcc，设置页据此展示 TCC：{data}"
    );
    assert!(
        data["enabled"].is_boolean(),
        "status 必须带 enabled：{data}"
    );
    assert!(
        data.get("windows_reason").is_some(),
        "status 必须带 windows_reason（可为 null）：{data}"
    );
}

#[test]
fn targets_endpoint_shape_matches_legacy_capture_source() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    let data = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(get_data(router, "/api/capture/targets"));

    let targets = data.as_array().expect("应当是数组");
    assert_eq!(targets.len(), 3, "两个屏 + 一个窗口");

    // 旧 preload 返回的 CaptureSource 形状：
    // { id, name, type, thumbnail, appIcon, isVisible, appName?, windowTitle?, windowId? }
    let first = &targets[0];
    assert_eq!(first["id"], "display-1");
    assert_eq!(first["name"], "Built-in Retina Display");
    assert_eq!(first["type"], "screen");
    let thumb = first["thumbnail"]
        .as_str()
        .expect("完整目标列表必须带预览缩略图");
    assert!(
        thumb.starts_with("data:image/png;base64,"),
        "缩略图必须是 PNG data URL: {thumb}"
    );
    assert!(first["appIcon"].is_null());
    assert_eq!(first["isVisible"], true);

    let window = targets
        .iter()
        .find(|t| t["type"] == "window")
        .expect("应当有窗口目标");
    assert_eq!(window["appName"], "Visual Studio Code");
    assert_eq!(window["windowTitle"], "main.rs — VSCode");
    let window_thumb = window["thumbnail"]
        .as_str()
        .expect("窗口目标也应有预览缩略图");
    assert!(window_thumb.starts_with("data:image/png;base64,"));
}

#[test]
fn targets_endpoint_filters_visible_only() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    let data = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(get_data(router, "/api/capture/targets?visible=1"));

    // FakeCaptureSource 里所有目标都是可见的，因此数量一致；
    // 这条测试锁定「visible=1 参数被正确解析」而不是被忽略。
    let items = data.as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert!(
        items[0]["thumbnail"].is_null(),
        "可见性轮询不应附带缩略图（高频路径）"
    );
}

// ---------------------------------------------------------------- 时间线

#[test]
fn screenshots_by_date_returns_legacy_shape() {
    let ctx = ctx();
    let db = &ctx.state.db;

    let blob = ctx
        .blobs
        .put_image(
            &image::RgbImage::from_pixel(64, 40, image::Rgb([10, 20, 30])),
            &mc_storage::blob::ImageMeta {
                captured_at: Timestamp::from_millis(DAY_MS),
                display_id: None,
            },
        )
        .unwrap();

    seed_observation(
        db,
        "obs-1",
        DAY_MS,
        Some(ImageRef {
            relative_path: blob.relative_path.clone(),
            content_hash: blob.content_hash.clone(),
            thumbnail_path: blob.thumbnail.as_ref().map(|t| t.relative_path.clone()),
            width: blob.width,
            height: blob.height,
            bytes: blob.bytes,
        }),
    );

    let router = router(Arc::clone(&ctx.state));
    let stats = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(get_data(router.clone(), "/api/monitoring/recording-stats"));
    let recent = stats["recent_screenshots"][0].as_str().unwrap();
    assert_eq!(recent, blob.relative_path);
    let image_data = tokio::runtime::Runtime::new().unwrap().block_on(get_data(
        router.clone(),
        &format!("/api/capture/screenshots/data?path={}", urlencode(recent)),
    ));
    assert_eq!(image_data["mime"], "image/png");
    assert!(image_data["data"].as_str().unwrap().starts_with("iVBOR"));
    let data = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(get_data(router, "/api/capture/screenshots?date=2026-09-30"));

    let items = data.as_array().expect("应当是数组");
    assert_eq!(items.len(), 1);

    // 渲染层消费的字段
    let item = &items[0];
    assert_eq!(item["id"], "obs-1");
    assert_eq!(item["date"], "2026-09-30");
    assert_eq!(item["timestamp"], DAY_MS);
    assert!(item["description"].is_null());
    assert!(item["created_at"].is_string());
    assert!(item.get("group_id").is_some(), "字段必须存在（可为 null）");

    // image_url 必须是**相对路径**，不能是文件系统绝对路径
    let image_url = item["image_url"].as_str().expect("必须有 image_url");
    assert!(
        !image_url.starts_with('/'),
        "image_url 不得为绝对路径：{image_url}"
    );
    assert!(image_url.starts_with("screenshots/"), "{image_url}");
}

#[test]
fn screenshots_by_date_accepts_both_date_formats() {
    let ctx = ctx();
    seed_observation(&ctx.state.db, "obs-1", DAY_MS, None);

    let rt = tokio::runtime::Runtime::new().unwrap();

    for date in ["2026-09-30", "20260930"] {
        let router = router(Arc::clone(&ctx.state));
        let data = rt.block_on(get_data(
            router,
            &format!("/api/capture/screenshots?date={date}"),
        ));
        assert_eq!(
            data.as_array().unwrap().len(),
            1,
            "日期格式 {date} 必须被接受（渲染层默认用 YYYYMMDD，写入时用 YYYY-MM-DD）"
        );
    }
}

#[test]
fn screenshots_by_date_excludes_other_days() {
    let ctx = ctx();
    seed_observation(&ctx.state.db, "obs-today", DAY_MS, None);
    seed_observation(&ctx.state.db, "obs-yesterday", DAY_MS - 86_400_000, None);

    let router = router(Arc::clone(&ctx.state));
    let data = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(get_data(router, "/api/capture/screenshots?date=2026-09-30"));

    let items = data.as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "obs-today");
}

#[test]
fn screenshots_by_date_excludes_privacy_blocked() {
    let ctx = ctx();
    seed_observation(&ctx.state.db, "obs-ok", DAY_MS, None);

    let mut blocked = {
        let mut obs = mc_storage::observations::NewObservation {
            id: "obs-blocked".to_string(),
            ts: Timestamp::from_millis(DAY_MS + 1_000),
            source_id: "macos:screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some("1Password".to_string()),
            app_bundle_id: None,
            window_title: None,
            domain: None,
            display_id: None,
            scale_factor: None,
            image: None,
            text_content: None,
            text_origin: None,
            change_kind: "pixel_major".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: "idem-blocked".to_string(),
        };
        obs.privacy_verdict = "blocked".to_string();
        obs
    };
    blocked.privacy_verdict = "blocked".to_string();
    ctx.state.db.insert_observation(&blocked).unwrap();

    let router = router(Arc::clone(&ctx.state));
    let data = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(get_data(router, "/api/capture/screenshots?date=2026-09-30"));

    let items = data.as_array().unwrap();
    assert_eq!(items.len(), 1, "被隐私拦截的观测不应出现在时间线里");
    assert_eq!(items[0]["id"], "obs-ok");
}

#[test]
fn screenshots_by_date_without_observations_returns_empty_array() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    let data = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(get_data(router, "/api/capture/screenshots?date=2020-01-01"));

    assert_eq!(
        data.as_array().unwrap().len(),
        0,
        "空日期应返回空数组而不是错误"
    );
}

#[test]
fn screenshots_by_date_rejects_malformed_date() {
    let ctx = ctx();
    let response = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(
            router(Arc::clone(&ctx.state))
                .oneshot(get("/api/capture/screenshots?date=not-a-date", Some(TOKEN))),
        )
        .unwrap();

    let json = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(body_json(response));
    assert_ne!(json["code"], 0, "非法日期必须报错：{json}");
    assert_eq!(json["error_code"], "domain_invalid_timestamp");
}

// ---------------------------------------------------------------- 图片数据

#[test]
fn screenshot_data_endpoint_returns_base64_without_fs_path() {
    let ctx = ctx();
    let image = image::RgbImage::from_pixel(32, 24, image::Rgb([200, 100, 50]));
    let blob = ctx
        .blobs
        .put_image(
            &image,
            &mc_storage::blob::ImageMeta {
                captured_at: Timestamp::from_millis(DAY_MS),
                display_id: None,
            },
        )
        .unwrap();

    let router = router(Arc::clone(&ctx.state));
    let data = tokio::runtime::Runtime::new().unwrap().block_on(get_data(
        router,
        &format!(
            "/api/capture/screenshots/data?path={}",
            urlencode(&blob.relative_path)
        ),
    ));

    let encoded = data["data"].as_str().expect("必须返回 base64");
    assert!(!encoded.is_empty());

    // 响应里绝不能出现文件系统绝对路径或盘符
    let raw = serde_json::to_string(&data).unwrap();
    assert!(!raw.contains("/private/"), "响应泄漏了绝对路径：{raw}");
    assert!(!raw.contains("/var/"), "响应泄漏了绝对路径：{raw}");
    assert!(!raw.contains(".."), "响应泄漏了相对穿越路径：{raw}");
    assert_eq!(data["mime"], "image/png");
}

#[test]
fn screenshot_data_rejects_path_traversal() {
    let ctx = ctx();
    let rt = tokio::runtime::Runtime::new().unwrap();

    for evil in [
        "../../../etc/passwd",
        "/etc/passwd",
        "screenshots/../../../etc/passwd",
        "thumbnails/../../secret",
    ] {
        let router = router(Arc::clone(&ctx.state));
        let response = rt
            .block_on(router.oneshot(get(
                &format!("/api/capture/screenshots/data?path={}", urlencode(evil)),
                Some(TOKEN),
            )))
            .unwrap();
        let json = rt.block_on(body_json(response));
        assert_ne!(
            json["code"], 0,
            "路径穿越 `{evil}` 必须被拒绝，实际: {json}"
        );
    }
}

#[test]
fn screenshot_data_reports_missing_file_clearly() {
    let ctx = ctx();
    let router = router(Arc::clone(&ctx.state));

    let response = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(router.oneshot(get(
            "/api/capture/screenshots/data?path=screenshots/2026/09/30/missing.png",
            Some(TOKEN),
        )))
        .unwrap();
    let json = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(body_json(response));

    assert_ne!(json["code"], 0);
    assert!(
        !json["message"].as_str().unwrap_or_default().is_empty(),
        "必须给用户可读的说明：{json}"
    );
}

// ---------------------------------------------------------------- 录制开关

#[test]
fn capture_start_stop_status_roundtrip() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let ctx = ctx();

    // 初始为停止；canRecord 取决于本机是否已授予屏幕录制权限
    let data = rt.block_on(get_data(
        router(Arc::clone(&ctx.state)),
        "/api/capture/status",
    ));
    assert_eq!(data["status"], "stopped");

    let ready = mc_capture::platform::probe_readiness().available;
    assert_eq!(
        data["canRecord"], ready,
        "canRecord 必须如实反映本机采集就绪度"
    );

    if !ready {
        // 未就绪时必须**拒绝启动并给出可执行建议**，而不是谎报 running ——
        // 否则用户会看到「正在录制」却什么都采不到（典型的失败形态）。
        let response = rt
            .block_on(
                router(Arc::clone(&ctx.state)).oneshot(post("/api/capture/start", Some(TOKEN))),
            )
            .unwrap();
        let json = rt.block_on(body_json(response));
        assert_ne!(json["code"], 0);
        // 拒绝的具体理由随环境而不同：没有屏幕录制权限是 capture_permission_denied，
        // 没有可用显示会话（无头 / 锁屏 / 远程）是 capture_no_display —— 两者都是
        // 「如实拒绝」。这里断言的是**契约**（非零 code + 已知理由 + 可执行建议），
        // 而不是某一台机器当下的具体原因；把断言钉死在某一个理由上，会让这条测试
        // 随机器状态变红，从而掩盖它真正要守的东西。
        let reason = json["error_code"].as_str().unwrap_or_default();
        assert!(
            matches!(reason, "capture_permission_denied" | "capture_no_display"),
            "拒绝理由必须在已知集合内：{json}"
        );
        assert!(
            !json["remediation"].as_str().unwrap_or_default().is_empty(),
            "必须告诉用户去哪里授权：{json}"
        );
        return;
    }

    // 已就绪：start/stop 往返
    let mut events = ctx.state.events.subscribe();
    let response = rt
        .block_on(router(Arc::clone(&ctx.state)).oneshot(post("/api/capture/start", Some(TOKEN))))
        .unwrap();
    let json = rt.block_on(body_json(response));
    assert_eq!(json["code"], 0, "{json}");

    let data = rt.block_on(get_data(
        router(Arc::clone(&ctx.state)),
        "/api/capture/status",
    ));
    assert_eq!(data["status"], "running");

    // 托盘切换录制时页面没有别的信息来源：状态变化必须推事件（裸字符串）
    let event = events.try_recv().expect("开始录制要推事件");
    assert_eq!(event.kind, "push:screen-monitor-status");
    assert_eq!(event.data, serde_json::json!("running"));

    rt.block_on(router(Arc::clone(&ctx.state)).oneshot(post("/api/capture/stop", Some(TOKEN))))
        .unwrap();

    let data = rt.block_on(get_data(
        router(Arc::clone(&ctx.state)),
        "/api/capture/status",
    ));
    assert_eq!(data["status"], "stopped");
    let event = events.try_recv().expect("停止录制要推事件");
    assert_eq!(event.data, serde_json::json!("stopped"));
}

#[test]
fn capture_endpoints_require_token() {
    let ctx = ctx();
    let rt = tokio::runtime::Runtime::new().unwrap();

    for uri in [
        "/api/capture/status",
        "/api/capture/targets",
        "/api/capture/permissions",
        "/api/capture/screenshots",
    ] {
        let response = rt
            .block_on(router(Arc::clone(&ctx.state)).oneshot(get(uri, None)))
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{uri} 必须要求 token"
        );
    }
}

#[test]
fn capture_endpoints_without_controls_report_unavailable() {
    // 没有接采集控制的进程（例如只跑检索的实例）也要给出明确状态，而不是 500
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(DAY_MS),
        dir.path().to_path_buf(),
    ));

    let rt = tokio::runtime::Runtime::new().unwrap();
    let response = rt
        .block_on(router(state).oneshot(get("/api/capture/status", Some(TOKEN))))
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = rt.block_on(body_json(response));
    assert_eq!(json["data"]["canRecord"], false);
    assert_eq!(json["data"]["status"], "stopped");
}

fn urlencode(value: &str) -> String {
    value.replace('/', "%2F").replace(' ', "%20")
}

// 不能录制时必须说明原因：用户看到「不能录制」却不知道是缺屏幕录制权限、
// 还是没有可用显示器，只能去点一次「开始」才知道。
// 本机是否可录制取决于屏幕权限，所以断言是条件式的：不可录制时必须有非空原因，
// 可录制时不该带拒绝原因。
#[test]
fn capture_status_explains_why_recording_is_unavailable() {
    let ctx = ctx();
    let response = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(router(Arc::clone(&ctx.state)).oneshot(get("/api/capture/status", Some(TOKEN))))
        .unwrap();
    let json = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(body_json(response));

    assert_eq!(json["code"], 0, "{json}");
    let can_record = json["data"]["canRecord"].as_bool().unwrap_or(false);
    if can_record {
        assert!(
            json["data"]["reason"].is_null(),
            "可录制时不该带拒绝原因：{json}"
        );
    } else {
        let reason = json["data"]["reason"].as_str().unwrap_or_default();
        assert!(!reason.is_empty(), "不可录制时必须说明原因：{json}");
    }
    assert!(
        json["data"]["capture_supported"].is_boolean(),
        "status 必须带 capture_supported：{json}"
    );
    assert_eq!(
        json["data"]["capture_supported"],
        mc_capture::platform::capture_supported(),
        "status.capture_supported 必须与平台探测一致：{json}"
    );
}
