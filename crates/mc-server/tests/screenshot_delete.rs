//! 主动删除单张截图。
//!
//! `deleteScreenshot(filePath)` **只 unlink 文件**，不动数据库 ——
//! 删完之后那条观测还留着 `image_path`，界面继续显示破图。
//! 本实现把两件事做成一个操作，并做两处刻意修正：
//!
//! 1. 删除后清掉数据库引用（磁盘与库一致）；
//! 2. **幂等**：重复删除同一张返回成功。对「文件已不存在」返回
//!    `success:false`，界面上会弹一个没有意义的错误提示。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use image::{Rgb, RgbImage};
use mc_capture::source::CaptureSource;
use mc_common::time::Timestamp;
use mc_server::{router, CaptureControls, ServerState};
use mc_storage::blob::{BlobStore, FileSystemBlobStore, ImageFormat, ImageMeta};
use mc_storage::observations::{ImageRef, NewObservation};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as NOW_MS;

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
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
    blobs: Arc<FileSystemBlobStore>,
}

fn ctx() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let blobs = Arc::new(
        FileSystemBlobStore::new(dir.path().join("blobs"), ImageFormat::Png).expect("blob store"),
    );
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(NOW_MS),
        dir.path().to_path_buf(),
    )
    .with_capture(Arc::new(CaptureControls::new(
        Arc::new(NullSource) as Arc<dyn CaptureSource>,
        Arc::clone(&blobs),
    )));

    Ctx {
        _dir: dir,
        state: Arc::new(state),
        blobs,
    }
}

