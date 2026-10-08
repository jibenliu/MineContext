//! 活动接口。
//!
//! 兼容面 `/api/db/activities*` 的字段名、JSON 字符串列、时间格式
//! **不能改**：`pages/screen-monitor` 直接消费它们，改一个字段就是白屏。
//! 扩展面 `/api/v1/activities*` 才带 `origin` / `confidence` / `evidence`。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::{Clock, SystemClock, Timestamp};
use mc_domain::activity::OverrideKind;
use mc_domain::projector::ProjectionOptions;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::envelope;
use crate::state::ServerState;

pub fn router() -> Router<Arc<ServerState>> {
    Router::new()
        .route("/api/db/activities", get(list_legacy))
        .route("/api/db/activities/latest", get(latest_legacy))
        .route("/api/v1/activities", get(list_v1))
        .route("/api/v1/activities/overrides", post(record_override))
        .route("/api/v1/backfill", post(backfill))
}

#[derive(Debug, Deserialize)]
pub struct LegacyQuery {
    start: Option<String>,
    end: Option<String>,
}

/// 解析调用方会发来的两种时间写法之一：
/// `YYYY-MM-DD HH:mm:ss` 与 ISO 8601。
///
/// 解析必须宽容、但**不能猜**：看不懂直接报错，
/// 否则时间过滤会静默返回空列表 —— 那是「列表永远查不到东西」的成因。
fn parse_time(raw: &str) -> Result<Timestamp, AppError> {
    let trimmed = raw.trim();

    if let Ok(at) = Timestamp::parse_rfc3339(trimmed) {
        return Ok(at);
    }

    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(trimmed, format) {
            return Ok(Timestamp::from_millis(naive.and_utc().timestamp_millis()));
        }
    }

    if let Ok(date) = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        return Ok(Timestamp::from_millis(
            date.and_hms_opt(0, 0, 0)
                .expect("00:00:00 一定合法")
                .and_utc()
                .timestamp_millis(),
        ));
    }

    Err(AppError::new(
        ErrorCode::DomainInvalidTimestamp,
        format!("无法解析时间 `{raw}`（期望 ISO 8601 或 YYYY-MM-DD HH:mm:ss）"),
    ))
}

/// `GET /api/db/activities`（全部）与 `?start&end`（区间）。
pub async fn list_legacy(
    State(state): State<Arc<ServerState>>,
    Query(query): Query<LegacyQuery>,
) -> Response {
    let range = match (&query.start, &query.end) {
        (Some(start), end) => {
            let from = match parse_time(start) {
                Ok(from) => from,
                Err(error) => return envelope::compat_failure(&error),
            };
            // 没给 end 就当作「到无限远」
            let to = match end {
                Some(end) => match parse_time(end) {
                    Ok(to) => to,
                    Err(error) => return envelope::compat_failure(&error),
                },
                None => Timestamp::from_millis(i64::MAX),
            };
            Some((from, to))
        }
        (None, _) => None,
    };

    match mc_storage::projectors::activities::read_legacy(&state.db, range) {
        Ok(rows) => envelope::ok(serde_json::to_value(rows).unwrap_or(Value::Null)),
        Err(error) => envelope::compat_failure(&error),
    }
}

pub async fn latest_legacy(State(state): State<Arc<ServerState>>) -> Response {
    match mc_storage::projectors::activities::latest_legacy(&state.db) {
        Ok(row) => envelope::ok(serde_json::to_value(row).unwrap_or(Value::Null)),
        Err(error) => envelope::compat_failure(&error),
    }
}

/// `GET /api/v1/activities` —— 富字段版本。
pub async fn list_v1(State(state): State<Arc<ServerState>>) -> Response {
    let rows = match mc_storage::projectors::activities::read_all(&state.db) {
        Ok(rows) => rows,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };

    let activities: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "legacy_id": row.legacy_id,
                "start": row.start.to_rfc3339(),
                "end": row.end.to_rfc3339(),
                "title": row.title,
                "original_title": row.original_title,
                "category": row.category,
                "origin": row.origin,
                "confidence": row.confidence,
                "evidence": row.evidence,
                "is_user_modified": row.is_user_modified(),
                "derived_from_seq": row.derived_from_seq,
            })
        })
        .collect();

    // v1 也必须走同一个信封：前端 `invoke()` 读的是 `envelope.data`，
    // 直接返回裸 JSON 会让它拿到 undefined（静默空列表）。
    envelope::ok(json!({ "activities": activities }))
}

/// 用户修正请求：**不带 `at`** —— 「什么时候改的」由服务端记录：
/// 让前端填，客户端时钟不准时会写出错乱的顺序。
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OverrideRequest {
    Rename {
        activity_id: String,
        title: String,
    },
    SetCategory {
        activity_id: String,
        category: Option<String>,
    },
    Merge {
        primary: String,
        absorbed: Vec<String>,
    },
    Split {
        activity_id: String,
        at: Timestamp,
        tail_title: String,
    },
}

