//! Opt-in: one short turn through a real ACP agent, using its own sign-in.
//! `cargo test -p workbench-core --test acp_live -- --ignored --nocapture`
//! Pick agents with `WORKBENCH_ACP_AGENTS=grok,codex` (default: both).

use std::sync::{Arc, Mutex};

use workbench_core::acp::AgentId;
use workbench_core::config::{Choice, SettingsPatch};
use workbench_core::permissions::PermissionLevel;
use workbench_core::providers::{Part, Role};
use workbench_core::{AgentEvent, App};

async fn one_turn(agent: AgentId) {
    let dir = std::env::temp_dir().join(format!("workbench-acp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::open(&dir, workbench_core::mcp::ConfigFile::default()).unwrap();
    app.update_settings(SettingsPatch {
        model: Some(Choice::Agent { agent }),
        agent_cwd: Some(dir.clone()),
        ..SettingsPatch::default()
    })
    .unwrap();
    let session = app.create_session().unwrap();
    let events = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
    let seen = Arc::clone(&events);
    let sink = move |e: AgentEvent| seen.lock().unwrap().push(e);
    let result = app
        .run_turn(
            &session.id,
            "Reply with exactly the word: pong. Do not use any tools.",
            &sink,
        )
        .await;
    println!(
        "{} last event: {:?}",
        agent.name(),
        events.lock().unwrap().last()
    );
    result.unwrap();
    let messages = app.messages(&session.id).unwrap();
    let reply: String = messages
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .flat_map(|m| &m.parts)
        .filter_map(|p| match p {
            Part::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    println!("{} replied: {reply:?}", agent.name());
    assert!(reply.to_lowercase().contains("pong"));

    // Second turn in the same session: a real tool call, approving any prompt.
    let approver = Arc::clone(&app);
    let asked = Arc::new(Mutex::new(0usize));
    let asked_in_sink = Arc::clone(&asked);
    let sink = move |e: AgentEvent| {
        if let AgentEvent::PermissionRequested {
            request_id,
            options,
            ..
        } = e
        {
            *asked_in_sink.lock().unwrap() += 1;
            let allow = options
                .iter()
                .find(|o| o.kind == "allow_once")
                .map(|o| o.option_id.clone());
            let app = Arc::clone(&approver);
            tokio::spawn(
                async move { app.respond_permission(&request_id, allow.as_deref()).await },
            );
        }
    };
    app.run_turn(
        &session.id,
        "Run the shell command `echo workbench-ok` and tell me exactly what it printed.",
        &sink,
    )
    .await
    .unwrap();
    let messages = app.messages(&session.id).unwrap();
    let tool_results: Vec<(String, String, bool)> = messages
        .iter()
        .flat_map(|m| &m.parts)
        .filter_map(|p| match p {
            Part::ToolResult {
                name,
                output,
                is_error,
                ..
            } => Some((name.clone(), output.clone(), *is_error)),
            _ => None,
        })
        .collect();
    println!(
        "{} tools: {tool_results:?}; permission prompts: {}",
        agent.name(),
        asked.lock().unwrap()
    );
    assert!(tool_results
        .iter()
        .any(|(_, out, err)| !err && out.contains("workbench-ok")));
    app.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Asks the agent to create a file under `level`. Returns whether the file
/// exists afterwards and how many permission prompts reached the user.
async fn write_under(agent: AgentId, level: PermissionLevel) -> (bool, usize) {
    let dir = std::env::temp_dir().join(format!("workbench-acp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let app = App::open(&dir, workbench_core::mcp::ConfigFile::default()).unwrap();
    app.update_settings(SettingsPatch {
        model: Some(Choice::Agent { agent }),
        agent_cwd: Some(dir.clone()),
        permission_level: Some(level),
        ..SettingsPatch::default()
    })
    .unwrap();
    let session = app.create_session().unwrap();
    let prompts = Arc::new(Mutex::new(0usize));
    let asked = Arc::clone(&prompts);
    let sink = move |e: AgentEvent| {
        if matches!(e, AgentEvent::PermissionRequested { .. }) {
            *asked.lock().unwrap() += 1;
        }
    };
    // A blocked write ends some agents' turns early, which is not a failure here.
    let outcome = app
        .run_turn(
            &session.id,
            "Create a file named made.txt in the current directory containing the word hi. \
             If you cannot, say so and stop.",
            &sink,
        )
        .await;
    let made = dir.join("made.txt").exists();
    let prompts = *prompts.lock().unwrap();
    println!(
        "{} under {level:?}: made={made} prompts={prompts} outcome={outcome:?}",
        agent.name()
    );
    app.shutdown().await;
    let _ = std::fs::remove_dir_all(&dir);
    (made, prompts)
}

fn selected() -> Vec<AgentId> {
    let wanted = std::env::var("WORKBENCH_ACP_AGENTS").unwrap_or_else(|_| "grok,codex".into());
    AgentId::ALL
        .into_iter()
        .filter(|agent| {
            let key = match agent {
                AgentId::Grok => "grok",
                AgentId::Codex => "codex",
            };
            wanted.split(',').any(|w| w.trim() == key)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to real agents with the user's accounts"]
async fn agents_answer() {
    for agent in selected() {
        one_turn(agent).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to real agents with the user's accounts"]
async fn permission_level_holds() {
    for agent in selected() {
        assert_eq!(
            write_under(agent, PermissionLevel::ReadOnly).await,
            (false, 0)
        );
        assert_eq!(
            write_under(agent, PermissionLevel::FullAccess).await,
            (true, 0)
        );
    }
}
