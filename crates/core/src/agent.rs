//! The model harness: stream a reply, run the tools it asks for, feed the
//! results back, repeat until it answers without tools or hits the round cap.

use anyhow::bail;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::config::Choice;
use crate::providers::{self, Cancelled, ChatMessage, CompletionRequest, Delta, Part, Role, ToolSpec};
use crate::skills;
use crate::store::{StoredMessage, UNTITLED};

const BASE_INSTRUCTIONS: &str = "You are the assistant inside Workbench, a local desktop workbench \
for making Deadlock mods (VPK merging, Blender, sound and texture edits). Use the tools you have \
when they help, and say plainly when something is outside what they can do. Keep answers short and \
concrete. Never claim a tool ran or succeeded unless its result says so.";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentEvent {
    MessageSaved { message: StoredMessage },
    TextDelta { text: String },
    ReasoningDelta { text: String },
    #[serde(rename_all = "camelCase")]
    ToolStarted { call_id: String, name: String },
    #[serde(rename_all = "camelCase")]
    SessionRenamed { session_id: String, title: String },
    Notice { message: String },
    #[serde(rename_all = "camelCase")]
    PermissionRequested {
        request_id: String,
        title: String,
        options: Vec<crate::acp::PermissionOption>,
    },
    #[serde(rename_all = "camelCase")]
    PermissionResolved { request_id: String },
    Finished,
    Cancelled,
    Failed { message: String },
}

pub(crate) type Sink<'a> = &'a (dyn Fn(AgentEvent) + Send + Sync);

impl App {
    pub fn cancel_turn(&self, session_id: &str) {
        if let Some((_, token)) = self.turns.lock().unwrap().remove(session_id) {
            token.cancel();
        }
    }

    /// Runs one user turn to completion. Every outcome, including failure,
    /// is reported through `sink`; the return value is for logging only.
    pub async fn run_turn(&self, session_id: &str, text: &str, sink: Sink<'_>) -> anyhow::Result<()> {
        let cancel = CancellationToken::new();
        let turn_id = uuid::Uuid::new_v4();
        if let Some((_, previous)) = self
            .turns
            .lock()
            .unwrap()
            .insert(session_id.to_owned(), (turn_id, cancel.clone()))
        {
            previous.cancel();
        }
        let result = self.turn(session_id, text, sink, &cancel).await;
        {
            let mut turns = self.turns.lock().unwrap();
            if turns.get(session_id).is_some_and(|(id, _)| *id == turn_id) {
                turns.remove(session_id);
            }
        }
        match &result {
            Ok(()) => sink(AgentEvent::Finished),
            Err(e) if e.is::<Cancelled>() => sink(AgentEvent::Cancelled),
            Err(e) => sink(AgentEvent::Failed {
                message: format!("{e:#}"),
            }),
        }
        result
    }

