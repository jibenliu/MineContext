//! 首页数据面（`/api/db/todos*`、`/api/db/tips`、`/api/db/heatmap`）。
//!
//! 这三组接口是首页（任务卡片、提示列表、热力图）的全部数据来源。
//! 验收标准是「渲染层零业务改动可用」：
//!
//! - `addTask` 读的是 `res.lastInsertRowid`，不是 `{id}`；
//! - 时间边界同时接受毫秒数（热力图）与 ISO 字符串（任务列表）；
//! - 热力图必须**每一天都有一行**（含零值），否则热力图会出现空洞。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::observations::NewObservation;
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";

/// 2026-09-30T09:00:00Z
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
const HOUR: i64 = 3_600_000;

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
}

/// 时区固定成 Asia/Shanghai：热力图按**本地日**分桶，
/// 用 UTC 断言不出错才说明时区处理是对的。
fn ctx() -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::Inline {
            name: "test".to_string(),
            toml: "[general]\ntimezone = \"Asia/Shanghai\"\n".to_string(),
        }],
        env: Vec::new(),
        read_process_env: false,
    })
    .unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        TOKEN.to_string(),
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
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

async fn call(
    state: &Arc<ServerState>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> serde_json::Value {
    let response = router(Arc::clone(state))
        .oneshot(request(method, uri, body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "{method} {uri}");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("必须返回 JSON 信封")
}

fn data(envelope: &serde_json::Value) -> serde_json::Value {
    assert_eq!(envelope["code"], 0, "渲染层只认 code == 0：{envelope}");
    envelope["data"].clone()
}

fn seed_observation(state: &ServerState, id: &str, at_ms: i64, kind: &str) {
    state
        .db
        .insert_observation(&NewObservation {
            id: id.to_string(),
            ts: Timestamp::from_millis(at_ms),
            source_id: "screen".to_string(),
            kind: kind.to_string(),
            app_name: Some("Visual Studio Code".to_string()),
            app_bundle_id: None,
            window_title: Some("main.rs".to_string()),
            domain: None,
            display_id: None,
            scale_factor: None,
            image: None,
            text_content: None,
            text_origin: None,
            change_kind: "new".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: format!("key-{id}"),
        })
        .expect("写观测");
}

// ---------------------------------------------------------------- 任务

#[tokio::test]
async fn add_task_returns_last_insert_rowid() {
    let ctx = ctx();
    let envelope = call(
        &ctx.state,
        "POST",
        "/api/db/todos",
        Some(serde_json::json!({ "content": "写测试", "urgency": 3 })),
    )
    .await;

    let payload = data(&envelope);
    assert!(
        payload["lastInsertRowid"].as_i64().unwrap_or(0) > 0,
        "首页读的是 res.lastInsertRowid：{payload}"
    );
    assert_eq!(payload["changes"], 1);
}

#[tokio::test]
async fn tasks_are_listed_for_the_requested_day_with_iso_bounds() {
    let ctx = ctx();
    // 前端传的是 ISO 字符串（dayjs().startOf('day').toISOString()）
    call(
        &ctx.state,
        "POST",
        "/api/db/todos",
        Some(serde_json::json!({
            "content": "今天的任务",
            "start_time": "2026-09-30T01:00:00.000Z"
        })),
    )
    .await;
    call(
        &ctx.state,
        "POST",
        "/api/db/todos",
        Some(serde_json::json!({
            "content": "明天的任务",
            "start_time": "2026-10-01T01:00:00.000Z"
        })),
    )
    .await;

    let rows = data(
        &call(
            &ctx.state,
            "GET",
            "/api/db/todos?start=2026-09-30T00:00:00.000Z&end=2026-09-30T23:59:59.999Z",
            None,
        )
        .await,
    );
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1, "只该拿到当天那条：{rows:?}");
    assert_eq!(rows[0]["content"], "今天的任务");
    assert_eq!(rows[0]["status"], 0);
    assert!(rows[0]["end_time"].is_null());
}

#[tokio::test]
async fn tasks_accept_millis_bounds_too() {
    let ctx = ctx();
    call(
        &ctx.state,
        "POST",
        "/api/db/todos",
        Some(serde_json::json!({
            "content": "毫秒边界内的任务",
            "start_time": "2026-09-30T09:00:00.000Z"
        })),
    )
    .await;

    let rows = data(
        &call(
            &ctx.state,
            "GET",
            &format!("/api/db/todos?start={}&end={}", T0 - HOUR, T0 + HOUR),
            None,
        )
        .await,
    );
    assert_eq!(rows.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn update_delete_and_toggle_tasks() {
    let ctx = ctx();
    let created = data(
        &call(
            &ctx.state,
            "POST",
            "/api/db/todos",
            Some(serde_json::json!({ "content": "原标题" })),
        )
        .await,
    );
    let id = created["lastInsertRowid"].as_i64().unwrap();

    let envelope = call(
        &ctx.state,
        "PATCH",
        &format!("/api/db/todos/{id}"),
        Some(serde_json::json!({ "content": "新标题", "urgency": 5 })),
    )
    .await;
    assert_eq!(data(&envelope)["changes"], 1);

    // 勾选：顺序字段 status 翻转成 1，并补上完成时间
    let envelope = call(
        &ctx.state,
        "POST",
        &format!("/api/db/todos/{id}/toggle"),
        None,
    )
    .await;
    assert_eq!(data(&envelope)["changes"], 1);

    let rows = data(&call(&ctx.state, "GET", "/api/db/todos", None).await);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows[0]["content"], "新标题");
    assert_eq!(rows[0]["status"], 1);
    assert_eq!(rows[0]["urgency"], 5);
    assert!(rows[0]["end_time"].is_string(), "完成时补 end_time");

    // 再勾一次回到未完成
    call(
        &ctx.state,
        "POST",
        &format!("/api/db/todos/{id}/toggle"),
        None,
    )
    .await;
    let rows = data(&call(&ctx.state, "GET", "/api/db/todos", None).await);
    assert_eq!(rows.as_array().unwrap()[0]["status"], 0);

    let envelope = call(&ctx.state, "DELETE", &format!("/api/db/todos/{id}"), None).await;
    assert_eq!(data(&envelope)["changes"], 1);
    assert_eq!(
        data(&call(&ctx.state, "GET", "/api/db/todos", None).await)
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn bad_time_bounds_are_reported_not_ignored() {
    let ctx = ctx();
    let envelope = call(&ctx.state, "GET", "/api/db/todos?start=昨天", None).await;
    assert_eq!(
        envelope["code"], 1,
        "解析不了的时间必须报错，而不是静默返回空"
    );
    assert!(envelope["error_code"].is_string());

    // 非数字 id 是调用方写错了：走错误信封，而不是 500 或静默 0 行
    let envelope = call(
        &ctx.state,
        "PATCH",
        "/api/db/todos/abc",
        Some(serde_json::json!({ "content": "x" })),
    )
    .await;
    assert_eq!(envelope["code"], 1);

    let envelope = call(&ctx.state, "DELETE", "/api/db/todos/abc", None).await;
    assert_eq!(envelope["code"], 1);
}

// ---------------------------------------------------------------- tips

#[tokio::test]
async fn tips_are_listed_newest_first() {
    let ctx = ctx();
    ctx.state
        .db
        .insert_tip("先写失败的测试", Timestamp::from_millis(T0))
        .unwrap();
    ctx.state
        .db
        .insert_tip("再让它变绿", Timestamp::from_millis(T0 + HOUR))
        .unwrap();

    let tips = data(&call(&ctx.state, "GET", "/api/db/tips", None).await);
    let tips = tips.as_array().unwrap();
    assert_eq!(tips.len(), 2);
    assert_eq!(tips[0]["content"], "再让它变绿");
    assert!(tips[0]["id"].is_number());
    assert!(tips[0]["created_at"].is_string());
}

// ---------------------------------------------------------------- 热力图

#[tokio::test]
async fn heatmap_covers_every_day_in_the_range() {
    let ctx = ctx();
    // 2026-09-30 → 10-02（本地日，Asia/Shanghai）
    let start = T0;
    let end = T0 + 2 * 24 * HOUR;

    let rows = data(
        &call(
            &ctx.state,
            "GET",
            &format!("/api/db/heatmap?start={start}&end={end}"),
            None,
        )
        .await,
    );
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 3, "含零值也要每天一行：{rows:?}");
    assert_eq!(rows[0]["date"], "2026-09-30");
    assert_eq!(rows[1]["date"], "2026-10-01");
    assert_eq!(rows[2]["date"], "2026-10-02");
    for row in rows {
        assert_eq!(row["total"], 0);
        for field in [
            "todos",
            "conversations",
            "vaults",
            "screenshots",
            "documents",
            "contexts",
        ] {
            assert_eq!(row[field], 0, "字段 {field} 必须存在且为 0");
        }
    }
}

#[tokio::test]
async fn heatmap_counts_every_source() {
    let ctx = ctx();

    // 9 月 30 日（本地）：2 张截图 + 1 条文档观测 + 1 条会话 + 1 篇笔记 + 1 条已完成任务
    seed_observation(&ctx.state, "obs-1", T0, "screen");
    seed_observation(&ctx.state, "obs-2", T0 + HOUR, "screen");
    seed_observation(&ctx.state, "obs-3", T0 + 2 * HOUR, "file");

    ctx.state
        .db
        .create_conversation(Some("问了点什么"), "home", Timestamp::from_millis(T0))
        .unwrap();
    ctx.state
        .db
        .insert_vault_row(
            &mc_storage::vaults::VaultUpsert {
                title: "随手记".to_string(),
                summary: String::new(),
                content: String::new(),
                tags: Vec::new(),
                parent_id: None,
                is_folder: false,
                document_type: "vaults".to_string(),
                sort_order: 0,
            },
            Timestamp::from_millis(T0),
        )
        .unwrap();

    let created = data(
        &call(
            &ctx.state,
            "POST",
            "/api/db/todos",
            Some(serde_json::json!({
                "content": "已完成的任务",
                "start_time": "2026-09-30T02:00:00.000Z",
                "status": 1
            })),
        )
        .await,
    );
    assert!(created["lastInsertRowid"].as_i64().unwrap_or(0) > 0);

    let end = T0 + 24 * HOUR;
    let rows = data(
        &call(
            &ctx.state,
            "GET",
            &format!("/api/db/heatmap?start={T0}&end={end}"),
            None,
        )
        .await,
    );
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);

    let first = &rows[0];
    assert_eq!(first["date"], "2026-09-30");
    assert_eq!(first["screenshots"], 2);
    assert_eq!(first["documents"], 1);
    assert_eq!(first["conversations"], 1);
    assert_eq!(first["vaults"], 1);
    assert_eq!(first["todos"], 1, "只统计已完成的任务（status=1）");
    assert_eq!(
        first["total"],
        2 + 1 + 1 + 1 + 1,
        "total 是各项之和：{first:?}"
    );
    assert_eq!(rows[1]["total"], 0);
}

/// 分桶按**配置时区**，不是 UTC：UTC 的 9 月 30 日 16:30 在上海是 10 月 1 日 00:30。
#[tokio::test]
async fn heatmap_buckets_by_the_configured_timezone() {
    let ctx = ctx();
    seed_observation(
        &ctx.state,
        "obs-late",
        T0 + 7 * HOUR + 30 * 60_000,
        "screen",
    );

    let rows = data(
        &call(
            &ctx.state,
            "GET",
            &format!("/api/db/heatmap?start={T0}&end={}", T0 + 24 * HOUR),
            None,
        )
        .await,
    );
    let rows = rows.as_array().unwrap();
    assert_eq!(rows[0]["date"], "2026-09-30");
    assert_eq!(rows[0]["screenshots"], 0, "UTC 当天不该算进去");
    assert_eq!(rows[1]["screenshots"], 1, "它落在上海的 10-01");
}

#[tokio::test]
async fn heatmap_rejects_bad_or_missing_bounds() {
    let ctx = ctx();

    let envelope = call(&ctx.state, "GET", "/api/db/heatmap", None).await;
    assert_eq!(envelope["code"], 1, "缺少 start/end 必须报错");

    let envelope = call(&ctx.state, "GET", "/api/db/heatmap?start=abc&end=def", None).await;
    assert_eq!(envelope["code"], 1);

    // end 早于 start 也要报错，而不是返回一串空行
    let envelope = call(
        &ctx.state,
        "GET",
        &format!("/api/db/heatmap?start={}&end={}", T0 + HOUR, T0),
        None,
    )
    .await;
    assert_eq!(envelope["code"], 1);
}

/// 范围过大要有上界：会在内存里为每一天建一个对象，
/// 传入「1970 → 2999」会让 daemon 直接吃满内存。
#[tokio::test]
async fn heatmap_refuses_absurdly_long_ranges() {
    let ctx = ctx();
    let envelope = call(
        &ctx.state,
        "GET",
        &format!("/api/db/heatmap?start=0&end={}", 4_000_000_000_000_i64),
        None,
    )
    .await;
    assert_eq!(envelope["code"], 1, "超长范围必须被拒绝");
    assert!(
        envelope["message"].as_str().unwrap_or("").contains("范围")
            || envelope["detail"].as_str().unwrap_or("").contains("天"),
        "要说明是范围问题：{envelope}"
    );
}
