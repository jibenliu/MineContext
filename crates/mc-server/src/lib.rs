//! `mc-server` — 本地控制面（HTTP + SSE）。
//!
//! 只监听 `127.0.0.1`，除 `/api/health` 外全部要求 `X-MC-Token`。
//!
//! 之所以选 HTTP 而不是绑定某个桌面框架的 IPC：集成测试可以直接对
//! `router()` 发请求，不需要启动 GUI。

pub mod activities;
pub mod capture;
pub mod capture_loop;
pub mod chat;
pub mod config_api;
pub mod context_lite;
pub mod domain_rules;
pub mod embedding;
pub mod envelope;
pub mod events;
pub mod failures;
pub mod file_ingest;
pub mod folder_ingest;
pub mod handoff_pack;
pub mod indexing;
pub mod jobs;
pub mod jobs_worker;
pub mod latest_activity;
pub mod learning_coach;
pub mod link_ingest;
pub mod local_text;
pub mod mcp;
pub mod mcp_serve;
pub mod middleware;
pub mod monitoring;
pub mod research_ingest;
pub mod retention;
pub mod retrieval;
pub mod risk_scan;
pub mod routes;
pub mod rss_ingest;
pub mod runtime;
pub mod sales_memory;
pub mod stages;
pub mod state;
pub mod summary;
pub mod task_assoc;
pub mod vault_backup;

pub use capture::CaptureControls;
pub use state::ServerState;

use std::sync::Arc;

use axum::routing::{any, get, post};
use axum::Router;

/// 兼容面路由契约（`scripts/extract-compat-routes.py` 产出）。
///
/// 编译期嵌入：契约成为二进制的一部分，运行时不会因为找不到文件而退化。
const COMPAT_ROUTES_JSON: &str = include_str!("../../../fixtures/contract/compat-routes.json");

/// 前端在用的路径里，已经有真实处理器的部分：**必须**登记在兼容面清单里。
const IMPLEMENTED_COMPAT_PATHS: &[&str] = &[
    "/api/health",
    "/health",
    // 对话接口（兼容面路径，名字不能改）
    "/api/agent/chat/stream",
    "/api/agent/chat/conversations",
    "/api/agent/chat/conversations/list",
    "/api/agent/chat/conversations/{cid}",
    "/api/agent/chat/conversations/{cid}/messages",
    "/api/agent/chat/messages/{mid}/interrupt",
    // 渲染层的写入路径（建消息 / 追分片 / 收尾 / 改标题 / 软删除对话）
    "/api/agent/chat/message/{mid}/create",
    "/api/agent/chat/message/stream/{mid}/create",
    "/api/agent/chat/message/{mid}/append",
    "/api/agent/chat/message/{mid}/update",
    "/api/agent/chat/message/{mid}/finished",
    "/api/agent/chat/conversations/{cid}/update",
    // 录制统计（兼容面路径）
    "/api/monitoring/recording-stats",
    // 设置页的模型配置（前端在用；get 不回明文，只回 hasApiKey + apiKeyMasked）
    "/api/model_settings/get",
    "/api/model_settings/update",
    "/api/model_settings/validate",
];

