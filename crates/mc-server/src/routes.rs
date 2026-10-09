//! 控制面路由。
//!
//! 三件事：健康检查（启动握手）、诊断（错误可见）、SSE 事件流
//! （推送走 SSE，不用轮询）。

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::stream;
use futures_util::StreamExt;
use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use serde_json::{json, Value};

use crate::envelope;
use crate::state::ServerState;

/// `GET /api/health` —— 启动握手。**不需要 token**，但只返回最小信息。
pub async fn health(State(state): State<Arc<ServerState>>) -> Response {
    Json(health_payload(&state)).into_response()
}

/// 健康检查的负载。
///
/// SSE 的启动帧（`push:init-check-data`）与这里**同源**：渲染层靠它判断
/// 「模型是否已配置」，从而决定进主界面还是进引导页。两处各写一份的话，
/// 前端看到的"是否已配置"会与 `/api/health` 不一致 —— 而这种不一致表现为
/// 「明明配好了模型，每次启动还是被要求重新选一遍」。
fn health_payload(state: &ServerState) -> Value {
    let config = state.config.current();

    let vision_configured =
        !config.config.ai.vision.base_url.is_empty() && !config.config.ai.vision.model.is_empty();
    let embedding_configured = !config.config.ai.embedding.base_url.is_empty()
        && !config.config.ai.embedding.model.is_empty();

    let llm_status = if vision_configured {
        "ok"
    } else {
        "unconfigured"
    };

    let capture_status = match state.capture.as_ref() {
        Some(controls) if controls.is_running() => "running",
        Some(_) => "stopped",
        None => "not_started",
    };

    json!({
        "status": "ok",
        "version": state.version,
        "data": {
            "components": {
                // 渲染层读的是 data.components.llm，键名不能改
                "llm":       { "status": llm_status, "message": if vision_configured { "" } else { "未配置视觉模型" } },
                "vlm":       { "status": llm_status },
                "embedding": { "status": if embedding_configured { "ok" } else { "unconfigured" } },
                "storage":   { "status": "ok" },
                // 采集是否在跑要如实报告：恒 "not_started" 会让诊断页看到假状态
                "capture":   { "status": capture_status }
            },
            "degraded": config.warnings.iter().map(|w| w.path.clone()).collect::<Vec<_>>()
        },
        "error_code": Value::Null
    })
}

/// `GET /api/backend/status` —— 渲染层的启动握手。
///
/// 渲染层据此决定「进主界面还是停在加载页」：这个回答来自**正在运行的
/// daemon 自己**，所以 `running` 不是推测出来的。端口不在这里重复报 ——
/// 渲染层从 `runtime.json` 拿到的那个才是权威，两处各报一次只会不一致。
pub async fn backend_status() -> Response {
    envelope::ok(json!({ "status": "running" }))
}

/// 平台支持范围 → JSON（`/api/diagnostics` 与诊断包共用同一份判定）。
fn platform_json() -> Value {
    match mc_common::platform::current() {
        Some(version) => {
            let report = mc_common::platform::support_report(version);
            json!({
                "os": std::env::consts::OS,
                "version": version.to_string(),
                "minimum": mc_common::platform::MIN_SUPPORTED_MACOS.to_string(),
                "supported": report.supported,
                "message": report.message,
            })
        }
        None => {
            // 非 macOS（或 sw_vers 不可用）：结论必须是布尔值，不能用 null
            // —— 诊断页要直接告诉用户「当前平台不受支持」。
            let os = std::env::consts::OS;
            json!({
                "os": os,
                "version": Value::Null,
                "minimum": mc_common::platform::MIN_SUPPORTED_MACOS.to_string(),
                "supported": false,
                "message": format!(
                    "MineContext 需要 macOS {} 及以上（当前平台：{os}）",
                    mc_common::platform::MIN_SUPPORTED_MACOS
                ),
            })
        }
    }
}

/// 诊断包的 schema 版本。字段增删都要改它 —— 诊断包会被存下来发给别人，
/// 半年后再看必须有版本可依。
pub const DIAGNOSTICS_EXPORT_SCHEMA: &str = "mc-diagnostics/1";

