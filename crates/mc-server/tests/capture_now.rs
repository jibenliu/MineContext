//! `POST /api/capture/now`（按需截一张）。
//!
//! 渲染层的 `screen-monitor:take-screenshot` 走这条路径（`channel-map.ts`）。
//! 契约（`electron.d.ts` 的 `takeScreenshot`）：
//! `{ success, screenshotInfo: { url, date, timestamp }, error? }`。
//!
//! 其中 `url` 会被原样交给 `readImageAsBase64(url)` →
//! `GET /api/capture/screenshots/data?path=<url>`，因此它必须是 **blob store 内的相对路径**
//! （`screenshots/2026/09/30/<hash>.png`）；返回绝对路径的话，第二步读取会被路径校验拒绝。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_capture::source::{
    CaptureContext, CaptureSource, CaptureTarget, PermissionState, RawCapture, SourceCapabilities,
    SourceHealth, SourceKind,
};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use mc_server::{router, CaptureControls, ServerState};
use mc_storage::blob::{FileSystemBlobStore, ImageFormat};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

/// 采集源：成功时给一张固定图，失败模式给 `Err`。
struct ScriptedSource {
    fail: bool,
}

#[async_trait::async_trait]
impl CaptureSource for ScriptedSource {
    fn id(&self) -> &str {
        "screen:display-1"
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Screen
    }
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::SCREEN
    }

    async fn enumerate(&self) -> Result<Vec<CaptureTarget>, AppError> {
        Ok(vec![CaptureTarget::screen("display-1", "Display 1", 2.0)])
    }

    async fn poll(&self, ctx: &CaptureContext) -> Result<Vec<RawCapture>, AppError> {
        if self.fail {
            return Err(AppError::new(ErrorCode::CaptureIo, "模拟采集失败"));
        }
        let image = image::RgbImage::from_fn(32, 24, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 6) as u8, 200])
        });
        Ok(vec![RawCapture {
            source_id: "fake:screen".to_string(),
            source_kind: SourceKind::Screen,
            target: CaptureTarget::screen(
                ctx.targets
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "display-1".to_string()),
                "Display 1",
                2.0,
            ),
            captured_at: ctx.now,
            image: Some(image),
            text: None,
        }])
    }

    async fn health(&self) -> SourceHealth {
        SourceHealth {
            available: true,
            permission: PermissionState::Granted,
            message: None,
        }
    }
}

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
}

fn ctx(mount_capture: bool, fail: bool) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();

    let mut state = ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    );

    if mount_capture {
        let blobs = Arc::new(
            FileSystemBlobStore::new(dir.path().join("blobs"), ImageFormat::Png)
                .expect("blob store"),
        );
        state = state.with_capture(Arc::new(CaptureControls::new(
            Arc::new(ScriptedSource { fail }) as Arc<dyn CaptureSource>,
            blobs,
        )));
    }

    Ctx {
        _dir: dir,
        state: Arc::new(state),
    }
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

async fn call(
    state: &Arc<ServerState>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(request(method, uri, body))
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

// ---------------------------------------------------------------- 成功路径

#[tokio::test]
async fn takes_a_screenshot_and_persists_the_observation() {
    let ctx = ctx(true, false);

    let envelope = call(
        &ctx.state,
        "POST",
        "/api/capture/now",
        Some(serde_json::json!({
            // 渲染层会把分组时间戳（毫秒字符串）一起发过来；服务端不使用它
            "group_interval": mc_testkit::fixtures::FIXTURE_EPOCH_MS.to_string(),
            "target_id": "display-1"
        })),
    )
    .await;
    let payload = data(&envelope);

    assert_eq!(payload["success"], true, "{payload}");
    let info = &payload["screenshotInfo"];
    assert!(
        info["url"]
            .as_str()
            .unwrap_or_default()
            .starts_with("screenshots/"),
        "url 必须是 blob store 内的相对路径（下一步要用它读图）：{info}"
    );
    assert!(info["date"].is_string(), "{info}");
    assert!(
        info["timestamp"].as_i64().unwrap_or(0) > 0,
        "timestamp 是毫秒：{info}"
    );

    // 截图必须真的落库（否则列表里看不到刚截的那张）
    assert_eq!(ctx.state.db.observation_count().unwrap(), 1);
    let rows = ctx
        .state
        .db
        .query_observations(&mc_storage::observations::ObservationQuery::default())
        .unwrap();
    assert_eq!(
        rows[0].image_path.as_deref(),
        info["url"].as_str(),
        "观测里的图片路径必须与返回的 url 一致"
    );
    assert_eq!(rows[0].kind, "screen");
}

#[tokio::test]
async fn the_returned_url_can_be_read_back_through_the_image_endpoint() {
    let ctx = ctx(true, false);

    let payload = data(
        &call(
            &ctx.state,
            "POST",
            "/api/capture/now",
            Some(serde_json::json!({ "target_id": "display-1" })),
        )
        .await,
    );
    let url = payload["screenshotInfo"]["url"]
        .as_str()
        .unwrap()
        .to_string();

    // 渲染层就是这么串起来的：先 takeScreenshot，再 readImageAsBase64(url)
    let envelope = call(
        &ctx.state,
        "GET",
        &format!("/api/capture/screenshots/data?path={url}"),
        None,
    )
    .await;
    let image = data(&envelope);
    assert_eq!(image["mime"], "image/png");
    let base64 = image["data"].as_str().expect("必须返回 base64");
    assert!(!base64.is_empty());

    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64)
        .expect("必须是合法 base64");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "必须是 PNG 字节流");
}

/// 不传 `target_id` 时用第一个可见目标（前端有些调用点是这么发的）
#[tokio::test]
async fn defaults_to_the_first_available_target() {
    let ctx = ctx(true, false);
    let payload = data(
        &call(
            &ctx.state,
            "POST",
            "/api/capture/now",
            Some(serde_json::json!({})),
        )
        .await,
    );
    assert_eq!(payload["success"], true, "{payload}");
}

// ---------------------------------------------------------------- 失败路径

/// 采集失败按旧语义返回 `{success:false, error}`，不抛异常
#[tokio::test]
async fn capture_failure_is_a_payload_error() {
    let ctx = ctx(true, true);
    let envelope = call(
        &ctx.state,
        "POST",
        "/api/capture/now",
        Some(serde_json::json!({ "target_id": "display-1" })),
    )
    .await;

    let payload = data(&envelope);
    assert_eq!(payload["success"], false, "{payload}");
    assert!(payload["error"].is_string());
    assert_eq!(ctx.state.db.observation_count().unwrap(), 0);
}

#[tokio::test]
async fn unknown_target_is_reported_clearly() {
    let ctx = ctx(true, false);
    let payload = data(
        &call(
            &ctx.state,
            "POST",
            "/api/capture/now",
            Some(serde_json::json!({ "target_id": "display-42" })),
        )
        .await,
    );
    assert_eq!(payload["success"], false, "{payload}");
    assert!(
        payload["error"]
            .as_str()
            .unwrap_or_default()
            .contains("display-42"),
        "报错要指出是哪个目标：{payload}"
    );
}

/// 只读实例（没有挂载采集控制）必须给出结构化错误，而不是 500
#[tokio::test]
async fn without_capture_controls_it_fails_structurally() {
    let ctx = ctx(false, false);
    let envelope = call(
        &ctx.state,
        "POST",
        "/api/capture/now",
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(envelope["code"], 1, "{envelope}");
    assert_eq!(envelope["error_code"], "storage_unavailable");
}
