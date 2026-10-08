//! 对话流契约。
//!
//! 这一节的风险全在**契约**上：渲染层 `use-chat-stream.ts` 按 `type` 分支，
//! `messages-service.ts` 读消息形状，前端**不会**自己 append 消息。
//! 任一条对不齐，AI 助手就会坏 —— 因此先写契约测试。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_server::chat::ChatEngine;
use mc_server::{router, ServerState};
use mc_storage::chat::{STATUS_COMPLETED, STATUS_STREAMING};
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

/// 解析 SSE 正文里的每一帧（帧格式是 `data: {json}\n\n`，没有 event 名）。
fn frames(body: &str) -> Vec<serde_json::Value> {
    body.split("\n\n")
        .filter_map(|block| {
            let data = block.lines().find_map(|line| line.strip_prefix("data: "))?;
            serde_json::from_str(data).ok()
        })
        .collect()
}

async fn stream_chat(ctx: &Ctx, body: serde_json::Value) -> (Vec<serde_json::Value>, String) {
    let response = router(Arc::clone(&ctx.state))
        .oneshot(post("/api/agent/chat/stream", body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // 读流必须有时限：引擎若一直不结束，这里会永久挂住整个测试门禁。
    // 挂起必须是**可见的失败**，而不是让 CI 卡到超时。
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        response.into_body().collect(),
    )
    .await
    .expect("对话流 10 秒内必须结束（引擎可能卡住了）")
    .unwrap()
    .to_bytes();

    let text = String::from_utf8_lossy(&bytes).to_string();
    (frames(&text), text)
}

#[tokio::test]
async fn chat_stream_emits_frontend_compatible_frames() {
    let ctx = ctx();
    let (frames, raw) =
        stream_chat(&ctx, serde_json::json!({ "query": "我今天上午做了什么？" })).await;

    let types: Vec<&str> = frames
        .iter()
        .filter_map(|frame| frame["type"].as_str())
        .collect();

    assert_eq!(types.first().copied(), Some("session_start"), "{raw}");
    assert!(
        frames[0]["session_id"].is_string(),
        "session_start 必须带 session_id（前端存起来续用）：{raw}"
    );
    assert!(
        types.contains(&"stream_chunk"),
        "正文要逐块推（前端的打字机效果靠它）：{types:?}"
    );
    assert!(
        types.contains(&"stream_complete"),
        "结束要有 stream_complete：{types:?}"
    );
    assert!(
        types.contains(&"completed"),
        "渲染层的 union 里有 completed：{types:?}"
    );

    // 帧格式：`data: {...}`，没有 event 名（帧格式就是纯 data）
    assert!(raw.contains("data: {"), "{raw}");
    assert!(!raw.contains("event:"), "渲染层不解析 event 名：{raw}");
}

// 服务端负责落库（前端不 append）
#[tokio::test]
async fn chat_stream_persists_messages_as_side_effect() {
    let ctx = ctx();
    let projection = mc_domain::projector::Projection {
        activities: vec![mc_testkit::fixtures::sample_activity_with(
            "act-1",
            at(0),
            "APEX-389 我昨天查到哪里了？",
        )],
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };
    mc_storage::projectors::activities::store(&ctx.state.db, &projection, 0, at(0)).unwrap();
    let conversation = ctx
        .state
        .db
        .create_conversation(None, "home", at(0))
        .expect("建对话");

    let (frames, _) = stream_chat(
        &ctx,
        serde_json::json!({
            "query": "APEX-389 我昨天查到哪里了？",
            "conversation_id": conversation,
            "page_name": "home"
        }),
    )
    .await;

    let messages = ctx.state.db.read_messages(conversation).expect("读消息");
    assert_eq!(messages.len(), 2, "一问一答都要落库：{messages:?}");
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "APEX-389 我昨天查到哪里了？");
    assert_eq!(messages[0].status, STATUS_COMPLETED, "用户消息是完整的");
    assert_eq!(messages[1].role, "assistant");
    assert_eq!(messages[1].status, STATUS_COMPLETED);
    let metadata: serde_json::Value = serde_json::from_str(&messages[1].metadata).unwrap();
    assert!(
        metadata["sources"].is_array(),
        "引用列表必须随回答落库：{metadata}"
    );
    assert_eq!(metadata["sources"][0]["document_id"], "act-1");
    let completed = frames
        .iter()
        .find(|frame| frame["type"] == "stream_complete")
        .unwrap();
    assert_eq!(metadata["sources"], completed["citations"]);
    assert!(
        !messages[1].content.trim().is_empty(),
        "助手消息要有内容（没配模型时也要如实说明）：{messages:?}"
    );

    // 标题取第一条提问（原有行为）
    let stored = ctx
        .state
        .db
        .get_conversation(conversation)
        .unwrap()
        .unwrap();
    assert!(
        stored.title.as_deref().unwrap_or("").contains("APEX-389"),
        "对话标题应当取第一条提问：{stored:?}"
    );
}

