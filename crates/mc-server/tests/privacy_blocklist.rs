//! 隐私黑名单必须**真的生效**（配置里写了就要拦）。
//!
//! 规则引擎（`mc-pipeline::pump::evaluate_privacy`）早就实现了应用名与窗口标题
//! 的拦截，但采集环必须真的把黑名单传给它 —— 传空列表时配置形同虚设。
//! 这类「配置看起来生效了、实际一次都没执行」的缺口，是隐私要求里最危险的一种：
//! 用户以为自己屏蔽了密码管理器，实际每张截图都落了盘。
//!
//! 这一片把配置接进策略，并断言两件事：
//! 1. 命中黑名单的帧**不落盘、不落库**（观测的图片字段为空）；
//! 2. 拦截**可见**（`privacy_blocked` 计数增加），但审计里不含窗口标题。

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_capture::source::{
    CaptureContext, CaptureSource, CaptureTarget, PermissionState, RawCapture, SourceCapabilities,
    SourceHealth, SourceKind,
};
use mc_common::error::AppError;
use mc_common::time::Timestamp;
use mc_server::{capture_loop, router, CaptureControls, ServerState};
use mc_storage::blob::{FileSystemBlobStore, ImageFormat};
use mc_storage::Database;
use tower::ServiceExt;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

const TOKEN: &str = "test-token";

/// 两个目标：一个在密码管理器里，一个在编辑器里。
struct TwoAppSource;

#[async_trait::async_trait]
impl CaptureSource for TwoAppSource {
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
        let mut vault = CaptureTarget::window("window-1", "1Password 主密码", "1Password");
        vault.is_visible = true;
        let mut editor = CaptureTarget::window("window-2", "main.rs", "Visual Studio Code");
        editor.is_visible = true;
        Ok(vec![vault, editor])
    }
    async fn poll(&self, ctx: &CaptureContext) -> Result<Vec<RawCapture>, AppError> {
        Ok(ctx
            .targets
            .iter()
            .map(|id| {
                let (title, app) = if id == "window-1" {
                    ("1Password 主密码", "1Password")
                } else {
                    ("main.rs", "Visual Studio Code")
                };
                let mut target = CaptureTarget::window(id.clone(), title, app);
                target.is_visible = true;
                RawCapture {
                    source_id: "fake:window".to_string(),
                    source_kind: SourceKind::Window,
                    target,
                    captured_at: ctx.now,
                    image: Some(image::RgbImage::from_pixel(
                        32,
                        24,
                        image::Rgb([(ctx.now.as_millis() % 251) as u8, 40, 60]),
                    )),
                    text: None,
                }
            })
            .collect())
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

fn ctx(privacy_toml: &str) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::Inline {
            name: "test".to_string(),
            toml: format!("[capture]\ninterval_secs = 1\ntarget_ids = [\"window-1\", \"window-2\"]\n\n[privacy]\n{privacy_toml}\n"),
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
        Arc::new(TwoAppSource) as Arc<dyn CaptureSource>,
        blobs,
    )));

    Ctx {
        _dir: dir,
        state: Arc::new(state),
    }
}

/// 跑采集环直到至少落了一条观测（或超时）。
async fn run_until_first_observation(ctx: &Ctx) {
    ctx.state.capture.as_ref().unwrap().start();
    // 注入固定信号：否则开发机空闲时调度器会把节奏降到 30 秒一次，测试随环境变红
    let handle = capture_loop::spawn_capture_loop_with_signals(
        Arc::clone(&ctx.state),
        Duration::from_millis(20),
        Arc::new(|| mc_capture::scheduler::CaptureSignals::RUNNING),
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while ctx.state.db.observation_count().unwrap() == 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // 再多跑两轮，让「不该落盘」的目标也有机会被处理
    tokio::time::sleep(Duration::from_millis(120)).await;
    handle.abort();
}

/// 某个应用名下**带图片**的观测条数。
///
/// 注意：被拦截的观测**仍会留下一条审计元数据**（应用名 + 时间，不含内容），
/// 因此「有没有这一行」不是判据 —— 判据是「有没有图片」。
fn stored_images_for(ctx: &Ctx, app: &str) -> usize {
    ctx.state
        .db
        .query_observations(&mc_storage::observations::ObservationQuery {
            exclude_blocked: false,
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .filter(|row| row.app_name.as_deref() == Some(app))
        .filter(|row| row.image_path.is_some())
        .count()
}

/// blob 目录里实际落盘的图片数（含缩略图之外的原图）。
fn blob_files(ctx: &Ctx) -> usize {
    let root = ctx.state.data_dir.join("blobs");
    fn walk(dir: &std::path::Path, count: &mut usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, count);
            } else if path.extension().is_some_and(|ext| ext == "png") {
                *count += 1;
            }
        }
    }
    let mut count = 0;
    walk(&root, &mut count);
    count
}

