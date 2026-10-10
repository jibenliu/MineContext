//! MCP 接线：注册表、凭据解析、Chat 受控 tool loop。
//!
//! 工具调用不进事件重放；出网类 server 复用 `privacy.ai_upload` 同意闸门。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use mc_common::error::{AppError, ErrorCode};
use mc_config::model::{Config, McpServerConfig};
use mc_mcp::permission::AuthContext;
use mc_mcp::registry::McpRegistry;
use mc_mcp::types::{
    qualified_tool_name, split_qualified_tool_name, McpTool, McpToolCall, McpToolResult,
};
use mc_mcp::DenyReason;
use serde::Deserialize;

use crate::chat::{ChatAnswer, ChatEngine, ChatEvent, ChatInput, Citation, ThinkingNote};

/// Chat tool loop 最多几轮（防止模型空转刷工具）。
const MAX_TOOL_ROUNDS: usize = 3;

pub fn auth_context_from_config(config: &Config) -> AuthContext<'_> {
    AuthContext {
        mcp_enabled: config.mcp.enabled,
        ai_upload: config.privacy.ai_upload,
        servers: &config.mcp.servers,
    }
}

type ResolvedSecrets = (HashMap<String, String>, Vec<(String, String)>);

/// 解析 server 的 env secret refs 与 HTTP Authorization 头。
pub fn resolve_server_secrets(server: &McpServerConfig) -> Result<ResolvedSecrets, AppError> {
    let secrets = mc_providers::credentials::KeychainCommand::default();
    let mut env = HashMap::new();
    for (key, reference) in &server.env {
        match mc_providers::credentials::resolve_secret(&secrets, Some(reference.as_str()))? {
            Some(value) => {
                env.insert(key.clone(), value);
            }
            None => {
                return Err(AppError::new(
                    ErrorCode::ConfigInvalid,
                    format!("MCP server {} 的凭据 {key} 未配置", server.id),
                ));
            }
        }
    }
    let mut headers = Vec::new();
    if let Some(token) = env
        .get("AUTHORIZATION")
        .cloned()
        .or_else(|| env.get("MCP_AUTH_TOKEN").map(|t| format!("Bearer {t}")))
    {
        headers.push(("Authorization".to_string(), token));
    }
    Ok((env, headers))
}

pub async fn reload_registry(registry: &McpRegistry, config: &Config) -> Result<(), AppError> {
    registry
        .reload_from_config(config, resolve_server_secrets)
        .await
}

/// 在底层 ChatEngine 外包一层：模型可用时按提示协议发起受控 MCP 工具调用。
pub struct ToolLoopEngine {
    inner: Box<dyn ChatEngine>,
    registry: Arc<McpRegistry>,
    config: Config,
}

