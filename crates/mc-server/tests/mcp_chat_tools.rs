//! Chat tool loop：在本地检索之外，按权限调用 MCP，拒绝时给出明确错误。

use std::sync::Arc;

use async_trait::async_trait;
use mc_common::error::AppError;
use mc_mcp::registry::McpRegistry;
use mc_mcp::transport::ScriptedSession;
use mc_mcp::types::McpTool;
use mc_server::chat::{ChatAnswer, ChatEngine, ChatEvent, ChatInput, Citation};
use mc_server::mcp::ToolLoopEngine;

struct ScriptedChat {
    /// 每次 answer 弹出的回复（从前往后）
    replies: std::sync::Mutex<Vec<String>>,
}

impl ScriptedChat {
    fn new(replies: Vec<String>) -> Self {
        Self {
            replies: std::sync::Mutex::new(replies),
        }
    }
}

#[async_trait]
impl ChatEngine for ScriptedChat {
    async fn answer(&self, _input: &ChatInput) -> Result<ChatAnswer, AppError> {
        let text = self
            .replies
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| "fallback".into());
        // pop 从尾部取：构造时把「先说的」放在 vec 末尾
        Ok(ChatAnswer {
            text,
            thinking: vec![],
            citations: vec![],
            model: Some("scripted".into()),
        })
    }
}

fn input(query: &str) -> ChatInput {
    ChatInput {
        query: query.into(),
        citations: vec![Citation {
            document_id: "act-1".into(),
            title: "本地活动：写周报".into(),
            kind: "activity".into(),
            at: 1,
            snippet: String::new(),
        }],
        history: vec![],
    }
}

fn config_with_wiki(ai_upload: bool) -> mc_config::Config {
    let toml = format!(
        r#"
[privacy]
ai_upload = {ai_upload}

[mcp]
enabled = true

[[mcp.servers]]
id = "wiki"
name = "Wiki"
enabled = true
transport = "stdio"
command = "true"
allowed_tools = ["search"]
"#
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.toml");
    std::fs::write(&path, toml).unwrap();
    let loaded = mc_config::load::load(&mc_config::load::LoadRequest {
        layers: vec![mc_config::load::LayerSource::File(path)],
        env: Vec::new(),
        read_process_env: false,
    })
    .unwrap();
    // 延长 dir 生命：把配置 clone 出来
    let _keep = dir;
    loaded.config
}

#[tokio::test]
async fn tool_loop_invokes_allowed_mcp_then_answers_with_local_citations() {
    let registry = Arc::new(McpRegistry::with_scripted(vec![ScriptedSession::new(
        "wiki",
        vec![McpTool {
            server_id: "wiki".into(),
            name: "search".into(),
            description: "Search wiki pages".into(),
            input_schema: serde_json::json!({}),
        }],
    )
    .on_tool("search", |_| Ok("Confluence: Q3 goals".into()))]));

    // pop 顺序：先取最后一项作为第一轮
    let inner = Box::new(ScriptedChat::new(vec![
        "结合本地活动与 Wiki：Q3 goals".into(), // 第二轮最终答案
        r#"TOOL_CALL:{"tool":"wiki__search","arguments":{"q":"goals"}}"#.into(), // 第一轮
    ]));
    let engine = ToolLoopEngine::new(inner, registry, config_with_wiki(false));
    let answer = engine
        .answer(&input("我上周在忙什么，Wiki 上有没有对应目标？"))
        .await
        .unwrap();
    assert!(
        answer.text.contains("Q3 goals"),
        "最终回答应包含工具结果：{}",
        answer.text
    );
    assert!(
        answer.citations.iter().any(|c| c.kind == "activity"),
        "应保留本地检索引用"
    );
    assert!(
        answer.citations.iter().any(|c| c.kind == "mcp_tool"),
        "应附带 MCP 工具引用"
    );
}

#[tokio::test]
async fn tool_loop_surfaces_clear_deny_when_tool_blocked() {
    let registry = Arc::new(McpRegistry::with_scripted(vec![ScriptedSession::new(
        "wiki",
        vec![McpTool {
            server_id: "wiki".into(),
            name: "search".into(),
            description: "Search".into(),
            input_schema: serde_json::json!({}),
        }],
    )]));

    let inner = Box::new(ScriptedChat::new(vec![
        r#"TOOL_CALL:{"tool":"wiki__delete","arguments":{}}"#.into(),
    ]));
    let engine = ToolLoopEngine::new(inner, registry, config_with_wiki(false));
    let mut events = Vec::new();
    let answer = engine
        .answer_stream(&input("删掉 wiki"), &mut |event| {
            if let ChatEvent::Delta(text) = event {
                events.push(text.to_string());
            }
        })
        .await
        .unwrap();
    assert!(
        answer.text.contains("无法调用") || answer.text.contains("白名单"),
        "拒绝时应给出可读错误：{}",
        answer.text
    );
}
