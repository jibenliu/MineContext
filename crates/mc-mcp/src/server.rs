//! 只读 MCP Server 协议面（JSON-RPC）。
//!
//! **不**碰 SQLite：工具实现由调用方注入（`mc-server` 编排检索 / 记忆 / context-pack）。
//! 权限 fail-closed：总开关、工具白名单、`ai_upload` 缺一不可。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::types::McpTool;

/// 对外 Server 的授权上下文。
#[derive(Debug, Clone, PartialEq)]
pub struct ServeAuthContext<'a> {
    /// `[mcp].enabled` 总开关（客户端与 Server 共用主闸）。
    pub mcp_enabled: bool,
    /// `[mcp.serve].enabled`
    pub serve_enabled: bool,
    /// 对外吐本地记忆需要 `privacy.ai_upload`。
    pub ai_upload: bool,
    pub allowed_tools: &'a [String],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeDenyReason {
    McpDisabled,
    ServeDisabled,
    AiUploadRequired,
    ToolNotAllowed,
}

impl ServeDenyReason {
    pub const fn code(self) -> &'static str {
        match self {
            Self::McpDisabled => "mcp_disabled",
            Self::ServeDisabled => "mcp_serve_disabled",
            Self::AiUploadRequired => "mcp_serve_ai_upload_required",
            Self::ToolNotAllowed => "mcp_serve_tool_not_allowed",
        }
    }

    pub const fn user_message(self) -> &'static str {
        match self {
            Self::McpDisabled => "MCP 总开关已关闭。",
            Self::ServeDisabled => "MCP Server（对外只读）未启用。",
            Self::AiUploadRequired => "对外暴露本地记忆需要允许 AI 出网（privacy.ai_upload）。",
            Self::ToolNotAllowed => "该工具不在 MCP Server 白名单中。",
        }
    }
}

/// 判定对外工具是否允许调用。
pub fn authorize_serve_tool(
    ctx: &ServeAuthContext<'_>,
    tool_name: &str,
) -> Result<(), ServeDenyReason> {
    if !ctx.mcp_enabled {
        return Err(ServeDenyReason::McpDisabled);
    }
    if !ctx.serve_enabled {
        return Err(ServeDenyReason::ServeDisabled);
    }
    if !ctx.ai_upload {
        return Err(ServeDenyReason::AiUploadRequired);
    }
    if !ctx
        .allowed_tools
        .iter()
        .any(|allowed| allowed == tool_name || allowed == "*")
    {
        return Err(ServeDenyReason::ToolNotAllowed);
    }
    Ok(())
}

/// 工具处理器：由 mc-server 实现，禁止在此 crate 开库。
pub trait ServeToolHandler: Send + Sync {
    fn list_tool_defs(&self) -> Vec<McpTool>;
    fn call_tool(&self, name: &str, arguments: Value) -> Result<String, String>;
}

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcErrorBody>,
}

#[derive(Debug, Serialize)]
struct JsonRpcErrorBody {
    code: i32,
    message: String,
}

/// 处理一条 JSON-RPC 请求（initialize / tools/list / tools/call）。
pub fn handle_jsonrpc(
    ctx: &ServeAuthContext<'_>,
    handler: &dyn ServeToolHandler,
    raw: &str,
) -> String {
    let req: JsonRpcRequest = match serde_json::from_str(raw) {
        Ok(req) => req,
        Err(error) => {
            return serde_json::to_string(&JsonRpcResponse {
                jsonrpc: "2.0",
                id: None,
                result: None,
                error: Some(JsonRpcErrorBody {
                    code: -32700,
                    message: format!("parse error: {error}"),
                }),
            })
            .unwrap_or_else(|_| {
                r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"parse error"}}"#.into()
            });
        }
    };

    let response = dispatch(ctx, handler, &req);
    serde_json::to_string(&response).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"serialize error"}}"#.into()
    })
}

