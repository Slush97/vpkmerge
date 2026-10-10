//! Full harness turn against a fake OpenAI-compatible server: the model asks
//! for a skill, gets it, then answers.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use workbench_core::config::{Choice, SettingsPatch};
use workbench_core::providers::{Part, ProviderId, Role};
use workbench_core::{AgentEvent, App};

fn sse(chunks: &[&str]) -> String {
    let mut out = String::from(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
    );
    for c in chunks {
        out.push_str("data: ");
        out.push_str(c);
        out.push_str("\n\n");
    }
    out.push_str("data: [DONE]\n\n");
    out
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> (String, String) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = stream.read(&mut chunk).await.unwrap();
        assert!(n > 0, "client closed early");
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let len: usize = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse().ok())?
        })
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = stream.read(&mut chunk).await.unwrap();
        buf.extend_from_slice(&chunk[..n]);
    }
    let path = head
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    (
        path,
        String::from_utf8_lossy(&buf[head_end..head_end + len]).to_string(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn skill_tool_round_trip() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let seen = Arc::clone(&bodies);
    tokio::spawn(async move {
        let replies = [
            sse(&[
                r#"{"choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"load_skill","arguments":""}}]}}]}"#,
                r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"name\":\"demo\"}"}}]}}]}"#,
                r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#,
            ]),
            sse(&[
                r#"{"choices":[{"index":0,"delta":{"content":"The skill says "}}]}"#,
                r#"{"choices":[{"index":0,"delta":{"content":"hello."}}]}"#,
                r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            ]),
        ];
        for reply in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let (path, body) = read_request(&mut stream).await;
            assert_eq!(path, "/v1/chat/completions");
            seen.lock()
                .unwrap()
                .push(serde_json::from_str(&body).unwrap());
            stream.write_all(reply.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });

    let dir = std::env::temp_dir().join(format!("workbench-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("skills/demo")).unwrap();
    std::fs::write(
        dir.join("skills/demo/SKILL.md"),
        "---\nname: demo\ndescription: Says hello.\n---\nhello from skill\n",
    )
    .unwrap();

    let app = App::open(&dir, workbench_core::mcp::ConfigFile::default()).unwrap();
    app.update_settings(SettingsPatch {
        model: Some(Choice::Model {
            provider: ProviderId::Local,
            model: "fake-model".into(),
        }),
        local_base_url: Some(format!("http://127.0.0.1:{port}/v1")),
        ..SettingsPatch::default()
    })
    .unwrap();
    let session = app.create_session().unwrap();

    let events = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
    let sink_events = Arc::clone(&events);
    let sink = move |e: AgentEvent| sink_events.lock().unwrap().push(e);
    app.run_turn(&session.id, "What does the demo skill say?", &sink)
        .await
        .unwrap();

    let events = events.lock().unwrap();
    assert!(matches!(events.last(), Some(AgentEvent::Finished)));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolStarted { name, .. } if name == "load_skill")));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::SessionRenamed { title, .. } if title == "What does the demo skill say?")));

    let messages = app.messages(&session.id).unwrap();
    let roles: Vec<Role> = messages.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [Role::User, Role::Assistant, Role::Tool, Role::Assistant]
    );
    assert!(matches!(
        &messages[2].parts[0],
        Part::ToolResult { output, is_error: false, .. } if output.contains("hello from skill")
    ));
    assert!(
        matches!(&messages[3].parts[0], Part::Text { text } if text == "The skill says hello.")
    );

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies[0]["model"], "fake-model");
    assert!(bodies[0]["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("- demo: Says hello."));
    let second = bodies[1]["messages"].as_array().unwrap();
    let tool_msg = second.iter().find(|m| m["role"] == "tool").unwrap();
    assert_eq!(tool_msg["tool_call_id"], "call_1");
    let assistant = second.iter().find(|m| m["role"] == "assistant").unwrap();
    assert_eq!(
        assistant["tool_calls"][0]["function"]["arguments"],
        r#"{"name":"demo"}"#
    );

    let _ = std::fs::remove_dir_all(&dir);
}
