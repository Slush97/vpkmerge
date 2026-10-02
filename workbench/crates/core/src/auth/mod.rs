//! Browser sign-in flows. Each one binds a loopback listener before opening
//! the browser, so the redirect can never race the listener.

pub mod openai;
pub mod openrouter;

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{bail, Context};
use base64::Engine;
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::providers::Cancelled;

pub type OpenUrl<'a> = &'a (dyn Fn(&str) -> anyhow::Result<()> + Send + Sync);

const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

pub(crate) fn random_token() -> String {
    let bytes: [u8; 32] = rand::random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> Self {
        let verifier = random_token();
        let digest = sha2::Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        Self { verifier, challenge }
    }
}

pub(crate) struct Loopback {
    listener: TcpListener,
}

impl Loopback {
    pub async fn bind(port: u16) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port)).await.with_context(|| {
            format!("port {port} is busy; close whatever is using it (another sign-in window?) and retry")
        })?;
        Ok(Self { listener })
    }

    pub fn port(&self) -> u16 {
        self.listener.local_addr().map_or(0, |a| a.port())
    }

    /// Waits for the redirect on `path`. Requests to other paths, or with the
    /// wrong `state`, are answered and ignored rather than trusted.
    pub async fn wait(
        &self,
        path: &str,
        state: Option<&str>,
        cancel: &CancellationToken,
    ) -> anyhow::Result<HashMap<String, String>> {
        let deadline = tokio::time::sleep(SIGN_IN_TIMEOUT);
        tokio::pin!(deadline);
        loop {
            let (mut stream, _) = tokio::select! {
                () = cancel.cancelled() => return Err(Cancelled.into()),
                () = &mut deadline => bail!("sign-in timed out after 5 minutes"),
                accepted = self.listener.accept() => accepted?,
            };
            let Some(target) = read_request_target(&mut stream).await else {
                continue;
            };
            let (req_path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
            if req_path != path {
                respond(&mut stream, 404, "Not found").await;
                continue;
            }
            let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect();
            if let Some(error) = params.get("error") {
                let detail = params
                    .get("error_description")
                    .map_or(error.as_str(), String::as_str);
                respond(&mut stream, 400, &format!("Sign-in failed: {detail}")).await;
                bail!("sign-in failed: {detail}");
            }
            if state.is_some_and(|s| params.get("state").map(String::as_str) != Some(s)) {
                respond(&mut stream, 400, "This sign-in link is stale. Start again from Workbench.").await;
                continue;
            }
            respond(&mut stream, 200, "Signed in. You can close this tab and go back to Workbench.").await;
            return Ok(params);
        }
    }
}

async fn read_request_target(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let mut first = head.lines().next()?.split_whitespace();
    (first.next()? == "GET").then(|| first.next().map(str::to_owned))?
}

async fn respond(stream: &mut tokio::net::TcpStream, status: u16, message: &str) {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Bad Request",
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Workbench</title></head>\
         <body style=\"margin:0;height:100vh;display:flex;align-items:center;justify-content:center;\
         background:#0f0f0f;color:#fafafa;font:15px system-ui,sans-serif\"><p>{message}</p></body></html>"
    );
    let reply = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[derive(serde::Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub scope: Option<String>,
}

pub(crate) async fn post_token_form(
    http: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> anyhow::Result<TokenResponse> {
    let resp = http
        .post(url)
        .header("Accept", "application/json")
        .form(form)
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("token request failed ({status}): {}", crate::providers::error_message(&text));
    }
    Ok(resp.json().await?)
}

/// Decodes a JWT payload without checking the signature. Only for tokens
/// received directly from a token endpoint over TLS (OIDC Core 3.1.3.7).
pub(crate) fn jwt_claims(token: &str) -> anyhow::Result<serde_json::Value> {
    let payload = token.split('.').nth(1).context("malformed ID token")?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .context("malformed ID token payload")?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    async fn get(port: u16, target: &str) -> String {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn loopback_ignores_wrong_path_and_state() {
        let lb = Loopback::bind(0).await.unwrap();
        let port = lb.port();
        let cancel = CancellationToken::new();
        let client = tokio::spawn(async move {
            assert!(get(port, "/favicon.ico").await.starts_with("HTTP/1.1 404"));
            assert!(get(port, "/cb?code=evil&state=nope").await.starts_with("HTTP/1.1 400"));
            assert!(get(port, "/cb?code=abc&state=s1").await.starts_with("HTTP/1.1 200"));
        });
        let params = lb.wait("/cb", Some("s1"), &cancel).await.unwrap();
        client.await.unwrap();
        assert_eq!(params["code"], "abc");
    }

    #[tokio::test]
    async fn loopback_reports_provider_errors_and_cancel() {
        let lb = Loopback::bind(0).await.unwrap();
        let port = lb.port();
        let cancel = CancellationToken::new();
        tokio::spawn(async move { get(port, "/cb?error=access_denied&error_description=User%20said%20no").await });
        let err = lb.wait("/cb", Some("s"), &cancel).await.unwrap_err();
        assert!(err.to_string().contains("User said no"));

        let cancel = CancellationToken::new();
        cancel.cancel();
        let err = lb.wait("/cb", None, &cancel).await.unwrap_err();
        assert!(err.is::<Cancelled>());
    }

    #[test]
    fn pkce_challenge_is_s256_of_verifier() {
        let p = Pkce::new();
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(p.verifier.as_bytes()));
        assert_eq!(p.challenge, expected);
        assert!(p.verifier.len() >= 43);
    }
}
