//! `POST /api/v1/rss` —— 用户提交公开 RSS/Atom，条目落成可检索笔记。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_providers::transport::HttpTransport;
use mc_search::{DocumentKind, SearchFilters};
use mc_server::retrieval::retrieve_filtered;
use mc_server::{router, ServerState};
use mc_storage::Database;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
use mc_testkit::provider::ScriptedTransport;
use tower::ServiceExt;

const TOKEN: &str = "test-token";

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
}

fn ctx(transport: Arc<ScriptedTransport>) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(
        ServerState::new(
            mc_config::ConfigHandle::new(config),
            db,
            TOKEN.to_string(),
            Timestamp::from_millis(T0),
            dir.path().to_path_buf(),
        )
        .with_link_transport(Arc::clone(&transport) as Arc<dyn HttpTransport>),
    );
    Ctx { _dir: dir, state }
}

async fn call(
    state: &Arc<ServerState>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/rss")
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
async fn importing_rss_feed_creates_searchable_notes_for_items() {
    let xml = r#"<?xml version="1.0"?>
<rss version="2.0"><channel>
<title>Context Digest</title>
<item>
  <title>Ownership Primer</title>
  <link>https://example.com/ownership</link>
  <description>Borrow checker keeps memory safe.</description>
</item>
<item>
  <title>RSS Into Vault</title>
  <link>https://example.com/rss-vault</link>
  <description>Feed entries become local notes.</description>
</item>
</channel></rss>"#;
    let transport = Arc::new(ScriptedTransport::new().push_json(200, xml));
    let ctx = ctx(transport);

    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({
            "url": "https://example.com/feed.xml",
            "limit": 10
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(envelope["code"], 0);
    assert_eq!(envelope["data"]["feed_title"], "Context Digest");
    let imported = envelope["data"]["imported"].as_array().expect("imported");
    assert_eq!(imported.len(), 2);

    let id = imported[0]["id"].as_i64().unwrap();
    let row = ctx.state.db.vault_row_by_id(id).unwrap().unwrap();
    assert!(row.tags.contains("rss"), "tags={}", row.tags);
    assert!(row.content.contains("Borrow checker"));

    let hits = retrieve_filtered(
        &ctx.state.db,
        "Borrow checker",
        10,
        SearchFilters {
            kinds: vec![DocumentKind::Document],
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert!(
        hits.iter()
            .any(|hit| hit.document.id == format!("note-{id}")),
        "{hits:?}"
    );
}

#[tokio::test]
async fn rejects_empty_feed() {
    let xml = r#"<?xml version="1.0"?><rss><channel><title>Empty</title></channel></rss>"#;
    let transport = Arc::new(ScriptedTransport::new().push_json(200, xml));
    let ctx = ctx(transport);
    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({ "url": "https://example.com/empty.xml" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{envelope}");
    assert_eq!(envelope["error_code"], "provider_invalid_response");
}
