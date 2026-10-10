//! MCP 配置段：默认关闭；stdio 缺 command 在加载期拒绝。

use mc_config::load::{load, LayerSource, LoadRequest};

fn load_with(toml: &str) -> mc_config::LoadedConfig {
    load(&LoadRequest {
        layers: vec![LayerSource::Inline {
            name: "test".to_string(),
            toml: toml.to_string(),
        }],
        env: Vec::new(),
        read_process_env: false,
    })
    .expect("配置必须可加载")
}

#[test]
fn default_mcp_disabled() {
    let loaded = load_with("");
    assert!(!loaded.config.mcp.enabled);
    assert!(loaded.config.mcp.servers.is_empty());
}

#[test]
fn loads_stdio_server() {
    let loaded = load_with(
        r#"
[mcp]
enabled = true

[[mcp.servers]]
id = "wiki"
name = "Wiki"
enabled = true
transport = "stdio"
command = "npx"
allowed_tools = ["search"]
"#,
    );
    assert!(loaded.config.mcp.enabled);
    assert_eq!(loaded.config.mcp.servers[0].id, "wiki");
    assert_eq!(loaded.config.mcp.servers[0].allowed_tools, vec!["search"]);
}

#[test]
fn rejects_stdio_without_command() {
    let err = load(&LoadRequest {
        layers: vec![LayerSource::Inline {
            name: "test".to_string(),
            toml: r#"
[mcp]
enabled = true
[[mcp.servers]]
id = "wiki"
transport = "stdio"
"#
            .to_string(),
        }],
        env: Vec::new(),
        read_process_env: false,
    })
    .unwrap_err();
    assert!(err.detail().contains("command"), "{}", err.detail());
}
