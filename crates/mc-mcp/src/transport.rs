//! MCP JSON-RPC 传输：stdio（换行分隔）与 HTTP POST。

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use mc_common::error::{AppError, ErrorCode};
use mc_common::observability::{info, warn};
use mc_config::model::{McpServerConfig, McpTransportKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::types::{McpTool, McpToolCall, McpToolResult};

/// 已初始化的 MCP 会话（一个配置的 server 对应一个会话）。
#[async_trait]
pub trait McpSession: Send + Sync {
    fn server_id(&self) -> &str;
    async fn list_tools(&self) -> Result<Vec<McpTool>, AppError>;
    async fn call_tool(&self, call: &McpToolCall) -> Result<McpToolResult, AppError>;
}

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    message: String,
}

/// 内存脚本会话：单测与 Chat tool loop 用，不拉起真实进程。
pub struct ScriptedSession {
    server_id: String,
    tools: Vec<McpTool>,
    handlers: HashMap<String, Arc<dyn Fn(Value) -> Result<String, String> + Send + Sync>>,
}

impl ScriptedSession {
    pub fn new(server_id: impl Into<String>, tools: Vec<McpTool>) -> Self {
        Self {
            server_id: server_id.into(),
            tools,
            handlers: HashMap::new(),
        }
    }

    pub fn on_tool<F>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(Value) -> Result<String, String> + Send + Sync + 'static,
    {
        self.handlers.insert(name.into(), Arc::new(handler));
        self
    }
}

#[async_trait]
impl McpSession for ScriptedSession {
    fn server_id(&self) -> &str {
        &self.server_id
    }

    async fn list_tools(&self) -> Result<Vec<McpTool>, AppError> {
        Ok(self.tools.clone())
    }

    async fn call_tool(&self, call: &McpToolCall) -> Result<McpToolResult, AppError> {
        if call.server_id != self.server_id {
            return Err(AppError::new(
                ErrorCode::DomainInvalidRange,
                format!(
                    "工具调用 server_id 不匹配：期望 {}，收到 {}",
                    self.server_id, call.server_id
                ),
            ));
        }
        match self.handlers.get(&call.name) {
            Some(handler) => match handler(call.arguments.clone()) {
                Ok(content) => Ok(McpToolResult {
                    server_id: call.server_id.clone(),
                    name: call.name.clone(),
                    content,
                    is_error: false,
                }),
                Err(message) => Ok(McpToolResult {
                    server_id: call.server_id.clone(),
                    name: call.name.clone(),
                    content: message,
                    is_error: true,
                }),
            },
            None => Err(AppError::new(
                ErrorCode::DomainInvalidRange,
                format!("脚本会话没有工具 {}", call.name),
            )),
        }
    }
}

/// HTTP JSON-RPC（单次 POST，适合本机/内网 MCP HTTP 适配器）。
pub struct HttpSession {
    server_id: String,
    url: String,
    headers: Vec<(String, String)>,
    client: reqwest::Client,
    next_id: AtomicU64,
}

impl HttpSession {
    pub fn new(
        server_id: impl Into<String>,
        url: impl Into<String>,
        headers: Vec<(String, String)>,
    ) -> Result<Self, AppError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|error| {
                AppError::new(
                    ErrorCode::ConfigInvalid,
                    format!("无法创建 MCP HTTP 客户端：{error}"),
                )
            })?;
        Ok(Self {
            server_id: server_id.into(),
            url: url.into(),
            headers,
            client,
            next_id: AtomicU64::new(1),
        })
    }

    async fn rpc(&self, method: &str, params: Option<Value>) -> Result<Value, AppError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let body = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };
        let mut request = self.client.post(&self.url).json(&body);
        for (key, value) in &self.headers {
            request = request.header(key, value);
        }
        let response = request.send().await.map_err(|error| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                format!("MCP HTTP 请求失败：{error}"),
            )
        })?;
        let status = response.status();
        let raw: Value = response.json().await.map_err(|error| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                format!("MCP HTTP 响应无法解析：{error}"),
            )
        })?;
        if !status.is_success() {
            return Err(AppError::new(
                ErrorCode::ConfigInvalid,
                format!("MCP HTTP 返回 {status}"),
            ));
        }
        let parsed: JsonRpcResponse = serde_json::from_value(raw).map_err(|error| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                format!("MCP JSON-RPC 形状无效：{error}"),
            )
        })?;
        if let Some(error) = parsed.error {
            return Err(AppError::new(
                ErrorCode::ConfigInvalid,
                format!("MCP 错误：{}", error.message),
            ));
        }
        parsed.result.ok_or_else(|| {
            AppError::new(ErrorCode::ConfigInvalid, "MCP JSON-RPC 缺少 result".to_string())
        })
    }
}