/// 新增接口，不属于前端在用的那套路径（因此不参与兼容面覆盖率的断言）。
const NEW_API_PATHS: &[&str] = &[
    "/api/diagnostics",
    "/api/v1/diagnostics/export",
    "/api/v1/stream",
    "/api/v1/search",
    // 作业队列（新增面；前端在用的那套接口里没有作业概念）
    "/api/v1/jobs/backfill",
    "/api/v1/jobs/{id}",
    // 向量索引暂停态 + 一键恢复（401/429 后不再静默空转）
    "/api/indexing/status",
    "/api/indexing/resume",
    // 设置页显式「复制」才取明文（仍要 token + 本机 Host；不属于旧兼容面）
    "/api/model_settings/api_key",
    // 设置页「允许 AI 出网」开关
    "/api/privacy",
    // MCP 插件管理与受控工具调用
    "/api/mcp",
    "/api/mcp/servers",
    "/api/mcp/servers/{id}",
    "/api/mcp/tools",
    "/api/mcp/tools/call",
    "/api/mcp/reload",
    "/api/mcp/serve",
    "/api/mcp/serve/rpc",
    // 以下不属于兼容面
    "/api/backend/status",
    "/api/capture/permissions",
    "/api/capture/targets",
    "/api/capture/status",
    "/api/capture/start",
    "/api/capture/stop",
    "/api/capture/screenshots",
    "/api/capture/screenshots/data",
    "/api/capture/now",
    "/api/capture/config",
    "/api/capture/targets/selection",
    // 以下不属于兼容面
    "/api/db/activities",
    "/api/db/activities/latest",
    "/api/v1/activities",
    "/api/v1/activities/overrides",
    "/api/v1/stages",
    "/api/v1/stages/{id}",
    "/api/v1/stages/{id}/close",
    "/api/v1/summaries",
    "/api/v1/summaries/{id}",
    "/api/v1/summaries/adhoc",
    "/api/v1/summaries/adhoc/preview",
    "/api/v1/summaries/adhoc/jobs",
    "/api/v1/summaries/adhoc/jobs/{job_id}",
    "/api/v1/summaries/adhoc/jobs/{job_id}/cancel",
    "/api/v1/summaries/{id}/regenerate",
    // 兼容面里没有「线索」概念，这是新增的检索面
    "/api/v1/threads",
    // context-lite：检索 + 引用 → 带 token 预算的上下文包
    "/api/v1/context/pack",
    // lite 任务关联（可纠正、可重放；非完整 Task OS）
    "/api/v1/tasks",
    "/api/v1/tasks/last",
    "/api/v1/tasks/sync",
    "/api/v1/tasks/correct",
    // 商业方向 3–6（本地启发式；隐私 fail-closed）
    "/api/v1/risks",
    "/api/v1/risks/report",
    "/api/v1/risks/dismiss",
    "/api/v1/sales/timeline",
    "/api/v1/sales/follow-ups",
    "/api/v1/sales/visit-prep",
    "/api/v1/learning/topics",
    "/api/v1/learning/stuck",
    "/api/v1/learning/review-plan",
    "/api/v1/handoff/candidates",
    "/api/v1/handoff/confirm",
    "/api/v1/handoff/export",
    // 以下不属于兼容面
    "/api/db/vaults",
    "/api/db/vaults/folders",
    "/api/db/vaults/{id}",
    "/api/db/vaults/{id}/soft-delete",
    "/api/db/vaults/{id}/restore",
    "/api/db/vaults/{id}/hard",
    // 首页数据面：任务 / 提示 / 热力图
    "/api/db/todos",
    "/api/db/todos/{id}",
    "/api/db/todos/{id}/toggle",
    "/api/db/tips",
    "/api/db/heatmap",
    // 上传文件与通用设置
    "/api/files",
    "/api/files/copy",
    "/api/files/{name}/data",
    // 文件上传进笔记树（与链接上传同形，不属于旧兼容面）
    "/api/v1/files/import",
    "/api/v1/files/import-folder",
    "/api/v1/files/track",
    "/api/v1/files/track/sync",
    "/api/v1/links",
    "/api/v1/rss",
    "/api/v1/research",
    // 笔记树 + uploads 备份（内容包，与诊断包不同）
    "/api/v1/vault/export",
    "/api/v1/vault/import",
    "/api/settings/{key}",
    // 首页「最新活动」推送开关
    "/api/v1/latest-activity/poll",
];

/// 构建控制面路由。
pub fn router(state: Arc<ServerState>) -> Router {
    let mut router = Router::new()
        .merge(routes::capture::router())
        .merge(routes::activities::router())
        .merge(routes::stages::router())
        .merge(routes::agent_chat::router())
        .merge(routes::threads::router())
        .merge(routes::vaults::router())
        .merge(routes::vault_backup::router())
        .merge(routes::links::router())
        .merge(routes::file_import::router())
        .merge(routes::folder_import::router())
        .merge(routes::rss::router())
        .merge(routes::research::router())
        .merge(routes::home::router())
        .merge(routes::files::router())
        .merge(routes::settings::router())
        .merge(routes::privacy::router())
        .merge(routes::mcp::router())
        .merge(routes::task_assoc::router())
        .merge(routes::risk_scan::router())
        .merge(routes::sales_memory::router())
        .merge(routes::learning_coach::router())
        .merge(routes::handoff_pack::router())
        .merge(monitoring::router())
        .merge(indexing::router())
        .route("/api/health", get(routes::health))
        .route("/health", get(routes::health))
        .route("/api/backend/status", get(routes::backend_status))
        .route("/api/diagnostics", get(routes::diagnostics))
        .route(
            "/api/v1/diagnostics/export",
            get(routes::diagnostics_export),
        )
        .route("/api/v1/stream", get(routes::stream))
        .route("/api/v1/search", get(routes::search))
        .route("/api/v1/context/pack", get(routes::context_pack))
        .route("/api/model_settings/get", get(routes::model_settings_get))
        .route(
            "/api/model_settings/update",
            post(routes::model_settings_update),
        )
        .route(
            "/api/model_settings/validate",
            post(routes::model_settings_validate),
        )
        .route(
            "/api/model_settings/api_key",
            get(routes::model_settings_api_key),
        )
        // 作业队列：入队补偿推断与查状态（消费者在 daemon 里，见 jobs_worker）
        .route(
            "/api/v1/jobs/backfill",
            post(routes::jobs::enqueue_backfill),
        )
        .route("/api/v1/jobs/{id}", get(routes::jobs::job_status));

    let contract_paths = compat_paths();

    // 新增接口不得与前端在用的路径撞名，否则会在路由注册时 panic（或更糟：静默覆盖语义）
    debug_assert!(
        NEW_API_PATHS
            .iter()
            .all(|path| !contract_paths.contains(&path.to_string())),
        "新增接口路径与前端在用的路径冲突：{NEW_API_PATHS:?}"
    );

    // 把契约里还没实现的路径也登记上，返回结构化「未实现」而不是 404。
    for path in contract_paths {
        if IMPLEMENTED_COMPAT_PATHS.contains(&path.as_str()) {
            continue;
        }
        router = router.route(&path, any(routes::compat_pending));
    }

    router
        .fallback(routes::not_found)
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            middleware::guard,
        ))
        // CORS 必须在鉴权**外面**：浏览器发的预检（OPTIONS）不带 token，
        // 让预检走到鉴权就永远是 401，于是渲染层的每个请求都被浏览器拦掉。
        // 桌面外壳的 webview 是跨源访问（`tauri://localhost` → `127.0.0.1`），
        // 带自定义头（`X-MC-Token`）就一定会触发预检，因此这不是可选项。
        .layer(cors_layer())
        .with_state(state)
}

