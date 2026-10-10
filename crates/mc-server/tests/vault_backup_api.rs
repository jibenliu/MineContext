//! `GET /api/v1/vault/export` / `POST /api/v1/vault/import` —— 笔记树 + uploads 备份。
//!
//! 验收：用户能拿到一份**自描述**的 zip（manifest + vaults.json + uploads/），
//! 重装后可选 import 恢复笔记与已摄入文件，而不是只搬走整个 Application Support。

use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::vaults::{VaultUpsert, DOCUMENT_TYPE_DAILY_REPORT, DOCUMENT_TYPE_VAULTS};
use mc_storage::Database;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
use tower::ServiceExt;
use zip::ZipArchive;

const TOKEN: &str = "vault-backup-token-0123456789abcdef";

struct Ctx {
    dir: tempfile::TempDir,
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
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    ));
    Ctx { dir, state }
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

async fn export_bytes(state: &Arc<ServerState>) -> (StatusCode, Vec<u8>, Option<String>) {
    let response = router(Arc::clone(state))
        .oneshot(request("GET", "/api/v1/vault/export", None))
        .await
        .unwrap();
    let status = response.status();
    let disposition = response
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, bytes.to_vec(), disposition)
}

fn seed_vault_and_upload(ctx: &Ctx) -> (i64, i64) {
    let at = Timestamp::from_millis(T0);
    let folder_id = ctx
        .state
        .db
        .insert_vault_row(
            &VaultUpsert {
                title: "工作".to_string(),
                summary: String::new(),
                content: String::new(),
                tags: vec![],
                parent_id: None,
                is_folder: true,
                document_type: DOCUMENT_TYPE_VAULTS.to_string(),
                sort_order: 0,
            },
            at,
        )
        .expect("建文件夹");
    let note_id = ctx
        .state
        .db
        .insert_vault_row(
            &VaultUpsert {
                title: "Sprint notes".to_string(),
                summary: "摘要".to_string(),
                content: format!(
                    "来源笔记\n\n![图](file://{}/uploads/note-image-1.png)\n\n正文 ownership",
                    ctx.dir.path().display()
                ),
                tags: vec!["link".to_string(), "work".to_string()],
                parent_id: Some(folder_id),
                is_folder: false,
                document_type: DOCUMENT_TYPE_VAULTS.to_string(),
                sort_order: 1,
            },
            at,
        )
        .expect("建笔记");
    let _daily = ctx
        .state
        .db
        .insert_vault_row(
            &VaultUpsert {
                title: "2026-10-10".to_string(),
                summary: String::new(),
                content: "日报正文".to_string(),
                tags: vec![],
                parent_id: None,
                is_folder: false,
                document_type: DOCUMENT_TYPE_DAILY_REPORT.to_string(),
                sort_order: 0,
            },
            at,
        )
        .expect("建日报");

    let uploads = ctx.dir.path().join("uploads");
    std::fs::create_dir_all(&uploads).unwrap();
    std::fs::write(uploads.join("note-image-1.png"), b"PNGDATA").unwrap();
    std::fs::write(uploads.join("sprint-notes.md"), b"# Sprint\nownership\n").unwrap();
    (folder_id, note_id)
}

fn read_zip_entry(archive: &mut ZipArchive<Cursor<Vec<u8>>>, name: &str) -> Vec<u8> {
    let mut file = archive.by_name(name).unwrap_or_else(|error| {
        panic!("zip 缺少 {name}: {error}");
    });
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    bytes
}

#[tokio::test]
async fn export_requires_the_token() {
    let ctx = ctx();
    let response = router(Arc::clone(&ctx.state))
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/vault/export")
                .header("host", "127.0.0.1:12345")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn export_zip_contains_manifest_vaults_and_uploads() {
    let ctx = ctx();
    let (folder_id, note_id) = seed_vault_and_upload(&ctx);

    let (status, bytes, disposition) = export_bytes(&ctx.state).await;
    assert_eq!(status, StatusCode::OK, "导出必须成功");
    let disposition = disposition.expect("必须带 Content-Disposition");
    assert!(
        disposition.contains("attachment") && disposition.contains(".zip"),
        "Disposition 要指向可下载 zip：{disposition}"
    );

    let mut archive = ZipArchive::new(Cursor::new(bytes)).expect("响应体必须是 zip");
    let manifest: serde_json::Value =
        serde_json::from_slice(&read_zip_entry(&mut archive, "manifest.json")).unwrap();
    assert_eq!(manifest["format"], "minecontext-vault-backup");
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["vault_count"], 3);
    assert_eq!(manifest["file_count"], 2);
    assert!(manifest["exported_at_ms"].is_i64());

    let vaults: Vec<serde_json::Value> =
        serde_json::from_slice(&read_zip_entry(&mut archive, "vaults.json")).unwrap();
    assert_eq!(vaults.len(), 3);
    assert!(
        vaults.iter().any(|row| row["id"] == folder_id && row["is_folder"] == 1),
        "文件夹必须进备份：{vaults:?}"
    );
    assert!(
        vaults
            .iter()
            .any(|row| row["id"] == note_id && row["title"] == "Sprint notes"),
        "笔记正文必须进备份：{vaults:?}"
    );
    assert!(
        vaults
            .iter()
            .any(|row| row["document_type"] == DOCUMENT_TYPE_DAILY_REPORT),
        "日报也要进备份：{vaults:?}"
    );

    assert_eq!(
        read_zip_entry(&mut archive, "uploads/note-image-1.png"),
        b"PNGDATA"
    );
    assert_eq!(
        read_zip_entry(&mut archive, "uploads/sprint-notes.md"),
        b"# Sprint\nownership\n"
    );
}

