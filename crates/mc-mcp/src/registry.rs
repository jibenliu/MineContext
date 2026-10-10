//! 从配置装配 MCP 会话；凭据经调用方解析后注入。

use std::collections::HashMap;
use std::sync::Arc;

use mc_common::error::AppError;
use mc_common::observability::{info, warn};
use mc_config::model::{Config, McpServerConfig};
use tokio::sync::RwLock;

use crate::client::McpClient;
use crate::permission::AuthContext;
use crate::transport::{open_session, McpSession, ScriptedSession};
use crate::types::{McpTool, McpToolCall, McpToolResult};

/// 运行时 MCP 注册表：配置变更后可 `reload`。
pub struct McpRegistry {
    client: RwLock<McpClient>,
}

impl Default for McpRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl McpRegistry {
    pub fn new() -> Self {
        Self {
            client: RwLock::new(McpClient::new(Vec::new())),
        }
    }

    pub fn with_scripted(sessions: Vec<ScriptedSession>) -> Self {
        Self {
            client: RwLock::new(McpClient::from_scripted(sessions)),
        }
    }

    pub async fn replace_sessions(&self, sessions: Vec<Arc<dyn McpSession>>) {
        let mut guard = self.client.write().await;
        *guard = McpClient::new(sessions);
    }

    /// 按配置重建已启用 server 的会话。
    ///
    /// `resolve_env`：把配置里的 secret ref 解析成环境变量值（失败则跳过该 server）。
    pub async fn reload_from_config<F>(
        &self,
        config: &Config,
        mut resolve_env: F,
    ) -> Result<(), AppError>
    where
        F: FnMut(
            &McpServerConfig,
        ) -> Result<(HashMap<String, String>, Vec<(String, String)>), AppError>,
    {
        if !config.mcp.enabled {
            self.replace_sessions(Vec::new()).await;
            info!(
                component = "mcp",
                event = "registry_cleared",
                "MCP 总开关关闭，已清空会话"
            );
            return Ok(());
        }

        let mut sessions: Vec<Arc<dyn McpSession>> = Vec::new();
        for server in &config.mcp.servers {
            if !server.enabled {
                continue;
            }
            match resolve_env(server) {
                Ok((env, headers)) => match open_session(server, env, headers).await {
                    Ok(session) => sessions.push(session),
                    Err(error) => {
                        warn!(
                            component = "mcp",
                            event = "session_open_failed",
                            server_id = %server.id,
                            "MCP 会话启动失败，已跳过该 server"
                        );
                        let _ = error;
                    }
                },
                Err(error) => {
                    warn!(
                        component = "mcp",
                        event = "credentials_resolve_failed",
                        server_id = %server.id,
                        "MCP 凭据解析失败，已跳过该 server"
                    );
                    let _ = error;
                }
            }
        }
        let count = sessions.len();
        self.replace_sessions(sessions).await;
        info!(
            component = "mcp",
            event = "registry_reloaded",
            sessions = count,
            "MCP 注册表已重载"
        );
        Ok(())
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, AppError> {
        self.client.read().await.list_tools().await
    }

    pub async fn list_allowed_tools(
        &self,
        ctx: &AuthContext<'_>,
    ) -> Result<Vec<McpTool>, AppError> {
        self.client.read().await.list_allowed_tools(ctx).await
    }

    pub async fn call_tool(
        &self,
        ctx: &AuthContext<'_>,
        call: &McpToolCall,
    ) -> Result<McpToolResult, AppError> {
        self.client.read().await.call_tool(ctx, call).await
    }
}
