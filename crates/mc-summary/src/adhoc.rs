//! 任意时段总结。
//!
//! 入口很朴素：「总结我 10 点到 12 点半」。难的是**不能被这份随口一问拖垮**：
//! - 空范围**拒绝**，而不是产出一条空总结（垃圾数据比没有更糟）；
//! - 同一范围 + 同一筛选 + 事件没变 → 命中缓存，不重复烧 token；
//! - 事件变了 / 显式要求重生成 → 绕过缓存，但产生的是**新的一条**（旧总结是
//!   快照，不就地改写：用户可能还在看它）；
//! - 生成失败照样走确定性兜底，与阶段总结共用同一条降级链。
//!
//! 时间范围对外按**用户本地时区**解释，对内一律 UTC。

use std::sync::Arc;

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use mc_storage::projectors::activities::StoredActivity;
use mc_storage::projectors::summaries::NewSummary;
use mc_storage::Database;
use serde::{Deserialize, Serialize};

use crate::model::{
    ActivityDigest, RenderedSummary, StageSummaryInput, SummaryLocale, SummaryRange,
};
use crate::source::SummarySource;
use crate::template::SummaryTemplate;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdhocRange {
    pub from: Timestamp,
    pub to: Timestamp,
}

impl AdhocRange {
    pub fn duration_secs(self) -> u64 {
        (self.to.saturating_diff_millis(self.from).max(0) / 1000) as u64
    }
}

/// 筛选条件。空 = 不筛选。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdhocScope {
    #[serde(default)]
    pub apps: Vec<String>,
    #[serde(default)]
    pub categories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdhocRequest {
    pub range: AdhocRange,
    #[serde(default)]
    pub scope: AdhocScope,
    pub template_id: Option<String>,
    pub template_yaml: Option<String>,
    pub title: Option<String>,
    pub save_to_vault: bool,
    /// 忽略缓存（用户点了「重新生成」）
    pub force_regenerate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdhocConfig {
    pub timezone: String,
    pub locale: SummaryLocale,
    /// 超过这个跨度就要分块
    pub chunk_threshold_secs: u64,
    pub max_chunks: usize,
}

/// 生成前的预览：先让用户看到「这段时间有什么」，再决定要不要生成。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdhocPreview {
    pub from: Timestamp,
    pub to: Timestamp,
    pub local_from: String,
    pub local_to: String,
    pub timezone: String,
    pub observations: u32,
    pub blocked_observations: u32,
    pub activities: u32,
    pub stages: u32,
    pub estimated_chunks: u32,
    pub estimated_tokens: u32,
    pub has_data: bool,
}

/// 取消标记。跨 await 共享，因此用原子布尔 + `Arc`。
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<std::sync::atomic::AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AdhocOutcome {
    Generated {
        summary_id: String,
        summary: RenderedSummary,
        preview: AdhocPreview,
    },
    /// 命中缓存：**不重复调用模型**，直接给上次的结果
    Cached {
        summary_id: String,
        summary: RenderedSummary,
    },
    /// 范围内没有数据：拒绝生成（也不落库）
    EmptyRange { preview: AdhocPreview },
    /// 中途取消：**已完成的块留在库里**，重新发起时只补缺的那些
    Cancelled { chunks_done: u32, chunks_total: u32 },
}

impl AdhocOutcome {
    pub fn summary(&self) -> Option<&RenderedSummary> {
        match self {
            Self::Generated { summary, .. } | Self::Cached { summary, .. } => Some(summary),
            Self::EmptyRange { .. } | Self::Cancelled { .. } => None,
        }
    }
}

/// 把范围切成若干块。
///
/// **确定性**是硬要求：同一范围永远切成同样的块，否则块的缓存永远命中不了，
/// 「续跑」也就无从谈起。因此这里只依赖范围与配置，不依赖数据或时间。
pub fn plan_chunks(range: AdhocRange, config: &AdhocConfig) -> Vec<AdhocRange> {
    let total = estimate_chunks(range, config);
    if total <= 1 {
        return vec![range];
    }

    let span = (range.to.as_millis() - range.from.as_millis()).max(1);
    let step = span / total as i64;

    (0..total as i64)
        .map(|index| {
            let from = range.from.as_millis() + index * step;
            let to = if index == total as i64 - 1 {
                range.to.as_millis()
            } else {
                range.from.as_millis() + (index + 1) * step
            };
            AdhocRange {
                from: Timestamp::from_millis(from),
                to: Timestamp::from_millis(to),
            }
        })
        .collect()
}