#[tokio::test]
async fn export_skips_soft_deleted_vault_rows() {
    let ctx = ctx();
    let at = Timestamp::from_millis(T0);
    let alive = ctx
        .state
        .db
        .insert_vault_row(
            &VaultUpsert {
                title: "保留".to_string(),
                summary: String::new(),
                content: "alive".to_string(),
                tags: vec![],
                parent_id: None,
                is_folder: false,
                document_type: DOCUMENT_TYPE_VAULTS.to_string(),
                sort_order: 0,
            },
            at,
        )
        .unwrap();
    let gone = ctx
        .state
        .db
        .insert_vault_row(
            &VaultUpsert {
                title: "回收站".to_string(),
                summary: String::new(),
                content: "gone".to_string(),
                tags: vec![],
                parent_id: None,
                is_folder: false,
                document_type: DOCUMENT_TYPE_VAULTS.to_string(),
                sort_order: 0,
            },
            at,
        )
        .unwrap();
    ctx.state.db.soft_delete_vault_row(gone, at).unwrap();

    let (status, bytes, _) = export_bytes(&ctx.state).await;
    assert_eq!(status, StatusCode::OK);
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let vaults: Vec<serde_json::Value> =
        serde_json::from_slice(&read_zip_entry(&mut archive, "vaults.json")).unwrap();
    assert_eq!(vaults.len(), 1);
    assert_eq!(vaults[0]["id"], alive);
    assert_eq!(vaults[0]["title"], "保留");
}

#[tokio::test]
async fn import_restores_vault_tree_and_uploads_with_rewritten_file_urls() {
    let source = ctx();
    seed_vault_and_upload(&source);
    let (status, zip_bytes, _) = export_bytes(&source.state).await;
    assert_eq!(status, StatusCode::OK);

    let target = ctx();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&zip_bytes);
    let response = router(Arc::clone(&target.state))
        .oneshot(request(
            "POST",
            "/api/v1/vault/import",
            Some(serde_json::json!({ "data": encoded })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let envelope: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(envelope["code"], 0, "{envelope}");
    assert_eq!(envelope["data"]["vault_count"], 3);
    assert_eq!(envelope["data"]["file_count"], 2);

    let rows = target
        .state
        .db
        .query_vault_rows(&mc_storage::vaults::VaultQuery::default())
        .unwrap();
    assert_eq!(rows.len(), 3);
    let note = rows
        .iter()
        .find(|row| row.title == "Sprint notes")
        .expect("笔记应被恢复");
    let folder = rows
        .iter()
        .find(|row| row.title == "工作" && row.is_folder == 1)
        .expect("文件夹应被恢复");
    assert_eq!(note.parent_id, Some(folder.id));
    assert!(
        note.content.contains(&format!(
            "file://{}/uploads/note-image-1.png",
            target.dir.path().display()
        )),
        "file:// 必须改写到新 data_dir：{}",
        note.content
    );
    assert!(
        !note.content.contains(&source.dir.path().display().to_string()),
        "不得残留源机器绝对路径：{}",
        note.content
    );

    let uploads = target.dir.path().join("uploads");
    assert_eq!(
        std::fs::read(uploads.join("note-image-1.png")).unwrap(),
        b"PNGDATA"
    );
    assert_eq!(
        std::fs::read(uploads.join("sprint-notes.md")).unwrap(),
        b"# Sprint\nownership\n"
    );
}

#[tokio::test]
async fn import_rejects_unknown_archive_format() {
    let ctx = ctx();
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("manifest.json", options).unwrap();
        zip.write_all(br#"{"format":"other","schema_version":1}"#)
            .unwrap();
        zip.finish().unwrap();
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(cursor.get_ref());
    let response = router(Arc::clone(&ctx.state))
        .oneshot(request(
            "POST",
            "/api/v1/vault/import",
            Some(serde_json::json!({ "data": encoded })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let envelope: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_ne!(envelope["code"], 0);
}
