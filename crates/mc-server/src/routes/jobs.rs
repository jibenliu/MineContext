//! 作业队列的 HTTP 面：入队补偿推断、查作业状态。
//!
//! 队列的语义在 `mc-storage::jobs`（幂等、租约、崩溃后仍在），消费者在
//! `crate::jobs_worker`。这里只做两件事：把请求翻译成一次入队，把作业状态读出来。
//! **不提供「取消」**：补偿推断是幂等且可重跑的，取消的收益不抵它带来的状态复杂度。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use mc_common::error::AppError;
use mc_common::time::Clock;
use serde::Deserialize;

use crate::envelope;
use crate::state::ServerState;

#[derive(Debug, Deserialize)]
pub struct BackfillBody {
    /// RFC3339（与任意时段总结的入口一致，都按用户本地时区解释）
    pub from: String,
    pub to: String,
    /// 可选：给定时区（默认跟随 `general.timezone`）
    #[serde(default)]
    pub timezone: Option<String>,
}

/// `POST /api/v1/jobs/backfill` —— 入队一次补偿推断。
///
/// 同一范围重复提交命中的是**同一条**作业（幂等键 = 范围），
/// 返回值里 `deduped` 会说明这次是新建还是命中已有。
pub async fn enqueue_backfill(
    State(state): State<Arc<ServerState>>,
    payload: Result<Json<BackfillBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(body) = match payload {
        Ok(body) => body,
        Err(rejection) => {
            return envelope::error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                &AppError::new(
                    mc_common::error::ErrorCode::DomainInvalidRange,
                    format!("请求体无法解析：{rejection}"),
                ),
            )
        }
    };

    let timezone = body
        .timezone
        .unwrap_or_else(|| crate::routes::stages::effective_timezone(&state));
    // 与任意时段总结共用同一条解析（RFC3339 或 YYYY-MM-DD），
    // 两边对「用户输入的时间」理解必须一致。
    let (from, to) = match (
        crate::routes::stages::parse_range_bound(&body.from, &timezone, false),
        crate::routes::stages::parse_range_bound(&body.to, &timezone, true),
    ) {
        (Ok(from), Ok(to)) => (from, to),
        (Err(error), _) | (_, Err(error)) => {
            return envelope::error_response(StatusCode::BAD_REQUEST, &error)
        }
    };

    let now = Clock::now(&mc_common::time::SystemClock);
    match crate::jobs_worker::enqueue_backfill(&state, from, to, now) {
        Ok((job_id, deduped)) => envelope::ok(serde_json::json!({
            "job_id": job_id,
            "deduped": deduped,
            "from": from.as_millis(),
            "to": to.as_millis(),
        })),
        Err(error) => envelope::error_response(StatusCode::BAD_REQUEST, &error),
    }
}

/// `GET /api/v1/jobs/{id}` —— 作业状态（state / attempts / skip_reason / last_error）。
pub async fn job_status(State(state): State<Arc<ServerState>>, Path(id): Path<i64>) -> Response {
    match state.db.job(id) {
        Ok(Some(job)) => envelope::ok(crate::jobs_worker::job_json(&job)),
        Ok(None) => envelope::error_response(
            StatusCode::NOT_FOUND,
            &AppError::new(
                mc_common::error::ErrorCode::DomainNothingToDo,
                format!("作业 {id} 不存在"),
            ),
        ),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}
