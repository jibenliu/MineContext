//! 向量索引作业。
//!
//! 「embedding 绝不能拖死采集」的落地点：
//!
//! - **不在采集请求路径里**：采集只写观测，索引是后台、有界、可跳过的；
//! - 按「库里还没有向量的活动」增量索引，中断后天然可续；
//! - 失败写 `pipeline_failures` 并落在报告里，采集/投影/总结不受影响。
//!
//! 唯一显式返回 `Err` 的情况是**向量维度与已建索引不一致**：那是用户换了
//! 模型，必须让用户看见 —— 静默跳过只会让检索从此悄悄返回空结果。

use std::sync::Arc;

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use mc_pipeline::embedding::{embed_documents, EmbeddingDocument};
use mc_providers::credentials::{resolve_secret, SecretStore};
use mc_providers::openai::{EndpointConfig, OpenAiCompatibleProvider};
use mc_providers::transport::ReqwestTransport;
use mc_providers::{EmbeddingProvider, Role};
use mc_storage::provider_calls::{ProviderCall, Purpose};
use mc_storage::vectors::{count_vectors, upsert_vectors, VectorRecord};

use crate::failures::record_embedding_failure;
use crate::state::ServerState;

/// 索引作业的报告。`failure` 有值表示「这一轮只索引了一部分」。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmbeddingIndexReport {
    pub indexed: usize,
    pub skipped: usize,
    pub requests: usize,
    pub prompt_tokens: u64,
    pub failure: Option<AppError>,
}

impl EmbeddingIndexReport {
    pub fn is_success(&self) -> bool {
        self.failure.is_none()
    }
}

/// 按配置组装 embedding Provider。
///
/// 返回 `Ok(None)` 表示「不做语义检索」而不是「出错」：没配模型、
/// 关了 AI 都属正常状态 —— 关键词检索照常工作（降级要求见 `docs/api-and-frontend.md`）。
pub fn build_embedding_provider(
    config: &mc_config::Config,
    secrets: &dyn SecretStore,
) -> Result<Option<Arc<dyn EmbeddingProvider>>, AppError> {
    if !config.ai.enabled {
        return Ok(None);
    }

    let endpoint = &config.ai.embedding;
    if endpoint.base_url.trim().is_empty() || endpoint.model.trim().is_empty() {
        return Ok(None);
    }

    let api_key = resolve_secret(secrets, endpoint.api_key_ref.as_deref())?;

    let provider = OpenAiCompatibleProvider::new(
        EndpointConfig {
            base_url: endpoint.base_url.clone(),
            model: endpoint.model.clone(),
            api_key,
            timeout: std::time::Duration::from_secs(endpoint.timeout_secs),
            max_image_edge: None,
            max_concurrency: endpoint.max_concurrency,
        },
        Arc::new(ReqwestTransport::default()),
        Role::Embedding,
    )?;

    Ok(Some(Arc::new(provider)))
}

/// 一条活动的可检索文本。
///
/// 只用**已经落库的展示内容**（标题 + 类别），不做任何推测性扩写：
/// 索引的文本会被引用给用户看，凭空生成的内容等于伪造证据。
pub fn activity_text(activity: &mc_storage::projectors::activities::StoredActivity) -> String {
    match activity.category.as_deref().filter(|c| !c.is_empty()) {
        Some(category) => format!("{} {}", activity.title, category),
        None => activity.title.clone(),
    }
}

