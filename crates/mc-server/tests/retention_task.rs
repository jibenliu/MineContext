//! 轮转删除接进 daemon，并在诊断里可见。
//!
//! 三件事必须同时成立，缺一个这项要求就没真正满足：
//!
//! 1. 轮转**真的在跑**（`enforce_retention` 在此之前从未被生产代码调用）；
//! 2. 用户能看到它跑了什么（删了多少、释放多少字节）；
//! 3. 策略来自配置（`capture.retention_days`），而不是写死的数字。

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use image::{Rgb, RgbImage};
use mc_capture::source::CaptureSource;
use mc_common::time::Timestamp;
use mc_server::{retention, router, CaptureControls, ServerState};
use mc_storage::blob::{BlobStore, FileSystemBlobStore, ImageFormat, ImageMeta};
use mc_storage::observations::{ImageRef, NewObservation};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";
const DAY_MS: i64 = 86_400_000;
/// 固定"现在"：2026-09-30T09:00:00Z
/// 基准取**真实当前时间**：保留策略按真实时钟判断，写死的时间戳会随时间推移
/// 变成"过期数据"，让这个测试在若干天后必然失败（本项目已经发生过）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

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

struct Ctx {
    /// 只为持有临时目录（drop 时删除）；测试不直接读它
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
    blobs: Arc<FileSystemBlobStore>,
}

/// `retention_days` 与是否挂载采集都可调：两个开关都会影响行为。
fn ctx(retention_days: u32, mount_capture: bool) -> Ctx {
    ctx_with(
        &format!("[capture]\nretention_days = {retention_days}\n"),
        mount_capture,
    )
}

fn ctx_with(toml: &str, mount_capture: bool) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let blobs = Arc::new(
        FileSystemBlobStore::new(dir.path().join("blobs"), ImageFormat::Png).expect("blob store"),
    );

    let config = mc_config::load::load(&mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::Inline {
            name: "test".to_string(),
            toml: toml.to_string(),
        }],
        env: Vec::new(),
        read_process_env: false,
    })
    .unwrap();

    let mut state = ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(now_ms()),
        dir.path().to_path_buf(),
    );

    if mount_capture {
        state = state.with_capture(Arc::new(CaptureControls::new(
            Arc::new(NullSource) as Arc<dyn CaptureSource>,
            Arc::clone(&blobs),
        )));
    }

    Ctx {
        _dir: dir,
        state: Arc::new(state),
        blobs,
    }
}