/// 写一张截图，返回（观测 id，相对路径）。
fn add_screenshot(ctx: &Ctx, index: usize) -> (String, String) {
    // 尺寸必须大于缩略图宽度（320），否则 blob store 不生成缩略图 ——
    // 这个用例要验证的是「只删缩略图」的路径
    let image = RgbImage::from_fn(640, 480, |x, y| {
        Rgb([(x % 251) as u8, (y % 241) as u8, (index * 11) as u8])
    });
    let stored = ctx
        .blobs
        .put_image(
            &image,
            &ImageMeta {
                captured_at: Timestamp::from_millis(NOW_MS),
                display_id: None,
            },
        )
        .expect("写图片");

    let id = format!("obs-{index}");
    ctx.state
        .db
        .insert_observation(&NewObservation {
            id: id.clone(),
            ts: Timestamp::from_millis(NOW_MS),
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

fn delete_request(path: Option<&str>) -> Request<Body> {
    let uri = match path {
        Some(path) => {
            let encoded: String = path
                .chars()
                .map(|ch| match ch {
                    '/' => "%2F".to_string(),
                    'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' => ch.to_string(),
                    other => format!("%{:02X}", other as u32),
                })
                .collect();
            format!("/api/capture/screenshots?path={encoded}")
        }
        None => "/api/capture/screenshots".to_string(),
    };

    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN)
        .body(Body::empty())
        .unwrap()
}

async fn call(state: &Arc<ServerState>, request: Request<Body>) -> serde_json::Value {
    let response = router(Arc::clone(state)).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("必须返回 JSON 信封")
}

fn image_path_of(ctx: &Ctx, id: &str) -> Option<String> {
    ctx.state
        .db
        .query_observations(&mc_storage::observations::ObservationQuery::default())
        .unwrap()
        .into_iter()
        .find(|row| row.id == id)
        .and_then(|row| row.image_path)
}

// ---------------------------------------------------------------- 删除

#[tokio::test]
async fn deleting_removes_the_file_and_clears_the_reference() {
    let ctx = ctx();
    let (id, path) = add_screenshot(&ctx, 0);

    let envelope = call(&ctx.state, delete_request(Some(&path))).await;
    let payload = &envelope["data"];
    assert_eq!(payload["success"], true, "{envelope}");

    assert!(!ctx.blobs.relative_exists(&path), "文件必须被删除");
    assert_eq!(
        image_path_of(&ctx, &id),
        None,
        "数据库引用必须一起清掉（只删文件会让界面显示破图）"
    );

    // 列表接口不再返回这张图
    let request = Request::builder()
        .method("GET")
        .uri("/api/capture/screenshots")
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN)
        .body(Body::empty())
        .unwrap();
    let envelope = call(&ctx.state, request).await;
    assert!(
        envelope["data"].as_array().unwrap().is_empty(),
        "删除后不该再出现在截图列表里：{envelope}"
    );
}

#[tokio::test]
async fn deleting_a_thumbnail_only_keeps_the_screenshot() {
    let ctx = ctx();
    let (id, path) = add_screenshot(&ctx, 0);
    let thumbnail = ctx
        .state
        .db
        .query_observations(&mc_storage::observations::ObservationQuery::default())
        .unwrap()
        .into_iter()
        .find(|row| row.id == id)
        .and_then(|row| row.thumbnail_path)
        .expect("应当有缩略图");

    let envelope = call(&ctx.state, delete_request(Some(&thumbnail))).await;
    assert_eq!(envelope["data"]["success"], true);

    assert!(!ctx.blobs.relative_exists(&thumbnail));
    assert!(ctx.blobs.relative_exists(&path), "原图不该被牵连删除");
    assert_eq!(
        image_path_of(&ctx, &id).as_deref(),
        Some(path.as_str()),
        "原图引用必须保留"
    );
}

/// 幂等：重复删除返回成功（对 ENOENT 返回 success:false 会让界面弹无意义错误）
#[tokio::test]
async fn deleting_twice_is_still_success() {
    let ctx = ctx();
    let (_id, path) = add_screenshot(&ctx, 0);

    let first = call(&ctx.state, delete_request(Some(&path))).await;
    assert_eq!(first["data"]["success"], true);

    let second = call(&ctx.state, delete_request(Some(&path))).await;
    assert_eq!(
        second["data"]["success"], true,
        "重复删除必须是成功：{second}"
    );
    assert!(second["data"]["error"].is_null(), "{second}");
}

#[tokio::test]
async fn deleting_one_screenshot_leaves_the_others_alone() {
    let ctx = ctx();
    let (keep_id, keep_path) = add_screenshot(&ctx, 0);
    let (_drop_id, drop_path) = add_screenshot(&ctx, 1);

    call(&ctx.state, delete_request(Some(&drop_path))).await;

    assert!(ctx.blobs.relative_exists(&keep_path));
    assert!(image_path_of(&ctx, &keep_id).is_some());
}

// ---------------------------------------------------------------- 校验

/// 路径穿越必须失败即关闭：token 只证明「是本应用」，不证明参数可信。
#[tokio::test]
async fn traversal_paths_are_rejected() {
    let ctx = ctx();
    let secret = ctx.blobs.root().parent().unwrap().join("secret.txt");
    std::fs::write(&secret, b"top secret").unwrap();

    let envelope = call(&ctx.state, delete_request(Some("../secret.txt"))).await;
    assert_eq!(envelope["code"], 1, "必须拒绝：{envelope}");
    assert!(secret.exists(), "目录外的文件绝不能被动到");

    let envelope = call(&ctx.state, delete_request(Some("/etc/passwd"))).await;
    assert_eq!(envelope["code"], 1, "{envelope}");
}

#[tokio::test]
async fn missing_path_is_reported() {
    let ctx = ctx();
    let envelope = call(&ctx.state, delete_request(None)).await;
    assert_eq!(envelope["code"], 1, "缺少 path 必须报错：{envelope}");
}

/// 未挂载 blob 存储的只读实例：结构化错误，不是 500
#[tokio::test]
async fn without_capture_controls_it_fails_structurally() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(NOW_MS),
        dir.path().to_path_buf(),
    ));

    let envelope = call(&state, delete_request(Some("screenshots/2026/09/30/x.png"))).await;
    assert_eq!(envelope["code"], 1);
    assert_eq!(envelope["error_code"], "storage_unavailable");
}

#[tokio::test]
async fn deleting_also_drops_the_path_from_the_activity_it_belongs_to() {
    let ctx = ctx();
    let (_id, path) = add_screenshot(&ctx, 0);

    // 活动行冗余存了一份截图列表（遗留 `activity` 表），删除必须两边一致
    let keep = "20260930/keep.png";
    ctx.state
        .db
        .with_write(|conn| {
            let resources =
                format!(r#"[{{"type":"image","id":"r1","path":"{path}"}},{{"type":"image","id":"r2","path":"{keep}"}}]"#);
            conn.execute(
                &format!(
                    "INSERT INTO activity (id, title, content, resources, metadata, start_time, end_time)
                     VALUES (1, '未知活动', '', '{resources}', NULL, {NOW_MS}, {})",
                    NOW_MS + 60_000
                ),
                [],
            )?;
            Ok(())
        })
        .unwrap();

    let envelope = call(&ctx.state, delete_request(Some(&path))).await;
    assert_eq!(envelope["data"]["success"], true, "{envelope}");

    let resources: String = ctx
        .state
        .db
        .with_read(|conn| {
            conn.query_row("SELECT resources FROM activity WHERE id = 1", [], |row| {
                row.get(0)
            })
        })
        .unwrap();
    assert!(
        !resources.contains(&path),
        "被删掉的路径不该再出现在活动里：{resources}"
    );
    assert!(
        resources.contains(keep),
        "同一条活动的其它截图要保留：{resources}"
    );
}
