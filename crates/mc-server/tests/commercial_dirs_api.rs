//! 商业方向 3–6：风险 / 销售跟进 / 学习教练 / 交接包 API 业务测试。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::Database;
use tower::ServiceExt;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

fn at(offset_secs: i64) -> Timestamp {
    Timestamp::from_millis(T0 + offset_secs * 1_000)
}

fn ctx() -> (tempfile::TempDir, Arc<ServerState>) {
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
    (dir, state)
}

fn seed_activities(state: &ServerState, rows: &[(&str, &str, i64)]) {
    let activities = rows
        .iter()
        .map(
            |(id, title, end_offset)| mc_domain::activity::ActivityView {
                id: (*id).to_string(),
                start: at(end_offset - 60),
                end: at(*end_offset),
                title: (*title).to_string(),
                original_title: (*title).to_string(),
                category: Some("工作".into()),
                observations: Vec::new(),
                origin: mc_domain::activity::Provenance::Observed,
                confidence: 1.0,
                is_user_modified: false,
            },
        )
        .collect();
    mc_storage::projectors::activities::store(
        state.db.as_ref(),
        &mc_domain::projector::Projection {
            activities,
            noise: Vec::new(),
            ai_requests: 0,
            unknown_event_kinds: 0,
        },
        0,
        at(rows.last().map(|r| r.2).unwrap_or(0)),
    )
    .unwrap();
}

async fn get(state: &Arc<ServerState>, uri: &str) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header("host", "127.0.0.1:12345")
                .header("x-mc-token", "test-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).unwrap()
}

async fn post(state: &Arc<ServerState>, uri: &str, body: serde_json::Value) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("host", "127.0.0.1:12345")
                .header("x-mc-token", "test-token")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn risk_scan_lists_findings_report_and_accepts_dismiss() {
    let (_dir, state) = ctx();
    seed_activities(
        &state,
        &[
            (
                "r1",
                "Need to finish release notes by Friday; waiting on legal",
                100,
            ),
            ("r2", "Open question: 是否迁移 vault？", 200),
            ("r3", "遗漏了回滚说明 FIXME", 300),
            ("noise", "ok", 400),
        ],
    );

    let json = get(&state, "/api/v1/risks").await;
    let findings = json["data"]["findings"].as_array().unwrap();
    assert!(!findings.is_empty(), "{json}");
    assert!(findings
        .iter()
        .any(|f| f["kind"] == "commitment" || f["kind"] == "waiting_on"));
    assert!(findings
        .iter()
        .any(|f| f["kind"] == "open_question" || f["kind"] == "unresolved"));

    let report = get(&state, "/api/v1/risks/report").await;
    let md = report["data"]["markdown"].as_str().unwrap();
    assert!(md.contains("项目风险与遗漏"), "{md}");

    let id = findings[0]["id"].as_str().unwrap().to_string();
    let _ = post(
        &state,
        "/api/v1/risks/dismiss",
        serde_json::json!({ "finding_id": id }),
    )
    .await;
    let again = get(&state, "/api/v1/risks").await;
    let remaining = again["data"]["findings"].as_array().unwrap();
    assert!(!remaining.iter().any(|f| f["id"] == id), "{again}");
}

#[tokio::test]
async fn sales_memory_builds_timeline_and_visit_prep() {
    let (_dir, state) = ctx();
    seed_activities(
        &state,
        &[
            ("s1", "Zoom Meeting with Contoso — pipeline review", 100),
            ("s2", "客户:Contoso | 承诺下周给报价方案 follow up", 200),
            ("s3", "Mail - jane@contoso.com — NDA", 300),
        ],
    );

    let timeline = get(&state, "/api/v1/sales/timeline").await;
    let events = timeline["data"]["events"].as_array().unwrap();
    assert!(
        events
            .iter()
            .any(|e| e["contact_id"].as_str().unwrap_or("").contains("contoso")),
        "{timeline}"
    );

    let follow = get(&state, "/api/v1/sales/follow-ups").await;
    assert!(!follow["data"]["follow_ups"].as_array().unwrap().is_empty());

    let pack = get(&state, "/api/v1/sales/visit-prep?contact_id=contoso").await;
    assert!(pack["data"]["pack"]["prep_notes"]
        .as_str()
        .unwrap()
        .contains("拜访准备"));
}

#[tokio::test]
async fn learning_coach_detects_topics_and_review_plan() {
    let (_dir, state) = ctx();
    seed_activities(
        &state,
        &[
            ("l1", "Reading Rust async tutorial", 100),
            ("l2", "Rust Book ownership chapter", 200),
            ("l3", "error: borrow checker panic", 300),
            ("l4", "error: borrow checker again", 400),
            ("l5", "React course hooks", 500),
        ],
    );

    let topics = get(&state, "/api/v1/learning/topics").await;
    assert!(topics["data"]["topics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["topic_id"] == "rust"));

    let stuck = get(&state, "/api/v1/learning/stuck").await;
    assert!(stuck["data"]["patterns"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["repeats"].as_u64().unwrap() >= 2));

    let plan = get(&state, "/api/v1/learning/review-plan").await;
    assert!(!plan["data"]["items"].as_array().unwrap().is_empty());
    assert!(plan["data"]["summary"].as_str().unwrap().contains("复习"));
}

#[tokio::test]
async fn handoff_pack_requires_confirmation_before_export() {
    let (_dir, state) = ctx();
    seed_activities(
        &state,
        &[
            ("h1", "事故：API 网关超时，回滚到 v1.0.6", 100),
            ("h2", "决策：选定本地 SQLite 作为 vault", 200),
            ("h3", "运维：值班 runbook 重启 daemon", 300),
        ],
    );

    let cands = get(&state, "/api/v1/handoff/candidates").await;
    let list = cands["data"]["candidates"].as_array().unwrap();
    assert!(list.len() >= 2, "{cands}");
    assert!(list.iter().all(|c| c["confirmed"] == false));

    let empty = get(&state, "/api/v1/handoff/export").await;
    assert_eq!(empty["data"]["manifest"]["item_count"], 0);

    let first_id = list[0]["id"].as_str().unwrap().to_string();
    let _ = post(
        &state,
        "/api/v1/handoff/confirm",
        serde_json::json!({ "candidate_id": first_id }),
    )
    .await;

    let exported = get(&state, "/api/v1/handoff/export").await;
    assert_eq!(exported["data"]["manifest"]["item_count"], 1);
    assert!(exported["data"]["markdown"]
        .as_str()
        .unwrap()
        .contains("交接包"));
    assert_eq!(
        exported["data"]["manifest"]["format"],
        "minecontext-handoff-pack"
    );
}
