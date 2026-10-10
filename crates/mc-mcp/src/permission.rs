//! MCP 工具调用的权限闸门。
//!
//! Fail-closed：总开关关、server 关、工具不在白名单、需要出网但未同意 —— 一律拒绝。
//! 拒绝原因可映射到设置页文案，避免「工具失败」与「被拦住」混在一起。

use mc_config::model::{McpServerConfig, McpTransportKind};

/// 调用时可见的授权上下文（由 mc-server 从当前配置拼出）。
#[derive(Debug, Clone, PartialEq)]
pub struct AuthContext<'a> {
    pub mcp_enabled: bool,
    pub ai_upload: bool,
    pub servers: &'a [McpServerConfig],
}

/// 拒绝原因（稳定字符串供 API / Chat 错误帧使用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    McpDisabled,
    ServerNotFound,
    ServerDisabled,
    ToolNotAllowed,
    NetworkConsentRequired,
}

impl DenyReason {
    pub const fn code(self) -> &'static str {
        match self {
            Self::McpDisabled => "mcp_disabled",
            Self::ServerNotFound => "mcp_server_not_found",
            Self::ServerDisabled => "mcp_server_disabled",
            Self::ToolNotAllowed => "mcp_tool_not_allowed",
            Self::NetworkConsentRequired => "mcp_network_consent_required",
        }
    }

    pub const fn user_message(self) -> &'static str {
        match self {
            Self::McpDisabled => "MCP 插件总开关已关闭。请在设置里启用后再试。",
            Self::ServerNotFound => "找不到该 MCP 服务器配置。",
            Self::ServerDisabled => "该 MCP 服务器未启用。",
            Self::ToolNotAllowed => "该工具不在白名单中，已被拒绝。",
            Self::NetworkConsentRequired => {
                "该 MCP 工具需要出网，但尚未允许 AI 出网。请先在设置里开启「允许 AI 出网」。"
            }
        }
    }
}

/// 判定一次工具调用是否允许。
///
/// 白名单为空 = 不允许任何工具（fail-closed）。
/// `requires_network`（HTTP 默认 true）时必须 `ai_upload`。
pub fn authorize_tool_call(
    ctx: &AuthContext<'_>,
    server_id: &str,
    tool_name: &str,
) -> Result<(), DenyReason> {
    if !ctx.mcp_enabled {
        return Err(DenyReason::McpDisabled);
    }
    let server = ctx
        .servers
        .iter()
        .find(|s| s.id == server_id)
        .ok_or(DenyReason::ServerNotFound)?;
    if !server.enabled {
        return Err(DenyReason::ServerDisabled);
    }
    if !server
        .allowed_tools
        .iter()
        .any(|allowed| allowed == tool_name || allowed == "*")
    {
        return Err(DenyReason::ToolNotAllowed);
    }
    if server_requires_network(server) && !ctx.ai_upload {
        return Err(DenyReason::NetworkConsentRequired);
    }
    Ok(())
}

fn server_requires_network(server: &McpServerConfig) -> bool {
    if server.requires_network {
        return true;
    }
    matches!(server.transport, McpTransportKind::Http)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_config::model::{McpServerConfig, McpTransportKind};

    fn stdio_server(enabled: bool, tools: &[&str]) -> McpServerConfig {
        McpServerConfig {
            id: "demo".into(),
            name: "Demo".into(),
            enabled,
            transport: McpTransportKind::Stdio,
            command: Some("echo".into()),
            args: vec![],
            cwd: None,
            url: None,
            env: Default::default(),
            allowed_tools: tools.iter().map(|s| (*s).to_string()).collect(),
            requires_network: false,
        }
    }

    #[test]
    fn denies_when_mcp_master_switch_off() {
        let servers = [stdio_server(true, &["search"])];
        let ctx = AuthContext {
            mcp_enabled: false,
            ai_upload: true,
            servers: &servers,
        };
        assert_eq!(
            authorize_tool_call(&ctx, "demo", "search"),
            Err(DenyReason::McpDisabled)
        );
    }

    #[test]
    fn denies_tool_not_on_whitelist() {
        let servers = [stdio_server(true, &["search"])];
        let ctx = AuthContext {
            mcp_enabled: true,
            ai_upload: false,
            servers: &servers,
        };
        assert_eq!(
            authorize_tool_call(&ctx, "demo", "write"),
            Err(DenyReason::ToolNotAllowed)
        );
    }

    #[test]
    fn empty_whitelist_allows_nothing() {
        let servers = [stdio_server(true, &[])];
        let ctx = AuthContext {
            mcp_enabled: true,
            ai_upload: false,
            servers: &servers,
        };
        assert_eq!(
            authorize_tool_call(&ctx, "demo", "search"),
            Err(DenyReason::ToolNotAllowed)
        );
    }

    #[test]
    fn allows_whitelisted_stdio_without_ai_upload() {
        let servers = [stdio_server(true, &["search"])];
        let ctx = AuthContext {
            mcp_enabled: true,
            ai_upload: false,
            servers: &servers,
        };
        assert!(authorize_tool_call(&ctx, "demo", "search").is_ok());
    }

    #[test]
    fn http_requires_ai_upload_consent() {
        let mut server = stdio_server(true, &["search"]);
        server.transport = McpTransportKind::Http;
        server.command = None;
        server.url = Some("http://127.0.0.1:3100/mcp".into());
        let servers = [server];
        let ctx = AuthContext {
            mcp_enabled: true,
            ai_upload: false,
            servers: &servers,
        };
        assert_eq!(
            authorize_tool_call(&ctx, "demo", "search"),
            Err(DenyReason::NetworkConsentRequired)
        );
        let ctx_ok = AuthContext {
            mcp_enabled: true,
            ai_upload: true,
            servers: &servers,
        };
        assert!(authorize_tool_call(&ctx_ok, "demo", "search").is_ok());
    }

    #[test]
    fn star_whitelist_allows_any_tool() {
        let servers = [stdio_server(true, &["*"])];
        let ctx = AuthContext {
            mcp_enabled: true,
            ai_upload: false,
            servers: &servers,
        };
        assert!(authorize_tool_call(&ctx, "demo", "anything").is_ok());
    }
}
