//! 对话接口。
//!
//! **契约优先**：渲染层直接消费这些帧与消息形状。三条不能变：
//!
//! 1. 帧是 `data: {json}\n\n`，`type` 决定前端分支（`session_start` / `thinking` /
//!    `running` / `stream_chunk` / `stream_complete` / `completed` / `fail` /
//!    `done` / `error` / `interrupted`）；
//! 2. **服务端负责落库**：先建用户消息，再建 `streaming` 助手消息并逐块追加；
//! 3. `metadata` 是 JSON 字符串、对话有 `page_name`。

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::stream;
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock};
use mc_storage::chat::{STATUS_CANCELLED, STATUS_COMPLETED, STATUS_FAILED, STATUS_STREAMING};
use serde::Deserialize;
use serde_json::json;

use crate::envelope;
use crate::state::ServerState;

/// 带进对话的检索条数。太多会把提示词撑长，成本上升而质量未必提升。
const RETRIEVAL_LIMIT: usize = 8;

pub fn router() -> axum::Router<Arc<ServerState>> {
    use axum::routing::{get, patch, post};
    // 路径与参数名照前端现有的写（`{cid}` / `{mid}`）：
    // 契约脚本会比对这几个字符串，改名会显示成「契约漂移」。
    axum::Router::new()
        .route("/api/agent/chat/stream", post(chat_stream))
        .route("/api/agent/chat/conversations", post(create_conversation))
        .route(
            "/api/agent/chat/conversations/list",
            get(list_conversations),
        )
        .route("/api/agent/chat/conversations/{cid}", get(get_conversation))
        .route(
            "/api/agent/chat/conversations/{cid}/messages",
            get(list_messages),
        )
        .route(
            "/api/agent/chat/messages/{mid}/interrupt",
            post(interrupt_message),
        )
        // 写入路径沿用前端现有的用法：前端自己建消息、追加分片、收尾、改标题、软删除对话。
        // 服务端自己的流式对话（`/stream`）不依赖它们，但渲染层仍会用。
        .route("/api/agent/chat/message/{mid}/create", post(create_message))
        .route(
            "/api/agent/chat/message/stream/{mid}/create",
            post(create_streaming_message),
        )
        .route("/api/agent/chat/message/{mid}/append", post(append_message))
        .route("/api/agent/chat/message/{mid}/update", post(update_message))
        .route(
            "/api/agent/chat/message/{mid}/finished",
            post(finish_message),
        )
        .route(
            "/api/agent/chat/conversations/{cid}/update",
            patch(update_conversation).delete(delete_conversation),
        )
}

#[derive(Debug, Deserialize)]
pub struct ChatStreamRequest {
    pub query: String,
    #[serde(default)]
    pub conversation_id: Option<i64>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub context: Option<serde_json::Value>,
    #[serde(default)]
    pub page_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConversationsQuery {
    limit: Option<u32>,
    offset: Option<u32>,
    page_name: Option<String>,
    user_id: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateConversationBody {
    #[serde(default)]
    page_name: Option<String>,
    #[serde(default)]
    document_id: Option<String>,
}

/// `POST /api/agent/chat/stream` —— SSE 流式对话。
pub async fn chat_stream(
    State(state): State<Arc<ServerState>>,
    payload: Result<Json<ChatStreamRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(request) => request,
        Err(rejection) => {
            return envelope::error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                &AppError::new(
                    ErrorCode::DomainInvalidRange,
                    format!("请求体无法解析：{rejection}"),
                ),
            )
        }
    };

    let now = SystemClock.now();
    let session_id = request
        .session_id
        .clone()
        .unwrap_or_else(|| format!("session-{}", now.as_millis()));

