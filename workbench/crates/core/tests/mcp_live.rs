//! Opt-in: connects to a real stdio MCP server.
//! `cargo test -p workbench-core --test mcp_live -- --ignored`
//! Override the server with `WORKBENCH_MCP_TEST_CMD="cmd arg1 arg2"`.

use std::collections::BTreeMap;

use workbench_core::mcp::{ConfigFile, McpManager, ServerConfig};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "spawns an external MCP server"]
async fn connects_and_lists_tools() {
    let cmd = std::env::var("WORKBENCH_MCP_TEST_CMD").unwrap_or_else(|_| "uvx blender-mcp".into());
    let mut words = cmd.split_whitespace().map(str::to_owned);
    let config = ConfigFile {
        mcp_servers: BTreeMap::from([(
            "live".to_owned(),
            ServerConfig {
                command: words.next(),
                args: words.collect(),
                ..ServerConfig::default()
            },
        )]),
    };
    let manager = McpManager::default();
    manager.apply(&config).await;
    let status = manager.status().await;
    assert_eq!(status[0].state, "connected", "{:?}", status[0].error);
    assert!(!status[0].tools.is_empty());
    let specs = manager.tool_specs().await;
    assert!(specs.iter().all(|s| s.name.starts_with("mcp__live__") && s.name.len() <= 64));
    println!("{} tools: {}", specs.len(), specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "));
    manager.shutdown_all().await;
}