/// 刻意**不**放进诊断包的东西。
///
/// 写进响应体而不是只写在文档里：用户在粘贴给别人之前，能自己看到边界。
const EXPORT_EXCLUDED: &[&str] = &[
    "窗口标题与活动标题",
    "无障碍 / OCR 文本内容",
    "文件路径（含截图相对路径与数据目录绝对路径）",
    "API Key、token 与密钥引用",
    "截图内容本身",
];

/// `GET /api/v1/diagnostics/export` —— 可安全分享的诊断包。
///
/// 与 [`diagnostics`] 的区别：那边是给自己看的（随时可能长出新字段），
/// 这边是**要发给别人**的，因此只放「稳定错误码 + 计数 + 状态」。
/// `pipeline_failures` 只带用户文案与建议，`context`（技术细节，可能含路径）不进。
pub async fn diagnostics_export(State(state): State<Arc<ServerState>>) -> Response {
    let config = state.config.current();
    let configured =
        !config.config.ai.vision.base_url.is_empty() && !config.config.ai.vision.model.is_empty();

    // 待推断积压：为一个计数不值得全量读活动表，直接查 origin 列。
    fn pending_inference(state: &ServerState) -> i64 {
        state
            .db
            .with_read(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM activities WHERE origin = 'observed'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .unwrap_or(0)
    }

    // 计数真的查库：诊断包里写死的 0 比没有更糟（看起来像「什么都没采到」）。
    //
    // 表名走白名单常量而不是 `format!` 拼接：现在是硬编码字面量所以没有注入风险，
    // 但拼接写法一旦被后来者传进参数就变成注入 —— 这里从形状上排除掉。
    let count = |table: &str| -> i64 {
        let sql = match table {
            "observations" => "SELECT COUNT(*) FROM observations",
            "events" => "SELECT COUNT(*) FROM events",
            "activities" => "SELECT COUNT(*) FROM activities",
            "stages" => "SELECT COUNT(*) FROM stages",
            "summaries" => "SELECT COUNT(*) FROM summaries",
            other => {
                debug_assert!(false, "未登记的表名：{other}");
                return 0;
            }
        };
        state
            .db
            .with_read(|conn| conn.query_row(sql, [], |row| row.get::<_, i64>(0)))
            .unwrap_or(0)
    };

    let stages_without_summary = state
        .db
        .with_read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM stages s
                 WHERE s.state = 'closed'
                   AND NOT EXISTS (SELECT 1 FROM summaries m WHERE m.stage_id = s.id)",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .unwrap_or(0);

    let failures: Vec<Value> = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT at_utc_ms, component, error_code, severity, message, remediation
                 FROM pipeline_failures ORDER BY at_utc_ms DESC LIMIT 20",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(json!({
                    "at_utc_ms": row.get::<_, i64>(0)?,
                    "component": row.get::<_, String>(1)?,
                    "error_code": row.get::<_, String>(2)?,
                    "severity": row.get::<_, String>(3)?,
                    "message": row.get::<_, String>(4)?,
                    "remediation": row.get::<_, Option<String>>(5)?,
                }))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();

    let capture = match state.capture.as_ref() {
        None => json!({ "status": "unavailable" }),
        Some(controls) => {
            let health = controls.source.health().await;
            json!({
                "status": if controls.is_running() { "running" } else { "stopped" },
                "permission": format!("{:?}", health.permission).to_lowercase(),
                "available": health.available,
                "stats": match state.capture_stats() {
                    Some(stats) => json!({
                        "persisted": stats.persisted,
                        "unchanged": stats.unchanged,
                        "throttled": stats.throttled,
                        "failed": stats.failed,
                        "privacy_blocked": stats.privacy_blocked,
                        "dropped": stats.dropped,
                    }),
                    None => Value::Null,
                },
            })
        }
    };

    // 走统一信封：适配层（HttpBackend）按 `{code, data}` 解析，
    // 裸对象会让「有没有 code」变成接口之间的差异
    envelope::ok(json!({
        "schema_version": DIAGNOSTICS_EXPORT_SCHEMA,
        "generated_at": mc_common::time::Clock::now(&mc_common::time::SystemClock).as_millis(),
        "version": state.version,
        "uptime_seconds": state.uptime_seconds(),
        "platform": platform_json(),
        "components": {
            "storage": { "status": "ok", "read_only": state.db.is_read_only() },
            "provider": {
                "status": if configured { "ok" } else { "unconfigured" },
                // 模型名是配置项，不是内容：留着能判断「是不是模型配错了」
                "model": config.config.ai.vision.model,
            },
            "capture": capture,
        },
        "counts": {
            "observations": count("observations"),
            "events": count("events"),
            "activities": count("activities"),
            "stages": count("stages"),
            "summaries": count("summaries"),
            // 待推断积压：origin 仍是 observed 的活动（vision 推断的输入量）。
            // 用户报「一个 job 跑几小时」时，第一眼要看的就是这个数。
            "pending_inference": pending_inference(&state),
        },
        "invariants": {
            "stages_without_summary": stages_without_summary,
        },
        "retention": match state.last_retention_run() {
            Some(snapshot) => json!({
                "last_run_at": snapshot.ran_at.to_rfc3339(),
                "deleted_files": snapshot.outcome.deleted_files,
                "freed_bytes": snapshot.outcome.freed_bytes,
                "kept_files": snapshot.outcome.kept_files,
            }),
            None => Value::Null,
        },
        "recent_failures": failures,
        "excluded": EXPORT_EXCLUDED,
    }))
}

