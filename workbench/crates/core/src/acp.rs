//! Client for external coding agents over JSON-RPC on stdio. Each agent signs
//! in with its own account; we never touch its credentials.
//! - Grok Build speaks the Agent Client Protocol natively (`grok agent stdio`).
//! - Codex runs as the user's own `codex app-server`; `codex_app` translates
//!   its protocol into ACP-shaped updates so one turn loop serves both.

mod codex_app;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

use crate::permissions::PermissionLevel;

/// ACP's "authentication required" error.
const AUTH_REQUIRED: i64 = -32000;
const TURN_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentId {
    Grok,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialect {
    Acp,
    CodexAppServer,
}

impl AgentId {
    pub const ALL: [Self; 2] = [Self::Grok, Self::Codex];

    pub fn name(self) -> &'static str {
        match self {
            Self::Grok => "Grok Build",
            Self::Codex => "Codex",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::Grok => "grok-build",
            Self::Codex => "codex",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Grok => "xAI's coding agent, run through your Grok Build CLI with its own Grok sign-in.",
            Self::Codex => "OpenAI's coding agent, run through your Codex CLI with its own ChatGPT sign-in.",
        }
    }

    fn dialect(self) -> Dialect {
        match self {
            Self::Grok => Dialect::Acp,
            Self::Codex => Dialect::CodexAppServer,
        }
    }

    /// The program and arguments, when the program is on this machine.
    pub fn command(self) -> Option<(PathBuf, Vec<String>)> {
        let home = crate::config::home_dir();
        match self {
            Self::Grok => {
                let bin = find_program("grok").or_else(|| existing(home.join(".grok/bin/grok")))?;
                // Grok has to ask before it acts, whatever its own config says,
                // so that the app's permission level is what answers.
                let args = ["--permission-mode", "default", "agent", "stdio"];
                Some((bin, args.map(String::from).to_vec()))
            }
            Self::Codex => {
                let bin = find_program("codex").or_else(|| existing(home.join(".npm-global/bin/codex")))?;
                Some((bin, vec!["app-server".into()]))
            }
        }
    }

    pub fn command_label(self) -> &'static str {
        match self {
            Self::Grok => "grok agent stdio",
            Self::Codex => "codex app-server",
        }
    }
}

fn existing(p: PathBuf) -> Option<PathBuf> {
    p.is_file().then_some(p)
}

fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

/// Messages routed to the turn that owns an agent session. Updates are
/// always in ACP's `session/update` shape, whatever the agent speaks.
pub enum Inbound {
    Update(Value),
    Permission { rpc_id: Value, params: Value },
}

struct Rpc {
    dialect: Dialect,
    stdin: tokio::sync::Mutex<ChildStdin>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>,
    routes: Mutex<HashMap<String, mpsc::UnboundedSender<Inbound>>>,
    /// Codex turns end with a `turn/completed` notification, not a response.
    codex_turns: Mutex<HashMap<String, codex_app::TurnWaiter>>,
    alive: AtomicBool,
}

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn exited() -> Self {
        Self { code: 0, message: "the agent process has exited".into() }
    }
}

impl Rpc {
    async fn write(&self, msg: &Value) -> anyhow::Result<()> {
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    async fn notify(&self, method: &str, params: Value) {
        let _ = self
            .write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await;
    }

    async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, RpcError> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(RpcError::exited());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(e) = self.write(&msg).await {
            self.pending.lock().unwrap().remove(&id);
            return Err(RpcError { code: 0, message: format!("writing to the agent failed: {e}") });
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(RpcError::exited()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(RpcError { code: 0, message: format!("{method} timed out") })
            }
        }
    }

    async fn respond(&self, id: Value, result: Value) {
        let _ = self
            .write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            .await;
    }

