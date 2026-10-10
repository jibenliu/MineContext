//! 检索入口 `GET /api/v1/search`（ 的服务端一半）。
//!
//! 前端此前**没有搜索页**、daemon 也**没有独立检索入口**（检索只被聊天流用到）。
//! 这一片先把服务端入口做出来：参数校验、鉴权、形状，以及与聊天流**同一条**
//! 检索路径（隐私语义因此一致：被拦截的内容不在文档集里）。
//!
//! 命中内容需要先造数据，那部分夹具与 `retrieval_*` 测试共用，登记为后续项；
//! 这里先钉住「入口本身」的三件事。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::Database;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
use tower::ServiceExt;

const TOKEN: &str = "search-token-0123456789";

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

async fn get(
    state: &Arc<ServerState>,
    uri: &str,
    token: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "127.0.0.1:12345");
    if let Some(token) = token {
        builder = builder.header("x-mc-token", token);
    }
    let response = router(Arc::clone(state))
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

#[tokio::test]
async fn search_requires_the_token() {
    let (_dir, state) = state();

    let (status, _) = get(&state, "/api/v1/search?q=日报", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "检索也必须鉴权");
}

#[tokio::test]
async fn empty_query_is_rejected_with_a_clear_reason() {
    let (_dir, state) = state();

    let (status, json) = get(&state, "/api/v1/search?q=%20", Some(TOKEN)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert!(
        json["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("检索词"),
        "要告诉调用方缺什么：{json}"
    );
}

#[tokio::test]
async fn empty_database_returns_the_shape_with_no_hits() {
    let (_dir, state) = state();

    let (status, json) = get(&state, "/api/v1/search?q=日报", Some(TOKEN)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 0, "{json}");
    assert_eq!(json["data"]["query"], "日报");
    assert!(
        json["data"]["results"]
            .as_array()
            .expect("results 必须是数组")
            .is_empty(),
        "空库不该凭空造结果：{json}"
    );
}

/// 检索页不依赖模型：始终本地关键词，并显式标 `mode`，前端才能亮「仅本地」。
#[tokio::test]
async fn search_reports_keyword_local_mode() {
    let (_dir, state) = state();

    let (status, json) = get(&state, "/api/v1/search?q=日报", Some(TOKEN)).await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(
        json["data"]["mode"], "keyword",
        "搜索入口不走模型，mode 必须是 keyword：{json}"
    );
}

#[tokio::test]
async fn time_window_filters_are_accepted() {
    let (_dir, state) = state();

    // 参数存在即生效（无命中时也要能正常返回，而不是 500/400）
    let (status, json) = get(
        &state,
        &format!("/api/v1/search?q=日报&start={T0}&end={}", T0 + 3_600_000),
        Some(TOKEN),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["code"], 0);
}

#[tokio::test]
async fn inverted_time_window_is_rejected() {
    let (_dir, state) = state();
    let (status, _) = get(&state, "/api/v1/search?q=work&start=2&end=1", Some(TOKEN)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn time_filter_applies_before_result_limit() {
    let (_dir, state) = state();
    let activities = [
        ("outside", T0, "work work work"),
        ("inside", T0 + 1000, "work"),
    ]
    .into_iter()
    .map(|(id, start, title)| mc_domain::activity::ActivityView {
        id: id.to_string(),
        start: Timestamp::from_millis(start),
        end: Timestamp::from_millis(start + 500),
        title: title.to_string(),
        original_title: title.to_string(),
        category: None,
        observations: Vec::new(),
        origin: mc_domain::activity::Provenance::Observed,
        confidence: 1.0,
        is_user_modified: false,
    })
    .collect();
    mc_storage::projectors::activities::store(
        &state.db,
        &mc_domain::projector::Projection {
            activities,
            noise: Vec::new(),
            ai_requests: 0,
            unknown_event_kinds: 0,
        },
        0,
        Timestamp::from_millis(T0),
    )
    .unwrap();
    let (_, unfiltered) = get(&state, "/api/v1/search?q=work&limit=1", Some(TOKEN)).await;
    let (expected, bound) = if unfiltered["data"]["results"][0]["id"] == "outside" {
        ("inside", format!("start={}", T0 + 1000))
    } else {
        ("outside", format!("end={T0}"))
    };
    let (status, json) = get(
        &state,
        &format!("/api/v1/search?q=work&limit=1&{bound}"),
        Some(TOKEN),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["results"][0]["id"], expected, "{json}");
}

/// 造一条活动（与 `retrieval_chat.rs` 的 `seed_activity` 同一套口径：
/// 先写观测，再用投影器写活动表）。
fn seed_activity(state: &Arc<ServerState>, id: &str, title: &str) {
    let at = |offset: i64| Timestamp::from_millis(T0 + offset);

    let projection = mc_domain::projector::Projection {
        activities: vec![mc_domain::activity::ActivityView {
            id: id.to_string(),
            start: at(0),
            end: at(1_800_000),
            title: title.to_string(),
            original_title: title.to_string(),
            category: Some("开发".to_string()),
            observations: Vec::new(),
            origin: mc_domain::activity::Provenance::Rule {
                rule_id: "coding".to_string(),
            },
            confidence: 1.0,
            is_user_modified: false,
        }],
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };
    mc_storage::projectors::activities::store(state.db.as_ref(), &projection, 0, at(1_800_000))
        .expect("存活动");
}

#[tokio::test]
async fn keyword_hits_are_returned_with_their_fields() {
    let (_dir, state) = state();
    seed_activity(&state, "act-1", "写导入脚本");

    let (status, json) = get(&state, "/api/v1/search?q=导入脚本", Some(TOKEN)).await;

    assert_eq!(status, StatusCode::OK, "{json}");
    let results = json["data"]["results"].as_array().expect("results 数组");
    assert_eq!(results.len(), 1, "关键词命中一条：{json}");
    assert_eq!(results[0]["id"], "act-1");
    assert_eq!(results[0]["kind"], "activity");
    assert!(
        results[0]["title"]
            .as_str()
            .unwrap_or_default()
            .contains("导入脚本"),
        "标题要能给人看：{json}"
    );
    assert!(results[0]["score"].as_f64().unwrap_or(0.0) > 0.0, "{json}");
}

#[tokio::test]
async fn time_window_excludes_hits_outside_it() {
    let (_dir, state) = state();
    seed_activity(&state, "act-1", "写导入脚本");

    // 窗口在活动**之后**（活动在 T0..T0+30min）
    let later = T0 + 7 * 24 * 3_600_000;
    let (status, json) = get(
        &state,
        &format!("/api/v1/search?q=导入脚本&start={later}"),
        Some(TOKEN),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{json}");
    assert!(
        json["data"]["results"].as_array().unwrap().is_empty(),
        "时间窗之外不该返回命中：{json}"
    );
}