/// `GET /api/diagnostics` —— 组件健康、不变量、队列水位、最近失败。
pub async fn diagnostics(State(state): State<Arc<ServerState>>) -> Response {
    let config = state.config.current();

    let vision_configured =
        !config.config.ai.vision.base_url.is_empty() && !config.config.ai.vision.model.is_empty();

    // 真实查询，不是写死：closed 阶段里没有对应总结的条数。
    // 非 0 就说明「有阶段没总结」这条最高优先级不变量被破坏。
    let stages_without_summary = state
        .db
        .with_read(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM stages s
                 WHERE s.state = 'closed'
                   AND NOT EXISTS (SELECT 1 FROM summaries m WHERE m.stage_id = s.id)",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .unwrap_or(0);

    let queue_depth = |kind: &str| -> i64 {
        state
            .db
            .with_read(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM jobs WHERE kind = ?1 AND state IN ('queued','running')",
                    [kind],
                    |row| row.get::<_, i64>(0),
                )
            })
            .unwrap_or(0)
    };

    let failures: Vec<Value> = state
        .db
        .with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT at_utc_ms, component, error_code, severity, message, remediation
                 FROM pipeline_failures ORDER BY at_utc_ms DESC LIMIT 20",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok(json!({
                    "at_utc_ms": row.get::<_, i64>(0)?,
                    "component": row.get::<_, String>(1)?,
                    "error_code": row.get::<_, String>(2)?,
                    "severity": row.get::<_, String>(3)?,
                    "message": row.get::<_, String>(4)?,
                    "remediation": row.get::<_, Option<String>>(5)?,
                }))
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .unwrap_or_default();

    let platform = platform_json();

    let retention = match state.last_retention_run() {
        Some(snapshot) => json!({
            "last_run_at": snapshot.ran_at.to_rfc3339(),
            "deleted_files": snapshot.outcome.deleted_files,
            "freed_bytes": snapshot.outcome.freed_bytes,
            "kept_files": snapshot.outcome.kept_files,
            "cleared_references": snapshot.outcome.cleared_references,
        }),
        None => json!({
            "last_run_at": Value::Null,
            "deleted_files": 0,
            "freed_bytes": 0,
            "kept_files": 0,
            "cleared_references": 0,
        }),
    };

    let capture_status = match state.capture.as_ref() {
        None => "unavailable",
        Some(controls) if controls.is_running() => "running",
        Some(_) => "stopped",
    };

    // 统一信封：`/api/diagnostics` 与其它接口一样返回 `{code, data}`。
    // 此前它是**裸对象**，只有它和 health 不一致 —— 消费方要按接口分别判形状。
    envelope::ok(json!({
        "version": state.version,
        "platform": platform,
        "uptime_seconds": state.uptime_seconds(),
        "components": {
            "storage":  { "status": "ok", "read_only": state.db.is_read_only() },
            "provider": {
                "status": if vision_configured { "ok" } else { "unconfigured" },
                "provider": "openai_compatible",
                "model": config.config.ai.vision.model,
            },
            "capture":  {
                "status": capture_status,
                "stats": match state.capture_stats() {
                    Some(stats) => json!({
                        "persisted": stats.persisted,
                        "unchanged": stats.unchanged,
                        "throttled": stats.throttled,
                        "failed": stats.failed,
                        "privacy_blocked": stats.privacy_blocked,
                        "dropped": stats.dropped,
                    }),
                    None => Value::Null,
                },
            },
            "stage":    { "status": "not_started" }
        },
        "invariants": {
            "stages_without_summary": stages_without_summary
        },
        "queues": {
            "capture": { "depth": 0, "inflight": 0, "capacity": config.config.capture.capture_queue_capacity },
            "vision":  { "depth": queue_depth("vision_l2"), "inflight": 0, "capacity": 0 },
            "summary": { "depth": queue_depth("summary_stage") + queue_depth("summary_adhoc"), "inflight": 0, "capacity": 0 }
        },
        "retention": retention,
        "warnings": config.warnings,
        "recent_failures": failures
    }))
}