    async fn turn(&self, session_id: &str, text: &str, sink: Sink<'_>, cancel: &CancellationToken) -> anyhow::Result<()> {
        let text = text.trim();
        if text.is_empty() {
            bail!("message is empty");
        }
        let settings = self.settings();
        let Some(choice) = settings.model.clone() else {
            bail!("pick a model first (model menu under the message box)");
        };

        let user = self
            .store
            .append(session_id, Role::User, vec![Part::Text { text: text.to_owned() }], None)?;
        sink(AgentEvent::MessageSaved { message: user });
        self.maybe_title(session_id, text, sink)?;

        let (provider, model) = match choice {
            Choice::Agent { agent } => return self.acp_turn(session_id, text, agent, sink, cancel).await,
            Choice::Model { provider, model } => (provider, model),
        };
        let endpoint = self.endpoint(provider).await?;
        let skill_list = self.skills();
        let mut instructions = BASE_INSTRUCTIONS.to_owned();
        let mut tools: Vec<ToolSpec> = self.mcp.tool_specs().await;
        if let Some(index) = skills::index_prompt(&skill_list) {
            instructions.push_str("\n\n");
            instructions.push_str(&index);
            tools.extend(skills::tool_specs());
        }

        for _ in 0..settings.max_tool_rounds {
            let history: Vec<ChatMessage> = self
                .store
                .messages(session_id)?
                .into_iter()
                .map(|m| ChatMessage { role: m.role, parts: m.parts })
                .collect();
            let request = CompletionRequest {
                model: &model,
                instructions: &instructions,
                messages: &history,
                tools: &tools,
            };
            let mut partial = String::new();
            let mut on_delta = |d: Delta<'_>| match d {
                Delta::Text(t) => {
                    partial.push_str(t);
                    sink(AgentEvent::TextDelta { text: t.to_owned() });
                }
                Delta::Reasoning(t) => sink(AgentEvent::ReasoningDelta { text: t.to_owned() }),
            };
            let completion = match providers::complete(&self.http, &endpoint, &request, cancel, &mut on_delta).await {
                Ok(c) => c,
                Err(e) => {
                    if e.is::<Cancelled>() && !partial.is_empty() {
                        let saved = self.store.append(
                            session_id,
                            Role::Assistant,
                            vec![Part::Text { text: partial }],
                            Some(&model),
                        )?;
                        sink(AgentEvent::MessageSaved { message: saved });
                    }
                    return Err(e);
                }
            };
            if let Some(note) = &completion.stop_note {
                sink(AgentEvent::Notice { message: note.clone() });
            }
            let parts = completion.into_parts();
            let calls: Vec<(String, String, String)> = parts
                .iter()
                .filter_map(|p| match p {
                    Part::ToolCall { id, name, arguments } => Some((id.clone(), name.clone(), arguments.clone())),
                    _ => None,
                })
                .collect();
            if parts.is_empty() {
                bail!("the model returned an empty reply");
            }
            let saved = self
                .store
                .append(session_id, Role::Assistant, parts, Some(&model))?;
            sink(AgentEvent::MessageSaved { message: saved });
            if calls.is_empty() {
                return Ok(());
            }

            for (call_id, name, arguments) in calls {
                sink(AgentEvent::ToolStarted {
                    call_id: call_id.clone(),
                    name: name.clone(),
                });
                let outcome = tokio::select! {
                    () = cancel.cancelled() => return Err(Cancelled.into()),
                    r = self.run_tool(&name, &arguments, &skill_list) => r,
                };
                let (output, is_error) = match outcome {
                    Ok(pair) => pair,
                    Err(e) => (format!("{e:#}"), true),
                };
                let saved = self.store.append(
                    session_id,
                    Role::Tool,
                    vec![Part::ToolResult {
                        call_id,
                        name,
                        output,
                        is_error,
                    }],
                    None,
                )?;
                sink(AgentEvent::MessageSaved { message: saved });
            }
        }
        bail!(
            "stopped after {} tool rounds without a final answer (raise the limit in Settings)",
            settings.max_tool_rounds
        )
    }

    async fn run_tool(&self, name: &str, arguments: &str, skill_list: &[skills::Skill]) -> anyhow::Result<(String, bool)> {
        if name == skills::LOAD_TOOL || name == skills::READ_FILE_TOOL {
            let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
            let skill = args["name"].as_str().unwrap_or_default();
            let text = if name == skills::LOAD_TOOL {
                skills::load(skill_list, skill)?
            } else {
                skills::read_file(skill_list, skill, args["path"].as_str().unwrap_or_default())?
            };
            return Ok((text, false));
        }
        if self.mcp.is_mcp_tool(name).await {
            return self.mcp.call(name, arguments).await;
        }
        bail!("no tool named {name}")
    }

    fn maybe_title(&self, session_id: &str, text: &str, sink: Sink<'_>) -> anyhow::Result<()> {
        let Some(session) = self.store.session(session_id)? else {
            bail!("session no longer exists");
        };
        if session.title != UNTITLED {
            return Ok(());
        }
        let first_line = text.lines().next().unwrap_or(text).trim();
        let mut title: String = first_line.chars().take(60).collect();
        if first_line.chars().count() > 60 {
            title = format!("{}...", title.trim_end());
        }
        self.store.rename_session(session_id, &title)?;
        sink(AgentEvent::SessionRenamed {
            session_id: session_id.to_owned(),
            title,
        });
        Ok(())
    }
}