#[async_trait]
impl McpSession for HttpSession {
    fn server_id(&self) -> &str {
        &self.server_id
    }

    async fn list_tools(&self) -> Result<Vec<McpTool>, AppError> {
        let _ = self
            .rpc(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "minecontext", "version": env!("CARGO_PKG_VERSION") }
                })),
            )
            .await;
        let result = self.rpc("tools/list", Some(serde_json::json!({}))).await?;
        parse_tools_list(&self.server_id, result)
    }

    async fn call_tool(&self, call: &McpToolCall) -> Result<McpToolResult, AppError> {
        let result = self
            .rpc(
                "tools/call",
                Some(serde_json::json!({
                    "name": call.name,
                    "arguments": call.arguments,
                })),
            )
            .await?;
        Ok(parse_tool_result(&call.server_id, &call.name, result))
    }
}

struct StdioInner {
    /// 持有子进程句柄，使 `kill_on_drop` 在会话结束时生效。
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

/// stdio MCP：子进程 + 换行分隔 JSON-RPC。
pub struct StdioSession {
    server_id: String,
    inner: Mutex<StdioInner>,
}

impl StdioSession {
    pub async fn spawn(
        config: &McpServerConfig,
        env: HashMap<String, String>,
    ) -> Result<Self, AppError> {
        let command = config.command.as_deref().filter(|c| !c.is_empty()).ok_or_else(|| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                format!("MCP server {} 的 stdio 传输缺少 command", config.id),
            )
        })?;
        let mut cmd = Command::new(command);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(cwd) = config.cwd.as_deref() {
            cmd.current_dir(cwd);
        }
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn().map_err(|error| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                format!("无法启动 MCP 子进程 {}: {error}", config.id),
            )
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            AppError::new(ErrorCode::ConfigInvalid, "MCP 子进程缺少 stdin".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            AppError::new(ErrorCode::ConfigInvalid, "MCP 子进程缺少 stdout".to_string())
        })?;
        let session = Self {
            server_id: config.id.clone(),
            inner: Mutex::new(StdioInner {
                _child: child,
                stdin,
                stdout: BufReader::new(stdout),
                next_id: 1,
            }),
        };
        // 握手失败就立刻报错，避免「列表永远为空」却看不出原因
        session
            .rpc(
                "initialize",
                Some(serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "minecontext", "version": env!("CARGO_PKG_VERSION") }
                })),
            )
            .await?;
        let _ = session
            .rpc("notifications/initialized", None)
            .await
            .ok();
        info!(
            component = "mcp",
            event = "session_started",
            server_id = %config.id,
            transport = "stdio",
            "MCP stdio 会话已启动"
        );
        Ok(session)
    }

    async fn rpc(&self, method: &str, params: Option<Value>) -> Result<Value, AppError> {
        let mut guard = self.inner.lock().await;
        let id = guard.next_id;
        guard.next_id += 1;
        let body = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };
        let mut line = serde_json::to_string(&body).map_err(|error| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                format!("MCP 请求无法序列化：{error}"),
            )
        })?;
        line.push('\n');
        guard.stdin.write_all(line.as_bytes()).await.map_err(|error| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                format!("MCP stdin 写入失败：{error}"),
            )
        })?;
        guard.stdin.flush().await.map_err(|error| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                format!("MCP stdin flush 失败：{error}"),
            )
        })?;

        // notifications 可能没有响应
        if method.starts_with("notifications/") {
            return Ok(Value::Null);
        }

        let mut response_line = String::new();
        let n = guard
            .stdout
            .read_line(&mut response_line)
            .await
            .map_err(|error| {
                AppError::new(
                    ErrorCode::ConfigInvalid,
                    format!("MCP stdout 读取失败：{error}"),
                )
            })?;
        if n == 0 {
            return Err(AppError::new(
                ErrorCode::ConfigInvalid,
                "MCP 子进程已结束（stdout EOF）".to_string(),
            ));
        }
        let raw: Value = serde_json::from_str(response_line.trim()).map_err(|error| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                format!("MCP 响应无法解析：{error}"),
            )
        })?;
        let parsed: JsonRpcResponse = serde_json::from_value(raw).map_err(|error| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                format!("MCP JSON-RPC 形状无效：{error}"),
            )
        })?;
        if let Some(error) = parsed.error {
            return Err(AppError::new(
                ErrorCode::ConfigInvalid,
                format!("MCP 错误：{}", error.message),
            ));
        }
        Ok(parsed.result.unwrap_or(Value::Null))
    }
}

