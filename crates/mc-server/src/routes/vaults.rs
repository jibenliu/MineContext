//! `/api/db/vaults*` —— 笔记树（`database:*` 渠道的 HTTP 面）。
//!
//! 不在兼容面路由表里，因此登记为新增面。
//!
//! 逐字对齐形状的理由：笔记树、回收站、日报视图都直接消费这些返回值 ——
//! `is_folder` 返回布尔的 `true` 而不是数字 `1`，笔记树就会把所有节点当普通
//! 笔记渲染；这类错误在类型宽松的 TS 里不报错，只会显示错。

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock};
use mc_storage::vaults::{VaultPatch, VaultQuery, VaultUpsert, DOCUMENT_TYPE_VAULTS};
use serde::Deserialize;
use serde_json::json;

use crate::envelope;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/db/vaults", get(list).post(insert))
        .route("/api/db/vaults/folders", post(create_folder))
        .route(
            "/api/db/vaults/{id}",
            get(by_id).patch(patch).delete(hard_delete),
        )
        .route("/api/db/vaults/{id}/soft-delete", post(soft_delete))
        .route("/api/db/vaults/{id}/restore", post(restore))
        .route(
            "/api/db/vaults/{id}/hard",
            axum::routing::delete(hard_delete),
        )
}

/// 查询参数。字段名是下划线风格，不做驼峰转换。
#[derive(Debug, Deserialize)]
struct ListQuery {
    document_type: Option<String>,
    parent_id: Option<i64>,
    title: Option<String>,
    is_folder: Option<i64>,
    is_deleted: Option<i64>,
}

fn now() -> mc_common::time::Timestamp {
    Clock::now(&SystemClock)
}

async fn list(State(state): State<Arc<ServerState>>, Query(query): Query<ListQuery>) -> Response {
    let vault_query = VaultQuery {
        document_type: query.document_type.into_iter().collect(),
        // 不传 parent_id 时不做父节点过滤（调用方自己在客户端拼树）
        parent_id: query.parent_id.map(Some),
        title: query.title,
        is_folder: query.is_folder,
        is_deleted: query.is_deleted,
    };

    match state.db.query_vault_rows(&vault_query) {
        Ok(rows) => envelope::ok(json!(rows)),
        Err(error) => envelope::compat_failure(&error),
    }
}

async fn by_id(State(state): State<Arc<ServerState>>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(error) => return envelope::compat_failure(&error),
    };

    match state.db.vault_row_by_id(id) {
        // 旧 IPC 返回 `undefined`；HTTP 面上等价的是 `data: null`
        Ok(row) => envelope::ok(json!(row)),
        Err(error) => envelope::compat_failure(&error),
    }
}

/// 新建请求体。字段名与旧 `Vault` 类型一致；缺省值由这里补齐（不是由前端）。
#[derive(Debug, Deserialize, Default)]
struct InsertBody {
    title: Option<String>,
    summary: Option<String>,
    content: Option<String>,
    /// 读出来是逗号分隔字符串（**不是**数组）时也容忍，两种都认
    #[serde(default, deserialize_with = "optional_tags")]
    tags: Option<Vec<String>>,
    parent_id: Option<i64>,
    is_folder: Option<i64>,
    document_type: Option<String>,
    sort_order: Option<i64>,
}

/// 兼容两种 `tags`：`"a,b"` 与 `["a","b"]`。
///
/// 返回 `Option` 是为了区分「字段没给」（`None` → 不要动）与
/// 「明确给了空值」（`Some([])` → 清空标签）——
/// `Option<Vec<String>>` 直接反序列化无法区分这两种情况。
fn optional_tags<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    // 注意：这里必须用 `Value::deserialize` 而不是 `Option<Value>` ——
    // 后者会把 JSON 的 `null` 也变成 `None`，于是「明确清空」与「没给」
    // 就被合并成同一种情况了（字段缺失由 `#[serde(default)]` 处理）。
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::Null => Some(Vec::new()),
        serde_json::Value::String(text) => Some(
            text.split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect(),
        ),
        serde_json::Value::Array(items) => Some(
            items
                .into_iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect(),
        ),
        _ => Some(Vec::new()),
    })
}

async fn insert(State(state): State<Arc<ServerState>>, Json(body): Json<InsertBody>) -> Response {
    let vault = VaultUpsert {
        title: body.title.unwrap_or_default(),
        summary: body.summary.unwrap_or_default(),
        content: body.content.unwrap_or_default(),
        tags: body.tags.unwrap_or_default(),
        parent_id: body.parent_id,
        is_folder: body.is_folder.unwrap_or(0) != 0,
        document_type: body
            .document_type
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DOCUMENT_TYPE_VAULTS.to_string()),
        sort_order: body.sort_order.unwrap_or(0),
    };

    match state.db.insert_vault_row(&vault, now()) {
        Ok(id) => envelope::ok(json!({ "id": id })),
        Err(error) => envelope::compat_failure(&error),
    }
}

