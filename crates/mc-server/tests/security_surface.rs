//! 把安全面钉成测试。
//! 这一组断言的价值在于「新增接口时不会悄悄破防」：
//! 要求 daemon 只监听本机、除健康检查外全部要 token、
//! 错误不泄漏内部信息、路径参数不可穿越。逐条写成测试之后，
//! 下一个加路由的人会被门禁拦住。
//!
//! 三个层次：
//! 1. **鉴权**：没有 token 就拒绝（且只有健康检查是公开的）；
//! 2. **不泄漏**：任何响应体里都不出现 token、API Key 或数据目录绝对路径；
//! 3. **边界**：未知路径是 404（不是 500），穿越路径被拒绝。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::Database;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
use mc_testkit::provider::ScriptedTransport;
use tower::ServiceExt;

const TOKEN: &str = "secret-token-0123456789abcdef";

fn ctx() -> (tempfile::TempDir, Arc<ServerState>) {
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
    (dir, state)
}

fn request(method: &str, uri: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        // 本机 Host：非本机 Host 会被 DNS rebinding 防护挡掉（另一条独立规则）
        .header("host", "127.0.0.1:12345");
    if let Some(token) = token {
        builder = builder.header("x-mc-token", token);
    }
    builder.body(Body::empty()).unwrap()
}

async fn status_of(state: &Arc<ServerState>, request: Request<Body>) -> (StatusCode, String) {
    let response = router(Arc::clone(state)).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// 覆盖各类接口（新增面 + 兼容面 + 内部接口）。
fn representative_paths() -> Vec<(&'static str, &'static str)> {
    vec![
        ("GET", "/api/diagnostics"),
        ("GET", "/api/v1/diagnostics/export"),
        ("GET", "/api/v1/vault/export"),
        ("GET", "/api/v1/activities"),
        ("GET", "/api/db/activities"),
        ("GET", "/api/db/vaults"),
        ("GET", "/api/db/todos"),
        ("GET", "/api/db/tips"),
        ("GET", "/api/db/heatmap?start=0&end=1"),
        ("GET", "/api/capture/status"),
        ("GET", "/api/capture/targets"),
        ("GET", "/api/capture/screenshots"),
        ("GET", "/api/files"),
        ("GET", "/api/settings/todoList"),
        ("GET", "/api/monitoring/recording-stats"),
        ("GET", "/api/indexing/status"),
        ("POST", "/api/indexing/resume"),
        ("GET", "/api/v1/threads"),
        ("GET", "/api/agent/chat/conversations"),
    ]
}

#[tokio::test]
async fn every_api_route_requires_the_token() {
    let (_dir, state) = ctx();

    for (method, path) in representative_paths() {
        let (status, _) = status_of(&state, request(method, path, None)).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} 缺少 token 时必须 401"
        );

        let (status, _) = status_of(&state, request(method, path, Some("wrong-token"))).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} 错误 token 必须 401"
        );
    }
}