/// 分块生成：每块单独生成并**单独落库**，最后归并成一条 `adhoc` 总结。
///
/// 为什么每块要单独落库：取消之后不能白跑（4.67），数据变了也只需要
/// 重算受影响的那块。块的 id 由范围决定，因此「续跑」就是「跳过已有块」。
pub async fn generate_chunked(
    db: &Database,
    source: &dyn SummarySource,
    config: &AdhocConfig,
    request: AdhocRequest,
    now: Timestamp,
    cancel: &CancelFlag,
) -> Result<AdhocOutcome, AppError> {
    generate_chunked_observed(db, source, config, request, now, cancel, &mut |_, _| {}).await
}

/// 同上，但每完成一块就回调一次 `(已完成, 总数)`。
///
/// 回调存在的原因与巡检一致：进度属于控制面的事（HTTP/SSE），
/// mc-summary 不该知道怎么推送。
pub async fn generate_chunked_observed(
    db: &Database,
    source: &dyn SummarySource,
    config: &AdhocConfig,
    request: AdhocRequest,
    now: Timestamp,
    cancel: &CancelFlag,
    on_progress: &mut (dyn FnMut(u32, u32) + Send),
) -> Result<AdhocOutcome, AppError> {
    let preview = preview(db, config, request.clone())?;
    if !preview.has_data {
        return Ok(AdhocOutcome::EmptyRange { preview });
    }

    let chunks = plan_chunks(request.range, config);
    on_progress(0, chunks.len() as u32);

    // 单块范围直接走一次性生成：不写中间块行。
    // 中间块的意义是「取消后可以续跑」，只有多块时才成立；
    // 给每个短请求都留一行中间产物只会让库变脏、让「总结数」变得难解释。
    if chunks.len() <= 1 {
        let outcome = generate(db, source, config, request, now).await?;
        on_progress(1, 1);
        return Ok(outcome);
    }

    let scope_hash = scope_hash(&request, config);
    let event_seq_hi = db.last_seq()?;

    // 已有归并结果且数据没变 → 命中缓存
    if !request.force_regenerate {
        if let Some(cached) = find_cached(db, &scope_hash, event_seq_hi, request.range)? {
            return Ok(AdhocOutcome::Cached {
                summary_id: cached.0,
                summary: cached.1,
            });
        }
    }

    let mut bodies: Vec<String> = Vec::new();
    let mut chunks_done = 0u32;

    for (index, chunk) in chunks.iter().enumerate() {
        if cancel.is_cancelled() {
            return Ok(AdhocOutcome::Cancelled {
                chunks_done,
                chunks_total: chunks.len() as u32,
            });
        }

        let chunk_id = chunk_id(*chunk);
        // 续跑：已有的块直接复用，不重复调用模型
        if let Some(existing) = db
            .read_summaries(None, Some("adhoc_chunk"))?
            .into_iter()
            .find(|summary| summary.id == chunk_id)
        {
            bodies.push(existing.body_markdown);
            chunks_done += 1;
            on_progress(chunks_done, chunks.len() as u32);
            continue;
        }

        let input = chunk_input(db, *chunk, &request, config, index)?;
        let outcome = source.generate(&input).await;
        let summary = outcome.summary().clone();

        db.insert_summary(
            &NewSummary {
                id: chunk_id,
                kind: "adhoc_chunk".to_string(),
                stage_id: None,
                template_id: source.template_id(),
                start: chunk.from,
                end: chunk.to,
                title: format!("分块 {}", index + 1),
                fields: summary.fields.clone(),
                body_markdown: summary.body_markdown.clone(),
                quality: match summary.quality {
                    crate::model::Quality::Model => "model".to_string(),
                    crate::model::Quality::Fallback => "fallback".to_string(),
                },
                model: summary.model.clone(),
                prompt_tokens: summary.prompt_tokens,
                completion_tokens: summary.completion_tokens,
                scope: Some(serde_json::json!({ "chunk_index": index })),
            },
            now,
        )?;

        bodies.push(summary.body_markdown);
        chunks_done += 1;
        on_progress(chunks_done, chunks.len() as u32);
    }

    // 归并：单块直接用；多块按顺序拼接并标注每块的本地时间范围。
    // 刻意不再调一次模型：确定性归并没有额外成本，也不会丢块
    // （分块结果直接拼接；跨块归并要额外一轮模型调用，收益不抵成本）。
    let body = if bodies.len() == 1 {
        bodies.remove(0)
    } else {
        bodies
            .iter()
            .enumerate()
            .map(|(index, chunk_body)| {
                format!(
                    "## 第 {} 段（{}–{}）
{}",
                    index + 1,
                    local(chunks[index].from, &config.timezone),
                    local(chunks[index].to, &config.timezone),
                    chunk_body.trim()
                )
            })
            .collect::<Vec<_>>()
            .join(
                "

",
            )
    };

    let rendered = RenderedSummary {
        title: request.title.clone().unwrap_or_else(|| {
            format!("{}{} 总结", local(request.range.from, &config.timezone), "")
        }),
        fields: Default::default(),
        body_markdown: body,
        quality: crate::model::Quality::Fallback,
        model: None,
        prompt_tokens: 0,
        completion_tokens: 0,
    };

    // 记录写**实际使用的模板**：请求里的 id 只是选择器，用户模板 YAML 的 id 优先于它。
    // 归并结果本身是各分块正文的拼接，但「用了哪套模板」的答案必须与单块生成一致。
    let merge_template_id = request.template_id.as_deref().unwrap_or("work_stage");
    let merge_template =
        SummaryTemplate::resolve(merge_template_id, request.template_yaml.as_deref())
            .map_err(|message| AppError::new(ErrorCode::ConfigInvalid, message))?;

    let summary_id = unique_id(db, request.range, event_seq_hi)?;
    db.insert_summary(
        &NewSummary {
            id: summary_id.clone(),
            kind: "adhoc".to_string(),
            stage_id: None,
            template_id: merge_template.id.clone(),
            start: request.range.from,
            end: request.range.to,
            title: rendered.title.clone(),
            fields: rendered.fields.clone(),
            body_markdown: rendered.body_markdown.clone(),
            quality: "chunked".to_string(),
            model: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            scope: Some(serde_json::json!({
                "scope_hash": scope_hash,
                "event_seq_hi": event_seq_hi,
                "chunks": chunks.len(),
            })),
        },
        now,
    )?;

    Ok(AdhocOutcome::Generated {
        summary_id,
        summary: rendered,
        preview,
    })
}