/// 把「还没有向量」的活动补上向量。
///
/// `at` 用于记账时间（可注入，便于测试与重放）。
pub async fn index_pending_activities(
    state: &ServerState,
    provider: &dyn EmbeddingProvider,
    model: &str,
    batch_limit: usize,
    at: Timestamp,
) -> Result<EmbeddingIndexReport, AppError> {
    let activities = mc_storage::projectors::activities::read_all(&state.db)?;
    let existing: std::collections::BTreeSet<String> = state.db.with_read(|conn| {
        let mut stmt = conn.prepare("SELECT doc_id FROM vectors WHERE kind = 'activity'")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<std::collections::BTreeSet<String>, _>>()
    })?;

    let pending: Vec<&mc_storage::projectors::activities::StoredActivity> = activities
        .iter()
        .filter(|activity| !existing.contains(&activity.id))
        .collect();

    let mut report = EmbeddingIndexReport {
        skipped: existing.len().min(activities.len()),
        ..Default::default()
    };
    if pending.is_empty() {
        return Ok(report);
    }

    let documents: Vec<EmbeddingDocument> = pending
        .iter()
        .map(|activity| EmbeddingDocument {
            kind: "activity".to_string(),
            doc_id: activity.id.clone(),
            text: activity_text(activity),
        })
        .collect();

    let outcome = embed_documents(provider, model, &documents, batch_limit).await;

    report.requests = outcome.calls.len();
    report.prompt_tokens = outcome.total_prompt_tokens();

    // 记账：成功的批次也要计时，否则「便宜但很慢」看不出来
    for call in &outcome.calls {
        let _ = state.db.record_provider_call(&ProviderCall {
            at,
            provider_id: format!("openai_compatible:{model}"),
            model: call.model.clone(),
            purpose: Purpose::Embedding,
            observation_id: None,
            stage_id: None,
            prompt_tokens: call.prompt_tokens,
            completion_tokens: 0,
            latency_ms: call.latency_ms,
            result: call.result,
            error_code: call.error_code.clone(),
        });
    }

    if !outcome.vectors.is_empty() {
        let records: Vec<VectorRecord> = outcome
            .vectors
            .iter()
            .map(|vector| VectorRecord {
                kind: vector.kind.clone(),
                doc_id: vector.doc_id.clone(),
                model: vector.model.clone(),
                values: vector.values.clone(),
            })
            .collect();

        // 维度不一致时这里返回可执行的错误（提示重建索引）——
        // 这是唯一需要用户介入的情况，不能降级成「静默没索引」。
        let written = upsert_vectors(&state.db, &records, at)?;
        report.indexed = written;
    }

    if let Some(failure) = outcome.failure {
        // 诊断页要能看到「语义检索为什么没建起来」，而不是只剩一行日志
        state.db.record_failure(at, "embedding", &failure, "warn")?;
        report.failure = Some(failure);
    }

    Ok(report)
}

/// 笔记的可检索文本（与 `retrieval::documents` 对齐：标题 + 摘要 + 正文前 4000 字）。
pub fn note_text(note: &mc_storage::vaults::VaultRow) -> String {
    let mut text = note.title.clone();
    if !note.summary.trim().is_empty() {
        text.push(' ');
        text.push_str(note.summary.trim());
    }
    let content = note.content.trim();
    if !content.is_empty() {
        text.push(' ');
        text.push_str(&content.chars().take(4000).collect::<String>());
    }
    text
}

