//! OpenAI Responses API. History is sent in full on every request because
//! Sign in with ChatGPT requires `store: false`.

use anyhow::bail;
use serde_json::{json, Value};

use super::{Completion, CompletionRequest, Delta, Part, Role};

pub(super) fn body(req: &CompletionRequest<'_>) -> Value {
    let mut input = Vec::new();
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
            Role::User => input.push(json!({ "role": "user", "content": text })),
            Role::Assistant => {
                if !text.is_empty() {
                    input.push(json!({ "role": "assistant", "content": text }));
                }
                for part in &msg.parts {
                    if let Part::ToolCall {
                        id,
                        name,
                        arguments,
                    } = part
                    {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": id,
                            "name": name,
                            "arguments": arguments,
                        }));
                    }
                }
            }
            Role::Tool => {
                for part in &msg.parts {
                    if let Part::ToolResult {
                        call_id, output, ..
                    } = part
                    {
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": call_id,
                            "output": output,
                        }));
                    }
                }
            }
        }
    }
    let mut body = json!({
        "model": req.model,
        "instructions": req.instructions,
        "input": input,
        "stream": true,
        "store": false,
    });
    if !req.tools.is_empty() {
        body["tools"] = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect();
    }
    body
}

#[derive(Default)]
pub(super) struct Decoder {
    out: Completion,
    completed: bool,
}

impl Decoder {
    /// Returns true once the response is complete.
    pub(super) fn feed(
        &mut self,
        v: &Value,
        on_delta: &mut (dyn FnMut(Delta<'_>) + Send),
    ) -> anyhow::Result<bool> {
        match v["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" => {
                let d = v["delta"].as_str().unwrap_or_default();
                self.out.text.push_str(d);
                on_delta(Delta::Text(d));
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let d = v["delta"].as_str().unwrap_or_default();
                self.out.reasoning.push_str(d);
                on_delta(Delta::Reasoning(d));
            }
            "response.output_item.done" if v["item"]["type"] == "function_call" => {
                let item = &v["item"];
                self.out.tool_calls.push((
                    item["call_id"].as_str().unwrap_or_default().to_owned(),
                    item["name"].as_str().unwrap_or_default().to_owned(),
                    item["arguments"].as_str().unwrap_or("{}").to_owned(),
                ));
            }
            "response.completed" => {
                self.completed = true;
                return Ok(true);
            }
            "response.incomplete" => {
                let reason = v["response"]["incomplete_details"]["reason"]
                    .as_str()
                    .unwrap_or("unknown reason");
                self.out.stop_note = Some(format!("Response cut short: {reason}"));
                self.completed = true;
                return Ok(true);
            }
            "response.failed" => {
                let err = &v["response"]["error"];
                let code = err["code"].as_str().unwrap_or("failed");
                let message = err["message"]
                    .as_str()
                    .unwrap_or("the model request failed");
                bail!("{code}: {message}");
            }
            "error" => {
                let message = v["message"]
                    .as_str()
                    .or_else(|| v["error"]["message"].as_str())
                    .unwrap_or("stream error");
                bail!("{message}");
            }
            _ => {}
        }
        Ok(false)
    }

    pub(super) fn finish(self) -> anyhow::Result<Completion> {
        if !self.completed {
            bail!("the response stream ended before completion");
        }
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::providers::{ChatMessage, ToolSpec};

    #[test]
    fn history_maps_to_input_items() {
        let messages = [
            ChatMessage {
                role: Role::User,
                parts: vec![Part::Text { text: "hi".into() }],
            },
            ChatMessage {
                role: Role::Assistant,
                parts: vec![
                    Part::Reasoning {
                        text: "thinking".into(),
                    },
                    Part::Text {
                        text: "checking".into(),
                    },
                    Part::ToolCall {
                        id: "c1".into(),
                        name: "t".into(),
                        arguments: "{}".into(),
                    },
                ],
            },
            ChatMessage {
                role: Role::Tool,
                parts: vec![Part::ToolResult {
                    call_id: "c1".into(),
                    name: "t".into(),
                    output: "ok".into(),
                    is_error: false,
                }],
            },
        ];
        let tools = [ToolSpec {
            name: "t".into(),
            description: "d".into(),
            parameters: json!({"type": "object"}),
        }];
        let req = CompletionRequest {
            model: "m",
            instructions: "sys",
            messages: &messages,
            tools: &tools,
        };
        let b = body(&req);
        assert_eq!(b["store"], false);
        assert_eq!(b["stream"], true);
        assert_eq!(b["instructions"], "sys");
        let input = b["input"].as_array().unwrap();
        assert_eq!(input.len(), 4);
        assert_eq!(
            input[1],
            json!({"role": "assistant", "content": "checking"})
        );
        assert_eq!(input[2]["type"], "function_call");
        assert_eq!(
            input[3],
            json!({"type": "function_call_output", "call_id": "c1", "output": "ok"})
        );
        assert_eq!(b["tools"][0]["type"], "function");
    }

    #[test]
    fn decoder_collects_text_and_calls() {
        let mut d = Decoder::default();
        let mut seen = String::new();
        let mut sink = |delta: Delta<'_>| {
            if let Delta::Text(t) = delta {
                seen.push_str(t);
            }
        };
        for ev in [
            json!({"type": "response.output_text.delta", "delta": "Hel"}),
            json!({"type": "response.output_text.delta", "delta": "lo"}),
            json!({"type": "response.output_item.done", "item": {"type": "function_call", "call_id": "c9", "name": "x", "arguments": "{\"a\":1}"}}),
        ] {
            assert!(!d.feed(&ev, &mut sink).unwrap());
        }
        assert!(d
            .feed(&json!({"type": "response.completed"}), &mut sink)
            .unwrap());
        let out = d.finish().unwrap();
        assert_eq!(seen, "Hello");
        assert_eq!(out.text, "Hello");
        assert_eq!(
            out.tool_calls,
            [("c9".into(), "x".into(), "{\"a\":1}".into())]
        );
    }

    #[test]
    fn decoder_surfaces_plan_limit_errors() {
        let mut d = Decoder::default();
        let ev = json!({"type": "response.failed", "response": {"error": {"code": "subscription_sharing_usage_limit_exceeded", "message": "limit hit"}}});
        let err = d.feed(&ev, &mut |_| {}).unwrap_err().to_string();
        assert!(err.contains("subscription_sharing_usage_limit_exceeded"));
        assert!(Decoder::default().finish().is_err());
    }
}
