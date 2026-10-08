//! 诊断包导出（`GET /api/v1/diagnostics/export`，安全属性清单第 7 条）。
//!
//! 为什么需要一个**专门的**导出接口而不是「把 `/api/diagnostics` 存成文件」：用户会把诊断包
//! 发给别人（贴 issue、发邮件），因此它必须能**当面证明**自己不含内容 —— 而 `/api/diagnostics`
//! 是给自己看的，随时可能长出新字段。这组测试钉住三件事：
//! 1. 和别的 v1 接口一样要 token；
//! 2. 形状稳定且**自我说明**（`schema_version` + 明确列出「刻意不放什么」）；
//! 3. 观测标题、活动标题、文件路径、token、API Key 一个都不能出现。
//!
//! 响应走**统一信封**（`{code, data}`）：裸对象会让「有没有 code」变成接口之间的差异，诊断包本体在 `data` 里。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::Database;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
use tower::ServiceExt;

const TOKEN: &str = "export-token-0123456789abcdef";
const API_KEY: &str = "sk-live-EXPORT0123456789";
const WINDOW_TITLE: &str = "季度财报 - 机密.xlsx";
const ACTIVITY_TITLE: &str = "整理季度财报";

fn state() -> (tempfile::TempDir, Arc<ServerState>) {
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

fn request(token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("GET")
        .uri("/api/v1/diagnostics/export")
        .header("host", "127.0.0.1:12345");
    if let Some(token) = token {
        builder = builder.header("x-mc-token", token);
    }
    builder.body(Body::empty()).unwrap()
}

async fn export(state: &Arc<ServerState>, token: Option<&str>) -> (StatusCode, String) {
    let response = router(Arc::clone(state))
        .oneshot(request(token))
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// 造一条「有内容」的数据：观测（带窗口标题 + 图片引用）与一条事件。
fn seed_content(state: &Arc<ServerState>, data_dir: &std::path::Path) {
    let blobs = mc_storage::blob::FileSystemBlobStore::new(
        data_dir.join("blobs"),
        mc_storage::blob::ImageFormat::Png,
    )
    .unwrap();
    let blob = mc_storage::blob::BlobStore::put_image(
        &blobs,
        &image::RgbImage::from_pixel(16, 12, image::Rgb([9, 9, 9])),
        &mc_storage::blob::ImageMeta {
            captured_at: Timestamp::from_millis(T0),
            display_id: None,
        },
    )
    .unwrap();

    state
        .db
        .insert_observation(&mc_storage::observations::NewObservation {
            id: "obs-export".to_string(),
            ts: Timestamp::from_millis(T0),
            source_id: "macos:window".to_string(),
            kind: "window".to_string(),
            app_name: Some("Excel".to_string()),
            app_bundle_id: None,
            window_title: Some(WINDOW_TITLE.to_string()),
            domain: None,
            display_id: None,
            scale_factor: Some(2.0),
            image: Some(mc_storage::observations::ImageRef {
                relative_path: blob.relative_path.clone(),
                content_hash: blob.content_hash.clone(),
                thumbnail_path: None,
                width: blob.width,
                height: blob.height,
                bytes: blob.bytes,
            }),
            text_content: Some("单元格 A1：营收 1234 万".to_string()),
            text_origin: Some("accessibility".to_string()),
            change_kind: "new".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: Some(7),
            idempotency: "obs-export".to_string(),
        })
        .expect("插入观测");

    state
        .db
        .append_events(&[mc_storage::NewEvent::new(
            "activity.created",
            Timestamp::from_millis(T0),
            serde_json::json!({ "title": ACTIVITY_TITLE, "category": "工作" }),
        )])
        .expect("插入事件");
}

#[tokio::test]
async fn export_requires_the_token() {
    let (_dir, state) = state();

    let (status, _) = export(&state, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "无 token 不得导出诊断包");

    let (status, _) = export(&state, Some("wrong")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn export_is_self_describing() {
    let (_dir, state) = state();

    let (status, body) = export(&state, Some(TOKEN)).await;
    assert_eq!(status, StatusCode::OK);

    let envelope: serde_json::Value = serde_json::from_str(&body).expect("导出必须是 JSON");
    assert_eq!(envelope["code"], 0, "{envelope}");
    let json = &envelope["data"];
    assert!(
        json["schema_version"].is_string(),
        "没有 schema_version 的报告，半年后没人知道怎么读：{json}"
    );
    assert!(json["generated_at"].is_number(), "{json}");
    assert!(json["platform"].is_object());
    assert!(json["components"].is_object());
    assert!(json["counts"].is_object());
    // 待推断积压：用户报「一个 job 跑几小时」时，诊断包里要能一眼看到积压量
    assert!(
        json["counts"]["pending_inference"].is_i64(),
        "诊断包缺少 pending_inference：{json}"
    );
    assert!(json["invariants"].is_object());

    // 自我说明：明确列出「刻意不放什么」，而不是让用户猜
    let excluded = json["excluded"].as_array().expect("excluded 必须是数组");
    let text = excluded
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    for expected in ["标题", "路径", "密钥", "内容"] {
        assert!(
            text.contains(expected),
            "excluded 要写明不含「{expected}」：{text}"
        );
    }
}

#[tokio::test]
async fn export_never_contains_content_or_secrets() {
    let (dir, state) = state();
    seed_content(&state, dir.path());

    let (_, body) = export(&state, Some(TOKEN)).await;

    for (what, needle) in [
        ("窗口标题", WINDOW_TITLE),
        ("活动标题", ACTIVITY_TITLE),
        ("无障碍文本", "营收 1234 万"),
        ("应用名（window 观测的审计字段）", "Excel"),
        ("数据目录绝对路径", &dir.path().display().to_string()),
        ("token", TOKEN),
        ("API Key", API_KEY),
    ] {
        assert!(!body.contains(needle), "诊断包里出现了{what}：{needle}");
    }
}

#[tokio::test]
async fn export_counts_are_real_numbers() {
    let (dir, state) = state();
    seed_content(&state, dir.path());

    let (_, body) = export(&state, Some(TOKEN)).await;
    let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();
    let json = &envelope["data"];

    assert_eq!(
        json["counts"]["observations"], 1,
        "计数要真的查库，而不是写死 0：{json}"
    );
    // 2 = 落观测时同事务写的那条事件 + 显式写的 activity.created
    assert_eq!(
        json["counts"]["events"], 2,
        "事件计数要把两条都算上：{json}"
    );
    assert!(json["counts"]["stages"].is_number());
    assert!(json["counts"]["summaries"].is_number());
}

#[tokio::test]
async fn failures_are_reported_by_code_without_free_form_message() {
    let (_dir, state) = state();

    // 失败详情里带一个绝对路径：它落在 `pipeline_failures.context` 列里，
    // 而诊断包只允许带稳定错误码与人话建议
    let error = mc_common::error::AppError::new(
        mc_common::error::ErrorCode::CapturePermissionDenied,
        "无法读取 /Users/someone/secret/path",
    );
    state
        .db
        .record_failure(Timestamp::from_millis(T0), "capture", &error, "error")
        .expect("写入失败记录");

    let (_, body) = export(&state, Some(TOKEN)).await;
    let envelope: serde_json::Value = serde_json::from_str(&body).unwrap();
    let json = &envelope["data"];

    let failures = json["recent_failures"].as_array().expect("失败列表");
    assert_eq!(failures.len(), 1, "{json}");
    assert_eq!(failures[0]["error_code"], "capture_permission_denied");
    assert_eq!(failures[0]["component"], "capture");
    assert!(
        failures[0]["remediation"].is_string(),
        "remediation 是给用户看的，要留着"
    );
    assert!(
        failures[0]["message"].is_string(),
        "用户文案（不含路径）可以留"
    );
    assert!(
        !body.contains("/Users/someone/secret/path"),
        "失败详情里的路径不能进诊断包：{body}"
    );
}