    // 落库副作用之一：用户消息由服务端落库（客户端只消费帧，不负责写库）
    let mut conversation_id = request.conversation_id;
    if let Some(id) = conversation_id {
        if let Err(error) =
            state
                .db
                .create_message(id, "user", &request.query, STATUS_COMPLETED, now)
        {
            return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
        }
        // 会话标题取第一条提问（50 字以内），之后不再改
        let title: String = request.query.trim().chars().take(50).collect();
        let _ = state.db.set_conversation_title_if_empty(id, &title, now);
    } else if request.page_name.is_some() {
        // 没给对话 id 但要求归属某个页面 → 现建一个
        match state.db.create_conversation(
            None,
            request.page_name.as_deref().unwrap_or("home"),
            now,
        ) {
            Ok(id) => {
                conversation_id = Some(id);
                // 标题取用户提问（不是助手回答）——
                // 用回答当标题会让对话列表变成一列「没有配置模型…」
                let title: String = request.query.trim().chars().take(50).collect();
                let _ = state.db.set_conversation_title_if_empty(id, &title, now);
            }
            Err(error) => {
                return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error)
            }
        }
    }

    // 落库副作用之二：助手消息先建为 `streaming`
    let assistant_message_id = match conversation_id {
        Some(id) => match state
            .db
            .create_message(id, "assistant", "", STATUS_STREAMING, now)
        {
            Ok(message_id) => Some(message_id),
            Err(error) => {
                return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error)
            }
        },
        None => None,
    };

    // 语义检索的输入：查询向量 + 落库向量的索引。
    // 拿不到（未配置 embedding / 限流 / 索引不存在）就退化为纯关键词 ——
    // 语义是加分项，不是对话可用性的前提。
    let vectors = crate::embedding::query_vectors(&state, &request.query).await;

    // 先检索、再回答：**检索结果决定引用，引用进提示词**
    // （因此 被隐私拦截的内容既进不了检索、也进不了模型）
    let hits = match crate::retrieval::retrieve_with_vectors(
        &state.db,
        &request.query,
        RETRIEVAL_LIMIT,
        false,
        vectors
            .as_ref()
            .map(|(index, query)| (index.as_ref(), query)),
    ) {
        Ok(hits) => hits,
        Err(error) => {
            // 检索失败不该让对话失败：降级为「没有引用」照常回答
            let _ = crate::activities::record_failure(&state, &error, now);
            Vec::new()
        }
    };

    // 真流式：provider 的增量一到就推帧，不再等整段生成完。
    // 引擎/provider 不支持流式时退化为「一次回调整段」——契约不变，只是没有变快。
    let input = crate::chat::ChatInput {
        query: request.query.clone(),
        citations: crate::retrieval::citations(&hits),
        history: recent_history(
            &state,
            conversation_id,
            assistant_message_id,
            &request.query,
        ),
    };
    let engine = crate::chat::engine_for(&state);

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
    // 首帧立刻发：前端拿到 session_id 才算连上，不必等模型。
    let _ = tx.send(json!({
        "type": "session_start",
        "session_id": session_id,
        "assistant_message_id": assistant_message_id,
        // 让前端知道这次提问落到了哪个对话（新会话时是服务端刚建的），
        // 以便把 id 记下来、刷新侧栏列表；没有对话（未落库）时为 null。
        "conversation_id": conversation_id,
    }));

    let task_state = Arc::clone(&state);
    tokio::spawn(async move {
        run_stream(
            task_state,
            tx,
            engine,
            input,
            assistant_message_id,
            conversation_id,
            now,
        )
        .await;
    });

    let stream = stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|payload| (Ok::<Event, Infallible>(frame(payload)), rx))
    });
    Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

/// 一次对话带给模型的最近轮数。太少记不住指代，太多会把提示词与成本推高。
const MAX_HISTORY_TURNS: usize = 6;

/// 取同一会话里最近的几轮消息（时间从早到晚），**排除**本次提问与刚建的助手占位。
///
/// 没有它，追问（「那第二点呢」）拿不到上下文；有它，多轮才成立。
/// 历史同样要出网，因此脱敏在引擎侧对每条历史再做一次（见 [`crate::chat`]）。
fn recent_history(
    state: &ServerState,
    conversation_id: Option<i64>,
    assistant_message_id: Option<i64>,
    query: &str,
) -> Vec<crate::chat::ChatTurn> {
    let Some(id) = conversation_id else {
        return Vec::new();
    };
    let Ok(rows) = state.db.read_messages(id) else {
        // 读历史失败不该让对话失败：退化为「没有历史」照常回答
        return Vec::new();
    };

    let mut turns: Vec<crate::chat::ChatTurn> = rows
        .into_iter()
        .filter(|row| Some(row.id) != assistant_message_id)
        // 空内容的助手消息（上一次中断留下的）不带给模型
        .filter(|row| !row.content.trim().is_empty())
        .map(|row| crate::chat::ChatTurn {
            role: row.role.clone(),
            content: row.content.clone(),
        })
        .collect();

    // 本次提问刚落库，是最后一条 user 消息：它由 `query` 单独传，不重复
    if turns
        .last()
        .is_some_and(|turn| turn.role == "user" && turn.content == query)
    {
        turns.pop();
    }

    let keep_from = turns.len().saturating_sub(MAX_HISTORY_TURNS);
    turns.split_off(keep_from)
}