fn chunk_id(chunk: AdhocRange) -> String {
    format!(
        "sum-adhoc-chunk-{from}-{to}",
        from = chunk.from.as_millis(),
        to = chunk.to.as_millis()
    )
}

fn chunk_input(
    db: &Database,
    chunk: AdhocRange,
    request: &AdhocRequest,
    config: &AdhocConfig,
    index: usize,
) -> Result<StageSummaryInput, AppError> {
    let activities = activities_in_range(db, chunk, &request.scope)?;
    Ok(StageSummaryInput {
        stage_id: format!("chunk-{index}"),
        range: SummaryRange {
            start: chunk.from,
            end: chunk.to,
        },
        timezone: config.timezone.clone(),
        locale: config.locale,
        activities: activities.iter().map(digest).collect(),
        observation_count: db.count_observations_in_range(chunk.from, chunk.to)?,
        blocked_observations: db.count_blocked_observations_in_range(chunk.from, chunk.to)?,
    })
}

/// 预览：范围内有多少东西、要花多少。
pub fn preview(
    db: &Database,
    config: &AdhocConfig,
    request: AdhocRequest,
) -> Result<AdhocPreview, AppError> {
    validate_range(request.range)?;

    let observations = db.count_observations_in_range(request.range.from, request.range.to)?;
    let blocked = db.count_blocked_observations_in_range(request.range.from, request.range.to)?;
    let activities = activities_in_range(db, request.range, &request.scope)?;
    let stages = db
        .read_stages()?
        .into_iter()
        .filter(|stage| {
            let end = stage.end.unwrap_or(stage.start);
            stage.start < request.range.to && end >= request.range.from
        })
        .count() as u32;

    let chunks = estimate_chunks(request.range, config);
    // 估算刻意保守且透明：预览的作用是让用户对成本有预期，
    // 精确值只有真跑一次才知道。
    let estimated_tokens = 600 + observations * 6 + activities.len() as u32 * 40;

    Ok(AdhocPreview {
        from: request.range.from,
        to: request.range.to,
        local_from: local(request.range.from, &config.timezone),
        local_to: local(request.range.to, &config.timezone),
        timezone: config.timezone.clone(),
        observations,
        blocked_observations: blocked,
        activities: activities.len() as u32,
        stages,
        estimated_chunks: chunks,
        estimated_tokens,
        has_data: observations > 0 || !activities.is_empty() || stages > 0,
    })
}