/// `GET /api/v1/stream` —— SSE。首帧 `ready` 之后转发事件总线上的事件。
///
/// 前端 `channel-map.ts` 里的订阅渠道（`push:*`）与总结/活动事件
/// 都走这一条连接：**一条连接、多路分发**，避免每个渠道各开一条 SSE。
pub async fn stream(State(state): State<Arc<ServerState>>) -> Response {
    let ready = json!({
        "port": 0,
        "version": state.version,
        "token_ok": true
    });

    let head = stream::iter(vec![
        Ok::<Event, Infallible>(Event::default().event("ready").data(ready.to_string())),
        // 启动帧：渲染层的 `getInitCheckData` 订阅的就是这个事件名，用来决定
        // 「进主界面还是进引导页」。**只发一次**（连接建立时）—— 不发的话，
        // 已经配好模型的用户每次启动都会被要求重新选一遍模型。
        Ok::<Event, Infallible>(
            Event::default()
                .event(crate::events::EVENT_PUSH_INIT_CHECK_DATA)
                .data(health_payload(&state).to_string()),
        ),
    ]);

    // 广播接收器 → SSE 帧。掉帧（Lagging）时补一个提示帧，
    // 让前端知道该做一次全量拉取，而不是默默不同步。
    let receiver = state.events.subscribe();
    let tail = stream::unfold(receiver, |mut receiver| async move {
        match receiver.recv().await {
            Ok(event) => {
                let frame = Event::default()
                    .event(event.kind)
                    .data(event.data.to_string());
                Some((Ok::<Event, Infallible>(frame), receiver))
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                // 掉帧时如实告知，让前端做一次全量拉取 —— 静默不同步更难查
                let frame = Event::default()
                    .event("stream:lagged")
                    .data(json!({ "skipped": skipped, "hint": "请做一次全量拉取" }).to_string());
                Some((Ok::<Event, Infallible>(frame), receiver))
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
        }
    });

    Sse::new(head.chain(tail))
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// 已登记但尚未实现的兼容面路径。
///
/// 返回结构化「未实现」而不是 404：调用方拿到 `{code:1, error_code:"not_implemented"}`
/// 能明确知道自己撞上了什么，而不是 axios 抛一个没有上下文的 404。
pub async fn compat_pending(
    axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
) -> Response {
    envelope::not_implemented(uri.path())
}

/// 完全未知的路径：这里返回真 404。
///
/// 这一点很重要 —— 如果所有路径都由兜底接管，`compat_routes_all_present`
/// 这类契约测试就永远通过、毫无意义。保留 404 才能让契约测试「可失败」。
pub async fn not_found() -> Response {
    envelope::not_found()
}

pub mod activities;
pub mod adhoc_jobs;
pub mod agent_chat;
pub mod capture;
pub mod files;
pub mod home;
pub mod jobs;
pub mod links;
pub mod privacy;
pub mod settings;
pub mod stages;
pub mod threads;
pub mod vaults;

// ---------------------------------------------------------------- 检索

#[derive(Debug, serde::Deserialize)]
pub struct SearchParams {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub limit: Option<usize>,
    /// 时间过滤（毫秒时间戳，包含边界）
    #[serde(default)]
    pub start: Option<i64>,
    #[serde(default)]
    pub end: Option<i64>,
    #[serde(default)]
    pub observed_only: Option<bool>,
}

/// `GET /api/v1/search?q=&limit=&start=&end=&observed_only=` —— 检索入口。
///
/// 与聊天流**用同一条检索路径**（`retrieval::retrieve_with_vectors`），
/// 因此隐私语义天然一致：被拦截的内容不在文档集里，谁也搜不到。
/// 时间过滤在排名与截断之前执行；接口的结束时间包含边界毫秒。
pub async fn search(
    State(state): State<Arc<ServerState>>,
    axum::extract::Query(params): axum::extract::Query<SearchParams>,
) -> Response {
    let query = params.q.trim();
    if query.is_empty() {
        return envelope::error_response(
            StatusCode::BAD_REQUEST,
            &AppError::new(ErrorCode::ConfigInvalid, "检索词不能为空（q=）"),
        );
    }

    if matches!((params.start, params.end), (Some(start), Some(end)) if start > end) {
        return envelope::error_response(
            StatusCode::BAD_REQUEST,
            &AppError::new(ErrorCode::DomainInvalidRange, "开始时间不能晚于结束时间"),
        );
    }
    let limit = params.limit.unwrap_or(20).clamp(1, 100);
    let hits = match crate::retrieval::retrieve_filtered(
        &state.db,
        query,
        limit,
        mc_search::SearchFilters {
            from: params.start.map(Timestamp::from_millis),
            to: params
                .end
                .and_then(|end| end.checked_add(1))
                .map(Timestamp::from_millis),
            observed_only: params.observed_only.unwrap_or(false),
            ..mc_search::SearchFilters::default()
        },
        None,
    ) {
        Ok(hits) => hits,
        Err(error) => return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };

    let results: Vec<Value> = hits
        .into_iter()
        .map(|hit| {
            json!({
                "id": hit.document.id,
                "kind": hit.document.kind.as_str(),
                "title": crate::retrieval::citations(std::slice::from_ref(&hit))
                    .first()
                    .map(|citation| citation.title.clone())
                    .unwrap_or_default(),
                "snippet": hit.document.text.chars().take(200).collect::<String>(),
                "score": hit.score,
                "at": hit.document.at.as_millis(),
            })
        })
        .collect();

    envelope::ok(json!({ "query": query, "results": results }))
}

// ---------------------------------------------------------------- 模型设置

/// 模型设置请求体（字段名与形状跟前端已有的 `ModelSettingsVO` 一致）。
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelSettingsBody {
    pub model_platform: String,
    pub model_id: String,
    pub base_url: String,
    pub api_key: String,
    pub embedding_model_platform: String,
    pub embedding_model_id: String,
    pub embedding_base_url: String,
    pub embedding_api_key: String,
}

impl ModelSettingsBody {
    fn from_config(config: &mc_config::Config) -> Self {
        Self {
            model_platform: String::new(),
            model_id: config.ai.vision.model.clone(),
            base_url: config.ai.vision.base_url.clone(),
            // 明文密钥**不进 get**：前端用 hasApiKey + apiKeyMasked 回显
            api_key: String::new(),
            embedding_model_platform: String::new(),
            embedding_model_id: config.ai.embedding.model.clone(),
            embedding_base_url: config.ai.embedding.base_url.clone(),
            embedding_api_key: String::new(),
        }
    }

    /// 形状校验。**不**调用远程模型：那需要网络与真实密钥，
    /// 失败原因也未必是配置错（可能是限流）。真正的连通性看诊断页。
    fn shape_problem(&self) -> Option<String> {
        if self.model_id.trim().is_empty() {
            return Some("VLM 模型名不能为空".to_string());
        }
        if self.embedding_model_id.trim().is_empty() {
            return Some("Embedding 模型名不能为空".to_string());
        }
        for (label, url) in [
            ("VLM Base URL", &self.base_url),
            (
                "Embedding Base URL",
                if self.embedding_base_url.trim().is_empty() {
                    &self.base_url
                } else {
                    &self.embedding_base_url
                },
            ),
        ] {
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Some(format!("{label} 必须以 http:// 或 https:// 开头"));
            }
        }
        None
    }
}

/// 请求体形状同前端已有的 `UpdateModelSettingsRequest`：`{config: {...}}`。
#[derive(Debug, serde::Deserialize)]
pub struct ModelSettingsRequest {
    pub config: ModelSettingsBody,
}

/// `GET /api/model_settings/get` —— 设置页读当前模型配置。
///
/// `apiKey` 字段始终为空（兼容旧表单形状）；是否已配置看 `hasApiKey`，
/// 回显用 `apiKeyMasked`（首尾可见、中间打码）。明文只走专用复制接口。
pub async fn model_settings_get(State(state): State<Arc<ServerState>>) -> Response {
    let config = state.config.current();
    let body = ModelSettingsBody::from_config(&config.config);
    // hasApiKey 以「能读到明文」为准：仅有 api_key_ref 但 sidecar/钥匙串都空时，
    // 复制接口也会 404，UI 不应假装「已配置可复制」。
    let stored = read_stored_model_api_key(&state).ok().flatten();
    let has_api_key = stored.is_some();
    let masked = stored
        .as_ref()
        .map(|key| mask_api_key(key))
        .unwrap_or_default();

    Json(json!({
        "code": 0,
        "status": 200,
        "message": "success",
        "data": {
            "config": body,
            "hasApiKey": has_api_key,
            "apiKeyMasked": masked,
        },
        "error_code": Value::Null,
        "remediation": Value::Null,
    }))
    .into_response()
}

/// `GET /api/model_settings/api_key` —— 设置页「复制」用：返回已存明文。
///
/// 仅本机 + token（与其它控制面相同）。**不要**把这条并进 get：
/// get 的契约是「永不回传明文」，复制是用户显式动作。
pub async fn model_settings_api_key(State(state): State<Arc<ServerState>>) -> Response {
    match read_stored_model_api_key(&state) {
        Ok(Some(api_key)) => envelope::ok(json!({ "apiKey": api_key })),
        Ok(None) => envelope::error_response(
            StatusCode::NOT_FOUND,
            &AppError::new(ErrorCode::ConfigInvalid, "尚未保存 API Key".to_string()),
        ),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

/// 脱敏：保留首尾各 4 个字符，中间用 • 代替；短密钥整段打码。
fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "••••••••".to_string();
    }
    let head: String = chars.iter().take(4).collect();
    let tail: String = chars.iter().rev().take(4).rev().collect();
    format!("{head}••••••••{tail}")
}

/// 诊断 / 录制统计用：能否读到已存明文（不回传内容）。
pub(crate) fn read_stored_model_api_key_for_diagnostics(state: &ServerState) -> bool {
    matches!(read_stored_model_api_key(state), Ok(Some(_)))
}

/// 已存密钥：优先读 0600 sidecar，再尝试钥匙串引用。
fn read_stored_model_api_key(state: &ServerState) -> Result<Option<String>, AppError> {
    if let Some(from_sidecar) = read_model_key_sidecar(state)? {
        return Ok(Some(from_sidecar));
    }
    let config = state.config.current();
    let key_ref = config.config.ai.vision.api_key_ref.as_deref().or(config
        .config
        .ai
        .embedding
        .api_key_ref
        .as_deref());
    let secrets = mc_providers::credentials::KeychainCommand::default();
    mc_providers::credentials::resolve_secret(&secrets, key_ref)
}

fn read_model_key_sidecar(state: &ServerState) -> Result<Option<String>, AppError> {
    let path = state.data_dir.join("model-keys.json");
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|error| {
        AppError::new(
            ErrorCode::StorageUnavailable,
            format!("无法读取密钥文件 {}: {error}", path.display()),
        )
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        AppError::new(
            ErrorCode::StorageUnavailable,
            format!("密钥文件格式无效 {}: {error}", path.display()),
        )
    })?;
    Ok(value
        .get("api_key")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string))
}

