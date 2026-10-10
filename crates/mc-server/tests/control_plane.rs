//! 控制面（axum）。
//!
//! 这些测试直接对 `Router` 发请求（`tower::ServiceExt::oneshot`），
//! 不启真实端口、不依赖网络 —— 这是「HTTP 作为可测试边界」这条选型的直接收益。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_storage::Database;
use tower::ServiceExt;

use mc_server::{router, ServerState};

const TOKEN: &str = "test-token-0123456789abcdef";

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
        Timestamp::from_millis(1_756_000_000_000),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
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

fn get_with_host(uri: &str, host: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", host);
    if let Some(t) = token {
        builder = builder.header("x-mc-token", t);
    }
    builder.body(Body::empty()).unwrap()
}

/// 浏览器预检：不带 token，带 Origin 与「想用的方法/头」。
fn preflight(uri: &str, origin: &str) -> Request<Body> {
    Request::builder()
        .method("OPTIONS")
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("origin", origin)
        .header("access-control-request-method", "GET")
        .header("access-control-request-headers", "x-mc-token")
        .body(Body::empty())
        .unwrap()
}

fn get_with_origin(uri: &str, origin: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("origin", origin);
    if let Some(t) = token {
        builder = builder.header("x-mc-token", t);
    }
    builder.body(Body::empty()).unwrap()
}

async fn json_body(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

#[tokio::test]
async fn health_returns_component_status() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(get("/api/health", None))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;

    // 渲染层 App.tsx 会读 data.components.llm 来决定是否引导配置
    assert_eq!(json["status"], "ok");
    assert!(json["data"]["components"].is_object());
    assert!(json["data"]["components"]["llm"].is_object());
    assert!(json["data"]["components"]["storage"].is_object());
    assert!(json["data"]["components"]["capture"].is_object());
    assert!(json["version"].is_string());
    assert!(json["data"]["components"]["llm"]["status"].is_string());
}

#[tokio::test]
async fn health_reports_embedding_paused_when_auth_paused() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let mut loaded = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    loaded.config.ai.embedding.base_url = "https://api.example.com/v1".to_string();
    loaded.config.ai.embedding.model = "embed-small".to_string();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(loaded),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(1_756_000_000_000),
        dir.path().to_path_buf(),
    ));
    state.set_embedding_auth_paused(true);
    let response = router(Arc::clone(&state))
        .oneshot(get("/api/health", None))
        .await
        .unwrap();
    let json = json_body(response).await;
    assert_eq!(
        json["data"]["components"]["embedding"]["status"], "paused",
        "{json}"
    );
}

#[tokio::test]
async fn health_works_without_token() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(get("/api/health", None))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

// 渲染层的启动握手：`App.tsx` 拿不到 running 就永远停在加载页
#[tokio::test]
async fn backend_status_reports_running() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(get("/api/backend/status", Some(TOKEN)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;
    assert_eq!(json["code"], 0);
    assert_eq!(json["data"]["status"], "running");
}

// 没有 token 时不回答：这个接口只服务本机渲染层
#[tokio::test]
async fn backend_status_requires_token() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(get("/api/backend/status", None))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// 0.24b — 健康检查不得泄漏敏感信息
#[tokio::test]
async fn health_does_not_leak_token_or_paths() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(get("/api/health", None))
        .await
        .unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);

    assert!(!text.contains(TOKEN), "健康检查泄漏了 token");
    assert!(!text.contains("/var/"), "健康检查泄漏了文件系统路径");
    assert!(!text.contains("minecontext.db"), "健康检查泄漏了数据库路径");
}

#[tokio::test]
async fn v1_routes_require_token() {
    let ctx = ctx();
    let app = router(Arc::clone(&ctx.state));

    let unauthorized = app
        .clone()
        .oneshot(get("/api/v1/stream", None))
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let wrong = app
        .clone()
        .oneshot(get("/api/v1/stream", Some("wrong-token")))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let ok = app
        .oneshot(get("/api/v1/stream", Some(TOKEN)))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
}

