//! Sign in with ChatGPT, open-source plan-usage path:
//! <https://developers.openai.com/siwc/token-sharing-open-source/sign-in>.
//! The first sign-in registers a per-user client (`dynamic_agent_client`);
//! the issued `client_id` is kept with the credential and reused after that.

use anyhow::{bail, Context};
use tokio_util::sync::CancellationToken;

use super::{jwt_claims, post_token_form, Loopback, OpenUrl, Pkce, TokenResponse};
use crate::secrets::Credential;

const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
const ISSUER: &str = "https://auth.openai.com";
const RESOURCE: &str = "https://api.openai.com/v1";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const REGISTRATION_CLIENT: &str = "dynamic_agent_client";
const PORT: u16 = 1455;
const CALLBACK_PATH: &str = "/auth/callback";
const APP_NAME: &str = "Workbench";

pub fn new_host_id() -> String {
    format!("urn:uuid:{}", uuid::Uuid::new_v4())
}

pub async fn sign_in(
    http: &reqwest::Client,
    host_id: &str,
    known_client_id: Option<&str>,
    open: OpenUrl<'_>,
    cancel: &CancellationToken,
) -> anyhow::Result<Credential> {
    let loopback = Loopback::bind(PORT).await?;
    let redirect_uri = format!("http://127.0.0.1:{PORT}{CALLBACK_PATH}");
    let pkce = Pkce::new();
    let state = super::random_token();
    let nonce = super::random_token();

    let mut url = url::Url::parse(AUTHORIZE_URL)?;
    url.query_pairs_mut()
        .append_pair("client_id", known_client_id.unwrap_or(REGISTRATION_CLIENT))
        .append_pair("agent_name_hint", APP_NAME)
        .append_pair("ext_agent_host_id", host_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", SCOPE)
        .append_pair("resource", RESOURCE)
        .append_pair("state", &state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", &pkce.challenge);
    open(url.as_str())?;

    let params = loopback.wait(CALLBACK_PATH, Some(&state), cancel).await?;
    let code = params.get("code").context("the callback had no authorization code")?;
    let client_id = params
        .get("client_id")
        .map(String::as_str)
        .or(known_client_id)
        .context("OpenAI did not return a client ID for this install")?
        .to_owned();

    let token = post_token_form(
        http,
        TOKEN_URL,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &client_id),
            ("code", code),
            ("code_verifier", &pkce.verifier),
            ("redirect_uri", &redirect_uri),
            ("resource", RESOURCE),
        ],
    )
    .await?;
    if token
        .scope
        .as_deref()
        .is_some_and(|s| !s.split_whitespace().any(|x| x == PLAN_SCOPE))
    {
        bail!("this account did not grant ChatGPT plan usage; check the plan and the consent screen");
    }
    let account = match &token.id_token {
        Some(id_token) => check_id_token(id_token, &client_id, &nonce)?,
        None => None,
    };
    Ok(credential(token, client_id, account, None))
}

pub async fn refresh(http: &reqwest::Client, current: &Credential) -> anyhow::Result<Credential> {
    let Credential::OAuth {
        refresh_token: Some(refresh_token),
        client_id: Some(client_id),
        account,
        ..
    } = current
    else {
        bail!("ChatGPT session cannot be refreshed; sign in again");
    };
    let token = post_token_form(
        http,
        TOKEN_URL,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("resource", RESOURCE),
        ],
    )
    .await
    .context("ChatGPT session expired; sign in again")?;
    Ok(credential(
        token,
        client_id.clone(),
        account.clone(),
        Some(refresh_token.clone()),
    ))
}

fn credential(
    token: TokenResponse,
    client_id: String,
    account: Option<String>,
    previous_refresh: Option<String>,
) -> Credential {
    Credential::OAuth {
        access_token: token.access_token,
        refresh_token: token.refresh_token.or(previous_refresh),
        expires_at: token.expires_in.map(|s| crate::now_secs() + s),
        client_id: Some(client_id),
        account,
    }
}

fn check_id_token(id_token: &str, client_id: &str, nonce: &str) -> anyhow::Result<Option<String>> {
    let claims = jwt_claims(id_token)?;
    if claims["iss"] != ISSUER {
        bail!("ID token has the wrong issuer");
    }
    let aud_ok = match &claims["aud"] {
        serde_json::Value::String(a) => a == client_id,
        serde_json::Value::Array(list) => list.iter().any(|a| a == client_id),
        _ => false,
    };
    if !aud_ok {
        bail!("ID token was issued for a different client");
    }
    if claims["nonce"] != nonce {
        bail!("ID token nonce mismatch");
    }
    if claims["exp"].as_i64().is_some_and(|exp| exp < crate::now_secs()) {
        bail!("ID token already expired");
    }
    Ok(claims["email"].as_str().map(str::to_owned))
}
