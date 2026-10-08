//! 响应形状契约：fixture 里的形状必须与 daemon 真实返回一致。
//!
//! 形状不对**不会报错**：把后端的 `{ summaries: [...] }` 当数组用，界面白屏；
//! 若前端测试的 mock 恰好写的是裸数组，还会**测试绿、真实白**。
//! 这里补上另一半：用真路由跑一遍，把返回形状与 `fixtures/contract/response-shapes.json`
//! 比对；前端测试的 mock 则直接从同一份 fixture 取。两边任一漂移都会红。

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_storage::projectors::summaries::NewSummary;
use mc_storage::Database;
use serde_json::Value;
use tower::ServiceExt;

const TOKEN: &str = "test-token-0123456789abcdef";

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
        TOKEN.to_string(),
        Timestamp::from_millis(1_756_000_000_000),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

use mc_server::ServerState;

async fn get(state: Arc<ServerState>, uri: &str) -> Value {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN)
        .body(Body::empty())
        .unwrap();
    let response = mc_server::router(state).oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{uri}");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("响应必须是 JSON")
}

/// 形状签名：只保留键与类型，递归。
///
/// 数组取所有元素签名的**并集**（元素是对象时，键集合取并集）——这样
/// "数组里有元素"与"数组为空"在形状上仍然可比（空数组记作 `[]`，不参与并集判断）。
fn signature(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Array(items) => {
            if items.is_empty() {
                return "[]".to_string();
            }
            let mut keys: BTreeSet<String> = BTreeSet::new();
            let mut scalar_types: BTreeSet<String> = BTreeSet::new();
            for item in items {
                match item {
                    Value::Object(map) => {
                        for (key, nested) in map {
                            keys.insert(format!("{key}:{}", signature(nested)));
                        }
                    }
                    other => {
                        scalar_types.insert(signature(other));
                    }
                }
            }
            if keys.is_empty() {
                // 标量数组：类型集合
                format!(
                    "[{}]",
                    scalar_types.into_iter().collect::<Vec<_>>().join("|")
                )
            } else {
                format!("[{{{}}}]", keys.into_iter().collect::<Vec<_>>().join(","))
            }
        }
        Value::Object(map) => {
            let mut parts: Vec<String> = map
                .iter()
                .map(|(key, nested)| format!("{key}:{}", signature(nested)))
                .collect();
            parts.sort();
            format!("{{{}}}", parts.join(","))
        }
    }
}

fn fixture() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/contract/response-shapes.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()));
    serde_json::from_str(&text).expect("fixture 必须是合法 JSON")
}

/// 断言某个渠道的真实响应形状 == fixture 里的样例形状。
fn assert_same_shape(channel: &str, live: &Value) {
    let document = fixture();
    let sample = &document["channels"][channel]["sample"];
    assert!(
        !sample.is_null(),
        "fixture 里没有 {channel} 的样例（形状契约不能缺项）"
    );
    assert_eq!(
        signature(live),
        signature(sample),
        "{channel} 的真实响应形状与 fixtures/contract/response-shapes.json 不一致。\n\
         真实：{}\nfixture：{}\n\
         改契约就同步改 fixture 与前端解包，别让界面先白屏。",
        signature(live),
        signature(sample)
    );
}