impl OverrideRequest {
    fn into_kind(self, at: Timestamp) -> OverrideKind {
        match self {
            Self::Rename { activity_id, title } => OverrideKind::Rename {
                activity_id,
                title,
                at,
            },
            Self::SetCategory {
                activity_id,
                category,
            } => OverrideKind::SetCategory {
                activity_id,
                category,
                at,
            },
            Self::Merge { primary, absorbed } => OverrideKind::Merge {
                primary,
                absorbed,
                at,
            },
            Self::Split {
                activity_id,
                at: split_at,
                tail_title,
            } => OverrideKind::Split {
                activity_id,
                at: split_at,
                tail_title,
            },
        }
    }
}

/// `POST /api/v1/backfill` —— 对「规则没判出来」的历史活动补跑一次推断。
///
/// **同步执行**，不进作业表：作业表没有生产者也没有 worker，
/// 往里塞一条只会得到「看起来排了队、实际没人做」。
/// 三道闸门（规则已判 / 预算 / 同窗口复用 + 滑动窗口限流）都在 worker 里，
/// 因此反复点也不会重复烧钱；返回这一轮**实际做了什么**，而不是「已提交」。
pub async fn backfill(State(state): State<Arc<ServerState>>) -> Response {
    let at = SystemClock.now();
    match crate::activities::infer_pending(&state, at).await {
        Ok(batch) => envelope::ok(json!({
            "suggestions": batch.suggestions.len(),
            "skipped": batch.skipped,
            "degraded": batch.degraded,
            "calls": batch.calls.len(),
        })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

/// `POST /api/v1/activities/overrides` —— 记录用户修正并立即重投影。
///
/// 修正**先作为事件落库**，再重算派生表。顺序不能反：
/// 事件是唯一真源，重算失败也只是「暂时没反映出来」，
/// 下次重放依然会把用户的修正带回来。
pub async fn record_override(
    State(state): State<Arc<ServerState>>,
    payload: Result<Json<OverrideRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(request) => request,
        Err(rejection) => {
            return envelope::error_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                &AppError::new(
                    ErrorCode::DomainInvalidOverride,
                    format!("修正请求无法解析：{rejection}"),
                ),
            )
        }
    };

    let at = SystemClock.now();
    if let Some(error) = validate(&request, at) {
        return envelope::error_response(StatusCode::UNPROCESSABLE_ENTITY, &error);
    }
    let kind = request.into_kind(at);

    let event = mc_storage::NewEvent::new(
        mc_domain::projector::KIND_ACTIVITY_OVERRIDDEN,
        at,
        serde_json::to_value(&kind).unwrap_or(Value::Null),
    )
    .by("user");

    if let Err(error) = state.db.append_events(&[event]) {
        return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }

    // 重投影：派生表随时可以丢，因此这里直接全量重算，不做增量补丁。
    // 重算失败不回滚事件 —— 事件已经在日志里，下次重放会带上。
    let rules = match crate::activities::load_rules(&state.data_dir) {
        Ok(rules) => rules,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let options = ProjectionOptions {
        policy: state.config.current().config.activity.aggregation_policy(),
        allow_inference: false,
    };

    match mc_storage::projectors::activities::replay(&state.db, &rules, options, at) {
        Ok(projection) => envelope::ok(json!({
            "applied": true,
            "activities": projection.activities.len(),
        })),
        // 事件已经落库，重算失败只是「暂时没反映出来」——
        // 返回 200 + applied:false，让前端提示而不是让它以为修正丢了。
        Err(error) => envelope::ok(json!({
            "applied": false,
            "reason": error.code().as_str(),
            "detail": error.detail(),
        })),
    }
}

/// 修正本身的合法性。校验放在**写日志之前**：
/// 非法修正写进去会污染唯一真源，而且没有任何办法清理。
fn validate(request: &OverrideRequest, at: Timestamp) -> Option<AppError> {
    let error = |detail: String| Some(AppError::new(ErrorCode::DomainInvalidOverride, detail));

    match request {
        OverrideRequest::Rename { title, .. } if title.trim().is_empty() => {
            error("标题不能为空".to_string())
        }
        OverrideRequest::Merge { primary, absorbed } if absorbed.is_empty() => {
            error(format!("合并 {primary} 时没有给出要被吸收的活动"))
        }
        OverrideRequest::Merge { primary, absorbed } if absorbed.iter().any(|id| id == primary) => {
            error(format!("主活动 {primary} 不能同时出现在被吸收列表里"))
        }
        OverrideRequest::Split { at: split_at, .. } if *split_at > at => {
            error(format!("切分点 {split_at} 晚于当前时间，切不出两段"))
        }
        _ => None,
    }
}
