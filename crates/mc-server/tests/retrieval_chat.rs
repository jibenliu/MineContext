//! 检索接入对话、隐私与页面归属。
//!
//! 三条要求：
//! - 回答要**能说出依据**（引用来自真实检索，不是编的）；
//! - 被隐私规则拦下的内容**既不进检索、也不进提示词**（硬性阻断项）；
//! - 对话能归属到不同页面（`page_name`，旧 Redux 依赖）。

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

fn frames(body: &str) -> Vec<serde_json::Value> {
    body.split("\n\n")
        .filter_map(|block| {
            let data = block.lines().find_map(|line| line.strip_prefix("data: "))?;
            serde_json::from_str(data).ok()
        })
        .collect()
}

/// 造一条活动（`blocked` 决定它的证据是否含被拦截观测）。
fn seed_activity(state: &ServerState, id: &str, title: &str, blocked: bool) {
    if blocked {
        // 被隐私规则拦下的观测：标题被清空，只留审计（采集侧就是这么做的）
        state
            .db
            .insert_observation(&mc_storage::observations::NewObservation {
                id: format!("obs-{id}"),
                ts: at(0),
                source_id: "macos:screen".to_string(),
                kind: "screen".to_string(),
                app_name: Some("1Password".to_string()),
                app_bundle_id: None,
                window_title: None,
                domain: None,
                display_id: None,
                scale_factor: None,
                image: None,
                text_content: None,
                text_origin: None,
                change_kind: "pixel_major".to_string(),
                privacy_verdict: "blocked".to_string(),
                phash: None,
                idempotency: format!("idem-{id}"),
            })
            .expect("写观测");
    }

    let projection = mc_domain::projector::Projection {
        activities: vec![mc_domain::activity::ActivityView {
            id: id.to_string(),
            start: at(0),
            end: at(1800),
            title: title.to_string(),
            original_title: title.to_string(),
            category: Some("开发".to_string()),
            observations: vec![mc_domain::activity::ObservationRef {
                id: format!("obs-{id}"),
                at: at(0),
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
    mc_storage::projectors::activities::store(state.db.as_ref(), &projection, 0, at(1800))
        .expect("存活动");
}

async fn chat(state: &Arc<ServerState>, query: &str) -> Vec<serde_json::Value> {
    let response = router(Arc::clone(state))
        .oneshot(post(
            "/api/agent/chat/stream",
            serde_json::json!({ "query": query }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    frames(&String::from_utf8_lossy(&bytes))
}

// 回答要带真实出处
#[tokio::test]
async fn chat_cites_retrieved_records() {
    let ctx = ctx();
    seed_activity(&ctx.state, "act-1", "排查 APEX-389 的验收标准", false);

    let frames = chat(&ctx.state, "APEX-389 我昨天查到哪里了").await;
    let complete = frames
        .iter()
        .find(|frame| frame["type"] == "stream_complete")
        .expect("要有 stream_complete");

    let citations = complete["citations"].as_array().expect("引用数组");
    assert_eq!(citations.len(), 1, "应当引用到那条活动：{complete}");
    assert_eq!(citations[0]["document_id"], "act-1");
    assert_eq!(citations[0]["kind"], "activity");

    // 没配模型时也要如实列出找到的内容
    let content: String = frames
        .iter()
        .filter(|frame| frame["type"] == "stream_chunk")
        .filter_map(|frame| frame["content"].as_str())
        .collect();
    assert!(
        content.contains("APEX-389"),
        "兜底回答要把找到的内容列出来：{content}"
    );
}

// 隐私：被拦截的内容既不进检索、也不进回答
#[tokio::test]
async fn chat_never_reads_blocked_content() {
    let ctx = ctx();
    seed_activity(&ctx.state, "act-secret", "1Password 主密码", true);
    seed_activity(&ctx.state, "act-ok", "APEX-389 的验收标准", false);

    // 检索层直接看不该有的那条
    let hits =
        mc_server::retrieval::retrieve(&ctx.state.db, "1Password 主密码", 10, false).expect("检索");
    assert!(
        hits.is_empty(),
        "证据里含被拦截观测的活动不该出现在检索结果里：{hits:?}"
    );

    // 对话里也不能出现
    let frames = chat(&ctx.state, "1Password 主密码").await;
    let complete = frames
        .iter()
        .find(|frame| frame["type"] == "stream_complete")
        .unwrap();
    let citations = complete["citations"].as_array().unwrap();
    assert!(citations.is_empty(), "引用里不能有被拦截内容：{complete}");

    let content: String = frames
        .iter()
        .filter(|frame| frame["type"] == "stream_chunk")
        .filter_map(|frame| frame["content"].as_str())
        .collect();
    assert!(
        !content.contains("主密码"),
        "回答里不能出现被拦截内容：{content}"
    );
}

// 检索本身能同时覆盖活动与总结
#[tokio::test]
async fn retrieval_covers_activities_and_summaries() {
    let ctx = ctx();
    seed_activity(&ctx.state, "act-1", "重构活动引擎", false);
    ctx.state
        .db
        .insert_summary(
            &mc_storage::projectors::summaries::NewSummary {
                id: "sum-1".to_string(),
                kind: "daily".to_string(),
                stage_id: None,
                template_id: "work_stage".to_string(),
                start: at(0),
                end: at(3600),
                title: "2026-09-30 日报".to_string(),
                fields: Default::default(),
                body_markdown: "下午在做缓存层的性能验证。".to_string(),
                quality: "fallback".to_string(),
                model: None,
                prompt_tokens: 0,
                completion_tokens: 0,
                scope: None,
            },
            at(3600),
        )
        .expect("写总结");

    let hits =
        mc_server::retrieval::retrieve(&ctx.state.db, "缓存层 性能", 10, false).expect("检索");
    assert!(
        hits.iter().any(|hit| hit.document.id == "sum-1"),
        "总结也要能被搜到：{hits:?}"
    );
}

// 对话可以归属到不同页面
#[tokio::test]
async fn conversations_can_belong_to_creation_page() {
    let ctx = ctx();
    let response = router(Arc::clone(&ctx.state))
        .oneshot(post(
            "/api/agent/chat/conversations",
            serde_json::json!({ "page_name": "creation" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["data"]["page_name"], "creation", "{json}");

    // 列表按页面过滤：home 的查询不该看到 creation 的对话
    let response = router(Arc::clone(&ctx.state))
        .oneshot(get("/api/agent/chat/conversations/list?page_name=home"))
        .await
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["data"]["total"], 0, "{json}");

    let response = router(Arc::clone(&ctx.state))
        .oneshot(get("/api/agent/chat/conversations/list?page_name=creation"))
        .await
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["data"]["total"], 1, "{json}");
}

// 流式请求带 page_name 时应当自动建一个归属正确的对话
#[tokio::test]
async fn streaming_with_page_name_creates_a_scoped_conversation() {
    let ctx = ctx();
    let _ = chat_with_page(&ctx.state, "帮我写一段说明", "creation").await;

    let conversations = ctx
        .state
        .db
        .list_conversations(Some("creation"), None)
        .expect("列对话");
    assert_eq!(conversations.len(), 1, "{conversations:?}");
    assert_eq!(conversations[0].page_name, "creation");
    assert!(
        conversations[0]
            .title
            .as_deref()
            .unwrap_or("")
            .contains("帮我写一段说明"),
        "标题仍取第一条提问：{conversations:?}"
    );
}

async fn chat_with_page(
    state: &Arc<ServerState>,
    query: &str,
    page_name: &str,
) -> Vec<serde_json::Value> {
    let response = router(Arc::clone(state))
        .oneshot(post(
            "/api/agent/chat/stream",
            serde_json::json!({ "query": query, "page_name": page_name }),
        ))
        .await
        .unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    frames(&String::from_utf8_lossy(&bytes))
}

// 5.5b —— 对话的引用里必须能出现**语义命中**（关键词完全搜不到的那条）。
// 这是「向量真的接进检索了」的端到端证据：不注入向量时引用为空，注入了就有。
#[tokio::test]
async fn chat_cites_semantic_hits_that_keywords_cannot_find() {
    use std::time::Duration;

    let ctx = ctx();
    seed_activity(&ctx.state, "act-1", "Figma", false);

    // 关键词检索对这条查询一无所获
    let events = chat(&ctx.state, "设计协作工具").await;
    assert!(
        !format!("{events:?}").contains("act-1"),
        "这条查询对 act-1 没有关键词命中，引用不该出现"
    );

    // 落库向量 + 注入 Provider（查询向量与活动向量一致）
    mc_storage::vectors::upsert_vectors(
        &ctx.state.db,
        &[mc_storage::vectors::VectorRecord {
            kind: "activity".to_string(),
            doc_id: "act-1".to_string(),
            model: "embed-small".to_string(),
            values: vec![1.0, 0.0],
        }],
        at(0),
    )
    .unwrap();

    let transport = std::sync::Arc::new(
        mc_testkit::provider::ScriptedTransport::new()
            .push_json(200, r#"{"data":[{"embedding":[1.0,0.0],"index":0}]}"#),
    );
    let provider: std::sync::Arc<dyn mc_providers::EmbeddingProvider> = std::sync::Arc::new(
        mc_providers::openai::OpenAiCompatibleProvider::new(
            mc_providers::openai::EndpointConfig {
                base_url: "https://api.example.com/v1".to_string(),
                model: "embed-small".to_string(),
                api_key: Some("sk-test".to_string()),
                timeout: Duration::from_secs(5),
                max_image_edge: None,
                max_concurrency: 1,
            },
            transport,
            mc_providers::Role::Embedding,
        )
        .unwrap(),
    );
    ctx.state.set_embedding_provider(provider);

    let events = chat(&ctx.state, "设计协作工具").await;
    assert!(
        format!("{events:?}").contains("act-1"),
        "注入向量后，语义命中必须出现在引用里"
    );
}

// ---------------------------------------------------------------- 线索（5.36–5.39）

#[tokio::test]
async fn threads_endpoint_groups_activities_by_entity() {
    let ctx = ctx();
    // 两天的活动提到同一个 ticket（一次写入，覆盖式写入的坑）
    let projection = mc_domain::projector::Projection {
        activities: vec![
            mc_domain::activity::ActivityView {
                id: "act-1".to_string(),
                start: at(0),
                end: at(1800),
                title: "排查 APEX-389 的验收标准".to_string(),
                original_title: "排查 APEX-389 的验收标准".to_string(),
                category: Some("开发".to_string()),
                observations: vec![mc_domain::activity::ObservationRef {
                    id: "obs-1".to_string(),
                    at: at(0),
                }],
                origin: mc_domain::activity::Provenance::Rule {
                    rule_id: "coding".to_string(),
                },
                confidence: 1.0,
                is_user_modified: false,
            },
            mc_domain::activity::ActivityView {
                id: "act-2".to_string(),
                start: at(86_400),
                end: at(86_400 + 1800),
                title: "继续 APEX-389 的修复".to_string(),
                original_title: "继续 APEX-389 的修复".to_string(),
                category: Some("开发".to_string()),
                observations: vec![mc_domain::activity::ObservationRef {
                    id: "obs-2".to_string(),
                    at: at(86_400),
                }],
                origin: mc_domain::activity::Provenance::Inferred {
                    model: "stub".to_string(),
                },
                confidence: 0.8,
                is_user_modified: false,
            },
        ],
        noise: Vec::new(),
        ai_requests: 0,
        unknown_event_kinds: 0,
    };
    mc_storage::projectors::activities::store(ctx.state.db.as_ref(), &projection, 0, at(86_400))
        .expect("存活动");

    let response = router(Arc::clone(&ctx.state))
        .oneshot(get("/api/v1/threads"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();

    let threads = json["data"]["threads"].as_array().expect("线索数组");
    assert_eq!(threads.len(), 1, "{json}");
    assert_eq!(threads[0]["entity"]["canonical"], "APEX-389");
    assert_eq!(threads[0]["days"].as_array().unwrap().len(), 2);
    assert_eq!(threads[0]["observed_count"], 1);
    assert_eq!(
        threads[0]["inferred_count"], 1,
        "推断出来的活动要单独计数：{json}"
    );
    assert!(
        threads[0]["brief"]
            .as_str()
            .is_some_and(|brief| brief.contains("推测")),
        "进展里要标注推测：{json}"
    );
}

#[tokio::test]
async fn threads_endpoint_skips_blocked_activities() {
    let ctx = ctx();
    seed_activity(&ctx.state, "act-secret", "1Password 主密码 BUG-9", true);

    let response = router(Arc::clone(&ctx.state))
        .oneshot(get("/api/v1/threads"))
        .await
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();

    assert_eq!(
        json["data"]["threads"].as_array().unwrap().len(),
        0,
        "被拦截内容不该形成线索：{json}"
    );
}

// 落库是「收尾一次写」，推送才是逐段 —— 中间内容不能丢。
// 分段由 provider 的增量决定（服务端不再自己按字符数切块）：
// 本地兜底引擎只产生一个增量，因此这里断言「推送块拼接 == 落库内容 == 完整回答」。
#[tokio::test]
async fn streamed_answer_persists_every_chunk_once_at_the_end() {
    let ctx = ctx();
    let long_title = "把旧库的笔记与待办导进新版数据目录并且逐条核对字段映射";
    seed_activity(&ctx.state, "act-long", long_title, false);

    // 必须带 page_name：只有给了 conversation_id 或 page_name 时才会建对话并落库
    let events = chat_with_page(&ctx.state, long_title, "home").await;

    let chunks: Vec<String> = events
        .iter()
        .filter(|event| event["type"] == "stream_chunk")
        .filter_map(|event| event["content"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        chunks.len(),
        1,
        "本地兜底引擎只有一个增量：分块不再由服务端切（真流式下由模型增量决定）：{events:?}"
    );
    assert!(
        chunks[0].chars().count() > 24,
        "这段话比原来的切块阈值长，用来证明「不再按字符数切块」：{events:?}"
    );

    // 落库内容 = 推送的所有块拼起来（一次性写入不能少一段）
    let conversations = ctx
        .state
        .db
        .list_conversations(Some("home"), None)
        .expect("列对话");
    let messages = ctx
        .state
        .db
        .read_messages(conversations[0].id)
        .expect("读消息");
    let assistant = messages
        .iter()
        .find(|message| message.role == "assistant")
        .expect("助手消息要在");
    assert_eq!(
        assistant.content,
        chunks.concat(),
        "落库内容必须等于推送内容的拼接"
    );
    assert_eq!(assistant.status, "completed");
}

// ---------------------------------------------------------------- 真流式（引擎层）

/// 逐 token 吐字的假 provider：不碰网络，只验证「增量按发生顺序到达」。
struct StreamingProvider {
    deltas: Vec<&'static str>,
}

#[async_trait::async_trait]
impl mc_providers::Provider for StreamingProvider {
    fn id(&self) -> String {
        "streaming-fake".to_string()
    }

    fn role(&self) -> mc_providers::Role {
        mc_providers::Role::Chat
    }

    fn capabilities(&self) -> mc_providers::ProviderCapabilities {
        mc_providers::ProviderCapabilities::for_role(mc_providers::Role::Chat, 1)
    }

    async fn health(&self) -> mc_providers::ProviderHealth {
        mc_providers::ProviderHealth {
            healthy: true,
            message: None,
        }
    }

    fn to_app_error(
        &self,
        error: &mc_providers::error::ProviderError,
    ) -> mc_common::error::AppError {
        error.to_app_error()
    }
}

#[async_trait::async_trait]
impl mc_providers::ChatProvider for StreamingProvider {
    async fn complete(
        &self,
        _request: &mc_providers::ChatRequest,
    ) -> Result<mc_providers::ChatResponse, mc_providers::error::ProviderError> {
        panic!("流式路径不该回退到 complete")
    }

    async fn stream(
        &self,
        _request: &mc_providers::ChatRequest,
        on_delta: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<mc_providers::ChatResponse, mc_providers::error::ProviderError> {
        for delta in &self.deltas {
            on_delta(delta);
        }
        Ok(mc_providers::ChatResponse {
            text: self.deltas.concat(),
            model: "fake-model".to_string(),
            finish_reason: Some("stop".to_string()),
            usage: mc_providers::TokenUsage {
                prompt_tokens: 3,
                completion_tokens: 2,
            },
        })
    }
}

#[tokio::test]
async fn engine_streams_deltas_in_order_with_thinking_first() {
    use mc_server::chat::{ChatEngine, ChatEvent, ChatInput, ProviderChatEngine};

    let engine = ProviderChatEngine::new(
        std::sync::Arc::new(StreamingProvider {
            deltas: vec!["今天", "写了", "导入脚本"],
        }),
        mc_summary::model::SummaryLocale::ZhCn,
    );

    let events = std::sync::Mutex::new(Vec::new());
    let answer = engine
        .answer_stream(
            &ChatInput {
                query: "我今天做了什么？".to_string(),
                citations: Vec::new(),
                history: Vec::new(),
            },
            &mut |event: ChatEvent<'_>| {
                let label = match event {
                    ChatEvent::Delta(text) => format!("delta:{text}"),
                    ChatEvent::Thinking(note) => format!("thinking:{}", note.stage),
                };
                events.lock().unwrap().push(label);
            },
        )
        .await
        .expect("流式回答应当成功");

    let events = events.into_inner().unwrap();
    assert_eq!(
        events,
        vec![
            "thinking:retrieval".to_string(),
            "delta:今天".to_string(),
            "delta:写了".to_string(),
            "delta:导入脚本".to_string()
        ],
        "思考说明必须先到，随后是逐 token 增量（顺序即契约）"
    );
    assert_eq!(answer.text, "今天写了导入脚本");
    assert_eq!(answer.model.as_deref(), Some("fake-model"));
    assert_eq!(answer.citations.len(), 0);
}

// 笔记必须进检索语料：用户自己写的内容是最该被问答命中的
#[tokio::test]
async fn chat_cites_notes_written_by_the_user() {
    let ctx = ctx();
    // 一句只可能出现在笔记里的话
    ctx.state
        .db
        .insert_vault_row(
            &mc_storage::vaults::VaultUpsert {
                title: "季度复盘".to_string(),
                summary: "记录本季度的取舍".to_string(),
                content: "我们把「部署流水线改成分阶段灰度」这件事推迟到了下个季度。".to_string(),
                tags: vec!["复盘".to_string()],
                parent_id: None,
                is_folder: false,
                document_type: "vaults".to_string(),
                sort_order: 0,
            },
            at(10),
        )
        .expect("写入笔记");

    let hits = mc_server::retrieval::documents(&ctx.state.db).expect("读语料");
    assert!(
        hits.iter().any(|doc| doc.text.contains("分阶段灰度")),
        "笔记正文必须进检索语料：{:?}",
        hits.iter().map(|doc| doc.text.as_str()).collect::<Vec<_>>()
    );
}

// 文件夹没有正文，不该被当成可检索内容
#[tokio::test]
async fn folders_are_not_part_of_the_retrieval_corpus() {
    let ctx = ctx();
    ctx.state
        .db
        .insert_vault_row(
            &mc_storage::vaults::VaultUpsert {
                title: "归档文件夹".to_string(),
                summary: String::new(),
                content: String::new(),
                tags: Vec::new(),
                parent_id: None,
                is_folder: true,
                document_type: "vaults".to_string(),
                sort_order: 0,
            },
            at(10),
        )
        .expect("写入文件夹");

    let documents = mc_server::retrieval::documents(&ctx.state.db).expect("读语料");
    assert!(
        !documents.iter().any(|doc| doc.text.contains("归档文件夹")),
        "文件夹不该进语料"
    );
}

// 已删除的笔记不该还能被检索到
#[tokio::test]
async fn deleted_notes_leave_the_corpus() {
    let ctx = ctx();
    let id = ctx
        .state
        .db
        .insert_vault_row(
            &mc_storage::vaults::VaultUpsert {
                title: "临时草稿".to_string(),
                summary: String::new(),
                content: "这句话不该还能被搜到。".to_string(),
                tags: Vec::new(),
                parent_id: None,
                is_folder: false,
                document_type: "vaults".to_string(),
                sort_order: 0,
            },
            at(10),
        )
        .expect("写入笔记");
    ctx.state
        .db
        .soft_delete_vault_row(id, at(20))
        .expect("软删除");

    let documents = mc_server::retrieval::documents(&ctx.state.db).expect("读语料");
    assert!(
        !documents
            .iter()
            .any(|doc| doc.text.contains("不该还能被搜到")),
        "软删除的笔记必须离开语料"
    );
}

// 多轮上下文：历史必须进提示词，且顺序是旧 → 新，本次提问只出现一次
struct CapturingProvider {
    sent: std::sync::Mutex<Vec<(String, String)>>,
}

#[async_trait::async_trait]
impl mc_providers::Provider for CapturingProvider {
    fn id(&self) -> String {
        "capturing".to_string()
    }
    fn role(&self) -> mc_providers::Role {
        mc_providers::Role::Chat
    }
    fn capabilities(&self) -> mc_providers::ProviderCapabilities {
        mc_providers::ProviderCapabilities::for_role(mc_providers::Role::Chat, 1)
    }
    async fn health(&self) -> mc_providers::ProviderHealth {
        mc_providers::ProviderHealth {
            healthy: true,
            message: None,
        }
    }
    fn to_app_error(
        &self,
        error: &mc_providers::error::ProviderError,
    ) -> mc_common::error::AppError {
        error.to_app_error()
    }
}

#[async_trait::async_trait]
impl mc_providers::ChatProvider for CapturingProvider {
    async fn complete(
        &self,
        request: &mc_providers::ChatRequest,
    ) -> Result<mc_providers::ChatResponse, mc_providers::error::ProviderError> {
        *self.sent.lock().unwrap() = request
            .messages
            .iter()
            .map(|message| (message.role.clone(), message.content.clone()))
            .collect();
        Ok(mc_providers::ChatResponse {
            text: "好的".to_string(),
            model: "capturing".to_string(),
            finish_reason: Some("stop".to_string()),
            usage: mc_providers::TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
            },
        })
    }
}

#[tokio::test]
async fn history_is_sent_in_order_before_the_new_question() {
    use mc_server::chat::{ChatEngine, ChatInput, ChatTurn, ProviderChatEngine};

    let provider = std::sync::Arc::new(CapturingProvider {
        sent: std::sync::Mutex::new(Vec::new()),
    });
    let engine = ProviderChatEngine::new(
        std::sync::Arc::clone(&provider) as std::sync::Arc<dyn mc_providers::ChatProvider>,
        mc_summary::model::SummaryLocale::ZhCn,
    );

    engine
        .answer(&ChatInput {
            query: "那第二点呢？".to_string(),
            citations: Vec::new(),
            history: vec![
                ChatTurn {
                    role: "user".to_string(),
                    content: "总结一下今天的部署改动".to_string(),
                },
                ChatTurn {
                    role: "assistant".to_string(),
                    content: "今天改了两点：一是灰度发布，二是回滚脚本。".to_string(),
                },
            ],
        })
        .await
        .expect("回答");

    let sent = provider.sent.lock().unwrap().clone();
    let roles: Vec<&str> = sent.iter().map(|(role, _)| role.as_str()).collect();
    assert_eq!(
        roles,
        vec!["system", "user", "assistant", "user"],
        "顺序必须是 system → 历史（旧→新）→ 本次提问：{sent:?}"
    );
    assert_eq!(sent[1].1, "总结一下今天的部署改动");
    assert_eq!(sent[2].1, "今天改了两点：一是灰度发布，二是回滚脚本。");
    assert_eq!(sent[3].1, "那第二点呢？");
    assert_eq!(
        sent.iter()
            .filter(|(_, content)| content == "那第二点呢？")
            .count(),
        1,
        "本次提问只应出现一次：{sent:?}"
    );
}