/// 把「还没有向量」的笔记补上向量（`kind=document`，`doc_id=note-{id}`）。
///
/// 笔记是问答最该命中的一类；只索引活动会让同义改写的笔记检索退化为关键词。
pub async fn index_pending_notes(
    state: &ServerState,
    provider: &dyn EmbeddingProvider,
    model: &str,
    batch_limit: usize,
    at: Timestamp,
) -> Result<EmbeddingIndexReport, AppError> {
    let notes = state.db.query_vault_rows(&mc_storage::vaults::VaultQuery {
        document_type: Vec::new(),
        parent_id: None,
        title: None,
        is_folder: Some(0),
        is_deleted: None,
    })?;
    let existing: std::collections::BTreeSet<String> = state.db.with_read(|conn| {
        let mut stmt = conn.prepare("SELECT doc_id FROM vectors WHERE kind = 'document'")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<std::collections::BTreeSet<_>, _>>()
    })?;

    let pending: Vec<&mc_storage::vaults::VaultRow> = notes
        .iter()
        .filter(|note| {
            let doc_id = format!("note-{}", note.id);
            !existing.contains(&doc_id) && !note_text(note).trim().is_empty()
        })
        .collect();

    let mut report = EmbeddingIndexReport {
        skipped: existing.len().min(notes.len()),
        ..Default::default()
    };
    if pending.is_empty() {
        return Ok(report);
    }

    let documents: Vec<EmbeddingDocument> = pending
        .iter()
        .map(|note| EmbeddingDocument {
            kind: "document".to_string(),
            doc_id: format!("note-{}", note.id),
            text: note_text(note),
        })
        .collect();

    let outcome = embed_documents(provider, model, &documents, batch_limit).await;

    report.requests = outcome.calls.len();
    report.prompt_tokens = outcome.total_prompt_tokens();

    for call in &outcome.calls {
        let _ = state.db.record_provider_call(&ProviderCall {
            at,
            provider_id: format!("openai_compatible:{model}"),
            model: call.model.clone(),
            purpose: Purpose::Embedding,
            observation_id: None,
            stage_id: None,
            prompt_tokens: call.prompt_tokens,
            completion_tokens: 0,
            latency_ms: call.latency_ms,
            result: call.result,
            error_code: call.error_code.clone(),
        });
    }

    if !outcome.vectors.is_empty() {
        let records: Vec<VectorRecord> = outcome
            .vectors
            .iter()
            .map(|vector| VectorRecord {
                kind: vector.kind.clone(),
                doc_id: vector.doc_id.clone(),
                model: vector.model.clone(),
                values: vector.values.clone(),
            })
            .collect();
        let written = upsert_vectors(&state.db, &records, at)?;
        report.indexed = written;
    }

    if let Some(failure) = outcome.failure {
        state.db.record_failure(at, "embedding", &failure, "warn")?;
        report.failure = Some(failure);
    }

    Ok(report)
}

/// 总结的可检索文本（与 `retrieval::documents` 对齐：标题 + 正文）。
pub fn summary_text(summary: &mc_storage::projectors::summaries::StoredSummary) -> String {
    format!("{} {}", summary.title, summary.body_markdown)
}

/// 把「还没有向量」的总结补上向量（`kind=summary`，`doc_id=summary.id`）。
///
/// 总结往往含「做了什么」的原话；只靠关键词会漏掉同义改写的问答命中。
pub async fn index_pending_summaries(
    state: &ServerState,
    provider: &dyn EmbeddingProvider,
    model: &str,
    batch_limit: usize,
    at: Timestamp,
) -> Result<EmbeddingIndexReport, AppError> {
    let summaries = state.db.read_summaries(None, None)?;
    let existing: std::collections::BTreeSet<String> = state.db.with_read(|conn| {
        let mut stmt = conn.prepare("SELECT doc_id FROM vectors WHERE kind = 'summary'")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<std::collections::BTreeSet<_>, _>>()
    })?;

    let pending: Vec<&mc_storage::projectors::summaries::StoredSummary> = summaries
        .iter()
        .filter(|summary| {
            !existing.contains(&summary.id) && !summary_text(summary).trim().is_empty()
        })
        .collect();

    let mut report = EmbeddingIndexReport {
        skipped: existing.len().min(summaries.len()),
        ..Default::default()
    };
    if pending.is_empty() {
        return Ok(report);
    }

    let documents: Vec<EmbeddingDocument> = pending
        .iter()
        .map(|summary| EmbeddingDocument {
            kind: "summary".to_string(),
            doc_id: summary.id.clone(),
            text: summary_text(summary),
        })
        .collect();

    let outcome = embed_documents(provider, model, &documents, batch_limit).await;

    report.requests = outcome.calls.len();
    report.prompt_tokens = outcome.total_prompt_tokens();

    for call in &outcome.calls {
        let _ = state.db.record_provider_call(&ProviderCall {
            at,
            provider_id: format!("openai_compatible:{model}"),
            model: call.model.clone(),
            purpose: Purpose::Embedding,
            observation_id: None,
            stage_id: None,
            prompt_tokens: call.prompt_tokens,
            completion_tokens: 0,
            latency_ms: call.latency_ms,
            result: call.result,
            error_code: call.error_code.clone(),
        });
    }

    if !outcome.vectors.is_empty() {
        let records: Vec<VectorRecord> = outcome
            .vectors
            .iter()
            .map(|vector| VectorRecord {
                kind: vector.kind.clone(),
                doc_id: vector.doc_id.clone(),
                model: vector.model.clone(),
                values: vector.values.clone(),
            })
            .collect();
        let written = upsert_vectors(&state.db, &records, at)?;
        report.indexed = written;
    }

    if let Some(failure) = outcome.failure {
        state.db.record_failure(at, "embedding", &failure, "warn")?;
        report.failure = Some(failure);
    }

    Ok(report)
}