/// `POST /api/model_settings/validate` —— 只校验形状，返回 `{valid, message}`。
pub async fn model_settings_validate(
    State(state): State<Arc<ServerState>>,
    Json(request): Json<ModelSettingsRequest>,
) -> Response {
    let body = request.config;
    let problem = body.shape_problem();
    let _ = state;
    envelope::ok(json!({
        "valid": problem.is_none(),
        "message": problem.unwrap_or_else(|| "配置形状合法（未调用模型验证连通性）".to_string()),
    }))
}

/// `POST /api/model_settings/update` —— 写进用户配置层并热重载。
///
/// 密钥处理是这里唯一需要小心的部分：**明文永远不进配置文件**，
/// 而是写进 0600 的 sidecar（`model-keys.json`）等用户导入 Keychain，
/// 配置里只留 `api_key_ref`。前端把密钥留在输入框里就行，不需要读回。
pub async fn model_settings_update(
    State(state): State<Arc<ServerState>>,
    Json(request): Json<ModelSettingsRequest>,
) -> Response {
    let body = request.config;
    if let Some(problem) = body.shape_problem() {
        return envelope::error_response(
            StatusCode::BAD_REQUEST,
            &AppError::new(ErrorCode::ConfigInvalid, problem),
        );
    }

    let key_ref = "keychain:mc:model".to_string();
    let mut patch = json!({
        "ai": {
            "vision": {
                "base_url": body.base_url,
                "model": body.model_id,
                "api_key_ref": key_ref,
            },
            "embedding": {
                "base_url": if body.embedding_base_url.trim().is_empty() {
                    body.base_url.clone()
                } else {
                    body.embedding_base_url.clone()
                },
                "model": body.embedding_model_id,
                "api_key_ref": key_ref,
            },
        }
    });

    // 没给密钥就沿用原来的引用，别把已配好的模型弄坏
    let api_key = if body.api_key.trim().is_empty() {
        body.embedding_api_key.trim()
    } else {
        body.api_key.trim()
    };
    if api_key.is_empty() {
        let current = state.config.current();
        let existing = current
            .config
            .ai
            .vision
            .api_key_ref
            .clone()
            .or_else(|| current.config.ai.embedding.api_key_ref.clone());

        match existing {
            // 没给新密钥但原来就配过：沿用旧引用，别把已配好的模型弄坏
            Some(existing) => {
                patch["ai"]["vision"]["api_key_ref"] = json!(existing);
                patch["ai"]["embedding"]["api_key_ref"] = json!(existing);
            }
            // 全新配置且没给密钥：**拒绝**而不是写一个空引用 ——
            // 写空引用会让配置看起来「保存成功」，实际调用模型时才失败
            None => {
                return envelope::error_response(
                    StatusCode::BAD_REQUEST,
                    &AppError::new(
                        ErrorCode::ConfigInvalid,
                        "请填写 API Key：它会写入 0600 的 model-keys.json 供导入钥匙串，                         配置文件里只保存引用",
                    ),
                )
            }
        }
    }

    let sidecar_note = if api_key.is_empty() {
        None
    } else {
        match write_model_key_sidecar(&state, api_key) {
            Ok(path) => Some(path),
            Err(error) => {
                return envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error)
            }
        }
    };

    match crate::config_api::apply_patch(&state, patch) {
        Ok(_) => envelope::ok(json!({
            "saved": true,
            "keySidecar": sidecar_note,
        })),
        Err(error) => envelope::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

/// 把明文密钥写进 0600 的 sidecar（绝不进 config.toml）。
fn write_model_key_sidecar(state: &Arc<ServerState>, api_key: &str) -> Result<String, AppError> {
    let path = state.data_dir.join("model-keys.json");
    let payload = json!({
        "note": "由设置页写入：请把密钥导入系统钥匙串，然后删除本文件",
        "account": "keychain:mc:model",
        "api_key": api_key,
    })
    .to_string();

    mc_common::fs::write_private_str(&path, &payload).map_err(|error| {
        AppError::new(
            ErrorCode::StorageUnavailable,
            format!("无法写入密钥文件 {}: {error}", path.display()),
        )
    })?;

    Ok(path.display().to_string())
}
