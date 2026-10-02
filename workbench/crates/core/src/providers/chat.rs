//! Chat Completions, the lowest common denominator across xAI, OpenRouter,
//! DeepSeek and local servers.

use std::collections::BTreeMap;

use anyhow::bail;
use serde_json::{json, Value};

use super::{Completion, CompletionRequest, Delta, Part, Role};

pub(super) fn body(req: &CompletionRequest<'_>) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": req.instructions })];
    for msg in req.messages {
        let text: String = msg
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        match msg.role {
            Role::User => messages.push(json!({ "role": "user", "content": text })),
            Role::Assistant => {
                let calls: Vec<Value> = msg
                    .parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::ToolCall { id, name, arguments } => Some(json!({
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": arguments },
                        })),
                        _ => None,
                    })
                    .collect();
                let mut m = json!({ "role": "assistant", "content": text });
                if !calls.is_empty() {
                    m["tool_calls"] = Value::Array(calls);
                }
                messages.push(m);
            }
            Role::Tool => {
                for part in &msg.parts {
                    if let Part::ToolResult { call_id, output, .. } = part {
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": output,
                        }));
                    }
                }
            }
        }
    }
    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "stream": true,
    });
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    },
                })
            })
            .collect();
    }
    body
}

#[derive(Default)]
pub(super) struct Decoder {
    out: Completion,
    /// Tool call fragments keyed by the stream's `index`.
    calls: BTreeMap<u64, (String, String, String)>,
}

impl Decoder {
    pub(super) fn feed(&mut self, v: &Value, on_delta: &mut (dyn FnMut(Delta<'_>) + Send)) -> anyhow::Result<bool> {
        if !v["error"].is_null() {
            let message = v["error"]["message"]
                .as_str()
                .or_else(|| v["error"].as_str())
                .unwrap_or("stream error");
            bail!("{message}");
        }
        let Some(choice) = v["choices"].get(0) else {
            return Ok(false);
        };
        let delta = &choice["delta"];
        if let Some(d) = delta["content"].as_str() {
            self.out.text.push_str(d);
            on_delta(Delta::Text(d));
        }
        let reasoning = delta["reasoning_content"]
            .as_str()
            .or_else(|| delta["reasoning"].as_str());
        if let Some(d) = reasoning {
            self.out.reasoning.push_str(d);
            on_delta(Delta::Reasoning(d));
        }
        if let Some(calls) = delta["tool_calls"].as_array() {
            for (pos, call) in calls.iter().enumerate() {
                let index = call["index"].as_u64().unwrap_or(pos as u64);
                let slot = self.calls.entry(index).or_default();
                if let Some(id) = call["id"].as_str() {
                    slot.0.push_str(id);
                }
                if let Some(name) = call["function"]["name"].as_str() {
                    slot.1.push_str(name);
                }
                if let Some(args) = call["function"]["arguments"].as_str() {
                    slot.2.push_str(args);
                }
            }
        }
        match choice["finish_reason"].as_str() {
            Some("length") => self.out.stop_note = Some("Response cut short: output token limit".into()),
            Some("content_filter") => self.out.stop_note = Some("Response stopped by the provider's content filter".into()),
            _ => {}
        }
        Ok(false)
    }

    pub(super) fn finish(mut self) -> Completion {
        for (index, (id, name, args)) in std::mem::take(&mut self.calls) {
            let id = if id.is_empty() { format!("call_{index}") } else { id };
            let args = if args.trim().is_empty() { "{}".to_owned() } else { args };
            self.out.tool_calls.push((id, name, args));
        }
        self.out
    }
}
