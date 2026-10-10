//! The model harness: stream a reply, run the tools it asks for, feed the
//! results back, repeat until it answers without tools or hits the round cap.

use anyhow::bail;
use serde::Serialize;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::app::App;
use crate::config::Choice;
use crate::permissions::{self, ActionKind, Decision, PendingPermission};
use crate::providers::{
    self, Cancelled, ChatMessage, CompletionRequest, Delta, Part, Role, ToolSpec,
};
use crate::skills;
use crate::store::{StoredMessage, UNTITLED};

const BASE_INSTRUCTIONS: &str =
    "You are the assistant inside Workbench, a local desktop workbench \
for making Deadlock mods (VPK merging, Blender, sound and texture edits). Use the tools you have \
when they help, and say plainly when something is outside what they can do. Keep answers short and \
concrete. Never claim a tool ran or succeeded unless its result says so.";

/// Tool results the model gets when a call is not allowed to run.
const BLOCKED: &str = "Blocked: Workbench is set to Read only, so this tool cannot run. \
Do not retry it. Tell the user what you wanted to do.";
const DECLINED: &str =
    "The user declined this tool call. Do not retry it. Ask what they want instead.";

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentEvent {
    MessageSaved {
        message: StoredMessage,
    },
    TextDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    ToolStarted {
        call_id: String,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    SessionRenamed {
        session_id: String,
        title: String,
    },
    Notice {
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    PermissionRequested {
        request_id: String,
        title: String,
        options: Vec<crate::acp::PermissionOption>,
    },
    #[serde(rename_all = "camelCase")]
    PermissionResolved {
        request_id: String,
    },
    Finished,
    Cancelled,
    Failed {
        message: String,
    },
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
    pub async fn run_turn(
        &self,
        session_id: &str,
        text: &str,
        sink: Sink<'_>,
    ) -> anyhow::Result<()> {
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

    async fn turn(
        &self,
        session_id: &str,
        text: &str,
        sink: Sink<'_>,
        cancel: &CancellationToken,
    ) -> anyhow::Result<()> {
        let text = text.trim();
        if text.is_empty() {
            bail!("message is empty");
        }
        let settings = self.settings();
        let Some(choice) = settings.model.clone() else {
            bail!("pick a model first (model menu under the message box)");
        };

        let user = self.store.append(
            session_id,
            Role::User,
            vec![Part::Text {
                text: text.to_owned(),
            }],
            None,
        )?;
        sink(AgentEvent::MessageSaved { message: user });
        self.maybe_title(session_id, text, sink)?;

        let (provider, model) = match choice {
            Choice::Agent { agent } => {
                return self.acp_turn(session_id, text, agent, sink, cancel).await
            }
            Choice::Model { provider, model } => (provider, model),
        };
        let endpoint = self.endpoint(provider).await?;
        let skill_list = self.skills();
        let mut instructions = BASE_INSTRUCTIONS.to_owned();
        if !settings.system_prompt.is_empty() {
            instructions.push_str("\n\n");
            instructions.push_str(&settings.system_prompt);
        }
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
                .map(|m| ChatMessage {
                    role: m.role,
                    parts: m.parts,
                })
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
            let completion =
                match providers::complete(&self.http, &endpoint, &request, cancel, &mut on_delta)
                    .await
                {
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
                sink(AgentEvent::Notice {
                    message: note.clone(),
                });
            }
            let parts = completion.into_parts();
            let calls: Vec<(String, String, String)> = parts
                .iter()
                .filter_map(|p| match p {
                    Part::ToolCall {
                        id,
                        name,
                        arguments,
                    } => Some((id.clone(), name.clone(), arguments.clone())),
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
                let outcome =
                    if let Some(refusal) = self.authorize(session_id, &name, sink, cancel).await? {
                        Ok((refusal.to_owned(), true))
                    } else {
                        sink(AgentEvent::ToolStarted {
                            call_id: call_id.clone(),
                            name: name.clone(),
                        });
                        tokio::select! {
                            () = cancel.cancelled() => return Err(Cancelled.into()),
                            r = self.run_tool(&name, &arguments, &skill_list) => r,
                        }
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

    /// Applies the permission level to one tool call. Returns what to tell the
    /// model when the call may not run.
    async fn authorize(
        &self,
        session_id: &str,
        name: &str,
        sink: Sink<'_>,
        cancel: &CancellationToken,
    ) -> anyhow::Result<Option<&'static str>> {
        // Skill tools only read skill files, so MCP tools are the ones to gate.
        let Some((title, read_only)) = self.mcp.describe(name).await else {
            return Ok(None);
        };
        let kind = if read_only {
            ActionKind::Read
        } else {
            ActionKind::Other
        };
        Ok(match self.settings().permission_level.decide(kind) {
            Decision::Allow => None,
            Decision::Deny => Some(BLOCKED),
            Decision::Ask => {
                (!self.confirm(session_id, name, title, sink, cancel).await?).then_some(DECLINED)
            }
        })
    }

    /// Asks the user about one tool call, unless they already allowed that
    /// tool for the rest of the session.
    async fn confirm(
        &self,
        session_id: &str,
        name: &str,
        title: String,
        sink: Sink<'_>,
        cancel: &CancellationToken,
    ) -> anyhow::Result<bool> {
        let grant = (session_id.to_owned(), name.to_owned());
        if self.tool_grants.lock().unwrap().contains(&grant) {
            return Ok(true);
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        let (answer, answered) = oneshot::channel();
        self.permissions
            .lock()
            .unwrap()
            .insert(request_id.clone(), PendingPermission::Tool(answer));
        sink(AgentEvent::PermissionRequested {
            request_id: request_id.clone(),
            title,
            options: permissions::tool_options(),
        });
        let choice = tokio::select! {
            () = cancel.cancelled() => {
                self.permissions.lock().unwrap().remove(&request_id);
                sink(AgentEvent::PermissionResolved { request_id });
                return Err(Cancelled.into());
            }
            choice = answered => choice.ok().flatten(),
        };
        match choice.as_deref() {
            Some(permissions::ALLOW_SESSION) => {
                self.tool_grants.lock().unwrap().insert(grant);
                Ok(true)
            }
            Some(permissions::ALLOW_ONCE) => Ok(true),
            _ => Ok(false),
        }
    }

    async fn run_tool(
        &self,
        name: &str,
        arguments: &str,
        skill_list: &[skills::Skill],
    ) -> anyhow::Result<(String, bool)> {
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    const TOOL: &str = "mcp__blender__run";

    fn open() -> (Arc<App>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("wb-permissions-{}", uuid::Uuid::new_v4()));
        let app = App::open(&dir, crate::mcp::ConfigFile::default()).unwrap();
        (app, dir)
    }

    /// Runs `confirm`, answering any prompt with `answer`, and counts prompts.
    async fn confirm(
        app: &Arc<App>,
        session: &str,
        answer: Option<&'static str>,
        prompts: &Arc<AtomicUsize>,
    ) -> bool {
        let responder = Arc::clone(app);
        let count = Arc::clone(prompts);
        let sink = move |event: AgentEvent| {
            if let AgentEvent::PermissionRequested { request_id, .. } = event {
                count.fetch_add(1, Ordering::SeqCst);
                let app = Arc::clone(&responder);
                tokio::spawn(async move { app.respond_permission(&request_id, answer).await });
            }
        };
        app.confirm(
            session,
            TOOL,
            "blender.run".into(),
            &sink,
            &CancellationToken::new(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_session_grant_stops_the_prompts_for_that_session_only() {
        let (app, dir) = open();
        let prompts = Arc::new(AtomicUsize::new(0));
        assert!(confirm(&app, "a", Some(permissions::ALLOW_SESSION), &prompts).await);
        assert!(confirm(&app, "a", None, &prompts).await);
        assert_eq!(prompts.load(Ordering::SeqCst), 1);
        assert!(!confirm(&app, "b", None, &prompts).await);
        assert_eq!(prompts.load(Ordering::SeqCst), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn allowing_once_asks_again_next_time() {
        let (app, dir) = open();
        let prompts = Arc::new(AtomicUsize::new(0));
        assert!(confirm(&app, "a", Some(permissions::ALLOW_ONCE), &prompts).await);
        assert!(!confirm(&app, "a", Some("deny"), &prompts).await);
        assert_eq!(prompts.load(Ordering::SeqCst), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cancelling_the_turn_withdraws_the_prompt() {
        let (app, dir) = open();
        let cancel = CancellationToken::new();
        let resolved = Arc::new(AtomicUsize::new(0));
        let (stop, seen) = (cancel.clone(), Arc::clone(&resolved));
        let sink = move |event: AgentEvent| match event {
            AgentEvent::PermissionRequested { .. } => stop.cancel(),
            AgentEvent::PermissionResolved { .. } => {
                seen.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        };
        let outcome = app
            .confirm("a", TOOL, "blender.run".into(), &sink, &cancel)
            .await;
        assert!(outcome.unwrap_err().is::<Cancelled>());
        assert_eq!(resolved.load(Ordering::SeqCst), 1);
        assert!(app.permissions.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
