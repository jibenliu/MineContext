//! `/api/capture/targets` 必须如实报告**选中状态**。
//!
//! 前端用它渲染勾选态。若这里只回一个静态 `true`，用户看到的界面
//! 会与实际采集的显示器不一致 —— 而且不会有任何报错。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_capture::source::{
    CaptureSource, CaptureTarget, PermissionState, RawCapture, SourceCapabilities, SourceHealth,
    SourceKind,
};
use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_server::{router, CaptureControls, ServerState};
use mc_storage::blob::{FileSystemBlobStore, ImageFormat};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

struct TwoScreenSource;

#[async_trait::async_trait]
impl CaptureSource for TwoScreenSource {
    fn id(&self) -> &str {
        "fake:screen"
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Screen
    }
    fn capabilities(&self) -> SourceCapabilities {
        SourceCapabilities::SCREEN
    }
    async fn enumerate(&self) -> Result<Vec<CaptureTarget>, AppError> {
        Ok(vec![
            CaptureTarget::screen("display-1", "Display 1", 2.0),
            CaptureTarget::screen("display-2", "Display 2", 1.0),
        ])
    }
    async fn poll(
        &self,
        _ctx: &mc_capture::source::CaptureContext,
    ) -> Result<Vec<RawCapture>, AppError> {
        Ok(Vec::new())
    }
    async fn health(&self) -> SourceHealth {
        SourceHealth {
            available: true,
            permission: PermissionState::Granted,
            message: None,
        }
    }
}

fn state_with_selection(target_ids: &[&str]) -> (tempfile::TempDir, Arc<ServerState>) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::Inline {
            name: "test".to_string(),
            toml: format!("[capture]\ntarget_ids = {target_ids:?}\n"),
        }],
        env: Vec::new(),
        read_process_env: false,
    })
    .unwrap();

    let blobs = Arc::new(
        FileSystemBlobStore::new(dir.path().join("blobs"), ImageFormat::Png).expect("blob store"),
    );
    let state = ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    )
    .with_capture(Arc::new(CaptureControls::new(
        Arc::new(TwoScreenSource) as Arc<dyn CaptureSource>,
        blobs,
    )));

    (dir, Arc::new(state))
}

async fn targets(state: &Arc<ServerState>) -> Vec<serde_json::Value> {
    let request = Request::builder()
        .method("GET")
        .uri("/api/capture/targets")
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN)
        .body(Body::empty())
        .unwrap();
    let response = router(Arc::clone(state)).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(envelope["code"], 0, "{envelope}");
    envelope["data"].as_array().unwrap().clone()
}

#[tokio::test]
async fn empty_selection_marks_everything_selected() {
    let (_dir, state) = state_with_selection(&[]);
    let items = targets(&state).await;
    assert_eq!(items.len(), 2);
    assert!(
        items.iter().all(|item| item["selected"] == true),
        "空选择 = 全部选中：{items:?}"
    );
}

#[tokio::test]
async fn selection_is_reported_per_target() {
    let (_dir, state) = state_with_selection(&["display-2"]);
    let items = targets(&state).await;

    let selected: Vec<&str> = items
        .iter()
        .filter(|item| item["selected"] == true)
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(selected, vec!["display-2"], "{items:?}");

    // 这些字段前端还在读，不能删
    let first = &items[0];
    for field in ["id", "name", "type", "isVisible", "scaleFactor"] {
        assert!(first.get(field).is_some(), "缺字段 {field}：{first}");
    }
}
