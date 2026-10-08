//! b：把**落库的向量**真正接进检索。
//!
//! 在这之前，向量只能写进库、读出来，检索仍然只走关键词 ——
//! 也就是说「语义检索」在真实运行里从未生效。这一组测试盯的就是这件事：
//!
//! - 语义命中：关键词完全不重合时也能搜到（这是向量存在的**唯一**理由）；
//! - 隐私失败即关闭：被拦截的活动连索引都进不去；
//! - 陌生 id 不进索引（索引里出现文档集之外的 id 说明数据不一致，
//!   宁可少搜也不能搜出不该出现的东西）；
//! - 查询向量拿不到时，退化回关键词而不是报错。

use std::sync::Arc;
use std::time::Duration;

use mc_common::time::Timestamp;
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::{EmbeddingProvider, Role};
use mc_search::{Embedding, EmbeddingSpace, VectorIndex};
use mc_server::{embedding, retrieval, ServerState};
use mc_storage::observations::NewObservation;
use mc_storage::vectors::{upsert_vectors, VectorRecord};
use mc_storage::Database;
use mc_testkit::provider::ScriptedTransport;

use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

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
        Timestamp::from_millis(T0),
        dir.path().to_path_buf(),
    ));
    Ctx { _dir: dir, state }
}

/// 写入一条活动（观测 + 事件 + 投影），返回活动 id。
fn add_activity(ctx: &Ctx, index: usize, app: &str, title: &str, verdict: &str) -> String {
    let at = T0 + (index as i64) * 45 * 60_000;
    let id = format!("obs-{index}");

    ctx.state
        .db
        .append_events(&[mc_storage::NewEvent::new(
            "observation.recorded",
            Timestamp::from_millis(at),
            serde_json::json!({
                "id": id,
                "at": at,
                "app_name": app,
                "window_title": title,
                "domain": null,
                "text": null,
                "privacy_verdict": verdict,
            }),
        )])
        .expect("追加事件");

    ctx.state
        .db
        .insert_observation(&NewObservation {
            id: id.clone(),
            ts: Timestamp::from_millis(at),
            source_id: "screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some(app.to_string()),
            app_bundle_id: None,
            window_title: Some(title.to_string()),
            domain: None,
            display_id: None,
            scale_factor: None,
            image: None,
            text_content: None,
            text_origin: None,
            change_kind: "new".to_string(),
            privacy_verdict: verdict.to_string(),
            phash: None,
            idempotency: format!("key-{index}"),
        })
        .expect("观测必须能落库");

    mc_storage::projectors::activities::replay(
        ctx.state.db.as_ref(),
        &mc_domain::rules::RuleSet::default(),
        mc_domain::projector::ProjectionOptions::default(),
        Timestamp::from_millis(at + 60_000),
    )
    .expect("投影必须成功");

    mc_storage::projectors::activities::read_all(&ctx.state.db)
        .unwrap()
        .into_iter()
        .find(|activity| activity.evidence.contains(&id))
        .map(|activity| activity.id)
        .expect("刚写入的活动必须能查到")
}

fn index_of(space: usize) -> VectorIndex {
    VectorIndex::new(EmbeddingSpace::new(space))
}

// ---------------------------------------------------------------- 语义命中

/// 关键词完全不重合时，向量是唯一能把它捞出来的东西。
#[test]
fn semantic_hit_is_found_when_keywords_do_not_overlap() {
    let ctx = ctx();
    // 没有活动规则时，活动标题退化为应用名（见 `mc-domain` 的兜底语义），
    // 所以「标题里含任务描述」这件事本身不可依赖 —— 关键词检索能命中什么，
    // 取决于应用名与规则产出的文本。
    let activity = add_activity(&ctx, 0, "Figma", "画布", "allowed");

    // 查询与「Figma」没有任何共同词元：关键词检索必然为空
    assert!(
        retrieval::retrieve(&ctx.state.db, "设计协作工具", 5, false)
            .unwrap()
            .is_empty(),
        "没有向量时，关键词不重合就搜不到 —— 这正是需要向量的场景"
    );

    upsert_vectors(
        &ctx.state.db,
        &[VectorRecord {
            kind: "activity".to_string(),
            doc_id: activity.clone(),
            model: "embed-small".to_string(),
            values: vec![1.0, 0.0],
        }],
        Timestamp::from_millis(T0),
    )
    .unwrap();

    let mut index = index_of(2);
    index
        .insert(&activity, Embedding::new(vec![1.0, 0.0]))
        .unwrap();

    let hits = retrieval::retrieve_with_vectors(
        &ctx.state.db,
        "认证故障",
        5,
        false,
        Some((&index, &Embedding::new(vec![1.0, 0.0]))),
    )
    .unwrap();

    assert_eq!(hits.len(), 1, "语义命中必须被检索到");
    assert_eq!(hits[0].document.id, activity);
}