/// 跑一次流式回答：把引擎事件翻成帧、按增量落库、按增量查中断。
///
/// 中断检查放在**每个增量之前**：用户点「停止」应当立刻停，
/// 而不是等整段生成完（那样「停止」只是个标记，界面还要继续滚字）。
async fn run_stream(
    state: Arc<ServerState>,
    tx: tokio::sync::mpsc::UnboundedSender<serde_json::Value>,
    engine: Box<dyn crate::chat::ChatEngine>,
    input: crate::chat::ChatInput,
    assistant_message_id: Option<i64>,
    _conversation_id: Option<i64>,
    now: mc_common::time::Timestamp,
) {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    let content = std::sync::Mutex::new(String::new());
    let chunks = AtomicU64::new(0);
    let interrupted = AtomicBool::new(false);
    let running_sent = AtomicBool::new(false);

    let cb_state = Arc::clone(&state);
    let cb_tx = tx.clone();
    let result = engine
        .answer_stream(&input, &mut |event| match event {
            crate::chat::ChatEvent::Thinking(note) => {
                if let Some(id) = assistant_message_id {
                    let _ = cb_state.db.add_message_thinking(
                        id,
                        &note.content,
                        Some(&note.stage),
                        note.progress,
                        now,
                    );
                }
                let _ = cb_tx.send(json!({
                    "type": "thinking",
                    "stage": note.stage,
                    "content": note.content,
                    "progress": note.progress,
                }));
            }
            crate::chat::ChatEvent::Delta(text) => {
                if interrupted.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(id) = assistant_message_id {
                    if cb_state.chat_streams.is_interrupted(id) {
                        interrupted.store(true, Ordering::SeqCst);
                        return;
                    }
                }
                // running 帧在第一段正文之前发（契约顺序：thinking → running → 正文）
                if !running_sent.swap(true, Ordering::SeqCst) {
                    let _ = cb_tx.send(json!({
                        "type": "running",
                        "stage": "answering",
                        "content": "正在生成回答",
                        "progress": 0.5,
                    }));
                }
                if let Ok(mut buffer) = content.lock() {
                    buffer.push_str(text);
                }
                chunks.fetch_add(1, Ordering::SeqCst);
                let _ = cb_tx.send(json!({ "type": "stream_chunk", "content": text }));
            }
        })
        .await;

    match result {
        Err(error) => {
            if let Some(id) = assistant_message_id {
                let _ =
                    state
                        .db
                        .mark_message_finished(id, STATUS_FAILED, Some(error.detail()), now);
            }
            let _ = tx.send(json!({
                "type": "error",
                "content": error.user_message(),
                "error_code": error.code().as_str(),
            }));
            let _ = tx.send(json!({ "type": "done" }));
        }
        Ok(answer) => {
            let text = content.into_inner().unwrap_or_default();
            let written = chunks.load(Ordering::SeqCst) as i64;

            if interrupted.load(Ordering::SeqCst) {
                // 已经生成的部分要落库：界面刷新后还能看到「说到一半」的内容
                if let Some(id) = assistant_message_id {
                    if !text.is_empty() {
                        let _ = state.db.append_message_content(id, &text, written, now);
                    }
                    let _ = state
                        .db
                        .mark_message_finished(id, STATUS_CANCELLED, None, now);
                    state.chat_streams.clear(id);
                }
                let _ = tx.send(json!({
                    "type": "interrupted",
                    "content": "Message generation was interrupted",
                }));
                let _ = tx.send(json!({ "type": "done" }));
                return;
            }

            if let Some(id) = assistant_message_id {
                if !text.is_empty() {
                    let _ = state.db.append_message_content(id, &text, written, now);
                }
                let _ = state.db.update_message_metadata(
                        id,
                        &json!({ "model": answer.model, "citations": answer.citations.len(), "sources": answer.citations.iter().map(|citation| json!({
                            "document_id": citation.document_id,
                            "title": citation.title,
                            "kind": citation.kind,
                        })).collect::<Vec<_>>() }),
                        now,
                    );
                let _ = state
                    .db
                    .mark_message_finished(id, STATUS_COMPLETED, None, now);
                state.chat_streams.clear(id);
            }

            let _ = tx.send(json!({
                "type": "stream_complete",
                "content": answer.text,
                "citations": answer.citations.iter().map(|citation| json!({
                    "document_id": citation.document_id,
                    "title": citation.title,
                    "kind": citation.kind,
                })).collect::<Vec<_>>(),
            }));
            let _ = tx.send(json!({
                "type": "completed",
                "stage": "completed",
                "content": "",
                "progress": 1.0,
            }));
            let _ = tx.send(json!({ "type": "done" }));
        }
    }
}

