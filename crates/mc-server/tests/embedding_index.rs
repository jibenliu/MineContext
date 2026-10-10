//! 、5.8：把活动索引进向量库的后台作业。
//!
//! 关键验收点（对应原始诉求「避免图片解析队列爆满」的同类风险）：
//! **向量化失败不能影响采集与投影**。所以这个作业：
//!
//! - 不在采集请求路径里，而是定时、有界地跑；
//! - 失败只写 `pipeline_failures` 与报告字段，采集链路照常；
//! - 已索引的活动会被跳过（不重复烧 token）；
//! - 每次请求的量都记进 `provider_calls(purpose='embedding')`。

use std::sync::Arc;
use std::time::Duration;

use mc_common::error::ErrorCode;
use mc_common::time::Timestamp;
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::{EmbeddingProvider, Role};
use mc_server::{embedding, ServerState};
use mc_storage::observations::NewObservation;
use mc_storage::Database;
use mc_testkit::provider::ScriptedTransport;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
}

fn provider(transport: Arc<ScriptedTransport>) -> Arc<dyn EmbeddingProvider> {
    Arc::new(
        OpenAiCompatibleProvider::new(
            EndpointConfig {
                base_url: "https://api.example.com/v1".to_string(),
                model: "embed-small".to_string(),
                api_key: Some("sk-test".to_string()),
                timeout: Duration::from_secs(5),
                max_image_edge: None,
                max_concurrency: 2,
            },
            transport,
            Role::Embedding,
        )
        .expect("Provider 必须可构造"),
    )
}

fn embedding_json(count: usize, prompt_tokens: u32) -> String {
    let data: Vec<serde_json::Value> = (0..count)
        .map(|index| serde_json::json!({ "embedding": [index as f32, 0.5], "index": index }))
        .collect();
    serde_json::json!({
        "data": data,
        "usage": { "prompt_tokens": prompt_tokens, "total_tokens": prompt_tokens },
    })
    .to_string()
}