/// 一轮后台索引：活动 → 笔记 → 总结。任一侧维度冲突都向上抛。
pub async fn index_pending(
    state: &ServerState,
    provider: &dyn EmbeddingProvider,
    model: &str,
    batch_limit: usize,
    at: Timestamp,
) -> Result<EmbeddingIndexReport, AppError> {
    let activities = index_pending_activities(state, provider, model, batch_limit, at).await?;
    if activities.failure.is_some() {
        return Ok(activities);
    }
    let notes = index_pending_notes(state, provider, model, batch_limit, at).await?;
    if notes.failure.is_some() {
        return Ok(EmbeddingIndexReport {
            indexed: activities.indexed + notes.indexed,
            skipped: activities.skipped + notes.skipped,
            requests: activities.requests + notes.requests,
            prompt_tokens: activities.prompt_tokens + notes.prompt_tokens,
            failure: notes.failure,
        });
    }
    let summaries = index_pending_summaries(state, provider, model, batch_limit, at).await?;
    Ok(EmbeddingIndexReport {
        indexed: activities.indexed + notes.indexed + summaries.indexed,
        skipped: activities.skipped + notes.skipped + summaries.skipped,
        requests: activities.requests + notes.requests + summaries.requests,
        prompt_tokens: activities.prompt_tokens + notes.prompt_tokens + summaries.prompt_tokens,
        failure: summaries.failure,
    })
}

/// 索引进度：给 `/api/diagnostics` 用的一行摘要（活动 + 笔记 + 总结）。
pub fn index_progress(state: &ServerState) -> Result<(usize, usize), AppError> {
    let activity_total = mc_storage::projectors::activities::read_all(&state.db)?.len();
    let note_total = state
        .db
        .query_vault_rows(&mc_storage::vaults::VaultQuery {
            document_type: Vec::new(),
            parent_id: None,
            title: None,
            is_folder: Some(0),
            is_deleted: None,
        })?
        .len();
    let summary_total = state.db.read_summaries(None, None)?.len();
    let total = activity_total + note_total + summary_total;
    let indexed = count_vectors(&state.db, "activity")?
        + count_vectors(&state.db, "document")?
        + count_vectors(&state.db, "summary")?;
    Ok((indexed.min(total), total))
}

/// 语义检索所需的「索引 + 查询向量」。拿不到就返回 `None`，检索退化为关键词。
pub async fn query_vectors(
    state: &ServerState,
    query: &str,
) -> Option<(std::sync::Arc<mc_search::VectorIndex>, mc_search::Embedding)> {
    if query.trim().is_empty() {
        return None;
    }

    // 索引：走缓存（失效键是向量表版本戳）。库里没有向量就没什么可做的
    let index = state.cached_vector_index().ok().flatten()?;

    let provider = match state.embedding_provider() {
        Some(provider) => provider,
        None => {
            let config = state.config.current().config.clone();
            let secrets = mc_providers::credentials::KeychainCommand::default();
            let built = build_embedding_provider(&config, &secrets).ok().flatten()?;
            state.set_embedding_provider(Arc::clone(&built));
            built
        }
    };

    // 查询向量只发一条文本：**不切批**，失败就退化。
    // 这里刻意不用 `embed_documents`：那是批量索引的语义（有报告、有记账），
    // 而对话路径的查询向量是「有则加分、无则继续」的一次尝试。
    match provider.embed(&[query.to_string()]).await {
        Ok(response) => {
            let values = response.vectors.into_iter().next()?;
            (!values.is_empty()).then(|| (index, mc_search::Embedding::new(values)))
        }
        Err(_) => None,
    }
}

