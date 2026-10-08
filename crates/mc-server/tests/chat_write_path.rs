//! 对话写入路径：前端在用的 7 条写入接口，原先只返回「未实现」，现在必须真的写库。
//!
//! 渲染层（`services/messages-service.ts`、`conversation-service.ts`）在改造期
//! 仍然可能走这几条：创建消息、追加流式分片、标记完成、改标题、软删除对话。
//! 在它们被实现之前，`/api/agent/chat/message/*` 只会返回结构化「未实现」——
//! 前端拿到的是一个「成功形状但什么都没发生」的响应。
//!
//! 这组测试把**写入真的发生**钉住：返回形状与前端在用的那几个一致（`int` / `bool` /
//! 对话对象 / `{success,id}`），同时库里那一行必须真的变了。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::Database;
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
use tower::ServiceExt;

const TOKEN: &str = "chat-write-token-0123456789";

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
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
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
            builder.body(Body::from(value.to_string())).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };

    let response = router(Arc::clone(state)).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

fn conversation(ctx: &Ctx) -> i64 {
    ctx.state
        .db
        .create_conversation(Some("第一次对话"), "home", Timestamp::from_millis(T0))
        .unwrap()
}

fn message_status(ctx: &Ctx, id: i64) -> (String, String, i64, Option<String>) {
    ctx.state
        .db
        .with_read(|conn| {
            conn.query_row(
                "SELECT status, COALESCE(content, ''), COALESCE(token_count, 0),
                        parent_message_id
                 FROM messages WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
        })
        .unwrap()
}

// ---------------------------------------------------------------- 创建消息

#[tokio::test]
async fn create_message_returns_the_new_id_and_records_the_parent() {
    let ctx = ctx();
    let cid = conversation(&ctx);

    let (status, json) = call(
        &ctx.state,
        "POST",
        "/api/agent/chat/message/user-1/create",
        Some(serde_json::json!({
            "conversation_id": cid,
            "role": "user",
            "content": "我今天上午做了什么？",
            "is_complete": true,
            "token_count": 12
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 0, "{json}");
    let id = json["data"]
        .as_i64()
        .expect("这个 id 前端按数字读，不是对象");
    assert!(id > 0);

    let (state, content, tokens, parent) = message_status(&ctx, id);
    assert_eq!(state, "completed", "is_complete=true 应当直接落 completed");
    assert_eq!(content, "我今天上午做了什么？");
    assert_eq!(tokens, 12);
    assert_eq!(
        parent.as_deref(),
        Some("user-1"),
        "URL 里的 mid 是 parent_message_id"
    );
}

#[tokio::test]
async fn streaming_placeholder_is_created_as_streaming() {
    let ctx = ctx();
    let cid = conversation(&ctx);

    let (_, json) = call(
        &ctx.state,
        "POST",
        "/api/agent/chat/message/stream/user-1/create",
        Some(serde_json::json!({ "conversation_id": cid, "role": "assistant" })),
    )
    .await;

    let id = json["data"].as_i64().expect("返回新消息 id");
    let (state, content, _, parent) = message_status(&ctx, id);
    assert_eq!(state, "streaming", "流式占位必须是 streaming");
    assert_eq!(content, "");
    assert_eq!(parent.as_deref(), Some("user-1"));
}

// ---------------------------------------------------------------- 流式追加

#[tokio::test]
async fn append_accumulates_content_and_tokens() {
    let ctx = ctx();
    let cid = conversation(&ctx);
    let (_, json) = call(
        &ctx.state,
        "POST",
        "/api/agent/chat/message/stream/user-1/create",
        Some(serde_json::json!({ "conversation_id": cid, "role": "assistant" })),
    )
    .await;
    let mid = json["data"].as_i64().unwrap();

    for (chunk, tokens) in [("上午", 3), ("在写导入脚本", 7)] {
        let (status, json) = call(
            &ctx.state,
            "POST",
            &format!("/api/agent/chat/message/{mid}/append"),
            Some(serde_json::json!({
                "message_id": mid,
                "content_chunk": chunk,
                "token_count": tokens
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["data"], serde_json::Value::Bool(true), "{json}");
    }

    let (state, content, tokens, _) = message_status(&ctx, mid);
    assert_eq!(content, "上午在写导入脚本", "分片要按顺序累加");
    assert_eq!(tokens, 10, "token 用量也要累加");
    assert_eq!(state, "streaming", "追加不改变状态");
}

#[tokio::test]
async fn append_to_a_missing_message_is_not_found() {
    let ctx = ctx();

    let (status, json) = call(
        &ctx.state,
        "POST",
        "/api/agent/chat/message/9999/append",
        Some(serde_json::json!({ "message_id": 9999, "content_chunk": "x" })),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_ne!(json["code"], 0, "{json}");
}

// ---------------------------------------------------------------- 更新与收尾

#[tokio::test]
async fn update_replaces_content_and_can_complete_the_message() {
    let ctx = ctx();
    let cid = conversation(&ctx);
    let (_, json) = call(
        &ctx.state,
        "POST",
        "/api/agent/chat/message/stream/user-1/create",
        Some(serde_json::json!({ "conversation_id": cid, "role": "assistant" })),
    )
    .await;
    let mid = json["data"].as_i64().unwrap();

    let (status, json) = call(
        &ctx.state,
        "POST",
        &format!("/api/agent/chat/message/{mid}/update"),
        Some(serde_json::json!({
            "message_id": mid,
            "new_content": "完整答案",
            "is_complete": true,
            "token_count": 42
        })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"], serde_json::Value::Bool(true));
    let (state, content, tokens, _) = message_status(&ctx, mid);
    assert_eq!(content, "完整答案", "update 是替换而不是追加");
    assert_eq!(tokens, 42);
    assert_eq!(state, "completed");
}

#[tokio::test]
async fn finished_marks_the_message_completed() {
    let ctx = ctx();
    let cid = conversation(&ctx);
    let (_, json) = call(
        &ctx.state,
        "POST",
        "/api/agent/chat/message/stream/user-1/create",
        Some(serde_json::json!({ "conversation_id": cid, "role": "assistant" })),
    )
    .await;
    let mid = json["data"].as_i64().unwrap();

    let (status, json) = call(
        &ctx.state,
        "POST",
        &format!("/api/agent/chat/message/{mid}/finished"),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"], serde_json::Value::Bool(true));
    assert_eq!(message_status(&ctx, mid).0, "completed");
}

#[tokio::test]
async fn finished_on_a_missing_message_is_not_found() {
    let ctx = ctx();

    let (status, _) = call(
        &ctx.state,
        "POST",
        "/api/agent/chat/message/424242/finished",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------- 对话本身

#[tokio::test]
async fn patch_update_returns_the_updated_conversation() {
    let ctx = ctx();
    let cid = conversation(&ctx);

    let (status, json) = call(
        &ctx.state,
        "PATCH",
        &format!("/api/agent/chat/conversations/{cid}/update"),
        Some(serde_json::json!({ "title": "改过的标题" })),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["data"]["title"], "改过的标题", "{json}");
    assert_eq!(json["data"]["id"], cid);
    // 形状要与 GET /conversations/{cid} 一致（前端复用同一个渲染路径）
    for field in ["user_id", "page_name", "status", "created_at", "updated_at"] {
        assert!(
            json["data"][field].is_object()
                || json["data"][field].is_string()
                || json["data"][field].is_number(),
            "缺字段 {field}: {json}"
        );
    }
}

#[tokio::test]
async fn delete_update_is_a_soft_delete() {
    let ctx = ctx();
    let cid = conversation(&ctx);

    let (status, json) = call(
        &ctx.state,
        "DELETE",
        &format!("/api/agent/chat/conversations/{cid}/update"),
        None,
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        json["data"]["success"],
        serde_json::Value::Bool(true),
        "{json}"
    );
    assert_eq!(json["data"]["id"], cid);

    // 软删除：行还在，状态变了（前端按「软删」语义读；硬删会连带消息一起没）
    let row = ctx.state.db.get_conversation(cid).unwrap().expect("行还在");
    assert_eq!(row.status, "deleted");
}

#[tokio::test]
async fn deleting_a_missing_conversation_is_not_found() {
    let ctx = ctx();

    let (status, _) = call(
        &ctx.state,
        "DELETE",
        "/api/agent/chat/conversations/9999/update",
        None,
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------- 契约完整性

#[tokio::test]
async fn these_paths_no_longer_answer_with_not_implemented() {
    let ctx = ctx();
    let cid = conversation(&ctx);
    let (_, json) = call(
        &ctx.state,
        "POST",
        "/api/agent/chat/message/stream/user-1/create",
        Some(serde_json::json!({ "conversation_id": cid, "role": "assistant" })),
    )
    .await;
    let mid = json["data"].as_i64().unwrap();

    for (method, uri) in [
        ("POST", format!("/api/agent/chat/message/{mid}/append")),
        ("POST", format!("/api/agent/chat/message/{mid}/update")),
        ("POST", format!("/api/agent/chat/message/{mid}/finished")),
        (
            "PATCH",
            format!("/api/agent/chat/conversations/{cid}/update"),
        ),
        (
            "DELETE",
            format!("/api/agent/chat/conversations/{cid}/update"),
        ),
    ] {
        let (_, json) = call(
            &ctx.state,
            method,
            &uri,
            Some(serde_json::json!({
                "message_id": mid,
                "new_content": "x",
                "content_chunk": "x",
                "title": "t",
            })),
        )
        .await;
        assert_ne!(
            json["error_code"], "not_implemented",
            "{method} {uri} 仍然返回「未实现」：{json}"
        );
    }
}

// V1：列表的分页与状态筛选必须下推到 SQL —— 之前是全量读出后内存分页，
// 且 `status` 被路由层显式忽略（前端传了筛选却拿到全部）。
#[tokio::test]
async fn conversations_list_paginates_and_filters_in_sql() {
    let ctx = ctx();
    for index in 0..3 {
        ctx.state
            .db
            .create_conversation(
                Some(&format!("会话 {index}")),
                "home",
                Timestamp::from_millis(T0 + index),
            )
            .unwrap();
    }

    // 分页：limit=1 只回一条，但 total 是"满足条件的总数"
    let (status, first) = call(
        &ctx.state,
        "GET",
        "/api/agent/chat/conversations/list?limit=1&offset=0",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        first["data"]["total"], 3,
        "total 是总数而不是本页条数：{first}"
    );
    assert_eq!(first["data"]["items"].as_array().unwrap().len(), 1);

    // 第二页拿到的是另一条（offset 真的生效）
    let (_, second) = call(
        &ctx.state,
        "GET",
        "/api/agent/chat/conversations/list?limit=1&offset=1",
        None,
    )
    .await;
    assert_ne!(
        first["data"]["items"][0]["id"], second["data"]["items"][0]["id"],
        "offset 没生效的话两页会是同一条"
    );

    // 状态筛选：把一条标成 deleted，默认列表不含它，显式筛选 status=deleted 只回它
    let deleted_id = first["data"]["items"][0]["id"].as_i64().unwrap();
    ctx.state
        .db
        .soft_delete_conversation(deleted_id, Timestamp::from_millis(T0 + 10))
        .unwrap();

    let (_, active) = call(
        &ctx.state,
        "GET",
        "/api/agent/chat/conversations/list",
        None,
    )
    .await;
    assert_eq!(
        active["data"]["total"], 2,
        "已删除的不该出现在默认列表：{active}"
    );

    let (_, only_deleted) = call(
        &ctx.state,
        "GET",
        "/api/agent/chat/conversations/list?status=deleted",
        None,
    )
    .await;
    assert_eq!(
        only_deleted["data"]["total"], 1,
        "显式筛选要生效：{only_deleted}"
    );
    assert_eq!(only_deleted["data"]["items"][0]["id"], deleted_id);
}