// 0.25b — 诊断接口同样受保护（它会暴露运行细节）
#[tokio::test]
async fn diagnostics_requires_token() {
    let ctx = ctx();
    let app = router(ctx.state);

    assert_eq!(
        app.clone()
            .oneshot(get("/api/diagnostics", None))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.oneshot(get("/api/diagnostics", Some(TOKEN)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

// 0.26 — DNS rebinding 防护：伪造 Host 必须被拒绝
#[tokio::test]
async fn host_header_must_be_loopback() {
    let ctx = ctx();
    let app = router(ctx.state);

    for bad in ["evil.example.com", "attacker.test:1234", "10.0.0.5"] {
        let response = app
            .clone()
            .oneshot(get_with_host("/api/health", bad, None))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "伪造 Host `{bad}` 必须被拒绝"
        );
    }

    for good in ["127.0.0.1:12345", "localhost:12345", "127.0.0.1"] {
        let response = app
            .clone()
            .oneshot(get_with_host("/api/health", good, None))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "loopback Host `{good}` 应当放行"
        );
    }
}

#[tokio::test]
async fn runtime_json_written_with_0600() {
    let dir = tempfile::tempdir().unwrap();
    let path = mc_server::runtime::write_runtime_file(
        dir.path(),
        &mc_server::runtime::RuntimeInfo {
            port: 17331,
            token: TOKEN.to_string(),
            pid: 4242,
            version: "0.1.0-test".to_string(),
            started_at: Timestamp::from_millis(1_756_000_000_000),
        },
    )
    .expect("runtime.json 必须能写出");

    assert_eq!(path.file_name().unwrap(), "runtime.json");

    let text = std::fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["port"], 17331);
    assert_eq!(json["token"], TOKEN);
    assert_eq!(json["pid"], 4242);
    assert!(json["started_at"].is_string());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "runtime.json 含 token，权限必须是 0600");
    }
}

#[tokio::test]
async fn diagnostics_includes_invariants() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(get("/api/diagnostics", Some(TOKEN)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = json_body(response).await;

    // 最高优先级不变量：非 0 就说明「有阶段没总结」被破坏
    let json = &json["data"];
    assert_eq!(json["invariants"]["stages_without_summary"], 0);
    assert!(json["version"].is_string());
    assert!(json["uptime_seconds"].as_u64().is_some());
    assert!(json["components"]["storage"]["status"].is_string());
    assert!(json["components"]["provider"]["status"].is_string());
    // 队列水位： 起就要求可观测
    assert!(json["queues"]["capture"].is_object());
    assert!(json["queues"]["vision"].is_object());
    assert!(json["queues"]["summary"].is_object());
    assert!(json["recent_failures"].is_array());
}

// 0.28b — 诊断必须反映真实的状态，不是写死的
#[tokio::test]
async fn diagnostics_reports_unconfigured_provider_honestly() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(get("/api/diagnostics", Some(TOKEN)))
        .await
        .unwrap();
    let json = json_body(response).await;

    // 默认配置里 base_url / model 为空 → 必须如实报告未配置，
    // 而不是「初始化失败也报告 healthy」
    assert_eq!(
        json["data"]["components"]["provider"]["status"],
        "unconfigured"
    );
}

// 0.29 — 兼容面失败时仍返回 HTTP 200 + 业务错误码
// （渲染层直接读 response.data.data，非 200 会直接抛异常）
#[tokio::test]
async fn error_envelope_is_200_with_code() {
    let ctx = ctx();
    // /api/add_screenshot 是契约里的真实路径， 尚未实现
    let response = router(ctx.state)
        .oneshot(get("/api/add_screenshot", Some(TOKEN)))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "兼容面不得返回非 200，否则渲染层会直接抛异常"
    );

    let json = json_body(response).await;
    assert_ne!(json["code"], 0, "失败必须在 body 里给出非零 code");
    assert!(json["message"].is_string());
    assert_eq!(json["data"], serde_json::Value::Null);
    assert_eq!(json["error_code"], "not_implemented");
    assert!(json["remediation"].is_string());
}

