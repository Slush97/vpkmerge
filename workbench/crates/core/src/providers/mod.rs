//! Model providers. Two wire formats cover all of them: the OpenAI Responses
//! API (required for Sign in with ChatGPT) and Chat Completions (xAI,
//! OpenRouter, DeepSeek and local OpenAI-compatible servers).

mod chat;
mod responses;

use anyhow::bail;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::secrets::Credential;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Openai,
    Xai,
    Openrouter,
    Deepseek,
    Local,
}

impl ProviderId {
    pub const ALL: [Self; 5] = [
        Self::Openai,
        Self::Xai,
        Self::Openrouter,
        Self::Deepseek,
        Self::Local,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Xai => "xai",
            Self::Openrouter => "openrouter",
            Self::Deepseek => "deepseek",
            Self::Local => "local",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Openai => "OpenAI",
            Self::Xai => "xAI Grok",
            Self::Openrouter => "OpenRouter",
            Self::Deepseek => "DeepSeek",
            Self::Local => "Local endpoint",
        }
    }

    /// Label for the browser sign-in button, when the provider has one.
    pub fn sign_in_label(self) -> Option<&'static str> {
        match self {
            Self::Openai => Some("Sign in with ChatGPT"),
            Self::Openrouter => Some("Sign in with OpenRouter"),
            Self::Xai | Self::Deepseek | Self::Local => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Part {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    #[serde(rename_all = "camelCase")]
    ToolResult {
        call_id: String,
        name: String,
        output: String,
        is_error: bool,
    },
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub struct CompletionRequest<'a> {
    pub model: &'a str,
    pub instructions: &'a str,
    pub messages: &'a [ChatMessage],
    pub tools: &'a [ToolSpec],
}

pub enum Delta<'a> {
    Text(&'a str),
    Reasoning(&'a str),
}

#[derive(Debug, Default)]
pub struct Completion {
    pub text: String,
    pub reasoning: String,
    pub tool_calls: Vec<(String, String, String)>,
    /// Set when the provider stopped early (length limit, content filter).
    pub stop_note: Option<String>,
}

impl Completion {
    pub fn into_parts(self) -> Vec<Part> {
        let mut parts = Vec::new();
        if !self.reasoning.trim().is_empty() {
            parts.push(Part::Reasoning {
                text: self.reasoning,
            });
        }
        if !self.text.is_empty() {
            parts.push(Part::Text { text: self.text });
        }
        parts.extend(
            self.tool_calls
                .into_iter()
                .map(|(id, name, arguments)| Part::ToolCall {
                    id,
                    name,
                    arguments,
                }),
        );
        parts
    }
}

/// Marker error so callers can tell a user cancel from a real failure.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wire {
    Responses,
    ChatCompletions,
}

pub struct Endpoint {
    provider: ProviderId,
    base_url: String,
    bearer: Option<String>,
    wire: Wire,
}

impl Endpoint {
    pub fn new(
        provider: ProviderId,
        credential: Option<&Credential>,
        local_base_url: &str,
    ) -> Self {
        let base_url = match provider {
            ProviderId::Openai => "https://api.openai.com/v1",
            ProviderId::Xai => "https://api.x.ai/v1",
            ProviderId::Openrouter => "https://openrouter.ai/api/v1",
            ProviderId::Deepseek => "https://api.deepseek.com",
            ProviderId::Local => local_base_url.trim_end_matches('/'),
        };
        Self {
            provider,
            base_url: base_url.to_owned(),
            bearer: credential.map(|c| c.bearer().to_owned()),
            wire: if provider == ProviderId::Openai {
                Wire::Responses
            } else {
                Wire::ChatCompletions
            },
        }
    }

    fn request(
        &self,
        http: &reqwest::Client,
        method: reqwest::Method,
        path: &str,
    ) -> reqwest::RequestBuilder {
        let mut req = http.request(method, format!("{}/{path}", self.base_url));
        if let Some(token) = &self.bearer {
            req = req.bearer_auth(token);
        }
        if self.provider == ProviderId::Openrouter {
            req = req
                .header("HTTP-Referer", "https://github.com/Slush97")
                .header("X-Title", "Workbench");
        }
        req
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
}

pub async fn list_models(http: &reqwest::Client, ep: &Endpoint) -> anyhow::Result<Vec<ModelInfo>> {
    let resp = checked(
        ep.request(http, reqwest::Method::GET, "models")
            .send()
            .await?,
    )
    .await?;
    let body: Value = resp.json().await?;
    let items = body["data"]
        .as_array()
        .or_else(|| body["models"].as_array())
        .cloned()
        .unwrap_or_default();
    let mut models: Vec<ModelInfo> = items
        .iter()
        .filter(|m| m["visibility"].as_str().is_none_or(|v| v == "list"))
        .filter_map(|m| {
            let id = m["slug"].as_str().or_else(|| m["id"].as_str())?.to_owned();
            let name = m["display_name"]
                .as_str()
                .or_else(|| m["name"].as_str())
                .unwrap_or(&id)
                .to_owned();
            Some(ModelInfo { id, name })
        })
        .filter(|m| ep.provider != ProviderId::Openai || looks_like_chat_model(&m.id))
        .collect();
    models.sort_by_key(|m| m.name.to_lowercase());
    Ok(models)
}

/// OpenAI's key-based model list includes embeddings, audio and image models.
fn looks_like_chat_model(id: &str) -> bool {
    const SKIP: [&str; 9] = [
        "embedding",
        "tts",
        "whisper",
        "dall-e",
        "moderation",
        "transcribe",
        "realtime",
        "audio",
        "image",
    ];
    !SKIP.iter().any(|s| id.contains(s))
}

pub async fn complete(
    http: &reqwest::Client,
    ep: &Endpoint,
    req: &CompletionRequest<'_>,
    cancel: &CancellationToken,
    on_delta: &mut (dyn FnMut(Delta<'_>) + Send),
) -> anyhow::Result<Completion> {
    let (path, body) = match ep.wire {
        Wire::Responses => ("responses", responses::body(req)),
        Wire::ChatCompletions => ("chat/completions", chat::body(req)),
    };
    let send = ep
        .request(http, reqwest::Method::POST, path)
        .json(&body)
        .send();
    let resp = tokio::select! {
        () = cancel.cancelled() => return Err(Cancelled.into()),
        resp = send => checked(resp?).await?,
    };
    let mut events = eventsource_stream::Eventsource::eventsource(resp.bytes_stream());
    let mut state = match ep.wire {
        Wire::Responses => Decoder::Responses(responses::Decoder::default()),
        Wire::ChatCompletions => Decoder::Chat(chat::Decoder::default()),
    };
    loop {
        let next = tokio::select! {
            () = cancel.cancelled() => return Err(Cancelled.into()),
            next = events.next() => next,
        };
        let Some(event) = next else { break };
        let event = event.map_err(|e| anyhow::anyhow!("stream error: {e}"))?;
        let data = event.data.trim();
        if data.is_empty() || data == "[DONE]" {
            if data == "[DONE]" {
                break;
            }
            continue;
        }
        let value: Value = serde_json::from_str(data)
            .map_err(|e| anyhow::anyhow!("bad stream chunk ({e}): {data}"))?;
        let finished = match &mut state {
            Decoder::Responses(d) => d.feed(&value, on_delta)?,
            Decoder::Chat(d) => d.feed(&value, on_delta)?,
        };
        if finished {
            break;
        }
    }
    match state {
        Decoder::Responses(d) => d.finish(),
        Decoder::Chat(d) => Ok(d.finish()),
    }
}

enum Decoder {
    Responses(responses::Decoder),
    Chat(chat::Decoder),
}

async fn checked(resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let text = resp.text().await.unwrap_or_default();
    bail!("{status}: {}", error_message(&text))
}

/// Pulls the human part out of the several error shapes providers use.
pub(crate) fn error_message(text: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return text.chars().take(500).collect();
    };
    v["error"]["message"]
        .as_str()
        .or_else(|| v["error"].as_str())
        .or_else(|| v["message"].as_str())
        .or_else(|| v["error_description"].as_str())
        .or_else(|| v["detail"].as_str())
        .map_or_else(|| text.chars().take(500).collect(), str::to_owned)
}
