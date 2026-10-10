//! `mc-mcp` — Model Context Protocol **客户端**。
//!
//! 提供：工具发现、受控调用、stdio / HTTP 传输。
//! **不**实现 MCP Server（对外暴露记忆是第二刀）。
//! 副作用只发生在用户显式启用的会话上，不进事件重放路径。

pub mod client;
pub mod permission;
pub mod registry;
pub mod transport;
pub mod types;

pub use client::{McpClient, ScriptedMcpClient};
pub use permission::{authorize_tool_call, AuthContext, DenyReason};
pub use registry::McpRegistry;
pub use types::{McpTool, McpToolCall, McpToolResult};