impl ToolLoopEngine {
    pub fn new(inner: Box<dyn ChatEngine>, registry: Arc<McpRegistry>, config: Config) -> Self {
        Self {
            inner,
            registry,
            config,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ToolCallLine {
    tool: String,
    #[serde(default)]
    arguments: serde_json::Value,
}

fn parse_tool_call_line(text: &str) -> Option<ToolCallLine> {
    let trimmed = text.trim();
    let json = if let Some(rest) = trimmed.strip_prefix("TOOL_CALL:") {
        rest.trim()
    } else if trimmed.starts_with('{') && trimmed.contains("\"tool\"") {
        trimmed
    } else {
        return None;
    };
    serde_json::from_str(json).ok()
}

fn tools_system_appendix(tools: &[McpTool]) -> String {
    if tools.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "\n\n你还可以调用用户已授权的 MCP 工具。需要时只输出一行：\n\
         TOOL_CALL:{\"tool\":\"server__name\",\"arguments\":{...}}\n\
         不需要工具时直接回答。可用工具：\n",
    );
    for tool in tools {
        out.push_str(&format!(
            "- {}：{}\n",
            qualified_tool_name(&tool.server_id, &tool.name),
            tool.description
        ));
    }
    out
}

async fn run_tool_rounds(
    registry: &McpRegistry,
    config: &Config,
    input: &ChatInput,
    inner: &dyn ChatEngine,
    on_event: &mut (dyn for<'a> FnMut(ChatEvent<'a>) + Send),
) -> Result<ChatAnswer, AppError> {
    let ctx = auth_context_from_config(config);
    let tools = registry.list_allowed_tools(&ctx).await.unwrap_or_default();
    if tools.is_empty() {
        return inner.answer_stream(input, on_event).await;
    }

    let appendix = tools_system_appendix(&tools);
    let working = input.clone();
    // 把工具说明并进首轮 query 旁路：通过 chat history 的 system 不可用时，
    // 用一条合成 user 前缀不安全；改为改写 citations 旁的思考说明 + query 前缀。
    let mut tool_notes: Vec<String> = Vec::new();

    for round in 0..MAX_TOOL_ROUNDS {
        let mut probe_input = working.clone();
        if !appendix.is_empty() {
            probe_input.query = format!("{appendix}\n\n用户问题：{}", working.query);
            if !tool_notes.is_empty() {
                probe_input.query.push_str("\n\n已执行的工具结果：\n");
                for note in &tool_notes {
                    probe_input.query.push_str(note);
                    probe_input.query.push('\n');
                }
            }
        }

        let thinking = ThinkingNote {
            stage: "mcp".to_string(),
            content: format!("检查已授权 MCP 工具（第 {} 轮）", round + 1),
            progress: 0.2 + (round as f64) * 0.1,
        };
        on_event(ChatEvent::Thinking(&thinking));

        // 中间轮用非流式，避免把 TOOL_CALL 行刷到界面；最后一轮若无工具再流式。
        let answer = inner.answer(&probe_input).await?;
        if let Some(call_line) = parse_tool_call_line(&answer.text) {
            let (server_id, tool_name) = match split_qualified_tool_name(&call_line.tool) {
                Some(parts) => parts,
                None => {
                    let deny = DenyReason::ToolNotAllowed;
                    let msg = format!("无法解析工具名：{}", call_line.tool);
                    on_event(ChatEvent::Thinking(&ThinkingNote {
                        stage: "mcp".into(),
                        content: msg.clone(),
                        progress: 0.5,
                    }));
                    tool_notes.push(format!("错误：{msg}（{}）", deny.code()));
                    continue;
                }
            };
            let call = McpToolCall {
                server_id: server_id.to_string(),
                name: tool_name.to_string(),
                arguments: call_line.arguments,
            };
            match registry.call_tool(&ctx, &call).await {
                Ok(result) => {
                    let note = format_tool_result(&result);
                    on_event(ChatEvent::Thinking(&ThinkingNote {
                        stage: "mcp".into(),
                        content: format!("已调用 {}__{}", result.server_id, result.name),
                        progress: 0.6,
                    }));
                    tool_notes.push(note);
                }
                Err(error) => {
                    let msg = error.detail().to_string();
                    on_event(ChatEvent::Thinking(&ThinkingNote {
                        stage: "mcp".into(),
                        content: format!("工具被拒绝或失败：{msg}"),
                        progress: 0.6,
                    }));
                    tool_notes.push(format!("错误：{msg}"));
                    // 权限拒绝时直接收束为对用户可见的说明，避免空转
                    if error
                        .context()
                        .get("deny")
                        .map(|v| v.as_str())
                        .is_some_and(|c| c.starts_with("mcp_"))
                    {
                        let text = format!("无法调用工具：{msg}");
                        on_event(ChatEvent::Delta(&text));
                        return Ok(ChatAnswer {
                            text,
                            thinking: vec![thinking],
                            citations: input.citations.clone(),
                            model: answer.model,
                        });
                    }
                }
            }
            continue;
        }

        // 最终答案：把正文流式推给调用方
        let text = answer.text.clone();
        on_event(ChatEvent::Delta(&text));
        return Ok(ChatAnswer {
            text,
            thinking: answer.thinking,
            citations: merge_citations(&input.citations, &tool_notes),
            model: answer.model,
        });
    }

    let text = "工具调用轮次已达上限，请缩小问题或检查 MCP 白名单后重试。".to_string();
    on_event(ChatEvent::Delta(&text));
    Ok(ChatAnswer {
        text,
        thinking: vec![],
        citations: input.citations.clone(),
        model: None,
    })
}

fn format_tool_result(result: &McpToolResult) -> String {
    format!(
        "[{}__{}] {}\n{}",
        result.server_id,
        result.name,
        if result.is_error { "error" } else { "ok" },
        result.content
    )
}

fn merge_citations(base: &[Citation], tool_notes: &[String]) -> Vec<Citation> {
    let mut out = base.to_vec();
    for (index, note) in tool_notes.iter().enumerate() {
        let title: String = note.chars().take(80).collect();
        out.push(Citation {
            document_id: format!("mcp-tool-{index}"),
            title,
            kind: "mcp_tool".into(),
            at: 0,
            snippet: String::new(),
        });
    }
    out
}

#[async_trait]
impl ChatEngine for ToolLoopEngine {
    async fn answer(&self, input: &ChatInput) -> Result<ChatAnswer, AppError> {
        let mut noop = |_event: ChatEvent<'_>| {};
        run_tool_rounds(
            &self.registry,
            &self.config,
            input,
            self.inner.as_ref(),
            &mut noop,
        )
        .await
    }

    async fn answer_stream(
        &self,
        input: &ChatInput,
        on_event: &mut (dyn for<'a> FnMut(ChatEvent<'a>) + Send),
    ) -> Result<ChatAnswer, AppError> {
        run_tool_rounds(
            &self.registry,
            &self.config,
            input,
            self.inner.as_ref(),
            on_event,
        )
        .await
    }
}