fn add_screenshot(ctx: &Ctx, index: usize, captured_at_ms: i64) -> (String, String) {
    let image = RgbImage::from_fn(24, 16, |x, y| {
        Rgb([(x * 5) as u8, (y * 7) as u8, (index * 11) as u8])
    });
    let stored = ctx
        .blobs
        .put_image(
            &image,
            &ImageMeta {
                captured_at: Timestamp::from_millis(captured_at_ms),
                display_id: Some("display-1".to_string()),
            },
        )
        .expect("写图片");

    let id = format!("obs-{index}");
    ctx.state
        .db
        .insert_observation(&NewObservation {
            id: id.clone(),
            ts: Timestamp::from_millis(captured_at_ms),
            source_id: "macos:screen".to_string(),
            kind: "screen".to_string(),
            app_name: None,
            app_bundle_id: None,
            window_title: None,
            domain: None,
            display_id: None,
            scale_factor: None,
            image: Some(ImageRef {
                relative_path: stored.relative_path.clone(),
                content_hash: stored.content_hash.clone(),
                thumbnail_path: stored.thumbnail.as_ref().map(|t| t.relative_path.clone()),
                width: stored.width,
                height: stored.height,
                bytes: stored.bytes,
            }),
            text_content: None,
            text_origin: None,
            change_kind: "new".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: format!("key-{index}"),
        })
        .expect("写观测");

    (id, stored.relative_path)
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

// ---------------------------------------------------------------- 执行

#[tokio::test]
async fn run_once_rotates_and_reports() {
    let ctx = ctx(7, true);
    let (_old_id, old_path) = add_screenshot(&ctx, 0, now_ms() - 30 * DAY_MS);
    let (_new_id, new_path) = add_screenshot(&ctx, 1, now_ms() - DAY_MS);

    let outcome = retention::run_once(&ctx.state)
        .expect("执行不该失败")
        .expect("挂了采集就必须真的执行");

    assert!(outcome.deleted_files >= 1, "{outcome:?}");
    assert!(outcome.freed_bytes > 0, "{outcome:?}");
    assert!(outcome.cleared_references >= 1, "{outcome:?}");
    assert!(!ctx.blobs.relative_exists(&old_path));
    assert!(ctx.blobs.relative_exists(&new_path));
}

/// 策略来自配置：`retention_days = 0` 且容量不限制时，不能按天删东西。
#[tokio::test]
async fn policy_comes_from_config() {
    let ctx = ctx_with(
        "[capture]\nretention_days = 0\n[storage]\nmax_total_gb = 0\nmax_screenshot_count = 0\n",
        true,
    );
    let (_id, path) = add_screenshot(&ctx, 0, now_ms() - 400 * DAY_MS);

    let outcome = retention::run_once(&ctx.state).unwrap().unwrap();
    assert_eq!(outcome.deleted_files, 0, "永久保留模式不能删：{outcome:?}");
    assert!(ctx.blobs.relative_exists(&path));
}

/// 容量上限来自 `storage.max_total_gb`，超限从最旧截图开始删。
#[tokio::test]
async fn capacity_cap_comes_from_storage_config() {
    // ~400 B：三张小 PNG + 缩略图必然超过；不能用 1 KiB，小图合计往往不到。
    let ctx = ctx_with(
        "[capture]\nretention_days = 0\n[storage]\nmax_total_gb = 0.0000004\nmax_screenshot_count = 0\n",
        true,
    );
    let limit = retention::max_total_bytes_from_gb(0.0000004);
    let (_a, old_path) = add_screenshot(&ctx, 0, now_ms() - 3 * DAY_MS);
    let (_b, _mid_path) = add_screenshot(&ctx, 1, now_ms() - 2 * DAY_MS);
    let (_c, new_path) = add_screenshot(&ctx, 2, now_ms() - DAY_MS);

    let before = ctx.blobs.stats().unwrap().total_bytes;
    assert!(
        before > limit,
        "fixture 必须先超过上限：before={before} limit={limit}"
    );

    let outcome = retention::run_once(&ctx.state).unwrap().unwrap();
    assert!(outcome.deleted_files >= 1, "{outcome:?}");
    let after = ctx.blobs.stats().unwrap().total_bytes;
    assert!(
        after <= limit,
        "轮转后必须压到上限内：before={before} after={after} limit={limit}"
    );
    assert!(
        !ctx.blobs.relative_exists(&old_path),
        "应优先删最旧图：old={old_path} new={new_path}"
    );
    assert!(ctx.blobs.relative_exists(&new_path), "最新图应尽量保留");
}

/// 轮转只动截图 blob，不能删用户 vault 笔记。
#[tokio::test]
async fn retention_does_not_delete_vault_notes() {
    let ctx = ctx_with(
        "[capture]\nretention_days = 0\n[storage]\nmax_total_gb = 0.0000004\nmax_screenshot_count = 0\n",
        true,
    );
    let vault_id = ctx
        .state
        .db
        .insert_vault_row(
            &mc_storage::vaults::VaultUpsert {
                title: "keep-me".into(),
                summary: String::new(),
                content: "user note body".into(),
                tags: vec!["note".into()],
                parent_id: None,
                is_folder: false,
                document_type: "vaults".into(),
                sort_order: 0,
            },
            Timestamp::from_millis(now_ms()),
        )
        .unwrap();
    add_screenshot(&ctx, 0, now_ms() - DAY_MS);
    add_screenshot(&ctx, 1, now_ms() - 2 * DAY_MS);

    let _ = retention::run_once(&ctx.state).unwrap().unwrap();

    let row = ctx
        .state
        .db
        .vault_row_by_id(vault_id)
        .unwrap()
        .expect("vault 笔记必须仍在");
    assert_eq!(row.title, "keep-me");
    assert!(row.content.contains("user note body"));
}

/// 只读实例（没有挂载 blob 存储）不该报错，也不该假装执行过。
#[tokio::test]
async fn without_blobs_it_is_a_no_op() {
    let ctx = ctx(7, false);
    assert!(retention::run_once(&ctx.state).unwrap().is_none());
}

// ---------------------------------------------------------------- 诊断可见

#[tokio::test]
async fn diagnostics_exposes_the_last_run() {
    let ctx = ctx(7, true);
    add_screenshot(&ctx, 0, now_ms() - 30 * DAY_MS);

    // 未跑过时字段存在但为空（前端可以据此显示「尚未执行」）
    let payload = diagnostics(&ctx.state).await;
    assert!(payload["data"]["retention"].is_object(), "{payload}");
    assert!(
        payload["data"]["retention"]["last_run_at"].is_null(),
        "{payload}"
    );

    retention::run_once(&ctx.state).unwrap().unwrap();

    let payload = diagnostics(&ctx.state).await;
    let section = &payload["data"]["retention"];
    assert!(section["last_run_at"].is_string(), "{section}");
    assert!(
        section["deleted_files"].as_u64().unwrap_or(0) >= 1,
        "{section}"
    );
    assert!(
        section["freed_bytes"].as_u64().unwrap_or(0) > 0,
        "{section}"
    );
    assert!(
        section["cleared_references"].as_u64().unwrap_or(0) >= 1,
        "{section}"
    );
}

// ---------------------------------------------------------------- 定时任务

#[tokio::test]
async fn the_task_runs_on_its_interval() {
    let ctx = ctx(7, true);
    let (_id, path) = add_screenshot(&ctx, 0, now_ms() - 30 * DAY_MS);

    let handle = retention::spawn_retention_task(Arc::clone(&ctx.state), Duration::from_millis(20));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while ctx.blobs.relative_exists(&path) && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    handle.abort();

    assert!(
        !ctx.blobs.relative_exists(&path),
        "定时任务必须真的删掉旧图"
    );

    let payload = diagnostics(&ctx.state).await;
    assert!(
        payload["data"]["retention"]["last_run_at"].is_string(),
        "跑过之后诊断里必须留下记录：{payload}"
    );
}

/// 磁盘上已经没有任何东西要清时，任务不该刷诊断表也不该报错
#[tokio::test]
async fn the_task_is_quiet_when_there_is_nothing_to_do() {
    let ctx = ctx(7, true);
    let handle = retention::spawn_retention_task(Arc::clone(&ctx.state), Duration::from_millis(10));
    tokio::time::sleep(Duration::from_millis(80)).await;
    handle.abort();

    let failures: i64 = ctx
        .state
        .db
        .with_read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM pipeline_failures WHERE component = 'retention'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(failures, 0, "无事可做不该产生失败记录");
}

#[tokio::test]
async fn capture_config_exposes_retention_and_disk_usage() {
    let ctx = ctx(7, true);
    add_screenshot(&ctx, 0, now_ms() - DAY_MS);

    let request = Request::builder()
        .method("GET")
        .uri("/api/capture/config")
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
    let data = &payload["data"];
    assert_eq!(data["retention_days"], 7);
    assert_eq!(data["max_total_gb"], 10.0);
    assert!(
        data["disk_usage"]["total_bytes"].as_u64().unwrap_or(0) > 0,
        "{data}"
    );
    assert!(
        data["disk_usage"]["blob_count"].as_u64().unwrap_or(0) >= 1,
        "{data}"
    );
}