/// `vector_index` 必须直接从库里组装出可用的索引（这是「接线」的定义）
#[test]
fn vector_index_is_built_from_persisted_vectors() {
    let ctx = ctx();
    let first = add_activity(
        &ctx,
        0,
        "Visual Studio Code",
        "重构 stage 状态机",
        "allowed",
    );
    let second = add_activity(&ctx, 1, "Google Chrome", "查 APEX-389 的文档", "allowed");

    upsert_vectors(
        &ctx.state.db,
        &[
            VectorRecord {
                kind: "activity".to_string(),
                doc_id: first.clone(),
                model: "embed-small".to_string(),
                values: vec![1.0, 0.0],
            },
            VectorRecord {
                kind: "activity".to_string(),
                doc_id: second.clone(),
                model: "embed-small".to_string(),
                values: vec![0.0, 1.0],
            },
        ],
        Timestamp::from_millis(T0),
    )
    .unwrap();

    let index = retrieval::vector_index(&ctx.state.db)
        .unwrap()
        .expect("有向量时必须产出索引");
    assert_eq!(index.len(), 2);
    assert_eq!(index.space().unwrap().dimensions(), 2);
}

#[test]
fn vector_index_is_none_when_nothing_is_persisted() {
    let ctx = ctx();
    add_activity(
        &ctx,
        0,
        "Visual Studio Code",
        "重构 stage 状态机",
        "allowed",
    );
    assert!(
        retrieval::vector_index(&ctx.state.db).unwrap().is_none(),
        "没有向量时返回 None → 检索自动退化为纯关键词"
    );
}

// ---------------------------------------------------------------- 隐私失败即关闭

/// 被拦截的活动连索引都不能进 —— 「不参与检索」必须包括向量检索。
#[test]
fn blocked_activities_are_absent_from_the_vector_index() {
    let ctx = ctx();
    let allowed = add_activity(
        &ctx,
        0,
        "Visual Studio Code",
        "重构 stage 状态机",
        "allowed",
    );
    let blocked = add_activity(&ctx, 1, "1Password", "私密钱包口令", "blocked");

    upsert_vectors(
        &ctx.state.db,
        &[
            VectorRecord {
                kind: "activity".to_string(),
                doc_id: allowed.clone(),
                model: "embed-small".to_string(),
                values: vec![1.0, 0.0],
            },
            VectorRecord {
                kind: "activity".to_string(),
                doc_id: blocked.clone(),
                model: "embed-small".to_string(),
                values: vec![1.0, 0.0],
            },
        ],
        Timestamp::from_millis(T0),
    )
    .unwrap();

    let index = retrieval::vector_index(&ctx.state.db).unwrap().unwrap();
    assert_eq!(index.len(), 1, "被拦截的活动不能进索引");

    // 即使查询向量与它完全一致，也搜不出任何东西
    let hits = retrieval::retrieve_with_vectors(
        &ctx.state.db,
        "口令",
        5,
        false,
        Some((&index, &Embedding::new(vec![1.0, 0.0]))),
    )
    .unwrap();
    assert!(
        hits.iter().all(|hit| hit.document.id != blocked),
        "被拦截的内容绝不能出现在结果里"
    );
}

/// 索引里出现文档集之外的 id（陈旧向量 / 脏数据）时，宁可少搜
#[test]
fn stale_vector_ids_do_not_enter_the_index() {
    let ctx = ctx();
    let activity = add_activity(
        &ctx,
        0,
        "Visual Studio Code",
        "重构 stage 状态机",
        "allowed",
    );

    upsert_vectors(
        &ctx.state.db,
        &[
            VectorRecord {
                kind: "activity".to_string(),
                doc_id: activity.clone(),
                model: "embed-small".to_string(),
                values: vec![1.0, 0.0],
            },
            VectorRecord {
                kind: "activity".to_string(),
                doc_id: "activity-that-no-longer-exists".to_string(),
                model: "embed-small".to_string(),
                values: vec![1.0, 0.0],
            },
        ],
        Timestamp::from_millis(T0),
    )
    .unwrap();

    let index = retrieval::vector_index(&ctx.state.db).unwrap().unwrap();
    assert_eq!(index.len(), 1, "陈旧 id 不得进索引");
}

// ---------------------------------------------------------------- 降级

#[test]
fn retrieve_with_empty_vector_index_behaves_like_keyword_only() {
    let ctx = ctx();
    add_activity(
        &ctx,
        0,
        "Visual Studio Code",
        "重构 stage 状态机",
        "allowed",
    );

    let index = index_of(2);
    let with_vectors = retrieval::retrieve_with_vectors(
        &ctx.state.db,
        "studio",
        5,
        false,
        Some((&index, &Embedding::new(vec![1.0, 0.0]))),
    )
    .unwrap();
    let keyword_only = retrieval::retrieve(&ctx.state.db, "studio", 5, false).unwrap();

    assert!(!with_vectors.is_empty());
    assert_eq!(
        with_vectors.len(),
        keyword_only.len(),
        "空索引不该改变关键词结果"
    );
}

