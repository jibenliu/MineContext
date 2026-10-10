//! `/api/mcp/*` —— MCP 插件管理：列表/增删改、启停、工具列表、受控调用。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::Json;
use mc_common::error::{AppError, ErrorCode};
use mc_config::model::{McpServerConfig, McpTransportKind};
use mc_mcp::types::McpToolCall;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::envelope;
use crate::mcp::{auth_context_from_config, reload_registry};
use crate::state::ServerState;

pub fn router() -> axum::Router<Arc<ServerState>> {
    use axum::routing::{delete, get, post, put};
    axum::Router::new()
        .route("/api/mcp", get(get_mcp).put(put_mcp_master))
        .route("/api/mcp/servers", get(list_servers).post(upsert_server))
        .route("/api/mcp/servers/{id}", put(upsert_server_by_id))
        .route("/api/mcp/servers/{id}", delete(delete_server))
        .route("/api/mcp/tools", get(list_tools))
        .route("/api/mcp/tools/call", post(call_tool))
        .route("/api/mcp/reload", post(reload))
}

async fn get_mcp(State(state): State<Arc<ServerState>>) -> Response {
    let config = state.config.current();
    envelope::ok(json!({
        "enabled": config.config.mcp.enabled,
        "servers": config.config.mcp.servers.iter().map(server_public).collect::<Vec<_>>(),
        "ai_upload": config.config.privacy.ai_upload,
    }))
}

#[derive(Debug, Deserialize)]
struct MasterPatch {
    enabled: bool,
}

async fn put_mcp_master(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<MasterPatch>,
) -> Response {
    match apply_mcp_patch(&state, json!({ "enabled": body.enabled })).await {
        Ok(data) => envelope::ok(data),
        Err(error) => envelope::error_response(StatusCode::BAD_REQUEST, &error),
    }
}

async fn list_servers(State(state): State<Arc<ServerState>>) -> Response {
    let config = state.config.current();
    envelope::ok(json!(config
        .config
        .mcp
        .servers
        .iter()
        .map(server_public)
        .collect::<Vec<_>>()))
}

#[derive(Debug, Deserialize)]
struct ServerBody {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    transport: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    env: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    requires_network: Option<bool>,
}

async fn upsert_server(
    State(state): State<Arc<ServerState>>,
    Json(body): Json<ServerBody>,
) -> Response {
    upsert_impl(&state, body).await
}

async fn upsert_server_by_id(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
    Json(mut body): Json<ServerBody>,
) -> Response {
    body.id = id;
    upsert_impl(&state, body).await
}

async fn upsert_impl(state: &ServerState, body: ServerBody) -> Response {
    if body.id.trim().is_empty() {
        return envelope::error_response(
            StatusCode::BAD_REQUEST,
            &AppError::new(ErrorCode::ConfigInvalid, "id 不能为空"),
        );
    }
    let config = state.config.current();
    let mut servers = config.config.mcp.servers.clone();
    let transport = match body.transport.as_deref() {
        Some("http") => McpTransportKind::Http,
        Some("stdio") | None => McpTransportKind::Stdio,
        Some(other) => {
            return envelope::error_response(
                StatusCode::BAD_REQUEST,
                &AppError::new(
                    ErrorCode::ConfigInvalid,
                    format!("未知 transport：{other}（仅支持 stdio / http）"),
                ),
            );
        }
    };

    let existing = servers.iter().position(|s| s.id == body.id);
    let mut next = existing
        .map(|i| servers[i].clone())
        .unwrap_or_else(|| McpServerConfig {
            id: body.id.clone(),
            ..McpServerConfig::default()
        });
    next.id = body.id.clone();
    if let Some(name) = body.name {
        next.name = name;
    } else if next.name.is_empty() {
        next.name = body.id.clone();
    }
    if let Some(enabled) = body.enabled {
        next.enabled = enabled;
    }
    next.transport = transport;
    if body.command.is_some() {
        next.command = body.command.filter(|c| !c.is_empty());
    }
    if let Some(args) = body.args {
        next.args = args;
    }
    if body.cwd.is_some() {
        next.cwd = body.cwd.filter(|c| !c.is_empty());
    }
    if body.url.is_some() {
        next.url = body.url.filter(|u| !u.is_empty());
    }
    if let Some(env) = body.env {
        next.env = env;
    }
    if let Some(allowed) = body.allowed_tools {
        next.allowed_tools = allowed;
    }
    if let Some(requires_network) = body.requires_network {
        next.requires_network = requires_network;
    }

    match existing {
        Some(i) => servers[i] = next,
        None => servers.push(next),
    }

    let patch = json!({ "servers": servers_to_toml_json(&servers) });
    match apply_mcp_patch(state, patch).await {
        Ok(data) => envelope::ok(data),
        Err(error) => envelope::error_response(StatusCode::BAD_REQUEST, &error),
    }
}

