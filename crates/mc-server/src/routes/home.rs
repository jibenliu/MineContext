//! `/api/db/todos*`、`/api/db/tips`、`/api/db/heatmap` —— 首页数据面。
//!
//! 不在兼容面路由表里，因此登记为新增面。两个容易错的地方
//! （都属于「不报错但结果不对」）：
//!
//! 1. **返回形状**：`addTask` 读 `res.lastInsertRowid` 而不是 `{id}`，写成
//!    `{id}` 首页会插入一条 id 为 `-1` 的本地任务，刷新后消失；
//! 2. **时间参数有两种形状**（热力图给毫秒数、任务列表给 ISO 字符串）：
//!    统一走 `Timestamp::parse_flexible`，解析不了就报错。

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock, Timestamp};
use mc_storage::todos::{NewTodo, TodoPatch, TodoQuery};
use serde::Deserialize;
use serde_json::json;

use crate::envelope;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/db/todos", get(list_todos).post(add_todo))
        .route("/api/db/todos/{id}", patch(patch_todo).delete(delete_todo))
        .route("/api/db/todos/{id}/toggle", post(toggle_todo))
        .route("/api/db/tips", get(list_tips))
        .route("/api/db/heatmap", get(heatmap))
        .route("/api/v1/latest-activity/poll", post(latest_activity_poll))
}

/// 首页「最新活动」推送开关。
///
/// 前端挂载时发 `running`、卸载时发 `stopped`，数据走 SSE ——
/// 这个接口本身不返回活动，只控制服务端要不要推。
#[derive(Debug, Deserialize)]
struct PollBody {
    state: String,
}

async fn latest_activity_poll(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<PollBody>,
) -> Response {
    match body.state.trim() {
        "running" => {
            crate::latest_activity::start(&state, crate::latest_activity::DEFAULT_POLL_INTERVAL);
            envelope::ok(json!({ "success": true, "state": "running" }))
        }
        "stopped" => {
            crate::latest_activity::stop(&state);
            envelope::ok(json!({ "success": true, "state": "stopped" }))
        }
        other => envelope::compat_failure(&AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("未知的轮询状态 {other:?}，只接受 running / stopped"),
        )),
    }
}

fn now() -> Timestamp {
    Clock::now(&SystemClock)
}

fn parse_id(raw: &str) -> Result<i64, AppError> {
    raw.trim().parse::<i64>().map_err(|_| {
        AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("任务 id 必须是整数，收到 {raw:?}"),
        )
    })
}

/// 宽松解析（毫秒 / ISO / 兼容层格式）。缺省返回 `None`。
fn parse_bound(raw: Option<&str>, field: &str) -> Result<Option<Timestamp>, AppError> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(None),
        Some(value) => Timestamp::parse_flexible(value).map(Some).map_err(|error| {
            AppError::new(
                error.code(),
                format!("{field} 无法解析：{}", error.detail()),
            )
        }),
    }
}

#[derive(Debug, Deserialize)]
struct TodosQuery {
    start: Option<String>,
    end: Option<String>,
    status: Option<i64>,
}

async fn list_todos(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<TodosQuery>,
) -> Response {
    let start = match parse_bound(query.start.as_deref(), "start") {
        Ok(value) => value,
        Err(error) => return envelope::compat_failure(&error),
    };
    let end = match parse_bound(query.end.as_deref(), "end") {
        Ok(value) => value,
        Err(error) => return envelope::compat_failure(&error),
    };

    match state.db.list_todos(&TodoQuery {
        start,
        end,
        status: query.status,
    }) {
        Ok(rows) => envelope::ok(json!(rows)),
        Err(error) => envelope::compat_failure(&error),
    }
}

/// 新建任务。字段名与旧 `ToDo` 一致；`start_time` / `end_time` 接受
/// ISO 字符串或毫秒数（前端两种都发过）。
#[derive(Debug, Deserialize, Default)]
struct AddTodoBody {
    content: Option<String>,
    #[serde(default, deserialize_with = "optional_time")]
    start_time: Option<Timestamp>,
    #[serde(default, deserialize_with = "optional_time")]
    end_time: Option<Timestamp>,
    status: Option<i64>,
    urgency: Option<i64>,
    assignee: Option<String>,
    reason: Option<String>,
}

/// 时间字段：字符串走宽松解析，数字当毫秒。
fn optional_time<'de, D>(deserializer: D) -> Result<Option<Timestamp>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    use serde::Deserialize;

    let value = serde_json::Value::deserialize(deserializer)?;
    let parsed = match value {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Number(number) => number.as_i64().map(Timestamp::from_millis),
        serde_json::Value::String(text) => {
            Some(Timestamp::parse_flexible(&text).map_err(D::Error::custom)?)
        }
        _ => return Err(D::Error::custom("时间必须是字符串或毫秒数")),
    };
    Ok(parsed)
}

async fn add_todo(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<AddTodoBody>,
) -> Response {
    let todo = NewTodo {
        content: body.content.unwrap_or_default(),
        start_time: body.start_time,
        end_time: body.end_time,
        status: body.status.unwrap_or(0),
        urgency: body.urgency.unwrap_or(0),
        assignee: body.assignee.filter(|value| !value.is_empty()),
        reason: body.reason.filter(|value| !value.is_empty()),
    };

    match state.db.insert_todo(&todo, now()) {
        // 渲染层读 `res.lastInsertRowid`（不是 `{id}`）
        Ok(id) => envelope::ok(json!({ "lastInsertRowid": id, "changes": 1 })),
        Err(error) => envelope::compat_failure(&error),
    }
}