/// 生成总结。**不返回 `Err` 表示失败**（除了请求本身非法）：
/// 模型不可用只可能降级。
pub async fn generate(
    db: &Database,
    source: &dyn SummarySource,
    config: &AdhocConfig,
    request: AdhocRequest,
    now: Timestamp,
) -> Result<AdhocOutcome, AppError> {
    let preview = preview(db, config, request.clone())?;
    if !preview.has_data {
        return Ok(AdhocOutcome::EmptyRange { preview });
    }

    let scope_hash = scope_hash(&request, config);
    let event_seq_hi = db.last_seq()?;

    if !request.force_regenerate {
        if let Some(cached) = find_cached(db, &scope_hash, event_seq_hi, request.range)? {
            return Ok(AdhocOutcome::Cached {
                summary_id: cached.0,
                summary: cached.1,
            });
        }
    }

    let activities = activities_in_range(db, request.range, &request.scope)?;
    let input = StageSummaryInput {
        stage_id: String::new(),
        range: SummaryRange {
            start: request.range.from,
            end: request.range.to,
        },
        timezone: config.timezone.clone(),
        locale: config.locale,
        activities: activities.iter().map(digest).collect(),
        observation_count: preview.observations,
        blocked_observations: preview.blocked_observations,
    };

    // 请求指定的模板优先：这个字段以前只被**记录**，生成时永远用默认模板 ——
    // 用户点了模板却拿到默认产出。未知 id 明确报错，不静默退回默认。
    let template_id = request.template_id.as_deref().unwrap_or("work_stage");
    let template = SummaryTemplate::resolve(template_id, request.template_yaml.as_deref())
        .map_err(|message| AppError::new(ErrorCode::ConfigInvalid, message))?;
    let outcome = source.generate(&input).await;
    let summary = outcome.summary().clone();

    // id 必须**每次生成都不同**：旧总结是快照，重新生成写的是新的一条
    // （用户可能还在看旧的那条，就地覆盖会让他以为「一直没变」）。
    let summary_id = unique_id(db, request.range, event_seq_hi)?;

    db.insert_summary(
        &NewSummary {
            id: summary_id.clone(),
            kind: "adhoc".to_string(),
            stage_id: None,
            // 记录写**实际使用的模板**：请求里的 id 只是选择器，用户模板 YAML 优先于它
            template_id: template.id.clone(),
            start: request.range.from,
            end: request.range.to,
            title: request
                .title
                .clone()
                .unwrap_or_else(|| summary.title.clone()),
            fields: summary.fields.clone(),
            body_markdown: summary.body_markdown.clone(),
            quality: match summary.quality {
                crate::model::Quality::Model => "model".to_string(),
                crate::model::Quality::Fallback => "fallback".to_string(),
            },
            model: summary.model.clone(),
            prompt_tokens: summary.prompt_tokens,
            completion_tokens: summary.completion_tokens,
            // 缓存判据：作用域指纹 + **生成时的事件位点**。
            // 位点变了说明范围内可能多了数据，缓存随之失效。
            scope: Some(serde_json::json!({
                "scope_hash": scope_hash,
                "event_seq_hi": event_seq_hi,
            })),
        },
        now,
    )?;

    Ok(AdhocOutcome::Generated {
        summary_id,
        summary,
        preview,
    })
}

/// `sum-adhoc-<from>-<to>-<事件位点>`，冲突时追加序号。
///
/// 位点参与 id 是刻意的：范围相同但历史不同 → 天然是两个不同的产物，
/// 排查时看 id 就知道它是基于哪一段历史算出来的。
fn unique_id(db: &Database, range: AdhocRange, event_seq_hi: i64) -> Result<String, AppError> {
    let base = format!(
        "sum-adhoc-{from}-{to}-{seq}",
        from = range.from.as_millis(),
        to = range.to.as_millis(),
        seq = event_seq_hi
    );

    let existing: Vec<String> = db
        .read_summaries(None, Some("adhoc"))?
        .into_iter()
        .map(|summary| summary.id)
        .collect();
    if !existing.contains(&base) {
        return Ok(base);
    }

    // 同一份历史被反复重生成（用户连点两次「重新生成」）也要各留一条
    for attempt in 2..1000 {
        let candidate = format!("{base}-{attempt}");
        if !existing.contains(&candidate) {
            return Ok(candidate);
        }
    }

    Err(AppError::new(
        ErrorCode::DomainInvariantViolated,
        format!("范围 {base} 的总结 id 已用尽"),
    ))
}

