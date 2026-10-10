//! MCP 工具面的领域形状（与传输无关）。

use serde::{Deserialize, Serialize};

/// 一个可调用的工具（来自某个已启用的 MCP server）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpTool {
    pub server_id: String,
    pub name: String,
    pub description: String,
    /// JSON Schema；调用方原样交给模型或 UI。
    #[serde(default = "empty_object")]
    pub input_schema: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}

/// 一次工具调用请求（已经过权限闸门之后才交给传输层）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpToolCall {
    pub server_id: String,
    pub name: String,
    #[serde(default = "empty_object")]
    pub arguments: serde_json::Value,
}

/// 工具调用结果（文本为主；二进制资源本切片不展开）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpToolResult {
    pub server_id: String,
    pub name: String,
    pub content: String,
    pub is_error: bool,
}

/// Chat 提示词里用的稳定工具名：`server_id__tool_name`。
pub fn qualified_tool_name(server_id: &str, tool: &str) -> String {
    format!("{server_id}__{tool}")
}

/// 解析 `server_id__tool_name`；非法形状返回 `None`。
pub fn split_qualified_tool_name(qualified: &str) -> Option<(&str, &str)> {
    let (server, tool) = qualified.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server, tool))
}
