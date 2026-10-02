//! OpenRouter PKCE: the browser flow ends by minting a user-owned API key.
//! OpenRouter echoes no `state`, so a random callback path stands in for it.

use anyhow::{bail, Context};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::{Loopback, OpenUrl, Pkce};
use crate::secrets::Credential;

pub async fn sign_in(
    http: &reqwest::Client,
    open: OpenUrl<'_>,
    cancel: &CancellationToken,
) -> anyhow::Result<Credential> {
    let loopback = Loopback::bind(0).await?;
    let path = format!("/callback/{}", super::random_token());
    let callback = format!("http://localhost:{}{path}", loopback.port());
    let pkce = Pkce::new();

    let mut url = url::Url::parse("https://openrouter.ai/auth")?;
    url.query_pairs_mut()
        .append_pair("callback_url", &callback)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("key_label", "Workbench");
    open(url.as_str())?;

    let params = loopback.wait(&path, None, cancel).await?;
    let code = params
        .get("code")
        .context("the callback had no authorization code")?;
    let resp = http
        .post("https://openrouter.ai/api/v1/auth/keys")
        .json(&json!({
            "code": code,
            "code_verifier": pkce.verifier,
            "code_challenge_method": "S256",
        }))
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!(
            "OpenRouter key exchange failed ({status}): {}",
            crate::providers::error_message(&text)
        );
    }
    let body: serde_json::Value = resp.json().await?;
    let key = body["key"].as_str().context("OpenRouter returned no key")?;
    Ok(Credential::ApiKey {
        key: key.to_owned(),
    })
}
