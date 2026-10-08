//! 诊断面必须如实报告平台支持范围与采集状态。
//!
//! 两条都是「不报错但会误导」的地方：
//! 用户装在不受支持的系统上时，需要看到明确的结论；
//! 采集是否在跑，也不能是一个写死的字符串。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_capture::source::CaptureSource;
use mc_common::time::Timestamp;
use mc_server::{router, CaptureControls, ServerState};
use mc_storage::blob::{FileSystemBlobStore, ImageFormat};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

struct NullSource;

#[async_trait::async_trait]
impl CaptureSource for NullSource {
    fn id(&self) -> &str {
        "null:screen"
    }
    fn kind(&self) -> mc_capture::source::SourceKind {
        mc_capture::source::SourceKind::Screen
    }
    fn capabilities(&self) -> mc_capture::source::SourceCapabilities {
        mc_capture::source::SourceCapabilities::SCREEN
    }
    async fn enumerate(
        &self,
    ) -> Result<Vec<mc_capture::source::CaptureTarget>, mc_common::error::AppError> {
        Ok(Vec::new())
    }
    async fn poll(
        &self,
        _ctx: &mc_capture::source::CaptureContext,
    ) -> Result<Vec<mc_capture::source::RawCapture>, mc_common::error::AppError> {
        Ok(Vec::new())
    }
    async fn health(&self) -> mc_capture::source::SourceHealth {
        mc_capture::source::SourceHealth {
            available: false,
            permission: mc_capture::source::PermissionState::NotRequired,
            message: None,
        }
    }
}

fn make_state(with_capture: bool) -> (tempfile::TempDir, Arc<ServerState>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let mut server = ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    );

    if with_capture {
        let blobs =
            Arc::new(FileSystemBlobStore::new(dir.path().join("blobs"), ImageFormat::Png).unwrap());
        server = server.with_capture(Arc::new(CaptureControls::new(
            Arc::new(NullSource) as Arc<dyn CaptureSource>,
            blobs,
        )));
    }

    (dir, Arc::new(server))
}

async fn diagnostics(state: &Arc<ServerState>) -> serde_json::Value {
    let request = Request::builder()
        .method("GET")
        .uri("/api/diagnostics")
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN)
        .body(Body::empty())
        .unwrap();

    let response = router(Arc::clone(state)).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn platform_section_reports_the_supported_range() {
    let (_dir, state) = make_state(true);
    let payload = diagnostics(&state).await;

    let platform = &payload["data"]["platform"];
    assert_eq!(platform["minimum"], "13.0", "最低支持 macOS 13：{platform}");
    assert!(
        platform["supported"].is_boolean(),
        "必须给出明确结论：{platform}"
    );
    assert!(
        platform["message"]
            .as_str()
            .unwrap_or_default()
            .contains("macOS"),
        "要能直接展示给用户：{platform}"
    );
    // 版本可能探测不到（非 macOS / 命令不可用），但字段必须在
    assert!(platform.get("version").is_some(), "{platform}");
}

#[tokio::test]
async fn capture_status_is_not_hardcoded() {
    let (_dir, state) = make_state(true);
    let payload = diagnostics(&state).await;
    assert_eq!(
        payload["data"]["components"]["capture"]["status"], "stopped",
        "未启动时必须是 stopped：{payload}"
    );

    state.capture.as_ref().unwrap().start();
    let payload = diagnostics(&state).await;
    assert_eq!(
        payload["data"]["components"]["capture"]["status"], "running",
        "启动后必须如实报告：{payload}"
    );

    // 只读实例（没挂载采集）要说清是「不可用」，而不是假装 stopped
    let (_dir2, readonly) = make_state(false);
    let payload = diagnostics(&readonly).await;
    assert_eq!(
        payload["data"]["components"]["capture"]["status"], "unavailable",
        "{payload}"
    );
}