#[tokio::test]
async fn sse_endpoint_emits_ready_frame() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(get("/api/v1/stream", Some(TOKEN)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/event-stream"),
        "SSE 的 content-type 必须是 text/event-stream，实际: {content_type}"
    );

    // 首帧必须是 ready，且带端口/版本
    let mut body = response.into_body();
    let frame = body
        .frame()
        .await
        .expect("SSE 必须立刻产出首帧")
        .expect("首帧不应是错误");
    let text = String::from_utf8_lossy(&frame.into_data().unwrap_or_default()).to_string();

    assert!(
        text.contains("event: ready"),
        "首帧应为 ready，实际: {text}"
    );
    assert!(text.contains("\"version\""), "ready 帧应带版本: {text}");

    // 帧格式必须能被前端解析器消费：`data: {json}\n\n`
    assert!(
        text.contains("data: "),
        "SSE 帧必须使用 `data: ` 前缀: {text}"
    );
    assert!(text.contains("\n\n"), "SSE 帧必须以空行结束: {text}");
}

// ---------------------------------------------------------------- 兼容面契约

const COMPAT_CONTRACT: &str = include_str!("../../../fixtures/contract/compat-routes.json");

/// 把 FastAPI 的 `{name:path}` 转换成 axum 的 `{*name}`。
fn normalize(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let inner = &rest[start + 1..start + end];
        match inner.split_once(':') {
            Some((name, "path")) => out.push_str(&format!("{{*{name}}}")),
            Some((name, _)) => out.push_str(&format!("{{{name}}}")),
            None => out.push_str(&format!("{{{inner}}}")),
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

// 契约里的每条兼容面路由都必须有响应，不能 404 静默失败
#[tokio::test]
async fn compat_routes_all_present() {
    let ctx = ctx();
    let contract: serde_json::Value = serde_json::from_str(COMPAT_CONTRACT).unwrap();
    let routes = contract["routes"].as_array().expect("契约必须含 routes");

    assert!(routes.len() > 50, "契约条目过少，文件可能损坏");

    let app = router(Arc::clone(&ctx.state));
    let mut missing = Vec::new();

    for route in routes {
        let method = route["method"].as_str().unwrap();
        let path = route["path"].as_str().unwrap();
        let uri = normalize(path);

        let request = Request::builder()
            .method(method)
            .uri(&uri)
            .header("host", "127.0.0.1:1")
            .header("x-mc-token", TOKEN)
            .body(Body::empty())
            .unwrap();

        let response = app.clone().oneshot(request).await.unwrap();
        if response.status() == StatusCode::NOT_FOUND {
            missing.push(format!("{method} {path}"));
        }
    }

    assert!(
        missing.is_empty(),
        "以下兼容面路由未注册（渲染层会 404）：{missing:#?}"
    );
}

// 0.31b —— 证明上面的契约测试**能失败**：未知路径必须是真 404。
// 如果所有路径都由兜底接管，compat_routes_all_present 就永远通过、毫无意义。
#[tokio::test]
async fn unknown_path_returns_404_so_the_contract_test_is_not_vacuous() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(get("/api/definitely-not-a-real-route", Some(TOKEN)))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "未知路径必须 404，否则兼容面契约测试失去意义"
    );
}

// 0.31c —— 契约里带路径参数的路由也要能匹配
#[tokio::test]
async fn compat_routes_with_path_params_are_reachable() {
    let ctx = ctx();
    let app = router(ctx.state);

    for uri in [
        "/api/vaults/123",
        "/api/agent/chat/conversations/7/messages",
        "/files/some/deep/path.txt",
    ] {
        let response = app.clone().oneshot(get(uri, Some(TOKEN))).await.unwrap();
        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "带参数路由 {uri} 未匹配"
        );
    }
}

