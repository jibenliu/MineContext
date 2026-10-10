//! `mc-mcp` — Model Context Protocol 客户端 + 只读 Server 协议面。
//!
//! - **Client**：工具发现、受控调用、stdio / HTTP 传输
//! - **Server**：JSON-RPC 协议与 fail-closed 闸门；工具实现由 `mc-server` 注入（禁止本 crate 开库）
//!
//! 副作用只发生在用户显式启用的会话上，不进事件重放路径。

pub mod client;
pub mod permission;
pub mod registry;
pub mod server;
pub mod transport;
pub mod types;

pub use client::{McpClient, ScriptedMcpClient};
pub use permission::{authorize_tool_call, AuthContext, DenyReason};
pub use registry::McpRegistry;
pub use server::{
    authorize_serve_tool, handle_jsonrpc, ServeAuthContext, ServeDenyReason, ServeToolHandler,
};
pub use types::{McpTool, McpToolCall, McpToolResult};