#[derive(Debug, Deserialize, Default)]
struct PatchTodoBody {
    content: Option<String>,
    #[serde(default, deserialize_with = "optional_time")]
    start_time: Option<Timestamp>,
    #[serde(default, deserialize_with = "nullable_time")]
    end_time: Option<Option<Timestamp>>,
    status: Option<i64>,
    urgency: Option<i64>,
    #[serde(default, deserialize_with = "nullable_text")]
    assignee: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable_text")]
    reason: Option<Option<String>>,
}

/// `null` → `Some(None)`（清空），字段缺失 → `None`（不动）。
fn nullable_time<'de, D>(deserializer: D) -> Result<Option<Option<Timestamp>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error as _;
    use serde::Deserialize;

    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::Null => Some(None),
        serde_json::Value::Number(number) => {
            number.as_i64().map(|ms| Some(Timestamp::from_millis(ms)))
        }
        serde_json::Value::String(text) => Some(Some(
            Timestamp::parse_flexible(&text).map_err(D::Error::custom)?,
        )),
        _ => return Err(D::Error::custom("结束时间必须是字符串、毫秒数或 null")),
    })
}

fn nullable_text<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value {
        serde_json::Value::Null => Some(None),
        serde_json::Value::String(text) => Some(Some(text)),
        _ => return Err(serde::de::Error::custom("必须是字符串或 null")),
    })
}

async fn patch_todo(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Json(body): Json<PatchTodoBody>,
) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(error) => return envelope::compat_failure(&error),
    };

    let patch = TodoPatch {
        content: body.content,
        start_time: body.start_time.map(Some),
        end_time: body.end_time,
        status: body.status,
        urgency: body.urgency,
        assignee: body.assignee,
        reason: body.reason,
    };

    match state.db.update_todo(id, &patch, now()) {
        Ok(changes) => envelope::ok(json!({ "changes": changes })),
        Err(error) => envelope::compat_failure(&error),
    }
}

async fn toggle_todo(State(state): State<Arc<ServerState>>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(error) => return envelope::compat_failure(&error),
    };
    match state.db.toggle_todo_status(id, now()) {
        Ok(changes) => envelope::ok(json!({ "changes": changes })),
        Err(error) => envelope::compat_failure(&error),
    }
}

async fn delete_todo(State(state): State<Arc<ServerState>>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(error) => return envelope::compat_failure(&error),
    };
    match state.db.delete_todo(id) {
        Ok(changes) => envelope::ok(json!({ "changes": changes })),
        Err(error) => envelope::compat_failure(&error),
    }
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    limit: Option<usize>,
}

/// 线索列表的默认条数与上限：界面一次只展示几条，但调用方可能显式要更多；
/// 上限存在的意义是别让一次请求把整表拉进内存。
const TIPS_DEFAULT_LIMIT: usize = 50;
const TIPS_MAX_LIMIT: usize = 500;

async fn list_tips(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<LimitQuery>,
) -> Response {
    match state.db.list_tips(
        query
            .limit
            .unwrap_or(TIPS_DEFAULT_LIMIT)
            .min(TIPS_MAX_LIMIT),
    ) {
        Ok(tips) => envelope::ok(json!(tips)),
        Err(error) => envelope::compat_failure(&error),
    }
}

#[derive(Debug, Deserialize)]
struct HeatmapQuery {
    start: Option<String>,
    end: Option<String>,
}

async fn heatmap(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<HeatmapQuery>,
) -> Response {
    let start = match parse_bound(query.start.as_deref(), "start") {
        Ok(Some(value)) => value,
        Ok(None) => {
            return envelope::compat_failure(&AppError::new(
                ErrorCode::DomainInvalidRange,
                "热力图必须提供 start 与 end（毫秒数或 ISO 字符串）",
            ));
        }
        Err(error) => return envelope::compat_failure(&error),
    };
    let end = match parse_bound(query.end.as_deref(), "end") {
        Ok(Some(value)) => value,
        Ok(None) => {
            return envelope::compat_failure(&AppError::new(
                ErrorCode::DomainInvalidRange,
                "热力图必须提供 start 与 end（毫秒数或 ISO 字符串）",
            ));
        }
        Err(error) => return envelope::compat_failure(&error),
    };

    let config = state.config.current();
    let timezone = config
        .config
        .general
        .timezone
        .clone()
        .unwrap_or_else(|| "UTC".to_string());

    match mc_storage::heatmap::heatmap_counts(&state.db, start, end, &timezone) {
        // 线上形状在这里定死：`total` 是各项之和，前端直接用。
        Ok(days) => {
            let payload: Vec<serde_json::Value> = days
                .iter()
                .map(|day| {
                    json!({
                        "date": day.date,
                        "todos": day.todos,
                        "conversations": day.conversations,
                        "vaults": day.vaults,
                        "screenshots": day.screenshots,
                        "documents": day.documents,
                        "contexts": day.contexts,
                        "total": day.total(),
                    })
                })
                .collect();
            envelope::ok(json!(payload))
        }
        Err(error) => envelope::compat_failure(&error),
    }
}