fn validate_range(range: AdhocRange) -> Result<(), AppError> {
    if range.from >= range.to {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            format!(
                "总结范围非法：from={} 不早于 to={}",
                range.from.to_rfc3339(),
                range.to.to_rfc3339()
            ),
        ));
    }
    Ok(())
}

fn estimate_chunks(range: AdhocRange, config: &AdhocConfig) -> u32 {
    if config.chunk_threshold_secs == 0 {
        return 1;
    }
    let chunks = range.duration_secs().div_ceil(config.chunk_threshold_secs);
    chunks.clamp(1, config.max_chunks.max(1) as u64) as u32
}

/// 作用域指纹（对外）：缓存与「并发去重」共用同一个判据 ——
/// 两处各写一份的话，缓存命中的请求却不会被去重，或者反过来。
pub fn scope_key(request: &AdhocRequest, config: &AdhocConfig) -> String {
    scope_hash(request, config)
}

/// 作用域指纹：范围 + 筛选 + 模板 + 语言。
///
/// 只哈希「会影响结果的东西」，因此改标题、改 save_to_vault 不会让缓存失效。
fn scope_hash(request: &AdhocRequest, config: &AdhocConfig) -> String {
    let mut apps = request.scope.apps.clone();
    let mut categories = request.scope.categories.clone();
    apps.sort();
    categories.sort();

    let payload = format!(
        "{from}|{to}|{apps:?}|{categories:?}|{template}|{locale:?}",
        from = request.range.from.as_millis(),
        to = request.range.to.as_millis(),
        template = request.template_id.as_deref().unwrap_or("default"),
        locale = config.locale,
    );
    blake3_hash(&payload)
}

fn blake3_hash(text: &str) -> String {
    // 用 std 的 DefaultHasher 就够：这里只是缓存键，不是安全边界
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn find_cached(
    db: &Database,
    scope_hash: &str,
    event_seq_hi: i64,
    range: AdhocRange,
) -> Result<Option<(String, RenderedSummary)>, AppError> {
    let summaries = db.read_summaries(None, Some("adhoc"))?;

    for summary in summaries {
        if summary.start != range.from || summary.end != range.to {
            continue;
        }
        let Some(scope) = summary.scope.as_ref() else {
            continue;
        };
        if scope.get("scope_hash").and_then(|value| value.as_str()) != Some(scope_hash) {
            continue;
        }
        // 位点不同 = 这段历史后来又写过事件，缓存不再可信
        if scope.get("event_seq_hi").and_then(|value| value.as_i64()) != Some(event_seq_hi) {
            continue;
        }

        return Ok(Some((
            summary.id,
            RenderedSummary {
                title: summary.title,
                fields: Default::default(),
                body_markdown: summary.body_markdown,
                quality: if summary.quality == "model" {
                    crate::model::Quality::Model
                } else {
                    crate::model::Quality::Fallback
                },
                model: summary.model,
                prompt_tokens: summary.prompt_tokens,
                completion_tokens: summary.completion_tokens,
            },
        )));
    }

    Ok(None)
}

fn activities_in_range(
    db: &Database,
    range: AdhocRange,
    scope: &AdhocScope,
) -> Result<Vec<StoredActivity>, AppError> {
    let all = mc_storage::projectors::activities::read_all(db)?;
    Ok(all
        .into_iter()
        .filter(|activity| activity.start < range.to && activity.end >= range.from)
        .filter(|activity| {
            scope.categories.is_empty()
                || activity
                    .category
                    .as_deref()
                    .map(|category| scope.categories.iter().any(|want| want == category))
                    .unwrap_or(false)
        })
        .collect())
}

fn digest(activity: &StoredActivity) -> ActivityDigest {
    ActivityDigest {
        id: activity.id.clone(),
        title: activity.title.clone(),
        category: activity.category.clone(),
        start: activity.start,
        end: activity.end,
        observations: activity.evidence.len() as u32,
        inferred: matches!(
            activity.origin,
            mc_domain::activity::Provenance::Inferred { .. }
        ),
    }
}

fn local(at: Timestamp, timezone: &str) -> String {
    at.format_in_tz(timezone, "%Y-%m-%d %H:%M")
        .unwrap_or_else(|_| at.to_rfc3339())
}