/// 建文件夹（`database:create-folder`）。
#[derive(Debug, Deserialize)]
struct FolderBody {
    title: String,
    parent_id: Option<i64>,
}

async fn create_folder(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<FolderBody>,
) -> Response {
    let vault = VaultUpsert {
        title: body.title,
        summary: String::new(),
        content: String::new(),
        tags: Vec::new(),
        parent_id: body.parent_id,
        is_folder: true,
        document_type: DOCUMENT_TYPE_VAULTS.to_string(),
        sort_order: 0,
    };

    match state.db.insert_vault_row(&vault, now()) {
        Ok(id) => envelope::ok(json!({ "id": id })),
        Err(error) => envelope::compat_failure(&error),
    }
}

/// 局部更新。字段名与旧 `Vault` 一致；`parent_id: null` 表示移到根目录。
#[derive(Debug, Deserialize, Default)]
struct PatchBody {
    title: Option<String>,
    summary: Option<String>,
    content: Option<String>,
    #[serde(default, deserialize_with = "optional_tags")]
    tags: Option<Vec<String>>,
    /// 用 `Option<Option<i64>>` 区分「没给」与「给了 null」：
    /// 前者保持原值，后者把笔记移到根目录。
    #[serde(default, deserialize_with = "parent_from_json")]
    parent_id: Option<Option<i64>>,
    is_folder: Option<i64>,
    is_deleted: Option<i64>,
    document_type: Option<String>,
    sort_order: Option<i64>,
}

/// 区分「没给 parent_id」（保持原值）与「给了 `null`」（移到根目录）。
///
/// 不能直接用 `Option<Option<i64>>`：serde 会把 JSON 的 `null` 与
/// 「字段缺失」都变成外层的 `None`，于是「移到根目录」会被静默忽略。
fn parent_from_json<'de, D>(deserializer: D) -> Result<Option<Option<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(Some(None)),
        serde_json::Value::Number(number) => number
            .as_i64()
            .map(|id| Some(Some(id)))
            .ok_or_else(|| serde::de::Error::custom("parent_id 超出范围")),
        _ => Err(serde::de::Error::custom("parent_id 必须是整数或 null")),
    }
}

async fn patch(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Json(body): Json<PatchBody>,
) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(error) => return envelope::compat_failure(&error),
    };

    let patch = VaultPatch {
        title: body.title,
        summary: body.summary,
        content: body.content,
        // 没给 tags 就不动它；给了（哪怕空）才写
        tags: body.tags,
        parent_id: body.parent_id,
        is_folder: body.is_folder.map(|value| value != 0),
        is_deleted: body.is_deleted.map(|value| value != 0),
        document_type: body.document_type,
        sort_order: body.sort_order,
    };

    match state.db.update_vault_row(id, &patch, now()) {
        Ok(changes) => envelope::ok(json!({ "changes": changes })),
        Err(error) => envelope::compat_failure(&error),
    }
}

async fn soft_delete(State(state): State<Arc<ServerState>>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(error) => return envelope::compat_failure(&error),
    };
    match state.db.soft_delete_vault_row(id, now()) {
        Ok(changes) => envelope::ok(json!({ "changes": changes })),
        Err(error) => envelope::compat_failure(&error),
    }
}

async fn restore(State(state): State<Arc<ServerState>>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(error) => return envelope::compat_failure(&error),
    };
    match state.db.restore_vault_row(id, now()) {
        Ok(changes) => envelope::ok(json!({ "changes": changes })),
        Err(error) => envelope::compat_failure(&error),
    }
}

/// `DELETE /api/db/vaults/{id}` 与 `DELETE .../hard` 是同一件事：
/// 软删除与硬删除是同一件事：两条路径都落到物理删除。
async fn hard_delete(State(state): State<Arc<ServerState>>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(error) => return envelope::compat_failure(&error),
    };
    match state.db.hard_delete_vault_row(id) {
        Ok(changes) => envelope::ok(json!({ "changes": changes })),
        Err(error) => envelope::compat_failure(&error),
    }
}

/// 路径参数必须能给出**可读**的错误：`/api/db/vaults/abc` 是调用方写错了，
/// 不能变成 500 或者悄悄当成 0。
fn parse_id(raw: &str) -> Result<i64, AppError> {
    raw.trim().parse::<i64>().map_err(|_| {
        AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("笔记 id 必须是整数，收到 {raw:?}"),
        )
    })
}
