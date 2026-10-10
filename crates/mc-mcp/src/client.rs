//! 面向调用方的薄封装：权限闸门 → 会话调用。

use std::sync::Arc;

use mc_common::error::{AppError, ErrorCode};

use crate::permission::{authorize_tool_call, AuthContext, DenyReason};
use crate::transport::{McpSession, ScriptedSession};
use crate::types::{McpTool, McpToolCall, McpToolResult};

/// 受控 MCP 客户端：所有 `call_tool` 都先过权限闸门。
pub struct McpClient {
    sessions: Vec<Arc<dyn McpSession>>,
}

impl McpClient {
    pub fn new(sessions: Vec<Arc<dyn McpSession>>) -> Self {
        Self { sessions }
    }

    pub fn from_scripted(sessions: Vec<ScriptedSession>) -> Self {
        Self {
            sessions: sessions
                .into_iter()
                .map(|s| Arc::new(s) as Arc<dyn McpSession>)
                .collect(),
        }
    }

    fn session(&self, server_id: &str) -> Option<&Arc<dyn McpSession>> {
        self.sessions.iter().find(|s| s.server_id() == server_id)
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, AppError> {
        let mut tools = Vec::new();
        for session in &self.sessions {
            tools.extend(session.list_tools().await?);
        }
        Ok(tools)
    }

    /// 列出工具，但只返回当前授权上下文允许的项（设置页「可见工具」用）。
    pub async fn list_allowed_tools(&self, ctx: &AuthContext<'_>) -> Result<Vec<McpTool>, AppError> {
        let all = self.list_tools().await?;
        Ok(all
            .into_iter()
            .filter(|tool| authorize_tool_call(ctx, &tool.server_id, &tool.name).is_ok())
            .collect())
    }

    pub async fn call_tool(
        &self,
        ctx: &AuthContext<'_>,
        call: &McpToolCall,
    ) -> Result<McpToolResult, AppError> {
        authorize_tool_call(ctx, &call.server_id, &call.name).map_err(deny_to_error)?;
        let session = self.session(&call.server_id).ok_or_else(|| {
            AppError::new(
                ErrorCode::DomainInvalidRange,
                format!("没有已连接的 MCP 会话：{}", call.server_id),
            )
        })?;
        session.call_tool(call).await
    }
}

/// 测试友好别名。
pub type ScriptedMcpClient = McpClient;

fn deny_to_error(reason: DenyReason) -> AppError {
    let code = match reason {
        DenyReason::NetworkConsentRequired => ErrorCode::PrivacyBlocked,
        _ => ErrorCode::DomainInvalidRange,
    };
    AppError::new(code, reason.user_message().to_string()).with_context("deny", reason.code())
}
