//! `/api/db/vaults*`（渲染层笔记树的 HTTP 面）。
//!
//! 这一组接口的验收标准只有一条：**渲染层零业务改动可用**。
//! 因此断言全部盯着前端在用的形状：
//!
//! - 统一信封 `{code:0, status:200, message, data}`；
//! - `is_folder` / `is_deleted` 是 0/1 数字；
//! - 写接口返回 `{id}` / `{changes}`（旧代码直接读 `result.id` / `result.changes`）；
//! - 单行查不到时 `data` 是 `null`（旧 IPC 返回 `undefined`），而不是报错。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::{router, ServerState};
use mc_storage::Database;
use tower::ServiceExt;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;
const TOKEN: &str = "test-token";

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
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{method} {uri} 必须是 HTTP 200"
    );
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("必须返回 JSON 信封")
}

/// 断言旧信封形状并返回 `data`。
fn data(envelope: &serde_json::Value) -> serde_json::Value {
    assert_eq!(envelope["code"], 0, "渲染层只认 code == 0：{envelope}");
    assert_eq!(envelope["status"], 200);
    assert!(envelope["message"].is_string());
    envelope["data"].clone()
}

async fn add_note(state: &Arc<ServerState>, title: &str, document_type: &str) -> i64 {
    let envelope = call(
        state,
        "POST",
        "/api/db/vaults",
        Some(serde_json::json!({
            "title": title,
            "summary": format!("{title} 摘要"),
            "content": format!("{title} 正文"),
            "tags": "工作,日报",
            "document_type": document_type,
        })),
    )
    .await;
    data(&envelope)["id"].as_i64().expect("必须返回新 id")
}

// ---------------------------------------------------------------- 读

#[tokio::test]
async fn list_returns_legacy_rows() {
    let ctx = ctx();
    add_note(&ctx.state, "随手记", "vaults").await;

    let envelope = call(&ctx.state, "GET", "/api/db/vaults", None).await;
    let rows = data(&envelope);
    let rows = rows.as_array().expect("data 必须是数组");
    assert_eq!(rows.len(), 1);

    let row = &rows[0];
    assert_eq!(row["title"], "随手记");
    assert_eq!(row["is_folder"], 0, "is_folder 必须是数字 0");
    assert_eq!(row["is_deleted"], 0);
    assert_eq!(row["tags"], "工作,日报");
    assert!(row["parent_id"].is_null(), "根节点 parent_id 是 null");
    assert!(row["created_at"].is_string());
    assert!(row["updated_at"].is_string());
    assert_eq!(row["document_type"], "vaults");
    assert_eq!(row["sort_order"], 0);
}

#[tokio::test]
async fn list_honours_the_document_type_filter() {
    let ctx = ctx();
    add_note(&ctx.state, "随手记", "vaults").await;
    add_note(&ctx.state, "2026-09-30", "DailyReport").await;

    let envelope = call(
        &ctx.state,
        "GET",
        "/api/db/vaults?document_type=DailyReport",
        None,
    )
    .await;
    let rows = data(&envelope);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["title"], "2026-09-30");
}

#[tokio::test]
async fn folders_filter_and_folder_creation_work() {
    let ctx = ctx();
    let envelope = call(
        &ctx.state,
        "POST",
        "/api/db/vaults/folders",
        Some(serde_json::json!({ "title": "Summary", "parent_id": null })),
    )
    .await;
    let folder = data(&envelope)["id"].as_i64().expect("建文件夹必须返回 id");
    add_note(&ctx.state, "随手记", "vaults").await;

    let envelope = call(&ctx.state, "GET", "/api/db/vaults?is_folder=1", None).await;
    let rows = data(&envelope);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], folder);
    assert_eq!(rows[0]["is_folder"], 1);
}

#[tokio::test]
async fn single_row_and_missing_row_behaviour() {
    let ctx = ctx();
    let id = add_note(&ctx.state, "随手记", "vaults").await;

    let envelope = call(&ctx.state, "GET", &format!("/api/db/vaults/{id}"), None).await;
    assert_eq!(data(&envelope)["id"], id);

    // 旧 IPC 返回 undefined；HTTP 面上等价的是 data: null（**不是**错误）
    let envelope = call(&ctx.state, "GET", "/api/db/vaults/4242", None).await;
    assert!(data(&envelope).is_null());

    // 非数字 id 是调用方写错了，必须报结构化错误而不是 500
    let envelope = call(&ctx.state, "GET", "/api/db/vaults/abc", None).await;
    assert_eq!(envelope["code"], 1);
    assert!(envelope["message"].is_string());
}

#[tokio::test]
async fn title_and_parent_filters_work() {
    let ctx = ctx();
    let folder = data(
        &call(
            &ctx.state,
            "POST",
            "/api/db/vaults/folders",
            Some(serde_json::json!({ "title": "Summary" })),
        )
        .await,
    )["id"]
        .as_i64()
        .unwrap();

    let envelope = call(
        &ctx.state,
        "POST",
        "/api/db/vaults",
        Some(serde_json::json!({ "title": "子笔记", "parent_id": folder })),
    )
    .await;
    let child = data(&envelope)["id"].as_i64().unwrap();

    let envelope = call(&ctx.state, "GET", "/api/db/vaults?title=子笔记", None).await;
    assert_eq!(data(&envelope).as_array().unwrap().len(), 1);

    let envelope = call(
        &ctx.state,
        "GET",
        &format!("/api/db/vaults?parent_id={folder}"),
        None,
    )
    .await;
    let rows = data(&envelope);
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], child);
}