// ---------------------------------------------------------------- 应用黑名单

#[tokio::test]
async fn blocked_app_frames_are_not_persisted() {
    let ctx = ctx("blocked_apps = [\"1Password\"]\n");
    run_until_first_observation(&ctx).await;

    assert_eq!(
        stored_images_for(&ctx, "1Password"),
        0,
        "黑名单里的应用不得留下任何图片"
    );
    assert!(
        stored_images_for(&ctx, "Visual Studio Code") >= 1,
        "未被拦截的应用必须照常记录图片"
    );
    // 磁盘上也不该有它的图：只有编辑器那一张
    assert_eq!(blob_files(&ctx), 1, "blob 目录里只应有未被拦截那一张");
}

/// 拦截必须可见（否则用户以为黑名单生效了，其实只是没采集）
#[tokio::test]
async fn blocked_frames_are_visible_in_diagnostics() {
    let ctx = ctx("blocked_apps = [\"1Password\"]\n");
    run_until_first_observation(&ctx).await;

    let stats = ctx.state.capture_stats().expect("跑过采集轮就有统计");
    assert!(
        stats.privacy_blocked >= 1,
        "被拦截的帧要计入 privacy_blocked：{stats:?}"
    );
    assert_eq!(stats.dropped, 0, "拦截不是丢弃：{stats:?}");
}

/// 审计只留应用名，不留可能含敏感内容的窗口标题
#[tokio::test]
async fn blocked_observations_never_store_the_window_title() {
    let ctx = ctx("blocked_apps = [\"1Password\"]\n");
    run_until_first_observation(&ctx).await;

    let titles: Vec<Option<String>> = ctx
        .state
        .db
        .query_observations(&mc_storage::observations::ObservationQuery {
            exclude_blocked: false,
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .filter(|row| row.app_name.as_deref() == Some("1Password"))
        .map(|row| row.window_title)
        .collect();

    assert!(
        titles.iter().all(|title| title.is_none()),
        "被拦截的观测不能留下窗口标题：{titles:?}"
    );
}

/// 窗口标题模式同样生效（黑名单不只认应用名）
#[tokio::test]
async fn blocked_window_patterns_are_enforced() {
    let ctx = ctx("blocked_window_patterns = [\"主密码\"]\n");
    run_until_first_observation(&ctx).await;

    assert_eq!(
        stored_images_for(&ctx, "1Password"),
        0,
        "标题命中模式的帧同样不得留下图片"
    );
    assert!(stored_images_for(&ctx, "Visual Studio Code") >= 1);
}

// ---------------------------------------------------------------- 对照与回归

/// 没配黑名单时两个应用都要记录（避免「黑名单把一切都拦了」这种反向 bug）
#[tokio::test]
async fn without_a_blocklist_nothing_is_blocked() {
    let ctx = ctx("");
    run_until_first_observation(&ctx).await;

    assert!(
        stored_images_for(&ctx, "1Password") >= 1
            && stored_images_for(&ctx, "Visual Studio Code") >= 1,
        "没有黑名单时两个应用都该存图"
    );
    let stats = ctx.state.capture_stats().unwrap();
    assert_eq!(stats.privacy_blocked, 0, "{stats:?}");
}

/// 大小写不敏感（用户写 `1password` 也要拦住）
#[tokio::test]
async fn blocklist_is_case_insensitive() {
    let ctx = ctx("blocked_apps = [\"1password\"]\n");
    run_until_first_observation(&ctx).await;
    assert_eq!(
        stored_images_for(&ctx, "1Password"),
        0,
        "应用名匹配必须大小写不敏感"
    );
}

/// 拦截后的诊断接口要能正常响应（回归：隐私字段改动不该影响 HTTP 面）
#[tokio::test]
async fn diagnostics_still_respond_with_a_blocklist() {
    let ctx = ctx("blocked_apps = [\"1Password\"]\n");
    run_until_first_observation(&ctx).await;

    let request = Request::builder()
        .method("GET")
        .uri("/api/diagnostics")
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN)
        .body(Body::empty())
        .unwrap();
    let response = router(Arc::clone(&ctx.state))
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let payload: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        payload["data"]["components"]["capture"]["stats"]["privacy_blocked"]
            .as_u64()
            .is_some(),
        "诊断里要能看到拦截计数：{payload}"
    );
}