    async fn reject(&self, id: Value, method: &str) {
        let _ = self
            .write(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("{method} is not supported by this client") },
            }))
            .await;
    }

    fn route(&self, session: &str) -> Option<mpsc::UnboundedSender<Inbound>> {
        self.routes.lock().unwrap().get(session).cloned()
    }

    async fn dispatch(self: &Arc<Self>, line: &str) {
        let Ok(msg) = serde_json::from_str::<Value>(line) else { return };
        if let Some(method) = msg["method"].as_str() {
            match self.dialect {
                Dialect::Acp => self.dispatch_acp(method, &msg).await,
                Dialect::CodexAppServer => codex_app::dispatch(self, method, &msg).await,
            }
            return;
        }
        let Some(id) = msg["id"].as_u64() else { return };
        let Some(tx) = self.pending.lock().unwrap().remove(&id) else { return };
        let result = if msg.get("error").is_some_and(|e| !e.is_null()) {
            Err(RpcError {
                code: msg["error"]["code"].as_i64().unwrap_or(0),
                message: error_text(&msg["error"]),
            })
        } else {
            Ok(msg["result"].clone())
        };
        let _ = tx.send(result);
    }

    async fn dispatch_acp(&self, method: &str, msg: &Value) {
        let params = msg["params"].clone();
        let route = self.route(params["sessionId"].as_str().unwrap_or_default());
        match (method, msg.get("id")) {
            ("session/update", None) => {
                if let Some(route) = route {
                    let _ = route.send(Inbound::Update(params["update"].clone()));
                }
            }
            ("session/request_permission", Some(id)) => {
                let delivered = route.is_some_and(|r| {
                    r.send(Inbound::Permission { rpc_id: id.clone(), params }).is_ok()
                });
                if !delivered {
                    self.respond(id.clone(), json!({ "outcome": { "outcome": "cancelled" } }))
                        .await;
                }
            }
            (_, Some(id)) => self.reject(id.clone(), method).await,
            _ => {}
        }
    }
}

/// Error text, preferring the provider's own message nested in `data`.
fn error_text(err: &Value) -> String {
    let nested = err["data"]["message"]
        .as_str()
        .map(crate::providers::error_message);
    nested
        .or_else(|| err["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "agent error".into())
}

static INSTANCES: AtomicU64 = AtomicU64::new(1);

pub struct AgentProcess {
    pub agent: AgentId,
    /// Distinguishes restarts, so stale session ids are never reused.
    pub instance: u64,
    rpc: Arc<Rpc>,
    _child: Mutex<Child>,
    auth_methods: Vec<AuthMethod>,
    http_mcp: bool,
}

impl AgentProcess {
    pub async fn spawn(agent: AgentId) -> anyhow::Result<Self> {
        let (program, args) = agent
            .command()
            .with_context(|| format!("{} is not installed (`{}` not found)", agent.name(), agent.command_label()))?;
        let mut child = Command::new(&program)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting {}", program.display()))?;
        let stdout = child.stdout.take().context("agent stdout")?;
        let stdin = child.stdin.take().context("agent stdin")?;
        let rpc = Arc::new(Rpc {
            dialect: agent.dialect(),
            stdin: tokio::sync::Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            routes: Mutex::new(HashMap::new()),
            codex_turns: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
        });
        let reader = Arc::clone(&rpc);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                reader.dispatch(&line).await;
            }
            reader.alive.store(false, Ordering::SeqCst);
            reader.pending.lock().unwrap().clear();
            reader.routes.lock().unwrap().clear();
            reader.codex_turns.lock().unwrap().clear();
        });

        let client_info = json!({ "name": "workbench", "title": "Workbench", "version": env!("CARGO_PKG_VERSION") });
        let init_params = match agent.dialect() {
            Dialect::Acp => json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false },
                    "terminal": false,
                },
                "clientInfo": client_info,
            }),
            Dialect::CodexAppServer => json!({ "clientInfo": client_info, "capabilities": null }),
        };
        let init = rpc
            .request("initialize", init_params, Duration::from_secs(60))
            .await
            .map_err(|e| anyhow!("{} did not start: {}", agent.name(), e.message))?;
        if agent.dialect() == Dialect::CodexAppServer {
            rpc.notify("initialized", json!({})).await;
        }
        Ok(Self {
            agent,
            instance: INSTANCES.fetch_add(1, Ordering::SeqCst),
            rpc,
            _child: Mutex::new(child),
            auth_methods: serde_json::from_value(init["authMethods"].clone()).unwrap_or_default(),
            http_mcp: init["agentCapabilities"]["mcpCapabilities"]["http"].as_bool().unwrap_or(false),
        })
    }

    pub fn is_alive(&self) -> bool {
        self.rpc.alive.load(Ordering::SeqCst)
    }

    pub fn auth_methods(&self) -> &[AuthMethod] {
        &self.auth_methods
    }

    pub fn supports_http_mcp(&self) -> bool {
        self.http_mcp || self.rpc.dialect == Dialect::CodexAppServer
    }

    pub async fn authenticate(&self, method_id: &str) -> anyhow::Result<()> {
        if self.rpc.dialect == Dialect::CodexAppServer {
            bail!("Codex uses its own login. Run `codex login` in a terminal, then try again.");
        }
        self.rpc
            .request("authenticate", json!({ "methodId": method_id }), Duration::from_secs(600))
            .await
            .map(|_| ())
            .map_err(|e| anyhow!("{} sign-in failed: {}", self.agent.name(), e.message))
    }

    pub async fn new_session(&self, cwd: &Path, config: &crate::mcp::ConfigFile) -> anyhow::Result<String> {
        let (method, params) = match self.rpc.dialect {
            Dialect::Acp => (
                "session/new",
                json!({ "cwd": cwd, "mcpServers": mcp_servers(config, self.http_mcp) }),
            ),
            Dialect::CodexAppServer => ("thread/start", codex_app::thread_start(cwd, config)),
        };
        let result = self
            .rpc
            .request(method, params, Duration::from_secs(120))
            .await
            .map_err(|e| self.explain(&e))?;
        result["sessionId"]
            .as_str()
            .or_else(|| result["thread"]["id"].as_str())
            .map(str::to_owned)
            .context("the agent returned no session id")
    }

    /// Updates and permission requests for `session_id` arrive on the receiver
    /// until [`Self::unregister`].
    pub fn register(&self, session_id: &str) -> mpsc::UnboundedReceiver<Inbound> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.rpc.routes.lock().unwrap().insert(session_id.to_owned(), tx);
        rx
    }

    pub fn unregister(&self, session_id: &str) {
        self.rpc.routes.lock().unwrap().remove(session_id);
    }

    /// Runs one prompt turn and returns the ACP stop reason.
    pub async fn prompt(&self, session_id: &str, blocks: Value, level: PermissionLevel) -> anyhow::Result<String> {
        if self.rpc.dialect == Dialect::CodexAppServer {
            return codex_app::run_turn(&self.rpc, session_id, &blocks, level)
                .await
                .map_err(|e| self.explain(&e));
        }
        let result = self
            .rpc
            .request(
                "session/prompt",
                json!({ "sessionId": session_id, "prompt": blocks }),
                TURN_TIMEOUT,
            )
            .await
            .map_err(|e| self.explain(&e))?;
        Ok(result["stopReason"].as_str().unwrap_or("end_turn").to_owned())
    }

    pub async fn cancel(&self, session_id: &str) {
        match self.rpc.dialect {
            Dialect::Acp => self.rpc.notify("session/cancel", json!({ "sessionId": session_id })).await,
            Dialect::CodexAppServer => codex_app::interrupt(&self.rpc, session_id).await,
        }
    }

    pub async fn answer_permission(&self, rpc_id: Value, option_id: Option<&str>) {
        let result = match self.rpc.dialect {
            Dialect::Acp => {
                let outcome = match option_id {
                    Some(id) => json!({ "outcome": "selected", "optionId": id }),
                    None => json!({ "outcome": "cancelled" }),
                };
                json!({ "outcome": outcome })
            }
            Dialect::CodexAppServer => json!({ "decision": option_id.unwrap_or("cancel") }),
        };
        self.rpc.respond(rpc_id, result).await;
    }

    fn explain(&self, e: &RpcError) -> anyhow::Error {
        if e.code == AUTH_REQUIRED {
            return anyhow!(
                "{} needs you to sign in. Open Settings, Accounts, and use its sign-in under Agents.",
                self.agent.name()
            );
        }
        anyhow!("{}: {}", self.agent.name(), e.message)
    }
}