/// 起一个**独立**的索引循环。
///
/// 刻意不是 `spawn_projector` 的一部分：模型卡住时
/// 索引会把投影循环一起拖停，活动时间线就不再更新 —— 那正是
/// 「AI 把采集拖死」的同一种结构。独立任务 + 独立节奏
/// 让这两件事互不影响。
///
/// 返回 `JoinHandle`，调用方（daemon）在关闭时 abort。
pub fn spawn_worker(state: Arc<ServerState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let secrets = mc_providers::credentials::KeychainCommand::default();
        let mut provider: Option<Arc<dyn EmbeddingProvider>> = None;
        let mut model = String::new();
        let mut retry = WorkerRetry::new(&state);

        loop {
            let seconds = state
                .config
                .current()
                .config
                .activity
                .project_tick_secs
                .max(5);
            tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;

            let at = mc_common::time::Clock::now(&mc_common::time::SystemClock);

            if retry.refresh(&state) {
                provider = None;
                state.clear_embedding_provider();
            }
            if retry.auth_failed {
                continue;
            }

            // 配置可能在运行中变了（用户刚填了 embedding 模型）
            if provider.is_none() {
                let config = state.config.current().config.clone();
                match build_embedding_provider(&config, &secrets) {
                    Ok(Some(built)) => {
                        model = config.ai.embedding.model.clone();
                        // 存一份到共享状态：对话检索要复用同一个 Provider
                        // （连接池是复用的，不是每次查询新建）
                        state.set_embedding_provider(Arc::clone(&built));
                        provider = Some(built);
                    }
                    Ok(None) => continue,
                    Err(error) => {
                        retry.observe(Some(&error));
                        record_embedding_failure(&state.db, at, &error);
                        continue;
                    }
                }
            }

            let Some(active) = provider.as_ref() else {
                continue;
            };

            let limit = state
                .config
                .current()
                .config
                .ai
                .embedding
                .batch_limit
                .max(1);

            match index_pending(&state, active.as_ref(), &model, limit, at).await {
                // 维度不一致需要用户介入：记下来，并丢弃已组装的 Provider，
                // 这样用户改完配置后下一轮会重新组装。
                Err(error) => {
                    retry.observe(Some(&error));
                    record_embedding_failure(&state.db, at, &error);
                    // 维度不一致：清掉缓存，用户改完配置后重新组装
                    state.clear_embedding_provider();
                    provider = None;
                }
                Ok(report) => retry.observe(report.failure.as_ref()),
            }
        }
    })
}

/// 供测试注入 Provider 的循环（生产路径用 [`spawn_worker`]）。
pub fn spawn_worker_with(
    state: Arc<ServerState>,
    provider: Arc<dyn EmbeddingProvider>,
    model: String,
    batch_limit: usize,
    interval: std::time::Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut retry = WorkerRetry::new(&state);
        loop {
            tokio::time::sleep(interval).await;
            retry.refresh(&state);
            if retry.auth_failed {
                continue;
            }
            let at = mc_common::time::Clock::now(&mc_common::time::SystemClock);
            match index_pending(&state, provider.as_ref(), &model, batch_limit, at).await {
                Ok(report) => retry.observe(report.failure.as_ref()),
                Err(error) => {
                    retry.observe(Some(&error));
                    record_embedding_failure(&state.db, at, &error);
                }
            }
        }
    })
}

struct WorkerRetry {
    config: Arc<mc_config::load::LoadedConfig>,
    auth_failed: bool,
}

impl WorkerRetry {
    fn new(state: &ServerState) -> Self {
        Self {
            config: state.config.current(),
            auth_failed: false,
        }
    }

    fn refresh(&mut self, state: &ServerState) -> bool {
        let current = state.config.current();
        if Arc::ptr_eq(&current, &self.config) {
            return false;
        }
        self.config = current;
        self.auth_failed = false;
        true
    }

    fn observe(&mut self, failure: Option<&AppError>) {
        self.auth_failed =
            failure.is_some_and(|error| error.code() == ErrorCode::ProviderAuthFailed);
    }
}