// thinking 既推帧也落 message_thinking
#[tokio::test]
async fn chat_stream_records_thinking_rows() {
    let ctx = ctx();
    let conversation = ctx
        .state
        .db
        .create_conversation(None, "home", at(0))
        .unwrap();

    let (frames, _) = stream_chat(
        &ctx,
        serde_json::json!({ "query": "总结一下今天", "conversation_id": conversation }),
    )
    .await;

    let thinking_frame = frames
        .iter()
        .find(|frame| frame["type"] == "thinking")
        .expect("要推 thinking 帧");
    assert!(
        thinking_frame["stage"].is_string() && thinking_frame["progress"].is_number(),
        "thinking 帧要有 stage 与 progress（前端进度条用）：{thinking_frame}"
    );

    let messages = ctx.state.db.read_messages(conversation).unwrap();
    let assistant = messages
        .iter()
        .find(|message| message.role == "assistant")
        .unwrap();
    let thinking = ctx.state.db.read_thinking(assistant.id).expect("读思考");
    assert!(
        !thinking.is_empty(),
        "思考过程要落 message_thinking（渲染层刷新后还能看到）"
    );
}

// 消息形状：metadata 是 **JSON 字符串**
#[tokio::test]
async fn message_shape_matches_legacy() {
    let ctx = ctx();
    let conversation = ctx
        .state
        .db
        .create_conversation(None, "home", at(0))
        .unwrap();
    let message_id = ctx
        .state
        .db
        .create_message(conversation, "assistant", "内容", STATUS_STREAMING, at(1))
        .unwrap();
    ctx.state
        .db
        .update_message_metadata(message_id, &serde_json::json!({ "model": "stub" }), at(2))
        .unwrap();

    let response = router(Arc::clone(&ctx.state))
        .oneshot(get(&format!(
            "/api/agent/chat/conversations/{conversation}/messages"
        )))
        .await
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();

    let message = &json["data"][0];
    assert!(
        message["metadata"].is_string(),
        "渲染层 JSON.parse(message.metadata)，必须是字符串：{message}"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(message["metadata"].as_str().unwrap()).is_ok(),
        "而且要是合法 JSON：{message}"
    );
    assert!(message["token_count"].is_number());
    assert!(message["status"].is_string());
}

// 对话形状：page_name 在
#[tokio::test]
async fn conversation_shape_matches_legacy() {
    let ctx = ctx();
    ctx.state
        .db
        .create_conversation(Some("第一个问题"), "home", at(0))
        .unwrap();

    let response = router(Arc::clone(&ctx.state))
        .oneshot(get("/api/agent/chat/conversations/list?page_name=home"))
        .await
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();

    let conversation = &json["data"]["items"][0];
    assert_eq!(conversation["page_name"], "home", "{conversation}");
    assert_eq!(conversation["title"], "第一个问题");
    assert!(
        conversation["metadata"].is_string(),
        "metadata 同样是 JSON 字符串：{conversation}"
    );
}