#[tokio::test]
async fn worker_pauses_auth_failures_until_config_reload() {
    let transport = Arc::new(
        ScriptedTransport::new()
            .push_json(401, r#"{"error":{"message":"invalid key"}}"#)
            .push_json(200, embedding_json(1, 3)),
    );
    let ctx = ctx(Arc::clone(&transport));
    seed(&ctx, 1);
    let worker = embedding::spawn_worker_with(
        Arc::clone(&ctx.state),
        provider(Arc::clone(&transport)),
        "embed-small".into(),
        64,
        Duration::from_millis(10),
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let before_reload = transport.call_count();
    let pause = ctx
        .state
        .indexing_pause()
        .expect("401 后必须暴露索引暂停原因");
    assert_eq!(pause.code, "api_key_invalid");
    assert!(
        pause.message.contains("暂停"),
        "暂停文案要给人看：{}",
        pause.message
    );
    assert!(
        ctx.state.embedding_auth_paused(),
        "401 后 health/托盘应能读到索引暂停"
    );
    ctx.state
        .config
        .reload(&mc_config::load::LoadRequest::default());
    tokio::time::sleep(Duration::from_millis(200)).await;
    worker.abort();
    let _ = worker.await;
    assert_eq!(before_reload, 1, "401 后不应按轮询频率反复重试");
    assert_eq!(transport.call_count(), 2, "配置重载后恢复索引");
    assert!(
        ctx.state.indexing_pause().is_none() && !ctx.state.embedding_auth_paused(),
        "配置重载后应清除索引暂停标志"
    );
}

#[tokio::test]
async fn worker_pauses_rate_limit_until_one_click_resume() {
    let transport = Arc::new(
        ScriptedTransport::new()
            .push_json(429, r#"{"error":{"message":"rate limited"}}"#)
            .push_json(200, embedding_json(1, 3)),
    );
    let ctx = ctx(Arc::clone(&transport));
    seed(&ctx, 1);
    let worker = embedding::spawn_worker_with(
        Arc::clone(&ctx.state),
        provider(Arc::clone(&transport)),
        "embed-small".into(),
        64,
        Duration::from_millis(10),
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    let before_resume = transport.call_count();
    assert_eq!(before_resume, 1, "429 后不应按轮询频率反复重试");
    let pause = ctx
        .state
        .indexing_pause()
        .expect("429 后必须暴露索引暂停原因");
    assert_eq!(pause.code, "provider_rate_limited");

    ctx.state.request_indexing_resume();
    tokio::time::sleep(Duration::from_millis(200)).await;
    worker.abort();
    let _ = worker.await;
    assert_eq!(transport.call_count(), 2, "一键恢复后应立刻重试索引");
    assert!(
        ctx.state.indexing_pause().is_none(),
        "恢复成功后暂停态应清空"
    );
}

/// 写入若干观测并投影成活动。
///
/// 观测之间**必须**用不同应用 + 大于空闲阈值的间隔，否则它们会被聚合成
/// 一个活动 —— 这是测试夹具陷阱。
fn seed(ctx: &Ctx, count: usize) {
    const APPS: [&str; 3] = ["Visual Studio Code", "Google Chrome", "Terminal"];
    const GAP_MS: i64 = 45 * 60_000;

    for index in 0..count {
        let at = T0 + (index as i64) * GAP_MS;
        let app = APPS[index % APPS.len()];
        let title = format!("stage.rs — 修复 APEX-{index}");

        ctx.state
            .db
            .append_events(&[mc_storage::NewEvent::new(
                "observation.recorded",
                Timestamp::from_millis(at),
                serde_json::json!({
                    "id": format!("obs-{index}"),
                    "at": at,
                    "app_name": app,
                    "window_title": title,
                    "domain": null,
                    "text": null,
                }),
            )])
            .expect("追加事件");

        ctx.state
            .db
            .insert_observation(&NewObservation {
                id: format!("obs-{index}"),
                ts: Timestamp::from_millis(at),
                source_id: "screen".to_string(),
                kind: "screen".to_string(),
                app_name: Some(app.to_string()),
                app_bundle_id: None,
                window_title: Some(title),
                domain: None,
                display_id: None,
                scale_factor: None,
                image: None,
                text_content: None,
                text_origin: None,
                change_kind: "new".to_string(),
                privacy_verdict: "allowed".to_string(),
                phash: None,
                idempotency: format!("key-{index}"),
            })
            .expect("观测必须能落库");
    }

    mc_storage::projectors::activities::replay(
        ctx.state.db.as_ref(),
        &mc_domain::rules::RuleSet::default(),
        mc_domain::projector::ProjectionOptions::default(),
        Timestamp::from_millis(T0 + 8 * 3_600_000),
    )
    .expect("投影必须成功");

    assert_eq!(
        mc_storage::projectors::activities::read_all(&ctx.state.db)
            .unwrap()
            .len(),
        count,
        "夹具必须产出 {count} 个独立活动，否则后续断言没有意义"
    );
}

fn ctx(transport: Arc<ScriptedTransport>) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default()).unwrap();
    let state = Arc::new(ServerState::new(
        mc_config::ConfigHandle::new(config),
        db,
        "test-token".to_string(),
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    ));
    let _ = &transport;
    Ctx { _dir: dir, state }
}

// ---------------------------------------------------------------- 5.4 索引活动

#[tokio::test]
async fn indexes_activities_and_accounts_embedding_usage() {
    let transport = Arc::new(ScriptedTransport::new().reply_with(|request| {
        // 按请求里的 `input` 条数返回同样多的向量
        let body: serde_json::Value =
            serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
        let count = body["input"].as_array().map(|a| a.len()).unwrap_or(0);
        embedding_json(count, 12)
    }));
    let ctx = ctx(Arc::clone(&transport));
    seed(&ctx, 3);

    let provider = provider(Arc::clone(&transport));
    let report = embedding::index_pending_activities(
        &ctx.state,
        provider.as_ref(),
        "embed-small",
        64,
        Timestamp::from_millis(T0 + 3_700_000),
    )
    .await
    .expect("索引本身不该返回 Err（失败在报告里）");

    assert!(report.failure.is_none(), "不该失败：{:?}", report.failure);
    assert_eq!(report.indexed, 3);
    assert_eq!(
        mc_storage::vectors::count_vectors(&ctx.state.db, "activity").unwrap(),
        3
    );

    // 用量必须落库，否则 embedding 的成本依旧不可见
    let usage = ctx
        .state
        .db
        .provider_usage(
            Timestamp::from_millis(T0),
            Timestamp::from_millis(T0 + 7_200_000),
        )
        .unwrap();
    assert_eq!(usage.calls, 1);
    assert_eq!(usage.prompt_tokens, 12);
    assert_eq!(usage.by_purpose.len(), 1);
    assert_eq!(usage.by_purpose[0].purpose, "embedding");
}

// 已索引的活动不重复请求 —— 这是「重建几万条向量」不会每轮重烧 token 的前提
#[tokio::test]
async fn already_indexed_activities_are_skipped() {
    let transport = Arc::new(ScriptedTransport::new().reply_with(|request| {
        let body: serde_json::Value =
            serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
        let count = body["input"].as_array().map(|a| a.len()).unwrap_or(0);
        embedding_json(count, 5)
    }));
    let ctx = ctx(Arc::clone(&transport));
    seed(&ctx, 2);
    let provider = provider(Arc::clone(&transport));

    let first = embedding::index_pending_activities(
        &ctx.state,
        provider.as_ref(),
        "embed-small",
        64,
        Timestamp::from_millis(T0 + 3_700_000),
    )
    .await
    .unwrap();
    assert_eq!(first.indexed, 2);
    let calls_after_first = transport.call_count();

    let second = embedding::index_pending_activities(
        &ctx.state,
        provider.as_ref(),
        "embed-small",
        64,
        Timestamp::from_millis(T0 + 3_800_000),
    )
    .await
    .unwrap();

    assert_eq!(second.indexed, 0);
    assert_eq!(second.skipped, 2);
    assert_eq!(
        transport.call_count(),
        calls_after_first,
        "全部已索引时不该再发请求"
    );
}

// 5.4 的核心断言：模型 429 时，采集与投影必须照常
#[tokio::test]
async fn embedding_failure_does_not_disturb_capture_or_projection() {
    let transport = Arc::new(
        ScriptedTransport::new().push_json(429, r#"{"error":{"message":"rate limited"}}"#),
    );
    let ctx = ctx(Arc::clone(&transport));
    seed(&ctx, 2);

    let before = ctx.state.db.observation_count().unwrap();
    let activities_before = mc_storage::projectors::activities::read_all(&ctx.state.db)
        .unwrap()
        .len();

    let provider = provider(Arc::clone(&transport));
    let report = embedding::index_pending_activities(
        &ctx.state,
        provider.as_ref(),
        "embed-small",
        64,
        Timestamp::from_millis(T0 + 3_700_000),
    )
    .await
    .expect("限流不是致命错误");

    assert_eq!(
        report.failure.as_ref().map(|f| f.code()),
        Some(ErrorCode::ProviderRateLimited)
    );
    assert_eq!(report.indexed, 0);
    assert_eq!(
        mc_storage::vectors::count_vectors(&ctx.state.db, "activity").unwrap(),
        0
    );

    // 采集与投影完全没受影响
    assert_eq!(ctx.state.db.observation_count().unwrap(), before);
    assert_eq!(
        mc_storage::projectors::activities::read_all(&ctx.state.db)
            .unwrap()
            .len(),
        activities_before
    );

    // 失败要被记录下来（否则用户只看到「搜不到东西」）
    let (component, code): (String, String) = ctx
        .state
        .db
        .with_read(|conn| {
            conn.query_row(
                "SELECT component, error_code FROM pipeline_failures ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .expect("embedding 失败必须写进 pipeline_failures");
    assert_eq!(component, "embedding");
    assert_eq!(code, "provider_rate_limited");
}

// 换 embedding 模型（维度变了）→ 明确错误 + 不写坏数据
#[tokio::test]
async fn dimension_change_is_reported_and_writes_nothing() {
    let transport = Arc::new(ScriptedTransport::new().reply_with(|request| {
        let body: serde_json::Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
        let count = body["input"].as_array().map(|a| a.len()).unwrap_or(0);
        // 3 维，而库里已经声明了 2 维
        let data: Vec<serde_json::Value> = (0..count)
            .map(|index| serde_json::json!({ "embedding": [index as f32, 0.5, 0.1], "index": index }))
            .collect();
        serde_json::json!({ "data": data }).to_string()
    }));
    let ctx = ctx(Arc::clone(&transport));
    seed(&ctx, 2);

    // 先建一个 2 维的向量空间
    mc_storage::vectors::upsert_vectors(
        &ctx.state.db,
        &[mc_storage::vectors::VectorRecord {
            kind: "activity".to_string(),
            doc_id: "legacy-doc".to_string(),
            model: "embed-old".to_string(),
            values: vec![1.0, 0.0],
        }],
        Timestamp::from_millis(T0),
    )
    .unwrap();

    let provider = provider(Arc::clone(&transport));
    let error = embedding::index_pending_activities(
        &ctx.state,
        provider.as_ref(),
        "embed-small",
        64,
        Timestamp::from_millis(T0 + 3_700_000),
    )
    .await
    .expect_err("维度不一致必须显式报错，而不是静默跳过");

    assert_eq!(error.code(), ErrorCode::StorageEmbeddingDimensionMismatch);
    assert_eq!(
        mc_storage::vectors::count_vectors(&ctx.state.db, "activity").unwrap(),
        1,
        "旧的向量不能被半截数据覆盖"
    );
}

// 没配置 embedding（用户没填模型）时，不是错误，也不该发请求
#[tokio::test]
async fn unconfigured_embedding_provider_is_not_an_error() {
    let config = mc_config::load::load(&mc_config::load::LoadRequest::default())
        .unwrap()
        .config;
    let secrets = mc_providers::credentials::StaticSecretStore::default();

    let provider = embedding::build_embedding_provider(&config, &secrets).expect("组装不该失败");
    assert!(
        provider.is_none(),
        "默认配置里 embedding 是空的 → 返回 None，而不是报错"
    );
}

// 配置齐全时才组装出 Provider，且模型名来自配置
#[tokio::test]
async fn embedding_provider_is_built_when_configured() {
    let mut config = mc_config::load::load(&mc_config::load::LoadRequest::default())
        .unwrap()
        .config;
    config.ai.embedding.base_url = "https://api.example.com/v1".to_string();
    config.ai.embedding.model = "embed-small".to_string();

    let secrets = mc_providers::credentials::StaticSecretStore::default();
    let provider = embedding::build_embedding_provider(&config, &secrets)
        .expect("组装不该失败")
        .expect("配置齐全时必须给出 Provider");
    assert_eq!(provider.role(), Role::Embedding);
}

// ---------------------------------------------------------------- 5.4 不阻塞采集

/// 一个永远不返回的 Provider：模拟「模型卡住 / 网络黑洞」。
///
/// 这是最坏情况，也是最容易挂掉的场景。
struct HangingProvider {
    inner: OpenAiCompatibleProvider,
}

#[async_trait::async_trait]
impl mc_providers::Provider for HangingProvider {
    fn id(&self) -> String {
        self.inner.id()
    }
    fn role(&self) -> Role {
        Role::Embedding
    }
    fn capabilities(&self) -> mc_providers::ProviderCapabilities {
        self.inner.capabilities()
    }
    async fn health(&self) -> mc_providers::ProviderHealth {
        mc_providers::ProviderHealth {
            healthy: false,
            message: Some("hanging".to_string()),
        }
    }
    fn to_app_error(&self, error: &mc_providers::ProviderError) -> mc_common::error::AppError {
        self.inner.to_app_error(error)
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for HangingProvider {
    async fn embed(
        &self,
        _inputs: &[String],
    ) -> Result<mc_providers::EmbeddingResponse, mc_providers::ProviderError> {
        std::future::pending::<()>().await;
        unreachable!("永远不会返回")
    }

    async fn probe_dimensions(&self) -> Result<usize, mc_providers::ProviderError> {
        Ok(2)
    }
}

/// 索引循环卡住时，采集与投影必须照常工作。
///
/// 这条测试是「图片解析队列爆满」同类风险在语义索引上的翻版：
/// **一个卡住的 AI 任务不允许冻结主链路**。
#[tokio::test]
async fn a_hanging_indexer_never_blocks_capture_or_projection() {
    let transport = Arc::new(ScriptedTransport::new());
    let ctx = ctx(Arc::clone(&transport));
    seed(&ctx, 2);

    let hanging = Arc::new(HangingProvider {
        inner: OpenAiCompatibleProvider::new(
            EndpointConfig {
                base_url: "https://api.example.com/v1".to_string(),
                model: "embed-small".to_string(),
                api_key: Some("sk-test".to_string()),
                timeout: Duration::from_secs(3600),
                max_image_edge: None,
                max_concurrency: 1,
            },
            transport,
            Role::Embedding,
        )
        .unwrap(),
    });

    let indexer = embedding::spawn_worker_with(
        Arc::clone(&ctx.state),
        hanging,
        "embed-small".to_string(),
        64,
        Duration::from_millis(10),
    );

    // 让索引循环进入「卡在 embed 里」的状态
    tokio::time::sleep(Duration::from_millis(120)).await;

    // 采集与投影必须继续
    let at = T0 + 9 * 3_600_000;
    ctx.state
        .db
        .insert_observation(&NewObservation {
            id: "obs-late".to_string(),
            ts: Timestamp::from_millis(at),
            source_id: "screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some("Preview".to_string()),
            app_bundle_id: None,
            window_title: Some("README.md".to_string()),
            domain: None,
            display_id: None,
            scale_factor: None,
            image: None,
            text_content: None,
            text_origin: None,
            change_kind: "new".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: "late-key".to_string(),
        })
        .expect("采集写入不能被索引拖住");

    assert_eq!(ctx.state.db.observation_count().unwrap(), 3);
    mc_storage::projectors::activities::replay(
        ctx.state.db.as_ref(),
        &mc_domain::rules::RuleSet::default(),
        mc_domain::projector::ProjectionOptions::default(),
        Timestamp::from_millis(at + 60_000),
    )
    .expect("投影不能被索引拖住");
    assert_eq!(
        mc_storage::projectors::activities::read_all(&ctx.state.db)
            .unwrap()
            .len(),
        3
    );

    indexer.abort();
}

#[tokio::test]
async fn indexes_vault_notes_as_document_vectors() {
    let transport = Arc::new(ScriptedTransport::new().reply_with(|request| {
        let body: serde_json::Value =
            serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
        let count = body["input"].as_array().map(|a| a.len()).unwrap_or(0);
        embedding_json(count, 7)
    }));
    let ctx = ctx(Arc::clone(&transport));
    let note_id = ctx
        .state
        .db
        .insert_vault_row(
            &mc_storage::vaults::VaultUpsert {
                title: "季度复盘".into(),
                summary: "关键结论".into(),
                content: "本季度重点是检索准确率".into(),
                tags: vec![],
                parent_id: None,
                is_folder: false,
                document_type: "vaults".into(),
                sort_order: 0,
            },
            Timestamp::from_millis(T0),
        )
        .expect("笔记必须能落库");

    let provider = provider(Arc::clone(&transport));
    let report = embedding::index_pending_notes(
        &ctx.state,
        provider.as_ref(),
        "embed-small",
        64,
        Timestamp::from_millis(T0 + 60_000),
    )
    .await
    .expect("笔记索引不该返回 Err");

    assert!(report.failure.is_none(), "{:?}", report.failure);
    assert_eq!(report.indexed, 1);
    assert_eq!(
        mc_storage::vectors::count_vectors(&ctx.state.db, "document").unwrap(),
        1
    );

    let index = mc_server::retrieval::vector_index(&ctx.state.db)
        .expect("组装索引")
        .expect("必须有向量");
    assert_eq!(index.len(), 1, "笔记向量必须进入可检索索引");
    let docs = mc_storage::vectors::load_vectors(&ctx.state.db, "document").unwrap();
    assert_eq!(docs[0].doc_id, format!("note-{note_id}"));

    let again = embedding::index_pending_notes(
        &ctx.state,
        provider.as_ref(),
        "embed-small",
        64,
        Timestamp::from_millis(T0 + 120_000),
    )
    .await
    .unwrap();
    assert_eq!(again.indexed, 0, "已索引的笔记不应重复烧 token");
}

#[tokio::test]
async fn indexes_summaries_as_summary_vectors() {
    let transport = Arc::new(ScriptedTransport::new().reply_with(|request| {
        let body: serde_json::Value =
            serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
        let count = body["input"].as_array().map(|a| a.len()).unwrap_or(0);
        embedding_json(count, 9)
    }));
    let ctx = ctx(Arc::clone(&transport));
    ctx.state
        .db
        .insert_summary(
            &mc_storage::projectors::summaries::NewSummary {
                id: "sum-1".into(),
                kind: "adhoc".into(),
                stage_id: None,
                template_id: "work".into(),
                start: Timestamp::from_millis(T0),
                end: Timestamp::from_millis(T0 + 3_600_000),
                title: "上午开发".into(),
                fields: Default::default(),
                body_markdown: "完成了检索准确率相关改动".into(),
                quality: "fallback".into(),
                model: None,
                prompt_tokens: 0,
                completion_tokens: 0,
                scope: None,
            },
            Timestamp::from_millis(T0 + 3_600_000),
        )
        .expect("总结必须能落库");

    let provider = provider(Arc::clone(&transport));
    let report = embedding::index_pending_summaries(
        &ctx.state,
        provider.as_ref(),
        "embed-small",
        64,
        Timestamp::from_millis(T0 + 3_700_000),
    )
    .await
    .expect("总结索引不该返回 Err");

    assert!(report.failure.is_none(), "{:?}", report.failure);
    assert_eq!(report.indexed, 1);
    assert_eq!(
        mc_storage::vectors::count_vectors(&ctx.state.db, "summary").unwrap(),
        1
    );

    let index = mc_server::retrieval::vector_index(&ctx.state.db)
        .expect("组装索引")
        .expect("必须有向量");
    assert_eq!(index.len(), 1, "总结向量必须进入可检索索引");
    let docs = mc_storage::vectors::load_vectors(&ctx.state.db, "summary").unwrap();
    assert_eq!(docs[0].doc_id, "sum-1");

    let again = embedding::index_pending_summaries(
        &ctx.state,
        provider.as_ref(),
        "embed-small",
        64,
        Timestamp::from_millis(T0 + 3_800_000),
    )
    .await
    .unwrap();
    assert_eq!(again.indexed, 0, "已索引的总结不应重复烧 token");
}