/// Our MCP servers in ACP's `session/new` shape.
fn mcp_servers(config: &crate::mcp::ConfigFile, http_ok: bool) -> Value {
    let pairs = |m: &std::collections::BTreeMap<String, String>| -> Vec<Value> {
        m.iter().map(|(k, v)| json!({ "name": k, "value": v })).collect()
    };
    config
        .mcp_servers
        .iter()
        .filter(|(_, c)| !c.disabled)
        .filter_map(|(name, c)| {
            if let Some(url) = &c.url {
                return http_ok.then(|| {
                    json!({ "type": "http", "name": name, "url": url, "headers": pairs(&c.headers) })
                });
            }
            Some(json!({
                "name": name,
                "command": c.command.clone()?,
                "args": c.args,
                "env": pairs(&c.env),
            }))
        })
        .collect()
}

/// Text of a finished tool call, for the stored tool message.
pub fn tool_output(update: &Value) -> String {
    let mut out = Vec::new();
    for item in update["content"].as_array().into_iter().flatten() {
        match item["type"].as_str() {
            Some("content") => {
                if let Some(text) = item["content"]["text"].as_str() {
                    out.push(text.to_owned());
                }
            }
            Some("diff") => {
                let path = item["path"].as_str().unwrap_or("a file");
                let lines = item["newText"].as_str().map_or(0, |t| t.lines().count());
                out.push(format!("Edited {path} ({lines} lines)"));
            }
            Some("terminal") => out.push("[terminal output]".into()),
            _ => {}
        }
    }
    if out.is_empty() && !update["rawOutput"].is_null() {
        return serde_json::to_string_pretty(&update["rawOutput"]).unwrap_or_default();
    }
    out.join("\n")
}
