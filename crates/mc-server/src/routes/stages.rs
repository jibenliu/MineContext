//! 阶段与总结接口。
//!
//! 形状围绕「总结卡片」设计：时间范围、状态、**结束原因**、
//! 包含哪些活动、有没有总结、总结的质量标记。
//!
//! 结束原因是刻意暴露的：用户看到「09:00–09:30 写代码」时会问
//! 「为什么到这里就断了」——答案就在 `end_reason` 里
//! （切走了 / 空闲了 / 锁屏了 / 跨天了）。

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock, Timestamp};
use mc_storage::projectors::stages::StageRow;
use mc_storage::projectors::summaries::StoredSummary;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::envelope;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/v1/stages", get(list_stages))
        .route("/api/v1/stages/{id}", get(get_stage))
        .route("/api/v1/stages/{id}/close", post(close_stage))
        .route("/api/v1/summaries", get(list_summaries))
        // 注意顺序：`adhoc/preview` 必须排在 `{id}` 之前，否则会被当成 id
        .route("/api/v1/summaries/adhoc/preview", get(adhoc_preview))
        // 作业接口必须排在 `adhoc/{id}` 之类的动态段之前
        .route(
            "/api/v1/summaries/adhoc/jobs",
            post(super::adhoc_jobs::submit),
        )
        .route(
            "/api/v1/summaries/adhoc/jobs/{job_id}",
            get(super::adhoc_jobs::status),
        )
        .route(
            "/api/v1/summaries/adhoc/jobs/{job_id}/cancel",
            post(super::adhoc_jobs::cancel),
        )
        .route("/api/v1/summaries/adhoc", post(adhoc_generate))
        .route("/api/v1/summaries/{id}", get(get_summary))
        .route("/api/v1/summaries/{id}/regenerate", post(regenerate))
}

#[derive(Debug, Deserialize)]
pub struct StagesQuery {
    /// `YYYY-MM-DD`（**本地日期**，按配置时区）
    date: Option<String>,
    state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AdhocQuery {
    from: String,
    to: String,
}

#[derive(Debug, Deserialize)]
pub struct AdhocBody {
    from: String,
    to: String,
    #[serde(default)]
    scope: Option<mc_summary::adhoc::AdhocScope>,
    #[serde(default)]
    template_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    save_to_vault: bool,
    #[serde(default)]
    force_regenerate: bool,
}

#[derive(Debug, Deserialize)]
pub struct SummariesQuery {
    stage_id: Option<String>,
    kind: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

/// 模板 id：请求没指定时用配置里的（`summary.template_id`）—— 否则这个配置项
/// 是个死旋钮：改了不生效，而记录里还写着用户以为的那个模板。
pub(crate) fn template_yaml_from_config(state: &ServerState) -> Option<String> {
    state.config.current().config.summary.template_yaml.clone()
}

/// 解析本次生成要用的模板：请求指定优先（空则用配置里的 id）。解析失败明确报错，
/// 不静默退回默认模板 —— 用户以为生效却拿到别的模板是最坏的结果。
pub(crate) fn resolve_template(
    state: &ServerState,
    requested: &str,
) -> Result<mc_summary::template::SummaryTemplate, AppError> {
    let id = if requested.trim().is_empty() {
        template_id_from_config(state)
    } else {
        requested.to_string()
    };
    // 用 ConfigInvalid 而不是 DomainInvariantViolated：模板 id 来自配置或请求，
    // 属于「配置有误」这一类，用户看到的 message 才不是「内部状态不一致」。
    // 具体是哪个 id、有哪些可用 id 放在 detail 里。
    mc_summary::template::SummaryTemplate::resolve(&id, template_yaml_from_config(state).as_deref())
        .map_err(|message| AppError::new(ErrorCode::ConfigInvalid, message))
}

pub(crate) fn template_id_from_config(state: &ServerState) -> String {
    state.config.current().config.summary.template_id.clone()
}

pub(crate) fn effective_timezone(state: &ServerState) -> String {
    state
        .config
        .current()
        .config
        .general
        .timezone
        .clone()
        .unwrap_or_else(|| "UTC".to_string())
}

/// 解析 `YYYY-MM-DD`。不合法要**报错**而不是退回今天 ——
/// 静默退回会让用户以为自己在看某一天，实际看的是另一天。
fn parse_day(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
        .map(|date| date.to_string())
        .map_err(|_| {
            AppError::new(
                ErrorCode::DomainInvalidTimestamp,
                format!("无法解析日期 `{raw}`，期望 YYYY-MM-DD"),
            )
        })
}

/// adhoc 范围边界：接受时间戳，也接受 `YYYY-MM-DD`（按**配置时区**展开成当天起止）。
///
/// 前端与「今天/昨天/最近 7 天」预设发的是纯日期；只认时间戳会让每次预览/生成
/// 都 400。纯日期不能按 UTC 展开：配置时区下的「9 月 30 日」与 UTC 差若干小时，
/// 那是差一整段窗口的静默错误，比直接报错更糟。
pub(crate) fn parse_range_bound(
    raw: &str,
    timezone: &str,
    end_of_day: bool,
) -> Result<Timestamp, AppError> {
    if let Ok(time) = parse_time(raw) {
        return Ok(time);
    }
    let day = chrono::NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d").map_err(|_| {
        AppError::new(
            ErrorCode::DomainInvalidTimestamp,
            format!("无法解析时间边界 `{raw}`，期望 RFC 3339 或 YYYY-MM-DD"),
        )
    })?;
    let (start, end) = Timestamp::day_bounds_for(day, timezone)?;
    Ok(if end_of_day { end } else { start })
}