/// 造一条活动（搜索的文档来源）。走投影存储，与生产同一条路径。
fn seed_activity(state: &ServerState, id: &str, title: &str) {
    let at = Timestamp::from_millis(1_756_000_000_000);
    let projection = mc_domain::projector::Projection {
        activities: vec![mc_domain::activity::ActivityView {
            id: id.to_string(),
            start: at,
            end: Timestamp::from_millis(1_756_001_800_000),
            title: title.to_string(),
            original_title: title.to_string(),
            category: Some("开发".to_string()),
            observations: vec![mc_domain::activity::ObservationRef {
                id: format!("obs-{id}"),
                at,
            }],
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
    mc_storage::projectors::activities::store(state.db.as_ref(), &projection, 0, at)
        .expect("存活动");
}

fn seed_summary(state: &ServerState) {
    // 总结挂在阶段上（外键）：先把阶段建出来
    state
        .db
        .upsert_stages(
            &[mc_storage::projectors::stages::StageRow {
                id: "stage-1".to_string(),
                start: Timestamp::from_millis(1_756_000_000_000),
                end: Some(Timestamp::from_millis(1_756_001_800_000)),
                state: "closed".to_string(),
                end_reason: Some("idle".to_string()),
                day: "2026-09-30".to_string(),
                activities: vec!["act-1".to_string()],
            }],
            0,
        )
        .expect("写阶段");
    state
        .db
        .insert_summary(
            &NewSummary {
                id: "sum-1".to_string(),
                kind: "stage".to_string(),
                stage_id: Some("stage-1".to_string()),
                template_id: "work_stage".to_string(),
                start: Timestamp::from_millis(1_756_000_000_000),
                end: Timestamp::from_millis(1_756_001_800_000),
                title: "09:00–09:30 阶段总结".to_string(),
                fields: Default::default(),
                body_markdown: "这段时间在写代码。".to_string(),
                quality: "model".to_string(),
                model: Some("openai_compatible:qwen-max".to_string()),
                prompt_tokens: 100,
                completion_tokens: 30,
                scope: None,
            },
            Timestamp::from_millis(1_756_002_000_000),
        )
        .expect("写总结");
}

#[tokio::test]
async fn summaries_payload_matches_fixture() {
    let ctx = ctx();
    seed_activity(&ctx.state, "act-1", "重构检索层");
    seed_summary(&ctx.state);
    let envelope = get(Arc::clone(&ctx.state), "/api/v1/summaries").await;
    assert_same_shape("v1:summaries", &envelope["data"]);
}

#[tokio::test]
async fn conversations_payload_matches_fixture() {
    let ctx = ctx();
    ctx.state
        .db
        .create_conversation(
            Some("APEX-389 我昨天查到哪里了？"),
            "home",
            Timestamp::from_millis(1_756_000_000_000),
        )
        .expect("建对话");
    let envelope = get(
        Arc::clone(&ctx.state),
        "/api/agent/chat/conversations/list?limit=20",
    )
    .await;
    assert_same_shape("v1:conversations", &envelope["data"]);
}

#[tokio::test]
async fn search_payload_matches_fixture() {
    let ctx = ctx();
    // 造一条**关键词能命中**的活动：搜到非空结果，元素形状才被真正钉住
    seed_activity(&ctx.state, "act-1", "重构检索层");

    let envelope = get(
        Arc::clone(&ctx.state),
        "/api/v1/search?q=%E9%87%8D%E6%9E%84",
    )
    .await;
    assert!(
        !envelope["data"]["results"]
            .as_array()
            .expect("results 必须是数组")
            .is_empty(),
        "搜索必须命中刚造的活动，否则元素形状没被覆盖：{envelope}"
    );
    assert_same_shape("v1:search", &envelope["data"]);
}

#[tokio::test]
async fn adhoc_preview_payload_matches_fixture() {
    let ctx = ctx();
    let envelope = get(
        Arc::clone(&ctx.state),
        "/api/v1/summaries/adhoc/preview?from=2026-09-30T09:00:00Z&to=2026-09-30T10:00:00Z",
    )
    .await;
    assert_same_shape("v1:adhoc-preview", &envelope["data"]);
}

#[tokio::test]
async fn backend_status_payload_matches_fixture() {
    let ctx = ctx();
    let envelope = get(Arc::clone(&ctx.state), "/api/backend/status").await;
    assert_same_shape("backend:get-status", &envelope["data"]);
}

/// SSE 启动帧与 `/api/health` 同源：形状必须与 fixture 里的一致。
#[tokio::test]
async fn init_check_payload_matches_fixture() {
    let ctx = ctx();
    let live = get(Arc::clone(&ctx.state), "/api/health").await;
    assert_same_shape("push:init-check-data", &live);
}
