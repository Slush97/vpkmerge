//! `codex app-server` protocol, translated into ACP-shaped session updates.
//! Uses the user's installed Codex and its own login, so new models work
//! as soon as Codex itself supports them.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::{Inbound, Rpc, RpcError, TURN_TIMEOUT};
use crate::mcp::ConfigFile;
use crate::permissions::PermissionLevel;

const MAX_OUTPUT: usize = 20_000;

pub(super) struct TurnWaiter {
    turn_id: Option<String>,
    done: Option<oneshot::Sender<Result<String, RpcError>>>,
}

/// `thread/start` params. Our MCP servers ride along as config overrides.
pub(super) fn thread_start(cwd: &Path, config: &ConfigFile) -> Value {
    let mut overrides = serde_json::Map::new();
    for (name, c) in config.mcp_servers.iter().filter(|(_, c)| !c.disabled) {
        let server = if let Some(url) = &c.url {
            json!({ "url": url, "http_headers": c.headers })
        } else if let Some(command) = &c.command {
            json!({ "command": command, "args": c.args, "env": c.env })
        } else {
            continue;
        };
        overrides.insert(format!("mcp_servers.{name}"), server);
    }
    json!({ "cwd": cwd, "config": overrides })
}

/// Codex's approval policy and sandbox for a permission level. Read only and
/// Full access never ask, so the sandbox is what holds the line there. The
/// levels in between make Codex ask about everything it does not know to be
/// harmless, and the level answers those requests.
fn policy(level: PermissionLevel) -> (&'static str, &'static str) {
    match level {
        PermissionLevel::ReadOnly => ("never", "readOnly"),
        PermissionLevel::Ask | PermissionLevel::AutoEdit => ("untrusted", "workspaceWrite"),
        PermissionLevel::FullAccess => ("never", "dangerFullAccess"),
    }
}

pub(super) async fn run_turn(
    rpc: &Rpc,
    thread_id: &str,
    blocks: &Value,
    level: PermissionLevel,
) -> Result<String, RpcError> {
    let (tx, rx) = oneshot::channel();
    rpc.codex_turns.lock().unwrap().insert(
        thread_id.to_owned(),
        TurnWaiter { turn_id: None, done: Some(tx) },
    );
    let input: Vec<Value> = blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["text"].as_str())
        .map(|text| json!({ "type": "text", "text": text, "text_elements": [] }))
        .collect();
    // Sent with every turn, so a level change reaches a thread that is already running.
    let (approval, sandbox) = policy(level);
    let params = json!({
        "threadId": thread_id,
        "input": input,
        "approvalPolicy": approval,
        "sandboxPolicy": { "type": sandbox },
    });
    match rpc.request("turn/start", params, Duration::from_secs(120)).await {
        Ok(started) => {
            if let Some(w) = rpc.codex_turns.lock().unwrap().get_mut(thread_id) {
                w.turn_id = started["turn"]["id"].as_str().map(str::to_owned);
            }
        }
        Err(e) => {
            rpc.codex_turns.lock().unwrap().remove(thread_id);
            return Err(e);
        }
    }
    match tokio::time::timeout(TURN_TIMEOUT, rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err(RpcError::exited()),
        Err(_) => Err(RpcError { code: 0, message: "the turn timed out".into() }),
    }
}

pub(super) async fn interrupt(rpc: &Rpc, thread_id: &str) {
    let turn_id = rpc
        .codex_turns
        .lock()
        .unwrap()
        .get(thread_id)
        .and_then(|w| w.turn_id.clone());
    if let Some(turn_id) = turn_id {
        let _ = rpc
            .request(
                "turn/interrupt",
                json!({ "threadId": thread_id, "turnId": turn_id }),
                Duration::from_secs(10),
            )
            .await;
    }
}

pub(super) async fn dispatch(rpc: &Arc<Rpc>, method: &str, msg: &Value) {
    let params = &msg["params"];
    let thread = params["threadId"].as_str().unwrap_or_default();
    if let Some(id) = msg.get("id") {
        match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                let kind = if method == "item/fileChange/requestApproval" { "edit" } else { "execute" };
                let permission = json!({
                    "toolCall": { "title": approval_title(method, params), "kind": kind },
                    "options": [
                        { "optionId": "accept", "name": "Allow once", "kind": "allow_once" },
                        { "optionId": "acceptForSession", "name": "Allow for this session", "kind": "allow_always" },
                        { "optionId": "decline", "name": "Deny", "kind": "reject_once" },
                    ],
                });
                let delivered = rpc.route(thread).is_some_and(|r| {
                    r.send(Inbound::Permission { rpc_id: id.clone(), params: permission }).is_ok()
                });
                if !delivered {
                    rpc.respond(id.clone(), json!({ "decision": "cancel" })).await;
                }
            }
            _ => rpc.reject(id.clone(), method).await,
        }
        return;
    }
    let update = match method {
        "item/agentMessage/delta" => Some(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": params["delta"] },
        })),
        "item/reasoning/summaryTextDelta" => Some(json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": { "type": "text", "text": params["delta"] },
        })),
        "item/started" => tool_started(&params["item"]),
        "item/completed" => tool_completed(&params["item"]),
        "turn/completed" => {
            finish_turn(rpc, thread, &params["turn"]);
            None
        }
        _ => None,
    };
    if let (Some(update), Some(route)) = (update, rpc.route(thread)) {
        let _ = route.send(Inbound::Update(update));
    }
}