/// 查询向量拿不到（未配置 / 限流 / 超时）→ 返回 None，检索退化，不报错
#[tokio::test]
async fn query_vectors_return_none_when_the_provider_fails() {
    let ctx = ctx();

    // 没配置 embedding：连 Provider 都没有
    assert!(
        embedding::query_vectors(&ctx.state, "重构").await.is_none(),
        "未配置时必须安静降级"
    );

    // 配置了但端点 429
    let transport = Arc::new(
        ScriptedTransport::new().push_json(429, r#"{"error":{"message":"rate limited"}}"#),
    );
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(
        OpenAiCompatibleProvider::new(
            EndpointConfig {
                base_url: "https://api.example.com/v1".to_string(),
                model: "embed-small".to_string(),
                api_key: Some("sk-test".to_string()),
                timeout: Duration::from_secs(5),
                max_image_edge: None,
                max_concurrency: 1,
            },
            transport,
            Role::Embedding,
        )
        .unwrap(),
    );
    ctx.state.set_embedding_provider(provider);

    assert!(
        embedding::query_vectors(&ctx.state, "重构").await.is_none(),
        "限流时检索必须退化为关键词，而不是报错"
    );
}

/// 注入的 Provider 正常时，查询向量与索引一起交给检索层
#[tokio::test]
async fn query_vectors_are_produced_and_usable() {
    let ctx = ctx();
    let activity = add_activity(&ctx, 0, "Figma", "画布", "allowed");

    let transport = Arc::new(
        ScriptedTransport::new().push_json(200, r#"{"data":[{"embedding":[0.0,1.0],"index":0}]}"#),
    );
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(
        OpenAiCompatibleProvider::new(
            EndpointConfig {
                base_url: "https://api.example.com/v1".to_string(),
                model: "embed-small".to_string(),
                api_key: Some("sk-test".to_string()),
                timeout: Duration::from_secs(5),
                max_image_edge: None,
                max_concurrency: 1,
            },
            transport,
            Role::Embedding,
        )
        .unwrap(),
    );
    ctx.state.set_embedding_provider(provider);

    upsert_vectors(
        &ctx.state.db,
        &[VectorRecord {
            kind: "activity".to_string(),
            doc_id: activity.clone(),
            model: "embed-small".to_string(),
            values: vec![0.0, 1.0],
        }],
        Timestamp::from_millis(T0),
    )
    .unwrap();

    let (index, query) = embedding::query_vectors(&ctx.state, "设计协作工具")
        .await
        .expect("Provider 正常时必须给出查询向量");
    assert_eq!(query.dimensions(), 2);

    let hits = retrieval::retrieve_with_vectors(
        &ctx.state.db,
        "设计协作工具",
        5,
        false,
        Some((&index, &query)),
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].document.id, activity);
}

// D5：语义索引缓存 —— 命中与失效都要可验证。
// 每次对话重建索引是 O(文档数 + 向量数) 的纯 CPU 开销，而向量只在索引任务跑完时
// 变化；失效键用「向量表版本戳（count + max(updated_at)）」，因此重算同一条文档
// 的向量也会让缓存失效（否则会把陈旧向量当新向量去检索）。
#[tokio::test]
async fn vector_index_cache_hits_and_invalidates() {
    let ctx = ctx();
    let activity = add_activity(&ctx, 0, "Figma", "画布", "allowed");

    upsert_vectors(
        &ctx.state.db,
        &[VectorRecord {
            kind: "activity".to_string(),
            doc_id: activity.clone(),
            model: "embed-small".to_string(),
            values: vec![1.0, 0.0],
        }],
        Timestamp::from_millis(T0),
    )
    .unwrap();

    let first = ctx
        .state
        .cached_vector_index()
        .expect("取索引")
        .expect("有向量后应当有索引");
    let second = ctx
        .state
        .cached_vector_index()
        .expect("取索引")
        .expect("第二次");
    assert!(
        std::sync::Arc::ptr_eq(&first, &second),
        "没有任何变化时必须命中缓存（否则等于没缓存）"
    );

    // 同一条文档重算向量：updated_at 变化 → 必须失效重建
    upsert_vectors(
        &ctx.state.db,
        &[VectorRecord {
            kind: "activity".to_string(),
            doc_id: activity,
            model: "embed-small".to_string(),
            values: vec![0.0, 1.0],
        }],
        Timestamp::from_millis(T0 + 60_000),
    )
    .unwrap();

    let third = ctx
        .state
        .cached_vector_index()
        .expect("取索引")
        .expect("仍应有索引");
    assert!(
        !std::sync::Arc::ptr_eq(&second, &third),
        "向量更新后缓存必须失效（陈旧索引会给出错的引用）"
    );
}
