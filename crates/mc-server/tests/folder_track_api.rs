//! 目录导入与文件跟踪：Obsidian / Memory Bank 本地目录 → 笔记树；跟踪同步幂等。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_search::{DocumentKind, SearchFilters};
use mc_server::retrieval::retrieve_filtered;
use mc_server::{router, ServerState};
use mc_storage::Database;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
use tower::ServiceExt;

const TOKEN: &str = "test-token";

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

async fn call(
    state: &Arc<ServerState>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN);
    let request = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            builder
                .body(Body::from(serde_json::to_vec(&value).unwrap()))
                .unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router(Arc::clone(state)).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).expect("必须返回 JSON 信封");
    (status, json)
}

#[tokio::test]
async fn import_folder_loads_obsidian_style_markdown_into_search() {
    let ctx = ctx();
    let vault = ctx.dir.path().join("obsidian-vault");
    std::fs::create_dir_all(vault.join("daily")).unwrap();
    std::fs::write(
        vault.join("daily/2026-10-10.md"),
        b"# Daily\n\nObsidian daily note about context sources.\n",
    )
    .unwrap();
    std::fs::write(vault.join("skip.xyz"), b"unknown").unwrap();

    let (status, envelope) = call(
        &ctx.state,
        "POST",
        "/api/v1/files/import-folder",
        Some(serde_json::json!({
            "path": vault.to_string_lossy(),
            "recursive": true
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let imported = envelope["data"]["imported"].as_array().unwrap();
    assert_eq!(imported.len(), 1);
    let id = imported[0]["id"].as_i64().unwrap();
    let hits = retrieve_filtered(
        &ctx.state.db,
        "Obsidian daily note",
        10,
        SearchFilters {
            kinds: vec![DocumentKind::Document],
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert!(hits
        .iter()
        .any(|hit| hit.document.id == format!("note-{id}")));
}

#[tokio::test]
async fn track_sync_imports_new_files_once() {
    let ctx = ctx();
    let watched = ctx.dir.path().join("memory-bank");
    std::fs::create_dir_all(&watched).unwrap();
    std::fs::write(
        watched.join("project.md"),
        b"# Memory\n\nMemory bank project facts for MineContext.\n",
    )
    .unwrap();

    let (status, envelope) = call(
        &ctx.state,
        "POST",
        "/api/v1/files/track",
        Some(serde_json::json!({ "path": watched.to_string_lossy() })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");

    let (status, envelope) = call(
        &ctx.state,
        "POST",
        "/api/v1/files/track/sync",
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(envelope["data"]["imported"].as_array().unwrap().len(), 1);

    let (status, envelope) = call(
        &ctx.state,
        "POST",
        "/api/v1/files/track/sync",
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert!(
        envelope["data"]["imported"].as_array().unwrap().is_empty(),
        "第二次同步不得重复导入：{envelope}"
    );

    let (status, envelope) = call(&ctx.state, "GET", "/api/v1/files/track", None).await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let folders = envelope["data"]["folders"].as_array().unwrap();
    assert_eq!(folders.len(), 1);
}