// ---------------------------------------------------------------- 写

#[tokio::test]
async fn insert_returns_the_new_id_and_defaults() {
    let ctx = ctx();
    let id = add_note(&ctx.state, "随手记", "vaults").await;
    assert!(id > 0);

    // 不写 document_type → 由 schema 默认成 'vaults'
    let envelope = call(
        &ctx.state,
        "POST",
        "/api/db/vaults",
        Some(serde_json::json!({ "title": "没有类型" })),
    )
    .await;
    let other = data(&envelope)["id"].as_i64().unwrap();
    let envelope = call(&ctx.state, "GET", &format!("/api/db/vaults/{other}"), None).await;
    assert_eq!(data(&envelope)["document_type"], "vaults");
}

#[tokio::test]
async fn patch_returns_changes_and_only_touches_given_fields() {
    let ctx = ctx();
    let id = add_note(&ctx.state, "原标题", "vaults").await;

    let envelope = call(
        &ctx.state,
        "PATCH",
        &format!("/api/db/vaults/{id}"),
        Some(serde_json::json!({ "title": "新标题", "content": "新正文" })),
    )
    .await;
    assert_eq!(data(&envelope)["changes"], 1);

    let envelope = call(&ctx.state, "GET", &format!("/api/db/vaults/{id}"), None).await;
    let row = data(&envelope);
    assert_eq!(row["title"], "新标题");
    assert_eq!(row["content"], "新正文");
    assert_eq!(row["summary"], "原标题 摘要", "没给的字段不能被清空");
    assert_eq!(row["tags"], "工作,日报", "没给 tags 时标签必须保留");
}

#[tokio::test]
async fn patch_can_move_a_note_back_to_the_root() {
    let ctx = ctx();
    let folder = data(
        &call(
            &ctx.state,
            "POST",
            "/api/db/vaults/folders",
            Some(serde_json::json!({ "title": "Summary" })),
        )
        .await,
    )["id"]
        .as_i64()
        .unwrap();
    let envelope = call(
        &ctx.state,
        "POST",
        "/api/db/vaults",
        Some(serde_json::json!({ "title": "子笔记", "parent_id": folder })),
    )
    .await;
    let child = data(&envelope)["id"].as_i64().unwrap();

    call(
        &ctx.state,
        "PATCH",
        &format!("/api/db/vaults/{child}"),
        Some(serde_json::json!({ "parent_id": null })),
    )
    .await;

    let envelope = call(&ctx.state, "GET", &format!("/api/db/vaults/{child}"), None).await;
    assert!(data(&envelope)["parent_id"].is_null());
}

#[tokio::test]
async fn soft_delete_restore_and_hard_delete() {
    let ctx = ctx();
    let id = add_note(&ctx.state, "随手记", "vaults").await;

    // 软删除：默认列表看不到，但显式查 is_deleted=1 能看到
    let envelope = call(
        &ctx.state,
        "POST",
        &format!("/api/db/vaults/{id}/soft-delete"),
        None,
    )
    .await;
    assert_eq!(data(&envelope)["changes"], 1);
    assert_eq!(
        data(&call(&ctx.state, "GET", "/api/db/vaults", None).await)
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let deleted = data(&call(&ctx.state, "GET", "/api/db/vaults?is_deleted=1", None).await);
    assert_eq!(deleted.as_array().unwrap().len(), 1);

    // 恢复
    let envelope = call(
        &ctx.state,
        "POST",
        &format!("/api/db/vaults/{id}/restore"),
        None,
    )
    .await;
    assert_eq!(data(&envelope)["changes"], 1);
    assert_eq!(
        data(&call(&ctx.state, "GET", "/api/db/vaults", None).await)
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // DELETE 与 /hard 都走物理删除（两者等价）
    let envelope = call(
        &ctx.state,
        "DELETE",
        &format!("/api/db/vaults/{id}/hard"),
        None,
    )
    .await;
    assert_eq!(data(&envelope)["changes"], 1);
    assert!(data(&call(&ctx.state, "GET", &format!("/api/db/vaults/{id}"), None).await).is_null());

    let envelope = call(&ctx.state, "DELETE", &format!("/api/db/vaults/{id}"), None).await;
    assert_eq!(data(&envelope)["changes"], 0, "重复删除是 0 行");
}

// ---------------------------------------------------------------- 与日报归档打通

#[tokio::test]
async fn daily_reports_written_by_the_engine_appear_in_the_tree() {
    let ctx = ctx();
    let folder = ctx
        .state
        .db
        .ensure_folder("Summary", Timestamp::from_millis(T0))
        .unwrap();
    ctx.state
        .db
        .upsert_vault_document(
            "DailyReport",
            "2026-09-30",
            "今天的摘要",
            "今天的正文",
            &["日报".to_string()],
            Some(folder),
            Timestamp::from_millis(T0),
        )
        .unwrap();

    let rows = data(
        &call(
            &ctx.state,
            "GET",
            "/api/db/vaults?document_type=DailyReport",
            None,
        )
        .await,
    );
    let rows = rows.as_array().unwrap().to_vec();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["title"], "2026-09-30");
    assert_eq!(rows[0]["parent_id"], folder, "日报要挂在 Summary 文件夹下");

    // 文件夹本身也在树里（渲染层的 VaultTree 靠它渲染分组）
    let folders = data(&call(&ctx.state, "GET", "/api/db/vaults?is_folder=1", None).await);
    let folders = folders.as_array().unwrap();
    assert!(folders.iter().any(|row| row["title"] == "Summary"));
}