fn finish_turn(rpc: &Rpc, thread: &str, turn: &Value) {
    let result = match turn["status"].as_str() {
        Some("completed") => Ok("end_turn".to_owned()),
        Some("interrupted") => Ok("cancelled".to_owned()),
        _ => Err(RpcError {
            code: 0,
            message: turn["error"]["message"]
                .as_str()
                .map_or_else(|| "the turn failed".to_owned(), crate::providers::error_message),
        }),
    };
    let waiter = rpc.codex_turns.lock().unwrap().remove(thread);
    if let Some(tx) = waiter.and_then(|mut w| w.done.take()) {
        let _ = tx.send(result);
    }
}

fn approval_title(method: &str, params: &Value) -> String {
    let reason = params["reason"].as_str().map(|r| format!(" ({r})")).unwrap_or_default();
    if method == "item/fileChange/requestApproval" {
        return format!("Apply file changes{reason}");
    }
    let command = params["command"].as_str().unwrap_or("a command");
    format!("Run {}{reason}", short(command))
}

fn short(s: &str) -> String {
    let line = s.lines().next().unwrap_or_default();
    if line.chars().count() > 80 {
        format!("{}...", line.chars().take(80).collect::<String>())
    } else {
        line.to_owned()
    }
}

/// Title and input shown on the tool card, for item types that are tools.
fn tool_view(item: &Value) -> Option<(String, Value)> {
    match item["type"].as_str()? {
        "commandExecution" => Some((
            format!("Run {}", short(item["command"].as_str().unwrap_or("a command"))),
            json!({ "command": item["command"], "cwd": item["cwd"] }),
        )),
        "fileChange" => {
            let paths: Vec<&str> = item["changes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["path"].as_str())
                .collect();
            let title = match paths.as_slice() {
                [one] => format!("Edit {one}"),
                many => format!("Edit {} files", many.len()),
            };
            Some((title, json!({ "files": paths })))
        }
        "mcpToolCall" => Some((
            format!(
                "{}.{}",
                item["server"].as_str().unwrap_or("mcp"),
                item["tool"].as_str().unwrap_or("tool")
            ),
            item["arguments"].clone(),
        )),
        "dynamicToolCall" => Some((
            item["tool"]
                .as_str()
                .or_else(|| item["name"].as_str())
                .unwrap_or("Tool")
                .to_owned(),
            item["arguments"].clone(),
        )),
        "webSearch" => Some(("Web search".into(), json!({ "query": item["query"] }))),
        _ => None,
    }
}

fn tool_started(item: &Value) -> Option<Value> {
    let (title, input) = tool_view(item)?;
    Some(json!({
        "sessionUpdate": "tool_call",
        "toolCallId": item["id"],
        "title": title,
        "rawInput": input,
        "status": "in_progress",
    }))
}

fn tool_completed(item: &Value) -> Option<Value> {
    let (title, _) = tool_view(item)?;
    let kind = item["type"].as_str().unwrap_or_default();
    let mut failed = matches!(item["status"].as_str(), Some("failed" | "declined"));
    let text = match kind {
        "commandExecution" => {
            let out = item["aggregatedOutput"].as_str().unwrap_or_default().trim_end();
            match item["exitCode"].as_i64() {
                Some(code) if code != 0 => {
                    failed = true;
                    format!("{out}\n[exit code {code}]")
                }
                _ => out.to_owned(),
            }
        }
        "fileChange" => item["changes"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|c| {
                let verb = c["kind"]["type"].as_str().or_else(|| c["kind"].as_str()).unwrap_or("update");
                format!("{verb} {}", c["path"].as_str().unwrap_or("?"))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        "mcpToolCall" => {
            if let Some(err) = item["error"]["message"].as_str() {
                failed = true;
                err.to_owned()
            } else {
                item["result"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        "webSearch" => "Searched the web.".into(),
        _ => String::new(),
    };
    let text: String = text.chars().take(MAX_OUTPUT).collect();
    Some(json!({
        "sessionUpdate": "tool_call_update",
        "toolCallId": item["id"],
        "title": title,
        "status": if failed { "failed" } else { "completed" },
        "content": [{ "type": "content", "content": { "type": "text", "text": text } }],
    }))
}
