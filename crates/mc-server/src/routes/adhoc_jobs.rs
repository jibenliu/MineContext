//! 异步总结作业接口。
//!
//! 长范围总结可能要跑几十秒。同步接口会让请求挂着（客户端超时、UI 无法显示进度），
//! 因此提供作业模型：立即返回 `job_id`，进度走 SSE，结果走查询。
//!
//! 与同步接口共用同一套分块与降级链 —— 作业只是「换个地方等」，
//! 内容产出的规则完全一样。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock};
use serde::Deserialize;
use serde_json::json;

use crate::envelope;
use crate::state::ServerState;

/// 事件名：进度帧。前端据此更新进度条。
pub const EVENT_SUMMARY_PROGRESS: &str = "summary:progress";

#[derive(Debug, Deserialize)]
pub struct AsyncAdhocBody {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub scope: Option<mc_summary::adhoc::AdhocScope>,
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub force_regenerate: bool,
}

/// `POST /api/v1/summaries/adhoc/jobs` —— 提交异步作业。
pub async fn submit(
    State(state): State<Arc<ServerState>>,
    payload: Result<Json<AsyncAdhocBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(body) = match payload {
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

    let request = match super::stages::adhoc_request_from_parts(
        &body.from,
        &body.to,
        body.scope,
        body.template_id
            .or_else(|| Some(super::stages::template_id_from_config(&state))),
        body.title,
        false,
        body.force_regenerate,
        super::stages::template_yaml_from_config(&state),
        &super::stages::adhoc_config_for(&state).timezone,
    ) {
        Ok(request) => request,
        Err(error) => return envelope::error_response(StatusCode::BAD_REQUEST, &error),
    };

    let config = super::stages::adhoc_config_for(&state);

    // 预览先算：空范围要立刻拒绝，不该占用一个作业
    let preview = match mc_summary::adhoc::preview(&state.db, &config, request.clone()) {
        Ok(preview) => preview,
        Err(error) => return envelope::error_response(StatusCode::BAD_REQUEST, &error),
    };
    if !preview.has_data {
        return envelope::error_response_with_data(
            StatusCode::UNPROCESSABLE_ENTITY,
            &AppError::new(
                ErrorCode::DomainNothingToDo,
                format!(
                    "范围 {} – {} 内没有可总结的内容",
                    preview.local_from, preview.local_to
                ),
            ),
            super::stages::preview_json_for(&preview),
        );
    }

    let template = match super::stages::resolve_template(
        &state,
        request.template_id.as_deref().unwrap_or_default(),
    ) {
        Ok(template) => template,
        Err(error) => return envelope::error_response(StatusCode::BAD_REQUEST, &error),
    };

    // 去重键：范围 + 筛选 + 模板（与缓存判据一致）
    let key = super::stages::adhoc_scope_key(&request, &config);
    let submitted = state
        .jobs
        .submit(key, preview.estimated_chunks, SystemClock.now());

    if submitted.deduped {
        // 已经在跑了：直接告诉调用方用哪个作业，不重复启动
        return envelope::ok(json!({
            "job_id": submitted.job_id,
            "deduped": true,
            "chunks_total": submitted.chunks_total,
            "state": "running",
        }));
    }

    let job_id = submitted.job_id.clone();
    let cancel = submitted.cancel.clone();
    let state_for_job = Arc::clone(&state);
    let job_id_for_task = job_id.clone();

    tokio::spawn(async move {
        let job_id = job_id_for_task;
        let source = crate::summary::source_for_with_template(&state_for_job, template);
        let now = SystemClock.now();
        state_for_job
            .jobs
            .mark_running(&job_id, preview.estimated_chunks);

        // 每完成一块：更新作业状态 + 推一帧进度（前端进度条靠它）
        let progress_state = Arc::clone(&state_for_job);
        let progress_job = job_id.clone();
        let mut on_progress = move |done: u32, total: u32| {
            progress_state
                .jobs
                .mark_progress(&progress_job, done, total);
            progress_state.publish(
                EVENT_SUMMARY_PROGRESS,
                json!({
                    "job_id": progress_job,
                    "chunks_done": done,
                    "chunks_total": total,
                    "progress": if total == 0 { 0.0 } else { done as f64 / total as f64 },
                }),
            );
        };

        let outcome = mc_summary::adhoc::generate_chunked_observed(
            &state_for_job.db,
            source.as_ref(),
            &config,
            request,
            now,
            &cancel,
            &mut on_progress,
        )
        .await;

        match outcome {
            Ok(mc_summary::adhoc::AdhocOutcome::Generated { summary_id, .. })
            | Ok(mc_summary::adhoc::AdhocOutcome::Cached { summary_id, .. }) => {
                state_for_job.jobs.mark_done(&job_id, &summary_id);
                super::stages::publish_adhoc(&state_for_job, &summary_id);
            }
            Ok(mc_summary::adhoc::AdhocOutcome::Cancelled { .. }) => {
                state_for_job.jobs.mark_cancelled(&job_id);
            }
            Ok(mc_summary::adhoc::AdhocOutcome::EmptyRange { .. }) => {
                state_for_job.jobs.mark_failed(&job_id, "范围内没有内容");
            }
            Err(error) => {
                state_for_job
                    .jobs
                    .mark_failed(&job_id, error.detail().to_string());
            }
        }
    });

    envelope::ok(json!({
        "job_id": job_id,
        "deduped": false,
        "chunks_total": submitted.chunks_total,
        "state": "running",
    }))
}

/// `GET /api/v1/summaries/adhoc/jobs/{job_id}` —— 查询进度与结果。
pub async fn status(State(state): State<Arc<ServerState>>, Path(job_id): Path<String>) -> Response {
    let Some(snapshot) = state.jobs.snapshot(&job_id) else {
        return envelope::error_response(
            StatusCode::NOT_FOUND,
            &AppError::new(
                ErrorCode::DomainNothingToDo,
                format!("作业 {job_id} 不存在"),
            ),
        );
    };

    let summary = snapshot.summary_id.as_deref().and_then(|id| {
        state
            .db
            .read_summaries(None, None)
            .ok()?
            .into_iter()
            .find(|s| s.id == id)
    });

    envelope::ok(json!({
        "job": snapshot,
        "summary": summary.map(|summary| super::stages::summary_json_for(&summary)),
    }))
}

/// `POST /api/v1/summaries/adhoc/jobs/{job_id}/cancel` —— 取消（已完成的块保留）。
pub async fn cancel(State(state): State<Arc<ServerState>>, Path(job_id): Path<String>) -> Response {
    if state.jobs.snapshot(&job_id).is_none() {
        return envelope::error_response(
            StatusCode::NOT_FOUND,
            &AppError::new(
                ErrorCode::DomainNothingToDo,
                format!("作业 {job_id} 不存在"),
            ),
        );
    }
    let accepted = state.jobs.cancel(&job_id);

    envelope::ok(json!({
        "job_id": job_id,
        "cancel_requested": accepted,
        "note": "已完成的块会保留，重新发起将继续",
    }))
}
