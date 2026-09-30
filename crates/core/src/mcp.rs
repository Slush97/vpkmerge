//! MCP client side. Servers are configured in `mcp.json` using the common
//! `{"mcpServers": {...}}` shape, so configs can be pasted from other hosts.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use anyhow::{bail, Context};
use rmcp::model::{CallToolRequestParams, Tool};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::providers::ToolSpec;

pub const TOOL_PREFIX: &str = "mcp__";
const MAX_OUTPUT_CHARS: usize = 60_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFile {
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, ServerConfig>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub name: String,
    pub transport: &'static str,
    pub state: &'static str,
    pub error: Option<String>,
    pub tools: Vec<ToolSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolSummary {
    pub name: String,
    pub description: String,
}

enum State {
    Disabled,
    Connecting,
    Connected {
        client: Arc<RunningService<RoleClient, ()>>,
        tools: Vec<Tool>,
    },
    Failed(String),
}

struct Slot {
    config: ServerConfig,
    state: State,
}

#[derive(Default)]
pub struct McpManager {
    servers: RwLock<BTreeMap<String, Slot>>,
    /// Model-facing tool name to (server, tool).
    routes: RwLock<HashMap<String, (String, String)>>,
}

impl McpManager {
    /// Replaces the server set and connects everything that is enabled.
    pub async fn apply(&self, config: &ConfigFile) {
        let old = std::mem::take(&mut *self.servers.write().await);
        for slot in old.into_values() {
            shutdown(slot.state).await;
        }
        {
            let mut servers = self.servers.write().await;
            for (name, cfg) in &config.mcp_servers {
                let state = if cfg.disabled { State::Disabled } else { State::Connecting };
                servers.insert(name.clone(), Slot { config: cfg.clone(), state });
            }
        }
        let names: Vec<String> = config
            .mcp_servers
            .iter()
            .filter(|(_, c)| !c.disabled)
            .map(|(n, _)| n.clone())
            .collect();
        futures::future::join_all(names.iter().map(|n| self.connect(n))).await;
        self.rebuild_routes().await;
    }

    pub async fn reconnect(&self, name: &str) -> anyhow::Result<()> {
        let old = {
            let mut servers = self.servers.write().await;
            let slot = servers.get_mut(name).with_context(|| format!("no MCP server named {name}"))?;
            std::mem::replace(&mut slot.state, State::Connecting)
        };
        shutdown(old).await;
        self.connect(name).await;
        self.rebuild_routes().await;
        Ok(())
    }

    async fn connect(&self, name: &str) {
        let Some(config) = self.servers.read().await.get(name).map(|s| s.config.clone()) else {
            return;
        };
        let state = match start(&config).await {
            Ok(client) => match client.peer().list_all_tools().await {
                Ok(tools) => State::Connected {
                    client: Arc::new(client),
                    tools,
                },
                Err(e) => State::Failed(format!("listing tools failed: {e}")),
            },
            Err(e) => State::Failed(format!("{e:#}")),
        };
        if let Some(slot) = self.servers.write().await.get_mut(name) {
            slot.state = state;
        }
    }

    async fn rebuild_routes(&self) {
        let servers = self.servers.read().await;
        let mut routes = HashMap::new();
        for (server, slot) in servers.iter() {
            if let State::Connected { tools, .. } = &slot.state {
                for tool in tools {
                    routes.insert(qualified(server, &tool.name), (server.clone(), tool.name.to_string()));
                }
            }
        }
        *self.routes.write().await = routes;
    }

    pub async fn status(&self) -> Vec<ServerStatus> {
        let servers = self.servers.read().await;
        servers
            .iter()
            .map(|(name, slot)| {
                let (state, error, tools) = match &slot.state {
                    State::Disabled => ("disabled", None, Vec::new()),
                    State::Connecting => ("connecting", None, Vec::new()),
                    State::Failed(e) => ("failed", Some(e.clone()), Vec::new()),
                    State::Connected { tools, .. } => (
                        "connected",
                        None,
                        tools
                            .iter()
                            .map(|t| ToolSummary {
                                name: t.name.to_string(),
                                description: t.description.as_deref().unwrap_or_default().to_owned(),
                            })
                            .collect(),
                    ),
                };
                ServerStatus {
                    name: name.clone(),
                    transport: if slot.config.url.is_some() { "http" } else { "stdio" },
                    state,
                    error,
                    tools,
                }
            })
            .collect()
    }

    pub async fn tool_specs(&self) -> Vec<ToolSpec> {
        let servers = self.servers.read().await;
        let mut specs = Vec::new();
        for (server, slot) in servers.iter() {
            if let State::Connected { tools, .. } = &slot.state {
                for tool in tools {
                    specs.push(ToolSpec {
                        name: qualified(server, &tool.name),
                        description: format!(
                            "[{server}] {}",
                            tool.description.as_deref().unwrap_or_default()
                        ),
                        parameters: serde_json::Value::Object((*tool.input_schema).clone()),
                    });
                }
            }
        }
        specs
    }

    pub async fn is_mcp_tool(&self, name: &str) -> bool {
        self.routes.read().await.contains_key(name)
    }

    /// Returns the tool output as text and whether the server flagged an error.
    pub async fn call(&self, qualified_name: &str, arguments: &str) -> anyhow::Result<(String, bool)> {
        let (server, tool) = self
            .routes
            .read()
            .await
            .get(qualified_name)
            .cloned()
            .with_context(|| format!("unknown tool {qualified_name}"))?;
        let client = match &self.servers.read().await.get(&server).map(|s| &s.state) {
            Some(State::Connected { client, .. }) => Arc::clone(client),
            _ => bail!("MCP server {server} is not connected"),
        };
        let args: serde_json::Value = if arguments.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(arguments).context("tool arguments were not valid JSON")?
        };
        let serde_json::Value::Object(args) = args else {
            bail!("tool arguments must be a JSON object");
        };
        let result = client
            .call_tool(CallToolRequestParams::new(tool).with_arguments(args))
            .await?;
        let mut text = String::new();
        for block in &result.content {
            match block.as_text() {
                Some(t) => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&t.text);
                }
                None => text.push_str("\n[non-text content omitted]"),
            }
        }
        if text.trim().is_empty() {
            if let Some(structured) = &result.structured_content {
                text = serde_json::to_string_pretty(structured)?;
            }
        }
        if text.chars().count() > MAX_OUTPUT_CHARS {
            text = text.chars().take(MAX_OUTPUT_CHARS).collect::<String>() + "\n[output truncated]";
        }
        Ok((text, result.is_error.unwrap_or(false)))
    }

    pub async fn shutdown_all(&self) {
        let old = std::mem::take(&mut *self.servers.write().await);
        for slot in old.into_values() {
            shutdown(slot.state).await;
        }
    }
}