// 桌面外壳的 webview 是跨源访问（tauri://localhost → 127.0.0.1），带 X-MC-Token
// 就一定触发预检；预检不带 token，因此它必须由 CORS 层在**鉴权之前**回答。
#[tokio::test]
async fn preflight_is_answered_before_auth() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(preflight("/api/backend/status", "tauri://localhost"))
        .await
        .unwrap();

    assert!(
        response.status().is_success(),
        "预检不能被鉴权拦下，实际 {}",
        response.status()
    );
    let headers = response.headers();
    assert_eq!(
        headers
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("tauri://localhost")
    );
    let allowed_headers = headers
        .get("access-control-allow-headers")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(
        allowed_headers.contains("x-mc-token"),
        "必须允许自定义 token 头，否则浏览器不会发真正的请求：{allowed_headers}"
    );
}

#[tokio::test]
async fn actual_request_carries_cors_header() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(get_with_origin(
            "/api/backend/status",
            "tauri://localhost",
            Some(TOKEN),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("tauri://localhost")
    );
}

// 白名单之外不放行：否则任意网页都能读这台机器上的采集数据
#[tokio::test]
async fn unknown_origin_is_not_allowed() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(preflight("/api/backend/status", "https://evil.example"))
        .await
        .unwrap();

    assert!(
        response
            .headers()
            .get("access-control-allow-origin")
            .is_none(),
        "白名单外的来源不该拿到 allow-origin"
    );
}

// CORS 不是鉴权：预检放行之后，真正的请求仍然要 token
#[tokio::test]
async fn cors_does_not_weaken_auth() {
    let ctx = ctx();
    let response = router(ctx.state)
        .oneshot(get_with_origin(
            "/api/backend/status",
            "tauri://localhost",
            None,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// 采集状态必须如实报告：恒 "not_started" 会让诊断页与健康检查看到假状态
#[tokio::test]
async fn health_reports_capture_status_honestly() {
    // 只读实例（没有采集控制）如实说 not_started
    let ctx = ctx();
    let response = router(Arc::clone(&ctx.state))
        .oneshot(get("/api/health", None))
        .await
        .unwrap();
    let json = json_body(response).await;
    assert_eq!(
        json["data"]["components"]["capture"]["status"],
        "not_started"
    );

    // 挂上采集控制并 start() 之后必须变成 running
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let blobs = Arc::new(
        mc_storage::blob::FileSystemBlobStore::new(
            dir.path().join("blobs"),
            mc_storage::blob::ImageFormat::Png,
        )
        .unwrap(),
    );
    let source: Arc<dyn mc_capture::source::CaptureSource> = Arc::new(
        mc_testkit::capture::FakeCaptureSource::builder()
            .screen("display-1", "Built-in Retina Display", 2.0)
            .build()
            .unwrap(),
    );
    let controls = Arc::new(mc_server::CaptureControls::new(source, blobs));
    controls.start();
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(
        ServerState::new(
            mc_config::ConfigHandle::new(config),
            db,
            TOKEN.to_string(),
            Timestamp::from_millis(1_756_000_000_000),
            dir.path().to_path_buf(),
        )
        .with_capture(controls),
    );

    let response = router(state)
        .oneshot(get("/api/health", None))
        .await
        .unwrap();
    let json = json_body(response).await;
    assert_eq!(json["data"]["components"]["capture"]["status"], "running");
}

// F8-2：locale 解析逻辑在两处各写了一份（mc-pipeline::prompts::Locale 与
// mc-summary::model::SummaryLocale）。两份实现一旦漂移，同一份配置会在"提示词语言"
// 与"证据语言"上得到不同结论 —— 这里用一组输入把两者钉在一起。
#[test]
fn locale_parsers_agree_across_crates() {
    let cases = [
        "zh-CN", "zh", "en-US", "en", "EN", "En-us", "", "fr", "ja", "de-DE",
    ];

    for value in cases {
        let prompts = mc_pipeline::prompts::Locale::from_config(value);
        let summary = mc_summary::model::SummaryLocale::from_config(value);
        let same = matches!(
            (prompts, summary),
            (
                mc_pipeline::prompts::Locale::ZhCn,
                mc_summary::model::SummaryLocale::ZhCn
            ) | (
                mc_pipeline::prompts::Locale::EnUs,
                mc_summary::model::SummaryLocale::EnUs
            )
        );
        assert!(
            same,
            "locale 解析在两处不一致：{value:?} → {prompts:?} / {summary:?}"
        );
    }
}