fn frame(payload: serde_json::Value) -> Event {
    // 帧格式是 `data: {json}\n\n`（无 event 名）
    Event::default().data(payload.to_string())
}

/// `GET /api/agent/chat/conversations` —— 对话列表（返回形状见下）。
pub async fn list_conversations(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<ConversationsQuery>,
) -> Response {
    // `user_id` 仍然忽略（单用户产品，契约里保留字段但不支持筛选）；
    // `status` 现在真的下推到 SQL 了 —— 之前是 `let _ = (&query.user_id, &query.status)`，
    // 前端传了筛选却拿到全部，属于「看起来支持、实际不支持」。
    let _ = &query.user_id;
    let limit = query.limit.unwrap_or(20) as i64;
    let offset = query.offset.unwrap_or(0) as i64;
    match state.db.list_conversations_page(
        query.page_name.as_deref(),
        query.status.as_deref(),
        limit,
        offset,
    ) {
        Ok((rows, total)) => {
            // 返回 `{items, total}`（不是 `{conversations}`）
            let items: Vec<serde_json::Value> = rows.iter().map(conversation_json).collect();
            envelope::ok(json!({ "items": items, "total": total }))
        }
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

/// `GET /api/agent/chat/conversations/{cid}` —— 单个对话（直接返回对象，不套壳）。
pub async fn get_conversation(
    State(state): State<Arc<ServerState>>,
    Path(cid): Path<i64>,
) -> Response {
    match state.db.get_conversation(cid) {
        Ok(Some(row)) => envelope::ok(conversation_json(&row)),
        Ok(None) => envelope::error_response(
            StatusCode::NOT_FOUND,
            &AppError::new(ErrorCode::DomainNothingToDo, format!("对话 {cid} 不存在")),
        ),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn conversation_json(row: &mc_storage::chat::ConversationRow) -> serde_json::Value {
    json!({
        "id": row.id,
        "title": row.title,
        "user_id": "local",
        "created_at": row.created_at,
        "updated_at": row.updated_at,
        "metadata": row.metadata,
        "page_name": row.page_name,
        "status": row.status,
    })
}

/// `POST /api/agent/chat/conversations` —— 新建对话。
pub async fn create_conversation(
    State(state): State<Arc<ServerState>>,
    payload: Result<Json<CreateConversationBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let body = payload.ok().map(|Json(body)| body);
    let page_name = body
        .as_ref()
        .and_then(|body| body.page_name.clone())
        .unwrap_or_else(|| "home".to_string());
    let now = SystemClock.now();

    match state.db.create_conversation(None, &page_name, now) {
        Ok(id) => {
            // 允许带 `document_id`（存进对话 metadata）：
            // 不带它就丢掉，UI 会以为「从文档发起对话」没有生效
            if let Some(document_id) = body.and_then(|body| body.document_id) {
                let _ = state.db.update_conversation_metadata(
                    id,
                    &json!({ "document_id": document_id }),
                    now,
                );
            }
            match state.db.get_conversation(id) {
                Ok(Some(row)) => envelope::ok(conversation_json(&row)),
                _ => envelope::ok(json!({ "id": id, "page_name": page_name })),
            }
        }
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

/// `GET /api/agent/chat/conversations/{id}/messages` —— 消息列表（返回形状见下）。
pub async fn list_messages(
    State(state): State<Arc<ServerState>>,
    Path(cid): Path<i64>,
) -> Response {
    match state.db.read_messages(cid) {
        // `data` 就是消息数组（不是 `{messages: [...]}`）
        Ok(rows) => envelope::ok(serde_json::to_value(rows).unwrap_or(json!([]))),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

/// `POST /api/agent/chat/messages/{id}/interrupt` —— 中断生成。
///
/// 中断的语义：标志 + 逐增量检查。
/// 因此这里只置位，真正的状态变更由流负责 —— 两边都改会出现竞态。
/// 消息不存在时的 404（与 `get_conversation` 用同一个错误码，保持兼容面一致）。
fn message_not_found(id: i64) -> Response {
    envelope::error_response(
        StatusCode::NOT_FOUND,
        &AppError::new(ErrorCode::DomainNothingToDo, format!("消息 {id} 不存在")),
    )
}

#[derive(Debug, Deserialize)]
pub struct CreateMessageBody {
    conversation_id: i64,
    role: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    is_complete: bool,
    #[serde(default)]
    token_count: i64,
}

/// `POST /api/agent/chat/message/{mid}/create` —— URL 上的 `mid` 是**父消息 id**。
pub async fn create_message(
    State(state): State<Arc<ServerState>>,
    Path(parent): Path<String>,
    Json(body): Json<CreateMessageBody>,
) -> Response {
    let status = if body.is_complete {
        "completed"
    } else {
        "pending"
    };
    let now = SystemClock.now();

    match state.db.create_message_with_parent(
        mc_storage::chat::NewChatMessage {
            conversation_id: body.conversation_id,
            role: &body.role,
            content: &body.content,
            status,
            parent_message_id: Some(&parent),
            token_count: body.token_count,
        },
        now,
    ) {
        // 返回**裸 int**：前端直接当 id 用
        Ok(id) => envelope::ok(json!(id)),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateStreamingMessageBody {
    conversation_id: i64,
    role: String,
}

/// `POST /api/agent/chat/message/stream/{mid}/create` —— 先占位，再逐片追加。
pub async fn create_streaming_message(
    State(state): State<Arc<ServerState>>,
    Path(parent): Path<String>,
    Json(body): Json<CreateStreamingMessageBody>,
) -> Response {
    let now = SystemClock.now();
    match state.db.create_message_with_parent(
        mc_storage::chat::NewChatMessage {
            conversation_id: body.conversation_id,
            role: &body.role,
            content: "",
            status: "streaming",
            parent_message_id: Some(&parent),
            token_count: 0,
        },
        now,
    ) {
        Ok(id) => envelope::ok(json!(id)),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

#[derive(Debug, Deserialize)]
pub struct AppendMessageBody {
    #[serde(default)]
    content_chunk: String,
    #[serde(default)]
    token_count: i64,
}

/// `POST /api/agent/chat/message/{mid}/append` —— 累加内容与 token。
pub async fn append_message(
    State(state): State<Arc<ServerState>>,
    Path(mid): Path<i64>,
    Json(body): Json<AppendMessageBody>,
) -> Response {
    let now = SystemClock.now();
    match state
        .db
        .append_message_content(mid, &body.content_chunk, body.token_count, now)
    {
        Ok(()) if message_exists(&state, mid) => envelope::ok(json!(true)),
        Ok(()) => message_not_found(mid),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateMessageBody {
    #[serde(default)]
    new_content: String,
    #[serde(default)]
    is_complete: bool,
    #[serde(default)]
    token_count: Option<i64>,
}

/// `POST /api/agent/chat/message/{mid}/update` —— **替换**内容（不是追加）。
pub async fn update_message(
    State(state): State<Arc<ServerState>>,
    Path(mid): Path<i64>,
    Json(body): Json<UpdateMessageBody>,
) -> Response {
    let status = if body.is_complete {
        "completed"
    } else {
        "streaming"
    };
    let now = SystemClock.now();

    match state
        .db
        .replace_message_content(mid, &body.new_content, status, body.token_count, now)
    {
        Ok(true) => envelope::ok(json!(true)),
        Ok(false) => message_not_found(mid),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

/// `POST /api/agent/chat/message/{mid}/finished` —— 收尾为 `completed`。
pub async fn finish_message(
    State(state): State<Arc<ServerState>>,
    Path(mid): Path<i64>,
) -> Response {
    let now = SystemClock.now();
    match state.db.mark_message_finished(mid, "completed", None, now) {
        Ok(()) if message_exists(&state, mid) => envelope::ok(json!(true)),
        Ok(()) => message_not_found(mid),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

/// `UPDATE` 不改到行时 SQLite 不报错，因此「有没有这行」要单独查一次。
fn message_exists(state: &Arc<ServerState>, mid: i64) -> bool {
    state
        .db
        .with_read(|conn| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1)",
                [mid],
                |row| row.get::<_, i64>(0),
            )
        })
        .map(|found| found == 1)
        .unwrap_or(false)
}

#[derive(Debug, Deserialize)]
pub struct UpdateConversationBody {
    title: String,
}

/// `PATCH /api/agent/chat/conversations/{cid}/update` —— 返回更新后的对话对象。
pub async fn update_conversation(
    State(state): State<Arc<ServerState>>,
    Path(cid): Path<i64>,
    Json(body): Json<UpdateConversationBody>,
) -> Response {
    let now = SystemClock.now();
    match state.db.update_conversation_title(cid, &body.title, now) {
        Ok(true) => match state.db.get_conversation(cid) {
            Ok(Some(row)) => envelope::ok(conversation_json(&row)),
            Ok(None) => conversation_not_found(cid),
            Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
        },
        Ok(false) => conversation_not_found(cid),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

/// `DELETE /api/agent/chat/conversations/{cid}/update` —— 软删除。
pub async fn delete_conversation(
    State(state): State<Arc<ServerState>>,
    Path(cid): Path<i64>,
) -> Response {
    let now = SystemClock.now();
    match state.db.soft_delete_conversation(cid, now) {
        // 形状保持 `{success, id}`，不改
        Ok(true) => envelope::ok(json!({ "success": true, "id": cid })),
        Ok(false) => conversation_not_found(cid),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

fn conversation_not_found(cid: i64) -> Response {
    envelope::error_response(
        StatusCode::NOT_FOUND,
        &AppError::new(ErrorCode::DomainNothingToDo, format!("对话 {cid} 不存在")),
    )
}

pub async fn interrupt_message(
    State(state): State<Arc<ServerState>>,
    Path(mid): Path<i64>,
) -> Response {
    state.chat_streams.interrupt(mid);
    // message_id 返回**字符串**（前端按字符串读）
    envelope::ok(json!({ "message_id": mid.to_string() }))
}

/// 中断标志表：进程内、按消息 id。
///
/// 与作业表同理：中断只对「正在生成的流」有意义，
/// 重启后没有流在跑，标志也就没有意义，因此不需要持久化。
#[derive(Default)]
pub struct ChatStreams {
    interrupted: std::sync::Mutex<std::collections::HashSet<i64>>,
}

impl ChatStreams {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn interrupt(&self, message_id: i64) {
        mc_common::lock::lock_or_recover(&self.interrupted).insert(message_id);
    }

    pub fn is_interrupted(&self, message_id: i64) -> bool {
        mc_common::lock::lock_or_recover(&self.interrupted).contains(&message_id)
    }

    pub fn clear(&self, message_id: i64) {
        mc_common::lock::lock_or_recover(&self.interrupted).remove(&message_id);
    }
}

#[cfg(test)]
mod streaming_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use mc_common::time::{Clock, SystemClock};

    use super::*;
    use crate::chat::{ChatAnswer, ChatEngine, ChatEvent, ChatInput};

    /// 逐段吐字的假引擎：可以在第 N 段之前触发「用户点了停止」。
    struct ScriptedEngine {
        deltas: Vec<&'static str>,
        interrupt_after: Option<(Arc<ServerState>, i64, usize)>,
        emitted: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ChatEngine for ScriptedEngine {
        async fn answer(&self, _input: &ChatInput) -> Result<ChatAnswer, AppError> {
            panic!("流式路径不该走 answer")
        }

        async fn answer_stream(
            &self,
            input: &ChatInput,
            on_event: &mut (dyn for<'a> FnMut(ChatEvent<'a>) + Send),
        ) -> Result<ChatAnswer, AppError> {
            for (index, delta) in self.deltas.iter().enumerate() {
                if let Some((state, id, after)) = &self.interrupt_after {
                    if index == *after {
                        state.chat_streams.interrupt(*id);
                    }
                }
                self.emitted.fetch_add(1, Ordering::SeqCst);
                on_event(ChatEvent::Delta(delta));
            }
            Ok(ChatAnswer {
                text: self.deltas.concat(),
                thinking: Vec::new(),
                citations: input.citations.clone(),
                model: Some("scripted".to_string()),
            })
        }
    }

    fn state_with_message() -> (tempfile::TempDir, Arc<ServerState>, i64, i64) {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(mc_storage::Database::open(dir.path().join("minecontext.db")).unwrap());
        let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
        let state = Arc::new(ServerState::new(
            mc_config::ConfigHandle::new(config),
            db,
            "test-token".to_string(),
            SystemClock.now(),
            dir.path().to_path_buf(),
        ));
        let now = SystemClock.now();
        let conversation = state.db.create_conversation(None, "chat", now).unwrap();
        let message = state
            .db
            .create_message(conversation, "assistant", "", "streaming", now)
            .unwrap();
        (dir, state, conversation, message)
    }

    /// 把通道里的帧取出来（`run_stream` 结束后发送端被丢弃，`recv` 自然结束）。
    async fn drain(
        mut rx: tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
    ) -> Vec<serde_json::Value> {
        let mut frames = Vec::new();
        while let Some(frame) = rx.recv().await {
            frames.push(frame);
        }
        frames
    }

    #[tokio::test]
    async fn deltas_are_pushed_as_they_arrive() {
        let (_dir, state, _conversation, message) = state_with_message();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        let engine = Box::new(ScriptedEngine {
            deltas: vec!["今天", "写了", "脚本"],
            interrupt_after: None,
            emitted: AtomicUsize::new(0),
        });

        run_stream(
            Arc::clone(&state),
            tx,
            engine,
            ChatInput {
                query: "我今天做了什么".to_string(),
                citations: Vec::new(),
                history: Vec::new(),
            },
            Some(message),
            None,
            SystemClock.now(),
        )
        .await;

        let payloads = drain(rx).await;
        let types: Vec<&str> = payloads
            .iter()
            .map(|value| value["type"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            types,
            vec![
                "running",
                "stream_chunk",
                "stream_chunk",
                "stream_chunk",
                "stream_complete",
                "completed",
                "done"
            ],
            "增量必须逐段推（不是攒完再切块）：{payloads:?}"
        );
        assert_eq!(payloads[1]["content"], "今天");
        assert_eq!(payloads[4]["content"], "今天写了脚本");
    }

    #[tokio::test]
    async fn interrupt_stops_forwarding_and_keeps_partial_content() {
        let (_dir, state, conversation, message) = state_with_message();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        let engine = Box::new(ScriptedEngine {
            deltas: vec!["第一段", "第二段", "第三段"],
            // 第二段之前用户点了「停止」
            interrupt_after: Some((Arc::clone(&state), message, 1)),
            emitted: AtomicUsize::new(0),
        });

        run_stream(
            Arc::clone(&state),
            tx,
            engine,
            ChatInput {
                query: "我今天做了什么".to_string(),
                citations: Vec::new(),
                history: Vec::new(),
            },
            Some(message),
            None,
            SystemClock.now(),
        )
        .await;

        let payloads = drain(rx).await;
        let chunks: Vec<&str> = payloads
            .iter()
            .filter(|value| value["type"] == "stream_chunk")
            .map(|value| value["content"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(chunks, vec!["第一段"], "中断之后不该再转发增量");
        assert!(
            payloads.iter().any(|value| value["type"] == "interrupted"),
            "要有 interrupted 帧：{payloads:?}"
        );
        assert_eq!(payloads.last().unwrap()["type"], "done");

        let rows = state.db.read_messages(conversation).unwrap();
        let message_row = rows
            .into_iter()
            .find(|row| row.id == message)
            .expect("消息还在");
        assert_eq!(message_row.status, STATUS_CANCELLED);
        assert_eq!(message_row.content, "第一段", "已经生成的部分必须落库");
    }
}