async fn start(config: &ServerConfig) -> anyhow::Result<RunningService<RoleClient, ()>> {
    if let Some(url) = &config.url {
        let mut cfg = StreamableHttpClientTransportConfig::with_uri(url.as_str());
        for (k, v) in &config.headers {
            if k.eq_ignore_ascii_case("authorization") {
                cfg = cfg.auth_header(v.trim_start_matches("Bearer ").to_owned());
            } else {
                cfg.custom_headers.insert(
                    http::HeaderName::from_bytes(k.as_bytes()).context("bad header name")?,
                    http::HeaderValue::from_str(v).context("bad header value")?,
                );
            }
        }
        let transport = StreamableHttpClientTransport::from_config(cfg);
        return Ok(().serve(transport).await?);
    }
    let command = config
        .command
        .as_deref()
        .context("server needs either `command` or `url`")?;
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(&config.args).envs(&config.env);
    let transport = TokioChildProcess::new(cmd).with_context(|| format!("could not start `{command}`"))?;
    Ok(().serve(transport).await?)
}

async fn shutdown(state: State) {
    if let State::Connected { client, .. } = state {
        if let Ok(client) = Arc::try_unwrap(client) {
            let _ = client.cancel().await;
        }
    }
}

/// Provider tool names allow `[A-Za-z0-9_-]{1,64}`.
fn qualified(server: &str, tool: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
            .collect()
    };
    let mut name = format!("{TOOL_PREFIX}{}__{}", clean(server), clean(tool));
    name.truncate(64);
    name
}
