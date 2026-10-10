//! MCP 插件管理 API：启停、白名单、出网闸门、工具调用拒绝原因。

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mc_common::time::Timestamp;
use mc_mcp::registry::McpRegistry;
use mc_mcp::transport::ScriptedSession;
use mc_mcp::types::McpTool;
use mc_server::{router, ServerState};
use mc_storage::Database;
use tower::ServiceExt;

const TOKEN: &str = "test-token";
use mc_testkit::fixtures::FIXTURE_EPOCH_MS as T0;

struct Ctx {
    _dir: tempfile::TempDir,
    state: Arc<ServerState>,
}

fn ctx_with_mcp(toml: &str, registry: McpRegistry) -> Ctx {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, toml).unwrap();

    let db = Arc::new(Database::open(dir.path().join("minecontext.db")).unwrap());
    let request = mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::File(config_path.clone())],
        env: Vec::new(),
        read_process_env: false,
    };
    let loaded = mc_config::load::load(&request).unwrap();
    let state = Arc::new(
        ServerState::new(
            mc_config::ConfigHandle::new(loaded),
            db,
            TOKEN.to_string(),
            Timestamp::from_millis(T0),
            dir.path().to_path_buf(),
        )
        .with_config_write(request, config_path)
        .with_mcp(Arc::new(registry)),
    );
    Ctx { _dir: dir, state }
}

async fn call(
    state: &Arc<ServerState>,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "127.0.0.1:12345")
        .header("x-mc-token", TOKEN);
    let request = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            builder.body(Body::from(value.to_string())).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = router(Arc::clone(state)).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

fn scripted_registry() -> McpRegistry {
    let session = ScriptedSession::new(
        "wiki",
        vec![McpTool {
            server_id: "wiki".into(),
            name: "search".into(),
            description: "Search wiki".into(),
            input_schema: serde_json::json!({"type":"object"}),
        }],
    )
    .on_tool("search", |args| {
        let q = args
            .get("q")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(format!("hit:{q}"))
    });
    McpRegistry::with_scripted(vec![session])
}

#[tokio::test]
async fn get_reports_mcp_disabled_by_default() {
    let ctx = ctx_with_mcp("", McpRegistry::new());
    let (status, json) = call(&ctx.state, "GET", "/api/mcp", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["code"], 0);
    assert_eq!(json["data"]["enabled"], false);
    assert_eq!(json["data"]["servers"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn enable_and_add_server_persists() {
    let ctx = ctx_with_mcp("", McpRegistry::new());
    let (status, json) = call(
        &ctx.state,
        "PUT",
        "/api/mcp",
        Some(serde_json::json!({ "enabled": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["enabled"], true);

    let (status, json) = call(
        &ctx.state,
        "POST",
        "/api/mcp/servers",
        Some(serde_json::json!({
            "id": "wiki",
            "name": "Wiki",
            "enabled": true,
            "transport": "stdio",
            "command": "true",
            "allowed_tools": ["search"],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["servers"][0]["id"], "wiki");
    assert_eq!(json["data"]["servers"][0]["allowed_tools"][0], "search");
}

#[tokio::test]
async fn call_tool_denied_when_not_on_whitelist() {
    let toml = r#"
[mcp]
enabled = true

[[mcp.servers]]
id = "wiki"
name = "Wiki"
enabled = true
transport = "stdio"
command = "true"
allowed_tools = ["search"]
"#;
    let ctx = ctx_with_mcp(toml, scripted_registry());
    let (status, json) = call(
        &ctx.state,
        "POST",
        "/api/mcp/tools/call",
        Some(serde_json::json!({
            "server_id": "wiki",
            "name": "delete_all",
            "arguments": {}
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let detail = json["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("白名单") || json["message"].as_str().unwrap_or("").contains("白名单"),
        "{json}"
    );
}

#[tokio::test]
async fn call_tool_denied_without_ai_upload_for_http() {
    let toml = r#"
[privacy]
ai_upload = false

[mcp]
enabled = true

[[mcp.servers]]
id = "wiki"
name = "Wiki"
enabled = true
transport = "http"
url = "http://127.0.0.1:9/mcp"
allowed_tools = ["search"]
"#;
    let ctx = ctx_with_mcp(toml, scripted_registry());
    let (status, json) = call(
        &ctx.state,
        "POST",
        "/api/mcp/tools/call",
        Some(serde_json::json!({
            "server_id": "wiki",
            "name": "search",
            "arguments": { "q": "x" }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{json}");
    let blob = json.to_string();
    assert!(
        blob.contains("出网") || blob.contains("mcp_network_consent"),
        "{json}"
    );
}

#[tokio::test]
async fn call_allowed_stdio_tool_returns_result() {
    let toml = r#"
[mcp]
enabled = true

[[mcp.servers]]
id = "wiki"
name = "Wiki"
enabled = true
transport = "stdio"
command = "true"
allowed_tools = ["search"]
"#;
    let ctx = ctx_with_mcp(toml, scripted_registry());
    let (status, json) = call(
        &ctx.state,
        "POST",
        "/api/mcp/tools/call",
        Some(serde_json::json!({
            "server_id": "wiki",
            "name": "search",
            "arguments": { "q": "sprint" }
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["data"]["content"], "hit:sprint");
    assert_eq!(json["data"]["is_error"], false);
}

#[tokio::test]
async fn list_tools_only_shows_allowed() {
    let toml = r#"
[mcp]
enabled = true

[[mcp.servers]]
id = "wiki"
name = "Wiki"
enabled = true
transport = "stdio"
command = "true"
allowed_tools = ["search"]
"#;
    let session = ScriptedSession::new(
        "wiki",
        vec![
            McpTool {
                server_id: "wiki".into(),
                name: "search".into(),
                description: "Search".into(),
                input_schema: serde_json::json!({}),
            },
            McpTool {
                server_id: "wiki".into(),
                name: "secret_write".into(),
                description: "Write".into(),
                input_schema: serde_json::json!({}),
            },
        ],
    );
    let ctx = ctx_with_mcp(toml, McpRegistry::with_scripted(vec![session]));
    let (status, json) = call(&ctx.state, "GET", "/api/mcp/tools", None).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let tools = json["data"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "search");
}
