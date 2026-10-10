//! `POST /api/v1/research` —— 主题 + URL 汇编成一篇可检索研究笔记。

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
        .with_link_transport(transport as Arc<dyn HttpTransport>),
    );
    Ctx { _dir: dir, state }
}

async fn call(
    state: &Arc<ServerState>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/research")
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
async fn compiling_urls_into_research_note_is_searchable() {
    let html_a = r#"<!doctype html><html><head><title>Paper A</title></head>
<body><p>Vector indexes speed retrieval for personal context.</p></body></html>"#;
    let html_b = r#"<!doctype html><html><head><title>Paper B</title></head>
<body><p>Hybrid search combines keywords and embeddings.</p></body></html>"#;
    let transport = Arc::new(
        ScriptedTransport::new()
            .push_json(200, html_a)
            .push_json(200, html_b),
    );
    let ctx = ctx(transport);

    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({
            "topic": "personal retrieval",
            "urls": [
                "https://example.com/a",
                "https://example.com/b"
            ]
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    let id = envelope["data"]["id"].as_i64().unwrap();
    assert!(envelope["data"]["title"]
        .as_str()
        .unwrap()
        .contains("personal retrieval"));
    let row = ctx.state.db.vault_row_by_id(id).unwrap().unwrap();
    assert!(row.tags.contains("research"), "tags={}", row.tags);
    assert!(row.content.contains("Vector indexes"));
    assert!(row.content.contains("Hybrid search"));

    let hits = retrieve_filtered(
        &ctx.state.db,
        "Vector indexes",
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
async fn rejects_research_without_urls() {
    let transport = Arc::new(ScriptedTransport::new());
    let ctx = ctx(transport);
    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({
            "topic": "lonely topic",
            "urls": []
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{envelope}");
    assert_eq!(envelope["error_code"], "domain_invalid_range");
}
