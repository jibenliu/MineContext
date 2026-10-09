//! `POST /api/v1/links` —— 用户提交 URL，正文落成笔记树文档并进入检索。
//!
//! 验收只盯业务结果：合法链接变成可搜的本地笔记；非法 / 隐私拦截 / 抓取失败
//! 给出结构化错误，而不是静默空笔记。

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

fn ctx_with(transport: Arc<ScriptedTransport>, privacy_toml: &str) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, privacy_toml).unwrap();
    let request = mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::File(config_path)],
        env: Vec::new(),
        read_process_env: false,
    };
    let loaded = mc_config::load::load(&request).unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let state = Arc::new(
        ServerState::new(
            mc_config::ConfigHandle::new(loaded),
            db,
            TOKEN.to_string(),
            Timestamp::from_millis(T0),
            dir.path().to_path_buf(),
        )
        .with_link_transport(Arc::clone(&transport) as Arc<dyn HttpTransport>),
    );
    Ctx { _dir: dir, state }
}

fn ctx(transport: Arc<ScriptedTransport>) -> Ctx {
    ctx_with(transport, "")
}

async fn call(
    state: &Arc<ServerState>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/links")
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
async fn importing_a_public_link_creates_a_searchable_vault_note() {
    let html = r#"<!doctype html>
<html><head><title>Rust Book Chapter</title></head>
<body>
  <script>ignore.me()</script>
  <p>Ownership and borrowing are core Rust ideas for safe memory.</p>
</body></html>"#;
    let transport = Arc::new(ScriptedTransport::new().push_json(200, html));
    let ctx = ctx(transport);

    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({ "url": "https://doc.rust-lang.org/book/" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{envelope}");
    assert_eq!(envelope["code"], 0);
    let data = &envelope["data"];
    let id = data["id"].as_i64().expect("必须返回笔记 id");
    assert_eq!(data["title"], "Rust Book Chapter");
    assert_eq!(data["url"], "https://doc.rust-lang.org/book/");
    assert_eq!(data["source_host"], "doc.rust-lang.org");

    let row = ctx
        .state
        .db
        .vault_row_by_id(id)
        .unwrap()
        .expect("笔记必须落库");
    assert_eq!(row.document_type, "vaults");
    assert!(row.content.contains("https://doc.rust-lang.org/book/"));
    assert!(row.content.contains("Ownership and borrowing"));
    assert!(!row.content.contains("ignore.me"));
    assert!(row.tags.contains("link"), "tags={}", row.tags);

    let hits = retrieve_filtered(
        &ctx.state.db,
        "Ownership and borrowing",
        10,
        SearchFilters {
            kinds: vec![DocumentKind::Document],
            ..Default::default()
        },
        None,
    )
    .expect("笔记必须进入文档检索");
    assert!(
        hits.iter()
            .any(|hit| hit.document.id == format!("note-{id}")),
        "检索应命中新笔记：{hits:?}"
    );
}

#[tokio::test]
async fn rejects_non_http_schemes_and_loopback_hosts() {
    let transport = Arc::new(ScriptedTransport::new());
    let ctx = ctx(transport);

    for url in [
        "file:///etc/passwd",
        "ftp://example.com/a",
        "https://127.0.0.1/secret",
        "http://localhost/admin",
        "https://[::1]/",
        "not-a-url",
        "",
    ] {
        let (status, envelope) = call(&ctx.state, Some(serde_json::json!({ "url": url }))).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "url={url} envelope={envelope}"
        );
        assert_ne!(envelope["code"], 0);
        assert!(
            envelope["error_code"] == "domain_invalid_range"
                || envelope["error_code"] == "privacy_blocked",
            "url={url} error={}",
            envelope["error_code"]
        );
    }
}

#[tokio::test]
async fn blocked_domain_does_not_call_transport() {
    let transport = Arc::new(ScriptedTransport::new().push_json(200, "<html></html>"));
    let recorded = Arc::clone(&transport);
    let ctx = ctx_with(
        transport,
        "[privacy]\nblocked_domains = [\"bank.example\"]\n",
    );

    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({ "url": "https://login.bank.example/home" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{envelope}");
    assert_eq!(envelope["error_code"], "privacy_blocked");
    assert_eq!(recorded.call_count(), 0, "命中黑名单时绝不能去抓页面");
}

#[tokio::test]
async fn fetch_failure_returns_structured_error_without_empty_note() {
    let transport = Arc::new(
        ScriptedTransport::new().push_failure(mc_providers::transport::TransportError::Timeout),
    );
    let ctx = ctx(transport);

    let (status, envelope) = call(
        &ctx.state,
        Some(serde_json::json!({ "url": "https://example.com/slow" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{envelope}");
    assert_eq!(envelope["error_code"], "provider_timeout");

    let rows = ctx
        .state
        .db
        .query_vault_rows(&mc_storage::vaults::VaultQuery {
            document_type: vec!["vaults".into()],
            parent_id: None,
            title: None,
            is_folder: Some(0),
            is_deleted: Some(0),
        })
        .unwrap();
    assert!(rows.is_empty(), "抓取失败不得留下空笔记：{rows:?}");
}