#[async_trait]
impl McpSession for StdioSession {
    fn server_id(&self) -> &str {
        &self.server_id
    }

    async fn list_tools(&self) -> Result<Vec<McpTool>, AppError> {
        let result = self.rpc("tools/list", Some(serde_json::json!({}))).await?;
        parse_tools_list(&self.server_id, result)
    }

    async fn call_tool(&self, call: &McpToolCall) -> Result<McpToolResult, AppError> {
        let result = self
            .rpc(
                "tools/call",
                Some(serde_json::json!({
                    "name": call.name,
                    "arguments": call.arguments,
                })),
            )
            .await?;
        Ok(parse_tool_result(&call.server_id, &call.name, result))
    }
}

fn parse_tools_list(server_id: &str, result: Value) -> Result<Vec<McpTool>, AppError> {
    let tools = result
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            AppError::new(
                ErrorCode::ConfigInvalid,
                "tools/list 响应缺少 tools 数组".to_string(),
            )
        })?;
    let mut out = Vec::with_capacity(tools.len());
    for tool in tools {
        let name = tool
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        out.push(McpTool {
            server_id: server_id.to_string(),
            name,
            description: tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            input_schema: tool
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        });
    }
    Ok(out)
}

fn parse_tool_result(server_id: &str, name: &str, result: Value) -> McpToolResult {
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let content = match result.get("content") {
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                if part.get("type").and_then(|t| t.as_str()) == Some("text") {
                    part.get("text").and_then(|t| t.as_str()).map(str::to_string)
                } else {
                    Some(part.to_string())
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
        None => result.to_string(),
    };
    McpToolResult {
        server_id: server_id.to_string(),
        name: name.to_string(),
        content,
        is_error,
    }
}

/// 按配置拉起会话（stdio / HTTP）。
pub async fn open_session(
    config: &McpServerConfig,
    env: HashMap<String, String>,
    auth_headers: Vec<(String, String)>,
) -> Result<Arc<dyn McpSession>, AppError> {
    match config.transport {
        McpTransportKind::Stdio => {
            let session = StdioSession::spawn(config, env).await?;
            Ok(Arc::new(session))
        }
        McpTransportKind::Http => {
            let url = config.url.as_deref().filter(|u| !u.is_empty()).ok_or_else(|| {
                AppError::new(
                    ErrorCode::ConfigInvalid,
                    format!("MCP server {} 的 http 传输缺少 url", config.id),
                )
            })?;
            let session = HttpSession::new(&config.id, url, auth_headers)?;
            warn!(
                component = "mcp",
                event = "http_session_opened",
                server_id = %config.id,
                "MCP HTTP 会话已打开（出网需 privacy.ai_upload）"
            );
            Ok(Arc::new(session))
        }
    }
}