fn parse_time(raw: &str) -> Result<Timestamp, AppError> {
    Timestamp::parse_rfc3339(raw.trim()).or_else(|_| {
        // 也接受 `YYYY-MM-DD HH:MM:SS`
        chrono::NaiveDateTime::parse_from_str(raw.trim(), "%Y-%m-%d %H:%M:%S")
            .map(|naive| Timestamp::from_millis(naive.and_utc().timestamp_millis()))
            .map_err(|_| {
                AppError::new(
                    ErrorCode::DomainInvalidTimestamp,
                    format!("无法解析时间 `{raw}`"),
                )
            })
    })
}

/// `GET /api/v1/stages?date=YYYY-MM-DD` —— 某一天的阶段视图。
pub async fn list_stages(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<StagesQuery>,
) -> Response {
    let day = match &query.date {
        Some(raw) => match parse_day(raw) {
            Ok(day) => day,
            Err(error) => return envelope::error_response(StatusCode::BAD_REQUEST, &error),
        },
        None => {
            // 没给日期就按「今天」（用户本地时区）
            let now = SystemClock.now();
            match now.to_local_date(&effective_timezone(&state)) {
                Ok(date) => date.to_string(),
                Err(error) => return envelope::error_response(StatusCode::BAD_REQUEST, &error),
            }
        }
    };

    let stages = match state.db.read_stages() {
        Ok(stages) => stages,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let summaries = match state.db.read_summaries(None, None) {
        Ok(summaries) => summaries,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };

    let rows: Vec<Value> = stages
        .iter()
        .filter(|stage| stage.day == day)
        .filter(|stage| {
            query
                .state
                .as_deref()
                .map(|wanted| stage.state == wanted)
                .unwrap_or(true)
        })
        .map(|stage| stage_json(stage, summary_for(&summaries, &stage.id)))
        .collect();

    envelope::ok(json!({ "date": day, "stages": rows }))
}

/// `GET /api/v1/stages/{id}` —— 单个阶段 + 它的总结。
pub async fn get_stage(State(state): State<Arc<ServerState>>, Path(id): Path<String>) -> Response {
    let stages = match state.db.read_stages() {
        Ok(stages) => stages,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let Some(stage) = stages.into_iter().find(|stage| stage.id == id) else {
        return stage_not_found(&id);
    };

    let summaries = match state.db.stage_summaries(&id) {
        Ok(summaries) => summaries,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };

    envelope::ok(json!({
        "stage": stage_json(&stage, summaries.first()),
        "summaries": summaries.iter().map(summary_json).collect::<Vec<_>>(),
    }))
}

/// `POST /api/v1/stages/{id}/close` —— 用户手动结束阶段。
///
/// 手动结束**必须立刻给出总结**：
/// 因此这里同步跑一次带 `deadline=0` 的生成 ——
/// 配了模型就用模型，没配或失败就退回确定性兜底；
/// 两条路径都不会出现「点了结束却没有总结」。
pub async fn close_stage(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> Response {
    let stages = match state.db.read_stages() {
        Ok(stages) => stages,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let Some(stage) = stages.into_iter().find(|stage| stage.id == id) else {
        return stage_not_found(&id);
    };
    if stage.is_closed() {
        return envelope::error_response(
            StatusCode::CONFLICT,
            &AppError::new(
                ErrorCode::DomainInvalidRange,
                format!("阶段 {id} 已经结束，不能重复结束"),
            ),
        );
    }

    let now = SystemClock.now();
    let closed = StageRow {
        id: stage.id.clone(),
        start: stage.start,
        end: Some(now),
        state: "closed".to_string(),
        end_reason: Some("manual".to_string()),
        day: stage.day.clone(),
        activities: stage.activities.clone(),
    };
    let last_seq = match state.db.last_seq() {
        Ok(seq) => seq,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    if let Err(error) = state.db.upsert_stage(&closed, last_seq) {
        return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }

    // deadline = 0：立刻就补，不等巡检
    let policy = crate::stages::policy_for(&state);
    let mut patrol = crate::stages::patrol_policy(&policy);
    patrol.deadline_secs = 0;
    patrol.min_stage_duration_secs = 0;

    let source = crate::summary::source_for(&state);
    let mut created: Vec<mc_storage::projectors::summaries::StoredSummary> = Vec::new();
    if let Err(error) = mc_summary::patrol::patrol_once_observed(
        &state.db,
        source.as_ref(),
        &patrol,
        now,
        &mut |summary| created.push(summary.clone()),
    )
    .await
    {
        return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    for summary in &created {
        crate::stages::publish_summary(&state, summary);
    }

    match state.db.stage_summaries(&id) {
        Ok(summaries) => envelope::ok(json!({
            "stage": stage_json(&closed, summaries.first()),
            "summary_id": summaries.first().map(|summary| summary.id.clone()),
            "quality": summaries.first().map(|summary| summary.quality.clone()),
        })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

/// `GET /api/v1/summaries` —— 按阶段 / 类型 / 时间范围过滤。
pub async fn list_summaries(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<SummariesQuery>,
) -> Response {
    let from = match query.from.as_deref().map(parse_time).transpose() {
        Ok(from) => from,
        Err(error) => return envelope::error_response(StatusCode::BAD_REQUEST, &error),
    };
    let to = match query.to.as_deref().map(parse_time).transpose() {
        Ok(to) => to,
        Err(error) => return envelope::error_response(StatusCode::BAD_REQUEST, &error),
    };

    let summaries = match state
        .db
        .read_summaries(query.stage_id.as_deref(), query.kind.as_deref())
    {
        Ok(summaries) => summaries,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };

    let rows: Vec<Value> = summaries
        .iter()
        .filter(|summary| from.map(|from| summary.start >= from).unwrap_or(true))
        .filter(|summary| to.map(|to| summary.start < to).unwrap_or(true))
        .map(summary_json)
        .collect();

    envelope::ok(json!({ "summaries": rows }))
}

pub async fn get_summary(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> Response {
    let summaries = match state.db.read_summaries(None, None) {
        Ok(summaries) => summaries,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    match summaries.into_iter().find(|summary| summary.id == id) {
        Some(summary) => envelope::ok(json!({ "summary": summary_json(&summary) })),
        None => envelope::error_response(
            StatusCode::NOT_FOUND,
            &AppError::new(ErrorCode::DomainNothingToDo, format!("总结 {id} 不存在")),
        ),
    }
}

/// 任意时段总结落库后广播（前端「生成完就出现」靠它）。
pub fn publish_adhoc(state: &ServerState, summary_id: &str) {
    if let Ok(summaries) = state.db.read_summaries(None, None) {
        if let Some(summary) = summaries.iter().find(|summary| summary.id == summary_id) {
            crate::stages::publish_summary(state, summary);
        }
    }
}

/// 供异步作业复用：构造任意时段请求（与同步接口同一套解析规则）。
#[allow(clippy::too_many_arguments)]
pub fn adhoc_request_from_parts(
    from: &str,
    to: &str,
    scope: Option<mc_summary::adhoc::AdhocScope>,
    template_id: Option<String>,
    title: Option<String>,
    save_to_vault: bool,
    force_regenerate: bool,
    template_yaml: Option<String>,
    timezone: &str,
) -> Result<mc_summary::adhoc::AdhocRequest, AppError> {
    adhoc_request(
        from,
        to,
        scope,
        template_id,
        title,
        save_to_vault,
        force_regenerate,
        template_yaml,
        timezone,
    )
}

pub fn adhoc_config_for(state: &ServerState) -> mc_summary::adhoc::AdhocConfig {
    adhoc_config(state)
}

pub fn preview_json_for(preview: &mc_summary::adhoc::AdhocPreview) -> Value {
    preview_json(preview)
}

pub fn summary_json_for(summary: &StoredSummary) -> Value {
    summary_json(summary)
}

/// 去重键：与缓存判据**共用同一个实现**（`adhoc::scope_key`）。
pub fn adhoc_scope_key(
    request: &mc_summary::adhoc::AdhocRequest,
    config: &mc_summary::adhoc::AdhocConfig,
) -> String {
    mc_summary::adhoc::scope_key(request, config)
}

fn adhoc_config(state: &ServerState) -> mc_summary::adhoc::AdhocConfig {
    let config = state.config.current();
    mc_summary::adhoc::AdhocConfig {
        timezone: effective_timezone(state),
        locale: mc_summary::model::SummaryLocale::from_config(&config.config.general.locale),
        chunk_threshold_secs: config.config.summary.adhoc_chunk_threshold_secs,
        max_chunks: config.config.summary.adhoc_max_chunks,
    }
}

// 与 `adhoc_request_from_parts` 一样：参数多但都是并列选项，拆结构体反而更难读
#[allow(clippy::too_many_arguments)]
fn adhoc_request(
    from: &str,
    to: &str,
    scope: Option<mc_summary::adhoc::AdhocScope>,
    template_id: Option<String>,
    title: Option<String>,
    save_to_vault: bool,
    force_regenerate: bool,
    template_yaml: Option<String>,
    timezone: &str,
) -> Result<mc_summary::adhoc::AdhocRequest, AppError> {
    Ok(mc_summary::adhoc::AdhocRequest {
        range: mc_summary::adhoc::AdhocRange {
            // `to` 用当天末尾：纯日期区间的右端是「含当天」
            from: parse_range_bound(from, timezone, false)?,
            to: parse_range_bound(to, timezone, true)?,
        },
        scope: scope.unwrap_or_default(),
        template_id,
        template_yaml,
        title,
        save_to_vault,
        force_regenerate,
    })
}

/// `GET /api/v1/summaries/adhoc/preview` —— 生成前先看范围内有什么。
pub async fn adhoc_preview(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<AdhocQuery>,
) -> Response {
    let request = match adhoc_request(
        &query.from,
        &query.to,
        None,
        Some(template_id_from_config(&state)),
        None,
        false,
        false,
        template_yaml_from_config(&state),
        &effective_timezone(&state),
    ) {
        Ok(request) => request,
        Err(error) => return envelope::error_response(StatusCode::BAD_REQUEST, &error),
    };

    match mc_summary::adhoc::preview(&state.db, &adhoc_config(&state), request) {
        Ok(preview) => envelope::ok(preview_json(&preview)),
        Err(error) => envelope::error_response(StatusCode::BAD_REQUEST, &error),
    }
}

/// `POST /api/v1/summaries/adhoc` —— 生成（同步；命中缓存直接返回）。
pub async fn adhoc_generate(
    State(state): State<Arc<ServerState>>,
    payload: Result<axum::Json<AdhocBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let axum::Json(body) = match payload {
        Ok(body) => body,
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

    let request = match adhoc_request(
        &body.from,
        &body.to,
        body.scope,
        body.template_id
            .or_else(|| Some(template_id_from_config(&state))),
        body.title,
        body.save_to_vault,
        body.force_regenerate,
        template_yaml_from_config(&state),
        &effective_timezone(&state),
    ) {
        Ok(request) => request,
        Err(error) => return envelope::error_response(StatusCode::BAD_REQUEST, &error),
    };

    // 生成源必须用**本次要用的模板**：否则记录写着新模板、产出还是默认模板的。
    let template =
        match resolve_template(&state, request.template_id.as_deref().unwrap_or_default()) {
            Ok(template) => template,
            Err(error) => return envelope::error_response(StatusCode::BAD_REQUEST, &error),
        };
    let source = crate::summary::source_for_with_template(&state, template);
    let now = SystemClock.now();
    // 长范围自动分块：每块单独落库，中途失败/取消不必从头再来。
    match mc_summary::adhoc::generate_chunked(
        &state.db,
        source.as_ref(),
        &adhoc_config(&state),
        request,
        now,
        &mc_summary::adhoc::CancelFlag::new(),
    )
    .await
    {
        Ok(mc_summary::adhoc::AdhocOutcome::Generated {
            summary_id,
            summary,
            preview,
        }) => {
            publish_adhoc(&state, &summary_id);
            envelope::ok(json!({
                "cached": false,
                "summary_id": summary_id,
                "summary": rendered_json(&summary_id, &summary),
                "preview": preview_json(&preview),
            }))
        }
        Ok(mc_summary::adhoc::AdhocOutcome::Cached {
            summary_id,
            summary,
        }) => envelope::ok(json!({
            "cached": true,
            "summary_id": summary_id,
            "summary": rendered_json(&summary_id, &summary),
        })),
        Ok(mc_summary::adhoc::AdhocOutcome::Cancelled { .. }) => envelope::error_response(
            StatusCode::CONFLICT,
            &AppError::new(
                ErrorCode::DomainNothingToDo,
                "生成已取消（已完成的块会保留，重新发起将继续）",
            ),
        ),
        Ok(mc_summary::adhoc::AdhocOutcome::EmptyRange { preview }) => {
            // `data` 里带上预览，前端才能解释「为什么不能生成」并禁用按钮
            envelope::error_response_with_data(
                StatusCode::UNPROCESSABLE_ENTITY,
                &AppError::new(
                    ErrorCode::DomainNothingToDo,
                    format!(
                        "范围 {} – {} 内没有可总结的内容",
                        preview.local_from, preview.local_to
                    ),
                ),
                preview_json(&preview),
            )
        }
        Err(error) => envelope::error_response(StatusCode::BAD_REQUEST, &error),
    }
}

/// `POST /api/v1/summaries/{id}/regenerate` —— 用同一个范围重新生成一条。
pub async fn regenerate(State(state): State<Arc<ServerState>>, Path(id): Path<String>) -> Response {
    let summaries = match state.db.read_summaries(None, None) {
        Ok(summaries) => summaries,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let Some(original) = summaries.into_iter().find(|summary| summary.id == id) else {
        return envelope::error_response(
            StatusCode::NOT_FOUND,
            &AppError::new(ErrorCode::DomainNothingToDo, format!("总结 {id} 不存在")),
        );
    };

    let request = match adhoc_request(
        &original.start.to_rfc3339(),
        &original.end.to_rfc3339(),
        None,
        Some(original.template_id.clone()),
        Some(original.title.clone()),
        false,
        true,
        template_yaml_from_config(&state),
        &effective_timezone(&state),
    ) {
        Ok(request) => request,
        Err(error) => return envelope::error_response(StatusCode::BAD_REQUEST, &error),
    };

    // 生成源必须用**本次要用的模板**：否则记录写着新模板、产出还是默认模板的。
    let template =
        match resolve_template(&state, request.template_id.as_deref().unwrap_or_default()) {
            Ok(template) => template,
            Err(error) => return envelope::error_response(StatusCode::BAD_REQUEST, &error),
        };
    let source = crate::summary::source_for_with_template(&state, template);
    let now = SystemClock.now();
    // 长范围自动分块：每块单独落库，中途失败/取消不必从头再来。
    match mc_summary::adhoc::generate_chunked(
        &state.db,
        source.as_ref(),
        &adhoc_config(&state),
        request,
        now,
        &mc_summary::adhoc::CancelFlag::new(),
    )
    .await
    {
        Ok(mc_summary::adhoc::AdhocOutcome::Generated { summary_id, .. }) => {
            publish_adhoc(&state, &summary_id);
            envelope::ok(json!({ "summary_id": summary_id, "regenerated_from": id }))
        }
        Ok(other) => envelope::error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            &AppError::new(
                ErrorCode::DomainNothingToDo,
                format!("无法重新生成：{other:?}"),
            ),
        ),
        Err(error) => envelope::error_response(StatusCode::BAD_REQUEST, &error),
    }
}

fn preview_json(preview: &mc_summary::adhoc::AdhocPreview) -> Value {
    json!({
        "range": {
            "from": preview.from.to_rfc3339(),
            "to": preview.to.to_rfc3339(),
            "local_from": preview.local_from,
            "local_to": preview.local_to,
        },
        "timezone": preview.timezone,
        "counts": {
            "observations": preview.observations,
            "blocked_observations": preview.blocked_observations,
            "activities": preview.activities,
            "stages": preview.stages,
        },
        "estimated_chunks": preview.estimated_chunks,
        "estimated_tokens": preview.estimated_tokens,
        "has_data": preview.has_data,
    })
}

fn rendered_json(id: &str, summary: &mc_summary::model::RenderedSummary) -> Value {
    json!({
        "id": id,
        "title": summary.title,
        "body_markdown": summary.body_markdown,
        "fields": summary.fields,
        "quality": match summary.quality {
            mc_summary::model::Quality::Model => "model",
            mc_summary::model::Quality::Fallback => "fallback",
        },
        "model": summary.model,
        "prompt_tokens": summary.prompt_tokens,
        "completion_tokens": summary.completion_tokens,
    })
}

fn stage_not_found(id: &str) -> Response {
    envelope::error_response(
        StatusCode::NOT_FOUND,
        &AppError::new(ErrorCode::DomainNothingToDo, format!("阶段 {id} 不存在")),
    )
}

fn summary_for<'a>(summaries: &'a [StoredSummary], stage_id: &str) -> Option<&'a StoredSummary> {
    summaries
        .iter()
        .find(|summary| summary.stage_id.as_deref() == Some(stage_id))
}

fn stage_json(stage: &StageRow, summary: Option<&StoredSummary>) -> Value {
    let end = stage.end.unwrap_or(stage.start);
    json!({
        "id": stage.id,
        "start": stage.start.to_rfc3339(),
        "end": stage.end.map(|end| end.to_rfc3339()),
        "day": stage.day,
        "state": stage.state,
        "end_reason": stage.end_reason,
        "activities": stage.activities,
        "duration_secs": (end.saturating_diff_millis(stage.start).max(0) / 1000),
        "has_summary": summary.is_some(),
        "summary_id": summary.map(|summary| summary.id.clone()),
        "quality": summary.map(|summary| summary.quality.clone()),
    })
}

fn summary_json(summary: &StoredSummary) -> Value {
    json!({
        "id": summary.id,
        "kind": summary.kind,
        "stage_id": summary.stage_id,
        "template_id": summary.template_id,
        "start": summary.start.to_rfc3339(),
        "end": summary.end.to_rfc3339(),
        "title": summary.title,
        "body_markdown": summary.body_markdown,
        "quality": summary.quality,
        "model": summary.model,
        "prompt_tokens": summary.prompt_tokens,
        "completion_tokens": summary.completion_tokens,
        "created_at": summary.created_at.to_rfc3339(),
    })
}
