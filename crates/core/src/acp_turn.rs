//! One user turn handed to an ACP agent. The agent runs its own loop and
//! tools; we mirror its stream into the same stored message shape the
//! built-in harness uses, so the chat renders both the same way.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::acp::{self, AgentId, AgentProcess, Inbound, PermissionOption};
use crate::agent::{AgentEvent, Sink};
use crate::app::App;
use crate::providers::{Cancelled, Part, Role};

const TRANSCRIPT_LIMIT: usize = 12_000;
const CANCEL_GRACE: Duration = Duration::from_secs(15);

struct Mirror<'a> {
    app: &'a App,
    session_id: &'a str,
    model: &'static str,
    sink: Sink<'a>,
    text: String,
    reasoning: String,
    /// Tool calls started but not finished, by id, with their titles.
    open: HashMap<String, String>,
}

impl Mirror<'_> {
    fn flush(&mut self, call: Option<Part>) -> anyhow::Result<()> {
        let mut parts = Vec::new();
        let reasoning = std::mem::take(&mut self.reasoning);
        if !reasoning.trim().is_empty() {
            parts.push(Part::Reasoning { text: reasoning });
        }
        let text = std::mem::take(&mut self.text);
        if !text.trim().is_empty() {
            parts.push(Part::Text { text });
        }
        parts.extend(call);
        if parts.is_empty() {
            return Ok(());
        }
        let saved = self
            .app
            .store
            .append(self.session_id, Role::Assistant, parts, Some(self.model))?;
        (self.sink)(AgentEvent::MessageSaved { message: saved });
        Ok(())
    }

    fn finish_call(&mut self, id: &str, update: &Value) -> anyhow::Result<()> {
        let Some(name) = self.open.remove(id) else {
            return Ok(());
        };
        let failed = update["status"] == "failed";
        let mut output = acp::tool_output(update);
        if output.trim().is_empty() {
            output = if failed { "The tool failed without output." } else { "Done." }.into();
        }
        let saved = self.app.store.append(
            self.session_id,
            Role::Tool,
            vec![Part::ToolResult {
                call_id: id.to_owned(),
                name,
                output,
                is_error: failed,
            }],
            None,
        )?;
        (self.sink)(AgentEvent::MessageSaved { message: saved });
        Ok(())
    }

    fn on_update(&mut self, u: &Value) -> anyhow::Result<()> {
        let done = matches!(u["status"].as_str(), Some("completed" | "failed"));
        match u["sessionUpdate"].as_str().unwrap_or_default() {
            "agent_message_chunk" => {
                if let Some(t) = u["content"]["text"].as_str() {
                    self.text.push_str(t);
                    (self.sink)(AgentEvent::TextDelta { text: t.to_owned() });
                }
            }
            "agent_thought_chunk" => {
                if let Some(t) = u["content"]["text"].as_str() {
                    self.reasoning.push_str(t);
                    (self.sink)(AgentEvent::ReasoningDelta { text: t.to_owned() });
                }
            }
            "tool_call" => {
                let id = u["toolCallId"].as_str().unwrap_or_default().to_owned();
                if id.is_empty() {
                    return Ok(());
                }
                if !self.open.contains_key(&id) {
                    let title = u["title"]
                        .as_str()
                        .filter(|t| !t.is_empty())
                        .or_else(|| u["kind"].as_str())
                        .unwrap_or("Tool")
                        .to_owned();
                    let arguments = if u["rawInput"].is_null() {
                        "{}".to_owned()
                    } else {
                        serde_json::to_string(&u["rawInput"])?
                    };
                    self.flush(Some(Part::ToolCall {
                        id: id.clone(),
                        name: title.clone(),
                        arguments,
                    }))?;
                    self.open.insert(id.clone(), title.clone());
                    (self.sink)(AgentEvent::ToolStarted { call_id: id.clone(), name: title });
                }
                if done {
                    self.finish_call(&id, u)?;
                }
            }
            "tool_call_update" => {
                let id = u["toolCallId"].as_str().unwrap_or_default().to_owned();
                if let (Some(title), Some(slot)) = (u["title"].as_str(), self.open.get_mut(&id)) {
                    title.clone_into(slot);
                }
                if done {
                    self.finish_call(&id, u)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

impl App {
    pub(crate) async fn acp_turn(
        &self,
        session_id: &str,
        text: &str,
        agent: AgentId,
        sink: Sink<'_>,
        cancel: &CancellationToken,
    ) -> anyhow::Result<()> {
        let process = tokio::select! {
            () = cancel.cancelled() => return Err(Cancelled.into()),
            p = self.agent(agent) => p?,
        };
        let (acp_session, fresh) = self.acp_session(session_id, &process, cancel).await?;

        let mut blocks = Vec::new();
        if fresh {
            if let Some(context) = self.transcript(session_id)? {
                blocks.push(json!({ "type": "text", "text": context }));
            }
        }
        blocks.push(json!({ "type": "text", "text": text }));

        let mut rx = process.register(&acp_session);
        let mut mirror = Mirror {
            app: self,
            session_id,
            model: agent.slug(),
            sink,
            text: String::new(),
            reasoning: String::new(),
            open: HashMap::new(),
        };
        let mut asked: Vec<String> = Vec::new();
        let mut cancelled = false;
        let mut rx_open = true;
        let grace = tokio::time::sleep(Duration::MAX);
        tokio::pin!(grace);
        let prompt = process.prompt(&acp_session, Value::Array(blocks));
        tokio::pin!(prompt);

        let outcome = loop {
            tokio::select! {
                biased;
                msg = rx.recv(), if rx_open => match msg {
                    Some(Inbound::Update(u)) => {
                        if let Err(e) = mirror.on_update(&u) {
                            break Err(e);
                        }
                    }
                    Some(Inbound::Permission { rpc_id, params }) => {
                        if cancelled {
                            process.answer_permission(rpc_id, None).await;
                        } else {
                            asked.push(self.ask_permission(&process, rpc_id, &params, sink));
                        }
                    }
                    None => rx_open = false,
                },
                () = cancel.cancelled(), if !cancelled => {
                    cancelled = true;
                    process.cancel(&acp_session).await;
                    self.drop_permissions(&asked, sink).await;
                    grace.as_mut().reset(tokio::time::Instant::now() + CANCEL_GRACE);
                }
                () = &mut grace => break Err(Cancelled.into()),
                res = &mut prompt => break res,
            }
        };

        while let Ok(msg) = rx.try_recv() {
            match msg {
                Inbound::Update(u) => mirror.on_update(&u)?,
                Inbound::Permission { rpc_id, .. } => process.answer_permission(rpc_id, None).await,
            }
        }
        process.unregister(&acp_session);
        self.drop_permissions(&asked, sink).await;
        mirror.flush(None)?;

        let stop = match outcome {
            Ok(stop) => stop,
            Err(e) => {
                if !process.is_alive() {
                    self.acp_sessions
                        .lock()
                        .unwrap()
                        .remove(&(session_id.to_owned(), agent));
                }
                return Err(e);
            }
        };
        if cancelled || stop == "cancelled" {
            return Err(Cancelled.into());
        }
        let note = match stop.as_str() {
            "max_tokens" => Some("Reply cut short: output token limit"),
            "max_turn_requests" => Some("The agent hit its step limit for this turn"),
            "refusal" => Some("The agent declined this request"),
            _ => None,
        };
        if let Some(message) = note {
            sink(AgentEvent::Notice { message: message.into() });
        }
        Ok(())
    }

    /// Reuses this conversation's ACP session while the same agent process
    /// is alive; otherwise opens one with our MCP servers attached.
    async fn acp_session(
        &self,
        session_id: &str,
        process: &AgentProcess,
        cancel: &CancellationToken,
    ) -> anyhow::Result<(String, bool)> {
        let key = (session_id.to_owned(), process.agent);
        let known = self
            .acp_sessions
            .lock()
            .unwrap()
            .get(&key)
            .filter(|(_, instance)| *instance == process.instance)
            .map(|(sid, _)| sid.clone());
        if let Some(sid) = known {
            return Ok((sid, false));
        }
        let config = self.mcp_config_for_agents();
        let cwd = self.settings().agent_cwd;
        let sid = tokio::select! {
            () = cancel.cancelled() => return Err(Cancelled.into()),
            sid = process.new_session(&cwd, &config) => sid?,
        };
        self.acp_sessions
            .lock()
            .unwrap()
            .insert(key, (sid.clone(), process.instance));
        Ok((sid, true))
    }

    fn ask_permission(&self, process: &Arc<AgentProcess>, rpc_id: Value, params: &Value, sink: Sink<'_>) -> String {
        let request_id = uuid::Uuid::new_v4().to_string();
        let title = params["toolCall"]["title"]
            .as_str()
            .filter(|t| !t.is_empty())
            .unwrap_or("Run a tool")
            .to_owned();
        let options: Vec<PermissionOption> =
            serde_json::from_value(params["options"].clone()).unwrap_or_default();
        self.permissions
            .lock()
            .unwrap()
            .insert(request_id.clone(), (Arc::clone(process), rpc_id));
        sink(AgentEvent::PermissionRequested {
            request_id: request_id.clone(),
            title,
            options,
        });
        request_id
    }

    /// Answers any still-open prompts from this turn as cancelled.
    async fn drop_permissions(&self, ids: &[String], sink: Sink<'_>) {
        for id in ids {
            let open = self.permissions.lock().unwrap().remove(id);
            if let Some((process, rpc_id)) = open {
                process.answer_permission(rpc_id, None).await;
                sink(AgentEvent::PermissionResolved { request_id: id.clone() });
            }
        }
    }

    /// Earlier messages, for an agent joining a conversation midway.
    fn transcript(&self, session_id: &str) -> anyhow::Result<Option<String>> {
        let messages = self.store.messages(session_id)?;
        let earlier = &messages[..messages.len().saturating_sub(1)];
        let mut lines = Vec::new();
        for m in earlier {
            for part in &m.parts {
                match (m.role, part) {
                    (Role::User, Part::Text { text }) => lines.push(format!("User: {text}")),
                    (Role::Assistant, Part::Text { text }) => lines.push(format!("Assistant: {text}")),
                    (Role::Assistant, Part::ToolCall { name, .. }) => {
                        lines.push(format!("(Assistant ran the tool: {name})"));
                    }
                    _ => {}
                }
            }
        }
        if lines.is_empty() {
            return Ok(None);
        }
        let mut body = lines.join("\n\n");
        if body.len() > TRANSCRIPT_LIMIT {
            let mut cut = body.len() - TRANSCRIPT_LIMIT;
            while !body.is_char_boundary(cut) {
                cut += 1;
            }
            body = format!("[earlier messages omitted]\n\n{}", &body[cut..]);
        }
        Ok(Some(format!(
            "Context: this conversation started before you joined it. Earlier messages:\n\n{body}\n\nThe new request follows."
        )))
    }
}
