//! 多 vault 隔离：会话列表与 RAG 不得跨 vault 污染。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_search::SearchFilters;
use mc_server::{router, ServerState};
use mc_storage::vaults::VaultUpsert;
use mc_storage::Database;
use tower::ServiceExt;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
}

fn ctx() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        "test-token".to_string(),
        at(0),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

fn post(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", "test-token")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", "test-token")
        .body(Body::empty())
        .unwrap()
}

fn folder(title: &str) -> VaultUpsert {
    VaultUpsert {
        title: title.to_string(),
        summary: String::new(),
        content: String::new(),
        tags: vec![],
        parent_id: None,
        is_folder: true,
        document_type: "vaults".to_string(),
        sort_order: 0,
    }
}

fn note(title: &str, content: &str, parent_id: Option<i64>) -> VaultUpsert {
    VaultUpsert {
        title: title.to_string(),
        summary: String::new(),
        content: content.to_string(),
        tags: vec![],
        parent_id,
        is_folder: false,
        document_type: "vaults".to_string(),
        sort_order: 0,
    }
}

#[test]
fn retrieval_scoped_to_vault_excludes_other_vault_notes() {
    let ctx = ctx();
    let work = ctx.state.db.insert_vault_row(&folder("Work"), at(1)).unwrap();
    let personal = ctx
        .state
        .db
        .insert_vault_row(&folder("Personal"), at(2))
        .unwrap();
    ctx.state
        .db
        .insert_vault_row(&note("w", "work-only-token-alpha", Some(work)), at(3))
        .unwrap();
    ctx.state
        .db
        .insert_vault_row(
            &note("p", "personal-only-token-beta", Some(personal)),
            at(4),
        )
        .unwrap();

    let work_hits = mc_server::retrieval::retrieve_filtered(
        &ctx.state.db,
        "token",
        10,
        SearchFilters {
            vault_id: Some(work),
            ..SearchFilters::default()
        },
        None,
    )
    .expect("work retrieval");
    assert!(
        work_hits
            .iter()
            .any(|hit| hit.document.text.contains("work-only-token-alpha")),
        "{work_hits:?}"
    );
    assert!(
        !work_hits
            .iter()
            .any(|hit| hit.document.text.contains("personal-only-token-beta")),
        "personal note must not leak into work vault RAG: {work_hits:?}"
    );

    let personal_hits = mc_server::retrieval::retrieve_filtered(
        &ctx.state.db,
        "token",
        10,
        SearchFilters {
            vault_id: Some(personal),
            ..SearchFilters::default()
        },
        None,
    )
    .expect("personal retrieval");
    assert!(personal_hits
        .iter()
        .any(|hit| hit.document.text.contains("personal-only-token-beta")));
    assert!(!personal_hits
        .iter()
        .any(|hit| hit.document.text.contains("work-only-token-alpha")));
}

#[tokio::test]
async fn conversations_list_filters_by_vault_id() {
    let ctx = ctx();
    let work = ctx.state.db.insert_vault_row(&folder("Work"), at(1)).unwrap();
    let personal = ctx
        .state
        .db
        .insert_vault_row(&folder("Personal"), at(2))
        .unwrap();

    for (vault, title) in [(work, "work chat"), (personal, "personal chat")] {
        let response = router(Arc::clone(&ctx.state))
            .oneshot(post(
                "/api/agent/chat/conversations",
                serde_json::json!({
                    "page_name": "assistant",
                    "vault_id": vault
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(json["data"]["vault_id"], vault);
        assert_eq!(json["data"]["title"], serde_json::Value::Null);
        let _ = title;
    }

    let response = router(Arc::clone(&ctx.state))
        .oneshot(get(&format!(
            "/api/agent/chat/conversations/list?page_name=assistant&vault_id={work}"
        )))
        .await
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["data"]["total"], 1, "{json}");
    assert_eq!(json["data"]["items"][0]["vault_id"], work);
}

#[tokio::test]
async fn stream_with_vault_id_creates_scoped_conversation() {
    let ctx = ctx();
    let work = ctx.state.db.insert_vault_row(&folder("Work"), at(1)).unwrap();

    let response = router(Arc::clone(&ctx.state))
        .oneshot(post(
            "/api/agent/chat/stream",
            serde_json::json!({
                "query": "hello vault",
                "page_name": "assistant",
                "vault_id": work
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let conversations = ctx
        .state
        .db
        .list_conversations(Some("assistant"), Some(work))
        .expect("list");
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0].vault_id, Some(work));
}
