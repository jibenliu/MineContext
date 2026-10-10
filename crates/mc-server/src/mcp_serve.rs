//! 只读 MCP Server 工具实现：记忆 / 活动 / 检索 / context-pack。
//!
//! 经现有检索与 vault API，**禁止**给 MCP 会话直接开 SQLite 句柄。

use std::sync::Arc;

use mc_mcp::server::ServeToolHandler;
use mc_mcp::types::McpTool;
use serde_json::{json, Value};

use crate::state::ServerState;

pub struct MemoryServeHandler {
    state: Arc<ServerState>,
}

impl MemoryServeHandler {
    pub fn new(state: Arc<ServerState>) -> Self {
        Self { state }
    }
}

impl ServeToolHandler for MemoryServeHandler {
    fn list_tool_defs(&self) -> Vec<McpTool> {
        vec![
            tool(
                "search",
                "Search local activities, notes, and summaries (keyword).",
                json!({
                    "type": "object",
                    "properties": {
                        "q": { "type": "string", "description": "Query text" },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                    },
                    "required": ["q"]
                }),
            ),
            tool(
                "memory",
                "List recent vault notes (title + summary), read-only.",
                json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                    }
                }),
            ),
            tool(
                "activity",
                "List recent activities (title + time), read-only.",
                json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                    }
                }),
            ),
            tool(
                "context_pack",
                "Build a token-budgeted context pack from retrieval + citations.",
                json!({
                    "type": "object",
                    "properties": {
                        "q": { "type": "string" },
                        "budget": { "type": "integer", "minimum": 64 },
                        "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                    },
                    "required": ["q"]
                }),
            ),
        ]
    }

    fn call_tool(&self, name: &str, arguments: Value) -> Result<String, String> {
        match name {
            "search" => self.search(arguments),
            "memory" => self.memory(arguments),
            "activity" => self.activity(arguments),
            "context_pack" => self.context_pack(arguments),
            other => Err(format!("unknown tool: {other}")),
        }
    }
}

impl MemoryServeHandler {
    fn search(&self, arguments: Value) -> Result<String, String> {
        let q = arguments
            .get("q")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if q.is_empty() {
            return Err("q is required".into());
        }
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .clamp(1, 50) as usize;
        let hits = crate::retrieval::retrieve(&self.state.db, q, limit, false)
            .map_err(|e| e.to_string())?;
        let results: Vec<Value> = hits
            .iter()
            .map(|hit| {
                json!({
                    "id": hit.document.id,
                    "kind": hit.document.kind.as_str(),
                    "title": hit.document.text.lines().next().unwrap_or("").chars().take(80).collect::<String>(),
                    "score": hit.score,
                    "at": hit.document.at.as_millis(),
                })
            })
            .collect();
        Ok(
            serde_json::to_string_pretty(&json!({ "query": q, "results": results }))
                .unwrap_or_else(|_| "{}".into()),
        )
    }

    fn memory(&self, arguments: Value) -> Result<String, String> {
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .clamp(1, 50) as usize;
        let notes = self
            .state
            .db
            .query_vault_rows(&mc_storage::vaults::VaultQuery {
                document_type: Vec::new(),
                parent_id: None,
                title: None,
                is_folder: Some(0),
                is_deleted: Some(0),
            })
            .map_err(|e| e.to_string())?;
        let items: Vec<Value> = notes
            .into_iter()
            .take(limit)
            .map(|note| {
                json!({
                    "id": note.id,
                    "title": note.title,
                    "summary": note.summary.chars().take(240).collect::<String>(),
                    "updated_at": note.updated_at,
                })
            })
            .collect();
        Ok(
            serde_json::to_string_pretty(&json!({ "notes": items }))
                .unwrap_or_else(|_| "{}".into()),
        )
    }

    fn activity(&self, arguments: Value) -> Result<String, String> {
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .clamp(1, 50) as usize;
        let mut activities = mc_storage::projectors::activities::read_all(&self.state.db)
            .map_err(|e| e.to_string())?;
        activities.sort_by_key(|a| std::cmp::Reverse(a.end.as_millis()));
        let items: Vec<Value> = activities
            .into_iter()
            .take(limit)
            .map(|activity| {
                json!({
                    "id": activity.id,
                    "title": activity.title.lines().next().unwrap_or("").chars().take(80).collect::<String>(),
                    "start": activity.start.as_millis(),
                    "end": activity.end.as_millis(),
                    "category": activity.category,
                })
            })
            .collect();
        Ok(
            serde_json::to_string_pretty(&json!({ "activities": items }))
                .unwrap_or_else(|_| "{}".into()),
        )
    }

    fn context_pack(&self, arguments: Value) -> Result<String, String> {
        let q = arguments
            .get("q")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if q.is_empty() {
            return Err("q is required".into());
        }
        let limit = arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(12)
            .clamp(1, 50) as usize;
        let budget = arguments
            .get("budget")
            .and_then(|v| v.as_u64())
            .unwrap_or(crate::context_lite::DEFAULT_TOKEN_BUDGET as u64)
            .max(64) as usize;
        let hits = crate::retrieval::retrieve(&self.state.db, q, limit, false)
            .map_err(|e| e.to_string())?;
        let pack = crate::context_lite::pack_from_hits(
            q,
            &hits,
            crate::context_lite::PackOptions {
                token_budget: budget,
                max_items: limit,
            },
        );
        Ok(crate::context_lite::render_pack_text(&pack))
    }
}

fn tool(name: &str, description: &str, schema: Value) -> McpTool {
    McpTool {
        server_id: "minecontext".into(),
        name: name.into(),
        description: description.into(),
        input_schema: schema,
    }
}

/// 从当前配置拼装 ServeAuthContext。
pub fn serve_auth<'a>(config: &'a mc_config::model::Config) -> mc_mcp::ServeAuthContext<'a> {
    mc_mcp::ServeAuthContext {
        mcp_enabled: config.mcp.enabled,
        serve_enabled: config.mcp.serve.enabled,
        ai_upload: config.privacy.ai_upload,
        allowed_tools: &config.mcp.serve.allowed_tools,
    }
}