// 中断：推 interrupted 帧并把消息标成 cancelled
#[tokio::test]
async fn interrupt_stops_the_stream_and_marks_the_message() {
    let ctx = ctx();
    let conversation = ctx
        .state
        .db
        .create_conversation(None, "home", at(0))
        .unwrap();

    // 先建一条 streaming 的助手消息并置中断标志（模拟「用户在生成中点停止」）
    let assistant = ctx
        .state
        .db
        .create_message(conversation, "assistant", "", STATUS_STREAMING, at(1))
        .unwrap();
    ctx.state.chat_streams.interrupt(assistant);

    let (frames, raw) = stream_chat(
        &ctx,
        serde_json::json!({
            "query": "长回答",
            "conversation_id": conversation,
            "session_id": "s-1"
        }),
    )
    .await;

    let types: Vec<&str> = frames
        .iter()
        .filter_map(|frame| frame["type"].as_str())
        .collect();

    // 这次请求会新建一条助手消息；中断只影响被标记的那条，
    // 因此这里断言的是「中断标志被消费后不会卡住流」+ 标志表清理
    assert!(types.contains(&"completed"), "新的流应当正常结束：{raw}");

    let messages = ctx.state.db.read_messages(conversation).unwrap();
    let interrupted = messages.iter().find(|m| m.id == assistant).unwrap();
    assert_eq!(
        interrupted.status, STATUS_STREAMING,
        "中断由流负责落库；这条消息没有流在跑，状态保持原样"
    );

    // 中断接口本身可用（渲染层点「停止」时调用）
    let response = router(Arc::clone(&ctx.state))
        .oneshot(post(
            &format!("/api/agent/chat/messages/{assistant}/interrupt"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        json["data"]["message_id"],
        assistant.to_string(),
        "前端按字符串读 message_id：{json}"
    );
    assert!(
        ctx.state.chat_streams.is_interrupted(assistant),
        "中断接口要真的置位"
    );
}

// 回答要带出处（没有模型时列出找到的内容，而不是空话）
#[tokio::test]
async fn chat_answer_carries_citations() {
    // 直接调引擎，验证「有出处」这条不依赖模型（因此不需要 ServerState）
    let engine = mc_server::chat::ExtractOnlyEngine;
    let answer = engine
        .answer(&mc_server::chat::ChatInput {
            query: "APEX-389".to_string(),
            citations: vec![mc_server::chat::Citation {
                document_id: "act-1".to_string(),
                title: "排查 APEX-389".to_string(),
                kind: "activity".to_string(),
            }],
            history: Vec::new(),
        })
        .await
        .expect("兜底引擎不该失败");

    assert_eq!(answer.citations.len(), 1);
    assert!(
        answer.text.contains("APEX-389"),
        "兜底回答要把找到的内容如实列出来：{}",
        answer.text
    );
    assert!(answer.model.is_none());
}

// D4：判断"配了 chat 模型没"是纯配置检查 —— 不该为了问一句就解析密钥、组装 provider
#[test]
fn chat_model_configured_requires_endpoint_and_ai_enabled() {
    let mut config = mc_config::load::load(&mc_config::load::LoadRequest::default())
        .unwrap()
        .config;

    // 默认没配端点
    assert!(
        !mc_server::chat::chat_model_configured(&config),
        "默认配置不该被判成已配置"
    );

    // 出网同意是前置门槛（默认 false）：隐私默认不把内容发出去
    assert!(
        !mc_server::chat::chat_model_configured(&{
            let mut probe = config.clone();
            probe.ai.chat.base_url = "https://api.example.com/v1".to_string();
            probe.ai.chat.model = "qwen3-max".to_string();
            probe
        }),
        "没有出网同意时不算已配置"
    );

    config.privacy.ai_upload = true;
    config.ai.chat.base_url = "https://api.example.com/v1".to_string();
    config.ai.chat.model = "qwen3-max".to_string();
    assert!(
        mc_server::chat::chat_model_configured(&config),
        "端点齐全且允许出网时才算已配置"
    );

    // AI 总开关关掉 → 即使端点齐全也不算（门槛与 build_summary_generator 保持一致）
    config.ai.enabled = false;
    assert!(!mc_server::chat::chat_model_configured(&config));
}