async fn delete_server(
    State(state): State<Arc<ServerState>>,
    Path(id): Path<String>,
) -> Response {
    let config = state.config.current();
    let servers: Vec<_> = config
        .config
        .mcp
        .servers
        .iter()
        .filter(|s| s.id != id)
        .cloned()
        .collect();
    let patch = json!({ "servers": servers_to_toml_json(&servers) });
    match apply_mcp_patch(&state, patch).await {
        Ok(data) => envelope::ok(data),
        Err(error) => envelope::error_response(StatusCode::BAD_REQUEST, &error),
    }
}

async fn list_tools(State(state): State<Arc<ServerState>>) -> Response {
    let config = state.config.current();
    let ctx = auth_context_from_config(&config.config);
    match state.mcp.list_allowed_tools(&ctx).await {
        Ok(tools) => envelope::ok(json!(tools)),
        Err(error) => envelope::error_response(StatusCode::BAD_REQUEST, &error),
    }
}

#[derive(Debug, Deserialize)]
struct CallBody {
    server_id: String,
    name: String,
    #[serde(default)]
    arguments: Value,
}

async fn call_tool(State(state): State<Arc<ServerState>>, Json(body): Json<CallBody>) -> Response {
    let config = state.config.current();
    let ctx = auth_context_from_config(&config.config);
    let call = McpToolCall {
        server_id: body.server_id,
        name: body.name,
        arguments: body.arguments,
    };
    match state.mcp.call_tool(&ctx, &call).await {
        Ok(result) => envelope::ok(json!(result)),
        Err(error) => {
            let status = if error
                .context()
                .get("deny")
                .is_some_and(|c| c.starts_with("mcp_"))
            {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::BAD_REQUEST
            };
            envelope::error_response(status, &error)
        }
    }
}

async fn reload(State(state): State<Arc<ServerState>>) -> Response {
    let config = state.config.current();
    match reload_registry(&state.mcp, &config.config).await {
        Ok(()) => envelope::ok(json!({ "ok": true })),
        Err(error) => envelope::error_response(StatusCode::BAD_REQUEST, &error),
    }
}

fn server_public(server: &McpServerConfig) -> Value {
    json!({
        "id": server.id,
        "name": server.name,
        "enabled": server.enabled,
        "transport": match server.transport {
            McpTransportKind::Stdio => "stdio",
            McpTransportKind::Http => "http",
        },
        "command": server.command,
        "args": server.args,
        "cwd": server.cwd,
        "url": server.url,
        // 只回引用名，不回解明文
        "env_refs": server.env.keys().cloned().collect::<Vec<_>>(),
        "allowed_tools": server.allowed_tools,
        "requires_network": server.requires_network
            || matches!(server.transport, McpTransportKind::Http),
    })
}

fn servers_to_toml_json(servers: &[McpServerConfig]) -> Value {
    Value::Array(
        servers
            .iter()
            .map(|s| {
                let mut map = serde_json::Map::new();
                map.insert("id".into(), json!(s.id));
                map.insert("name".into(), json!(s.name));
                map.insert("enabled".into(), json!(s.enabled));
                map.insert(
                    "transport".into(),
                    json!(match s.transport {
                        McpTransportKind::Stdio => "stdio",
                        McpTransportKind::Http => "http",
                    }),
                );
                if let Some(command) = &s.command {
                    map.insert("command".into(), json!(command));
                }
                map.insert("args".into(), json!(s.args));
                if let Some(cwd) = &s.cwd {
                    map.insert("cwd".into(), json!(cwd));
                }
                if let Some(url) = &s.url {
                    map.insert("url".into(), json!(url));
                }
                map.insert("env".into(), json!(s.env));
                map.insert("allowed_tools".into(), json!(s.allowed_tools));
                map.insert("requires_network".into(), json!(s.requires_network));
                Value::Object(map)
            })
            .collect(),
    )
}

async fn apply_mcp_patch(state: &ServerState, mcp_patch: Value) -> Result<Value, AppError> {
    let patch = json!({ "mcp": mcp_patch });
    let loaded = crate::config_api::apply_patch(state, patch)?;
    // 配置落盘后重载会话（失败不回滚配置；跳过的 server 仍可稍后 /reload）
    let _ = reload_registry(&state.mcp, &loaded.config).await;
    Ok(json!({
        "enabled": loaded.config.mcp.enabled,
        "servers": loaded.config.mcp.servers.iter().map(server_public).collect::<Vec<_>>(),
        "ai_upload": loaded.config.privacy.ai_upload,
    }))
}