fn dispatch(
    ctx: &ServeAuthContext<'_>,
    handler: &dyn ServeToolHandler,
    req: &JsonRpcRequest,
) -> JsonRpcResponse {
    match req.method.as_str() {
        "initialize" => ok(
            req.id.clone(),
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "minecontext", "version": env!("CARGO_PKG_VERSION") },
            }),
        ),
        "notifications/initialized" | "ping" => ok(req.id.clone(), json!({})),
        "tools/list" => {
            if let Err(reason) = gate_serve(ctx) {
                return err(req.id.clone(), -32000, reason);
            }
            let tools: Vec<Value> = handler
                .list_tool_defs()
                .into_iter()
                .filter(|tool| authorize_serve_tool(ctx, &tool.name).is_ok())
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema,
                    })
                })
                .collect();
            ok(req.id.clone(), json!({ "tools": tools }))
        }
        "tools/call" => {
            let params = req.params.clone().unwrap_or(json!({}));
            let name = params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if let Err(reason) = authorize_serve_tool(ctx, &name) {
                return err(req.id.clone(), -32000, reason);
            }
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            match handler.call_tool(&name, arguments) {
                Ok(content) => ok(
                    req.id.clone(),
                    json!({
                        "content": [{ "type": "text", "text": content }],
                        "isError": false,
                    }),
                ),
                Err(message) => ok(
                    req.id.clone(),
                    json!({
                        "content": [{ "type": "text", "text": message }],
                        "isError": true,
                    }),
                ),
            }
        }
        other => JsonRpcResponse {
            jsonrpc: "2.0",
            id: req.id.clone(),
            result: None,
            error: Some(JsonRpcErrorBody {
                code: -32601,
                message: format!("method not found: {other}"),
            }),
        },
    }
}

fn gate_serve(ctx: &ServeAuthContext<'_>) -> Result<(), ServeDenyReason> {
    if !ctx.mcp_enabled {
        return Err(ServeDenyReason::McpDisabled);
    }
    if !ctx.serve_enabled {
        return Err(ServeDenyReason::ServeDisabled);
    }
    if !ctx.ai_upload {
        return Err(ServeDenyReason::AiUploadRequired);
    }
    Ok(())
}

fn ok(id: Option<Value>, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    }
}

fn err(id: Option<Value>, code: i32, reason: ServeDenyReason) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcErrorBody {
            code,
            message: format!("{} ({})", reason.user_message(), reason.code()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::McpTool;

    struct Stub;

    impl ServeToolHandler for Stub {
        fn list_tool_defs(&self) -> Vec<McpTool> {
            vec![McpTool {
                server_id: "minecontext".into(),
                name: "search".into(),
                description: "search".into(),
                input_schema: json!({"type":"object"}),
            }]
        }

        fn call_tool(&self, name: &str, _arguments: Value) -> Result<String, String> {
            Ok(format!("called:{name}"))
        }
    }

    fn auth<'a>(ai: bool, tools: &'a [String]) -> ServeAuthContext<'a> {
        ServeAuthContext {
            mcp_enabled: true,
            serve_enabled: true,
            ai_upload: ai,
            allowed_tools: tools,
        }
    }

    #[test]
    fn denies_tools_without_ai_upload() {
        let tools = vec!["search".to_string()];
        let ctx = auth(false, &tools);
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"search","arguments":{}}}"#;
        let resp: Value = serde_json::from_str(&handle_jsonrpc(&ctx, &Stub, raw)).unwrap();
        assert!(resp.get("error").is_some(), "{resp}");
        assert!(
            resp["error"]["message"]
                .as_str()
                .unwrap()
                .contains("ai_upload"),
            "{resp}"
        );
    }

    #[test]
    fn lists_and_calls_allowed_tool_when_consent_present() {
        let tools = vec!["search".to_string()];
        let ctx = auth(true, &tools);
        let list = handle_jsonrpc(
            &ctx,
            &Stub,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let list_v: Value = serde_json::from_str(&list).unwrap();
        assert_eq!(list_v["result"]["tools"][0]["name"], "search");

        let call = handle_jsonrpc(
            &ctx,
            &Stub,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search","arguments":{"q":"x"}}}"#,
        );
        let call_v: Value = serde_json::from_str(&call).unwrap();
        assert_eq!(call_v["result"]["isError"], false);
        assert!(
            call_v["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("called:search"),
            "{call_v}"
        );
    }
}