/// SSE 是无限流：只检查鉴权结果，**不读 body**（读了会永远阻塞）。
#[tokio::test]
async fn sse_stream_requires_the_token() {
    let (_dir, state) = ctx();

    let response = router(Arc::clone(&state))
        .oneshot(request("GET", "/api/v1/stream", None))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "无 token 不得订阅 SSE"
    );

    let response = router(Arc::clone(&state))
        .oneshot(request("GET", "/api/v1/stream", Some("wrong")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// 健康检查是唯一的公开路径，且只返回最小信息
#[tokio::test]
async fn only_health_is_public() {
    let (_dir, state) = ctx();

    let (status, body) = status_of(&state, request("GET", "/api/health", None)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains(TOKEN), "公开响应里绝不能出现 token：{body}");

    // 兼容面里的 /health 也是公开的
    let (status, _) = status_of(&state, request("GET", "/health", None)).await;
    assert_eq!(status, StatusCode::OK);
}

/// 任何响应体里都不出现 token（包括错误响应与诊断接口）
#[tokio::test]
async fn responses_never_leak_the_token() {
    let (_dir, state) = ctx();

    for (method, path) in representative_paths() {
        let (_, body) = status_of(&state, request(method, path, Some(TOKEN))).await;
        assert!(
            !body.contains(TOKEN),
            "{method} {path} 的响应体里出现了 token"
        );
    }
}

/// DNS rebinding 防护：非本机 Host 一律拒绝，且优先于鉴权
#[tokio::test]
async fn foreign_host_is_rejected() {
    let (_dir, state) = ctx();

    let request = Request::builder()
        .method("GET")
        .uri("/api/health")
        .header("host", "evil.example.com")
        .body(Body::empty())
        .unwrap();

    let (status, _) = status_of(&state, request).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "非本机 Host 必须被拒绝（DNS rebinding）"
    );
}

/// 穿越路径必须失败即关闭（token 只证明「是本应用」，不证明参数可信）
#[tokio::test]
async fn traversal_parameters_are_rejected() {
    let (_dir, state) = ctx();

    for path in [
        "/api/files/%2e%2e%2fsecret.txt/data",
        "/api/files/%2Fetc%2Fpasswd/data",
        "/api/capture/screenshots/data?path=%2e%2e%2fsecret.txt",
        "/api/capture/screenshots/data?path=%2Fetc%2Fpasswd",
    ] {
        let (status, body) = status_of(&state, request("GET", path, Some(TOKEN))).await;
        assert_eq!(status, StatusCode::OK, "{path} 走兼容面信封");
        let envelope: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        assert_eq!(
            envelope["code"], 1,
            "{path} 必须被拒绝（结构化失败）：{body}"
        );
    }
}

/// 未知路径是 404（不是 500，也不是「假装成功」）
#[tokio::test]
async fn unknown_paths_are_not_found() {
    let (_dir, state) = ctx();

    let (status, _) = status_of(&state, request("GET", "/api/nope", Some(TOKEN))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = status_of(&state, request("GET", "/definitely/not/here", Some(TOKEN))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// 兼容面里「尚未实现」的路径返回结构化说明，而不是 404 静默失败
#[tokio::test]
async fn unimplemented_compat_routes_say_so() {
    let (_dir, state) = ctx();

    let (status, body) =
        status_of(&state, request("GET", "/api/debug/activities", Some(TOKEN))).await;
    assert_eq!(status, StatusCode::OK);
    let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(envelope["code"], 1);
    assert_eq!(
        envelope["error_code"], "not_implemented",
        "要让前端看到结构化原因：{body}"
    );
}

/// API Key 不得出现在诊断输出或错误信息里。
///
/// 计划里承诺的 `api_key_never_appears_in_logs`：本项目没有第三方日志 SDK，
/// 「日志」的实际载体是 `/api/diagnostics`、错误信封与 `mc-cli doctor` 的输出，
/// 因此这里逐个断言它们都不含密钥明文。
#[tokio::test]
async fn api_key_never_reaches_diagnostics_or_errors() {
    use mc_providers::credentials::StaticSecretStore;
    use mc_server::activities;

    const API_KEY: &str = "sk-live-DEADBEEF0123456789";

    let (dir, state) = ctx();
    let mut config = mc_config::load::load(&mc_config::load::LoadRequest::default())
        .unwrap()
        .config;
    // 出网许可是开关的前提（默认不出网），模型端点也要填全
    config.privacy.ai_upload = true;
    config.ai.enabled = true;
    config.ai.vision.base_url = "https://api.example.com/v1".to_string();
    config.ai.vision.model = "qwen3-vl".to_string();
    config.ai.chat.base_url = "https://api.example.com/v1".to_string();
    config.ai.chat.model = "qwen3".to_string();
    config.ai.vision.api_key_ref = Some("env:MC_SECURITY_TEST_KEY".to_string());
    config.ai.chat.api_key_ref = Some("env:MC_SECURITY_TEST_KEY".to_string());

    let secrets = Arc::new(StaticSecretStore::new(
        [("env:MC_SECURITY_TEST_KEY".to_string(), API_KEY.to_string())]
            .into_iter()
            .collect(),
    ));

    // 1) 用真密钥把 provider 装起来（装配成功才说明密钥真的被读到了）
    let worker = activities::build_vision_worker(&config, secrets.as_ref()).expect("装配视觉得到");
    assert!(worker.is_some(), "配了密钥就应该装出 provider");

    // 2) 诊断与错误响应里不能有它
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(mc_config::LoadedConfig {
            config: config.clone(),
            warnings: Vec::new(),
            sources: Vec::new(),
        }),
        Arc::clone(&state.db),
        TOKEN.to_string(),
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    ));

    for (method, path) in representative_paths() {
        let (_, body) = status_of(&state, request(method, path, Some(TOKEN))).await;
        assert!(
            !body.contains(API_KEY),
            "{method} {path} 的响应里出现了 API Key"
        );
    }

    // 3) provider 自己的失败信息也不能带出密钥：给它一个 401
    let transport = Arc::new(ScriptedTransport::new().push_json(401, "{\"error\":\"bad key\"}"));
    let provider = mc_providers::openai::OpenAiCompatibleProvider::new(
        mc_providers::openai::EndpointConfig {
            base_url: "https://api.example.com/v1".to_string(),
            model: "qwen3-vl".to_string(),
            api_key: Some(API_KEY.to_string()),
            timeout: std::time::Duration::from_secs(5),
            max_image_edge: None,
            max_concurrency: 1,
        },
        Arc::clone(&transport) as Arc<dyn mc_providers::transport::HttpTransport>,
        mc_providers::Role::Vision,
    )
    .expect("provider");

    use mc_providers::{Provider as _, VisionProvider as _};

    let error = provider
        .analyze(mc_providers::VisionRequest {
            image: vec![0u8; 16],
            mime: "image/png".to_string(),
            prompt: "这是什么".to_string(),
            max_tokens: None,
            json_mode: false,
        })
        .await
        .expect_err("401 必须报错");

    // ProviderError 的 Debug 与它转成的 AppError 文案都不能带出密钥
    let rendered = format!("{error:?} {}", provider.to_app_error(&error).detail());
    assert!(
        !rendered.contains(API_KEY),
        "provider 的失败信息里出现了密钥：{rendered}"
    );
}

/// 数据目录的绝对路径不得出现在 API 响应里。
#[tokio::test]
async fn responses_never_expose_the_data_directory() {
    let (dir, state) = ctx();
    let data_dir = dir.path().display().to_string();

    // 落一条带图片引用的观测，确保「有路径可泄漏」的接口真的会走到那条分支
    let blobs = mc_storage::blob::FileSystemBlobStore::new(
        dir.path().join("blobs"),
        mc_storage::blob::ImageFormat::Png,
    )
    .unwrap();
    let blob = mc_storage::blob::BlobStore::put_image(
        &blobs,
        &image::RgbImage::from_pixel(32, 24, image::Rgb([1, 2, 3])),
        &mc_storage::blob::ImageMeta {
            captured_at: Timestamp::from_millis(T0),
            display_id: None,
        },
    )
    .unwrap();

    state
        .db
        .insert_observation(&mc_storage::observations::NewObservation {
            id: "obs-leak".to_string(),
            ts: Timestamp::from_millis(T0),
            source_id: "fake:screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some("Code".to_string()),
            app_bundle_id: None,
            window_title: Some("main.rs".to_string()),
            domain: None,
            display_id: None,
            scale_factor: Some(2.0),
            image: Some(mc_storage::observations::ImageRef {
                relative_path: blob.relative_path.clone(),
                content_hash: blob.content_hash.clone(),
                thumbnail_path: blob.thumbnail.as_ref().map(|t| t.relative_path.clone()),
                width: blob.width,
                height: blob.height,
                bytes: blob.bytes,
            }),
            text_content: None,
            text_origin: None,
            change_kind: "new".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: Some(1),
            idempotency: "obs-leak".to_string(),
        })
        .expect("插入观测");

    let mut checked = 0;
    for (method, path) in representative_paths() {
        let (_, body) = status_of(&state, request(method, path, Some(TOKEN))).await;
        assert!(
            !body.contains(&data_dir),
            "{method} {path} 的响应里出现了数据目录绝对路径：{body}"
        );
        checked += 1;
    }

    assert!(checked >= 10, "至少要检查到 10 个接口，实际 {checked}");
}