/// 允许哪些来源跨源访问控制面。
///
/// 白名单而不是 `*`：`*` 会让任意网页都能读这台机器上的采集数据。
/// 允许的三种来源分别是：macOS/Linux 的 webview、Windows 的 webview、开发服务器。
fn cors_layer() -> tower_http::cors::CorsLayer {
    use axum::http::{header, HeaderValue, Method};
    use tower_http::cors::{AllowOrigin, CorsLayer};

    let origins = [
        "tauri://localhost",
        "http://tauri.localhost",
        "http://localhost:5173",
        "http://127.0.0.1:5173",
    ]
    .iter()
    .filter_map(|origin| HeaderValue::from_str(origin).ok())
    .collect::<Vec<_>>();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([
            header::CONTENT_TYPE,
            axum::http::HeaderName::from_static("x-mc-token"),
        ])
}

/// 契约中的全部路径（去重、并转换为 axum 的路径参数语法）。
pub fn compat_paths() -> Vec<String> {
    let parsed: serde_json::Value =
        serde_json::from_str(COMPAT_ROUTES_JSON).expect("兼容面契约必须是合法 JSON");

    let mut paths: Vec<String> = parsed["routes"]
        .as_array()
        .expect("契约必须包含 routes 数组")
        .iter()
        .filter_map(|route| route["path"].as_str())
        .map(normalize_path)
        .collect();

    paths.sort();
    paths.dedup();
    paths
}

/// FastAPI 的 `{name:path}` 转换器在 axum 里是 `{*name}`；
/// 其它 `{name:converter}` 直接降级为普通参数。
fn normalize_path(path: &str) -> String {
    let mut result = String::with_capacity(path.len());
    let mut chars = path.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            let mut inner = String::new();
            for next in chars.by_ref() {
                if next == '}' {
                    break;
                }
                inner.push(next);
            }
            match inner.split_once(':') {
                Some((name, "path")) => result.push_str(&format!("{{*{name}}}")),
                Some((name, _)) => result.push_str(&format!("{{{name}}}")),
                None => result.push_str(&format!("{{{inner}}}")),
            }
        } else {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_is_embedded_and_parsable() {
        let paths = compat_paths();
        assert!(
            paths.len() > 50,
            "兼容面契约条目过少（{}），契约文件可能损坏",
            paths.len()
        );
        assert!(paths.contains(&"/api/health".to_string()));
        assert!(paths.contains(&"/api/add_screenshot".to_string()));
    }

    #[test]
    fn fastapi_path_converter_is_normalized() {
        assert_eq!(
            normalize_path("/files/{file_path:path}"),
            "/files/{*file_path}"
        );
        assert_eq!(normalize_path("/a/{b}"), "/a/{b}");
        assert_eq!(normalize_path("/x/{y:int}"), "/x/{y}");
    }

    #[test]
    fn implemented_compat_paths_are_in_the_contract() {
        let paths = compat_paths();
        for path in IMPLEMENTED_COMPAT_PATHS {
            assert!(
                paths.contains(&path.to_string()),
                "{path} 声明为已实现的兼容面路由，但不在契约里"
            );
        }
    }

    #[test]
    fn new_api_paths_are_not_part_of_the_legacy_contract() {
        let paths = compat_paths();
        for path in NEW_API_PATHS {
            assert!(
                !paths.contains(&path.to_string()),
                "{path} 是新增接口，不应出现在兼容面清单里（否则说明契约抽取或命名有误）"
            );
        }
    }
}
