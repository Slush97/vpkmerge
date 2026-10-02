use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{bail, Context};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::acp::{AgentId, AgentProcess};
use crate::auth::{self, OpenUrl};
use crate::config::{Settings, SettingsPatch};
use crate::mcp::{self, McpManager};
use crate::permissions::PendingPermission;
use crate::providers::{self, Endpoint, ModelInfo, ProviderId};
use crate::secrets::{Credential, Vault};
use crate::skills::{self, Skill};
use crate::store::{Session, StoredMessage, Store};

pub struct App {
    data_dir: PathBuf,
    pub(crate) http: reqwest::Client,
    pub(crate) store: Store,
    vault: Arc<Vault>,
    settings: RwLock<Settings>,
    pub(crate) mcp: McpManager,
    /// Servers that ship with the app. `mcp.json` is layered on top.
    builtin_mcp: mcp::ConfigFile,
    pub(crate) turns: Mutex<HashMap<String, (uuid::Uuid, CancellationToken)>>,
    sign_in: Mutex<Option<CancellationToken>>,
    refresh_lock: tokio::sync::Mutex<()>,
    agents: tokio::sync::Mutex<HashMap<AgentId, Arc<AgentProcess>>>,
    agent_errors: Mutex<HashMap<AgentId, String>>,
    /// (our session, agent) to (ACP session id, agent process instance).
    pub(crate) acp_sessions: Mutex<HashMap<(String, AgentId), (String, u64)>>,
    /// Permission prompts waiting on the user, by request id.
    pub(crate) permissions: Mutex<HashMap<String, PendingPermission>>,
    /// (session, tool) pairs the user allowed for the rest of that session.
    pub(crate) tool_grants: Mutex<HashSet<(String, String)>>,
}

/// What vpkmerge's `list_hero_animations` returns, for the preview viewer.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnimations {
    /// Prefix of the hero's own animation names.
    pub codename: String,
    pub animations: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub id: AgentId,
    pub name: &'static str,
    pub description: &'static str,
    pub installed: bool,
    pub command: &'static str,
    pub auth_methods: Vec<crate::acp::AuthMethod>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: &'static str,
    pub data_dir: PathBuf,
    pub secret_storage: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub id: ProviderId,
    pub name: &'static str,
    pub sign_in_label: Option<&'static str>,
    pub accepts_api_key: bool,
    pub connected: bool,
    /// "oauth" or "apiKey" when connected.
    pub method: Option<&'static str>,
    pub account: Option<String>,
}

impl App {
    pub fn open(data_dir: &Path, builtin_mcp: mcp::ConfigFile) -> anyhow::Result<Arc<Self>> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("creating {}", data_dir.display()))?;
        std::fs::create_dir_all(data_dir.join("skills"))?;
        let settings = Settings::load(&data_dir.join("settings.json"))?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .user_agent(concat!("Workbench/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Arc::new(Self {
            store: Store::open(&data_dir.join("workbench.db"))?,
            vault: Arc::new(Vault::open(data_dir.join("credentials.json"))),
            data_dir: data_dir.to_owned(),
            http,
            settings: RwLock::new(settings),
            mcp: McpManager::default(),
            builtin_mcp,
            turns: Mutex::new(HashMap::new()),
            sign_in: Mutex::new(None),
            refresh_lock: tokio::sync::Mutex::new(()),
            agents: tokio::sync::Mutex::new(HashMap::new()),
            agent_errors: Mutex::new(HashMap::new()),
            acp_sessions: Mutex::new(HashMap::new()),
            permissions: Mutex::new(HashMap::new()),
            tool_grants: Mutex::new(HashSet::new()),
        }))
    }

    pub fn info(&self) -> AppInfo {
        AppInfo {
            version: env!("CARGO_PKG_VERSION"),
            data_dir: self.data_dir.clone(),
            secret_storage: self.vault.backend_name(),
        }
    }

    // ---- settings ------------------------------------------------------

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn update_settings(&self, patch: SettingsPatch) -> anyhow::Result<Settings> {
        if let Some(dir) = &patch.agent_cwd {
            if !dir.is_dir() {
                bail!("{} is not a folder", dir.display());
            }
        }
        if let Some(accent) = &patch.accent {
            let hex = accent.strip_prefix('#').unwrap_or_default();
            if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                bail!("accent must look like #0a84ff");
            }
        }
        self.edit_settings(|s| {
            if let Some(dir) = patch.agent_cwd {
                s.agent_cwd = dir;
            }
            if let Some(level) = patch.permission_level {
                s.permission_level = level;
            }
            if let Some(theme) = patch.theme {
                s.theme = theme;
            }
            if let Some(accent) = patch.accent {
                s.accent = accent.to_ascii_lowercase();
            }
            if let Some(model) = patch.model {
                s.model = Some(model);
            }
            if let Some(url) = patch.local_base_url {
                url.trim().clone_into(&mut s.local_base_url);
            }
            if let Some(n) = patch.max_tool_rounds {
                s.max_tool_rounds = n.clamp(1, 200);
            }
            if let Some(prompt) = patch.system_prompt {
                prompt.trim().clone_into(&mut s.system_prompt);
            }
        })
    }

    fn edit_settings(&self, f: impl FnOnce(&mut Settings)) -> anyhow::Result<Settings> {
        let mut guard = self.settings.write().unwrap();
        let mut next = guard.clone();
        f(&mut next);
        next.save(&self.data_dir.join("settings.json"))?;
        *guard = next.clone();
        Ok(next)
    }

    // ---- sessions ------------------------------------------------------

    pub fn sessions(&self) -> anyhow::Result<Vec<Session>> {
        self.store.sessions()
    }

    pub fn create_session(&self) -> anyhow::Result<Session> {
        self.store.create_session()
    }

    pub fn rename_session(&self, id: &str, title: &str) -> anyhow::Result<()> {
        let title = title.trim();
        if title.is_empty() {
            bail!("title cannot be empty");
        }
        self.store.rename_session(id, title)
    }

    pub fn delete_session(&self, id: &str) -> anyhow::Result<()> {
        self.cancel_turn(id);
        self.store.delete_session(id)
    }

    pub fn messages(&self, session_id: &str) -> anyhow::Result<Vec<StoredMessage>> {
        self.store.messages(session_id)
    }

    // ---- accounts ------------------------------------------------------

    async fn credential(&self, provider: ProviderId) -> anyhow::Result<Option<Credential>> {
        let vault = Arc::clone(&self.vault);
        tokio::task::spawn_blocking(move || vault.get(provider)).await?
    }

    async fn save_credential(&self, provider: ProviderId, credential: Credential) -> anyhow::Result<()> {
        let vault = Arc::clone(&self.vault);
        tokio::task::spawn_blocking(move || vault.set(provider, &credential)).await?
    }

    pub async fn providers(&self) -> anyhow::Result<Vec<ProviderStatus>> {
        let mut out = Vec::new();
        for id in ProviderId::ALL {
            let cred = self.credential(id).await?;
            let (method, account) = match &cred {
                Some(Credential::ApiKey { .. }) => (Some("apiKey"), None),
                Some(Credential::OAuth { account, .. }) => (Some("oauth"), account.clone()),
                None => (None, None),
            };
            out.push(ProviderStatus {
                id,
                name: id.name(),
                sign_in_label: id.sign_in_label(),
                accepts_api_key: true,
                connected: cred.is_some() || id == ProviderId::Local,
                method,
                account,
            });
        }
        Ok(out)
    }

    pub async fn sign_in(&self, provider: ProviderId, open: OpenUrl<'_>) -> anyhow::Result<()> {
        let cancel = CancellationToken::new();
        if let Some(previous) = self.sign_in.lock().unwrap().replace(cancel.clone()) {
            previous.cancel();
        }
        let credential = match provider {
            ProviderId::Openai => {
                let host_id = if let Some(id) = self.settings().openai_host_id {
                    id
                } else {
                    let id = auth::openai::new_host_id();
                    self.edit_settings(|s| s.openai_host_id = Some(id.clone()))?;
                    id
                };
                let known_client = match self.credential(provider).await? {
                    Some(Credential::OAuth { client_id, .. }) => client_id,
                    _ => None,
                };
                auth::openai::sign_in(&self.http, &host_id, known_client.as_deref(), open, &cancel).await?
            }
            ProviderId::Openrouter => auth::openrouter::sign_in(&self.http, open, &cancel).await?,
            _ => bail!("{} connects with an API key", provider.name()),
        };
        self.save_credential(provider, credential).await
    }

    pub fn cancel_sign_in(&self) {
        if let Some(token) = self.sign_in.lock().unwrap().take() {
            token.cancel();
        }
    }

    pub async fn set_api_key(&self, provider: ProviderId, key: &str) -> anyhow::Result<()> {
        let key = key.trim();
        if key.is_empty() {
            bail!("the key is empty");
        }
        self.save_credential(provider, Credential::ApiKey { key: key.to_owned() })
            .await
    }

    pub async fn sign_out(&self, provider: ProviderId) -> anyhow::Result<()> {
        let vault = Arc::clone(&self.vault);
        tokio::task::spawn_blocking(move || vault.remove(provider)).await?
    }

    pub(crate) async fn endpoint(&self, provider: ProviderId) -> anyhow::Result<Endpoint> {
        let local_url = self.settings().local_base_url;
        let mut cred = self.credential(provider).await?;
        if cred.as_ref().is_some_and(Credential::needs_refresh) {
            let _guard = self.refresh_lock.lock().await;
            cred = self.credential(provider).await?;
            if let Some(current) = cred.as_ref().filter(|c| c.needs_refresh()) {
                let fresh = match provider {
                    ProviderId::Openai => auth::openai::refresh(&self.http, current).await?,
                    _ => bail!("{} session expired; sign in again", provider.name()),
                };
                self.save_credential(provider, fresh.clone()).await?;
                cred = Some(fresh);
            }
        }
        if cred.is_none() && provider != ProviderId::Local {
            bail!("{} is not connected. Open Settings, then Accounts.", provider.name());
        }
        Ok(Endpoint::new(provider, cred.as_ref(), &local_url))
    }

    pub async fn models(&self, provider: ProviderId) -> anyhow::Result<Vec<ModelInfo>> {
        let ep = self.endpoint(provider).await?;
        providers::list_models(&self.http, &ep).await
    }

    // ---- skills --------------------------------------------------------

    pub fn skill_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = vec![self.data_dir.join("skills")];
        dirs.extend(self.settings().skill_dirs);
        dirs
    }

    pub fn skills(&self) -> Vec<Skill> {
        skills::scan(&self.skill_dirs(), &self.settings().disabled_skills)
    }

    pub fn set_skill_enabled(&self, name: &str, enabled: bool) -> anyhow::Result<()> {
        self.edit_settings(|s| {
            if enabled {
                s.disabled_skills.remove(name);
            } else {
                s.disabled_skills.insert(name.to_owned());
            }
        })?;
        Ok(())
    }

    pub fn add_skill_dir(&self, dir: PathBuf) -> anyhow::Result<()> {
        if !dir.is_dir() {
            bail!("{} is not a folder", dir.display());
        }
        self.edit_settings(|s| {
            if !s.skill_dirs.contains(&dir) {
                s.skill_dirs.push(dir);
            }
        })?;
        Ok(())
    }

    pub fn remove_skill_dir(&self, dir: &Path) -> anyhow::Result<()> {
        self.edit_settings(|s| s.skill_dirs.retain(|d| d != dir))?;
        Ok(())
    }

    // ---- MCP -----------------------------------------------------------

    fn mcp_path(&self) -> PathBuf {
        self.data_dir.join("mcp.json")
    }

    pub fn mcp_config_text(&self) -> anyhow::Result<String> {
        match std::fs::read_to_string(self.mcp_path()) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(serde_json::to_string_pretty(&mcp::ConfigFile::default())?)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The built-in servers with the user's `mcp.json` layered on top.
    fn mcp_config(&self) -> anyhow::Result<mcp::ConfigFile> {
        let user = serde_json::from_str(&self.mcp_config_text()?).context("mcp.json is not valid")?;
        Ok(self.builtin_mcp.clone().with_user(user))
    }

    /// Connects the configured servers. Call once at startup.
    pub async fn start_mcp(&self) -> anyhow::Result<()> {
        // A broken mcp.json must not take the built-in servers down with it.
        let config = self.mcp_config();
        self.mcp
            .apply(config.as_ref().unwrap_or(&self.builtin_mcp))
            .await;
        config.map(drop)
    }

    pub async fn save_mcp_config(&self, text: &str) -> anyhow::Result<()> {
        let user: mcp::ConfigFile = serde_json::from_str(text).context("not valid MCP config JSON")?;
        crate::write_atomic(&self.mcp_path(), serde_json::to_string_pretty(&user)?.as_bytes())?;
        self.mcp.apply(&self.builtin_mcp.clone().with_user(user)).await;
        Ok(())
    }

    pub async fn mcp_status(&self) -> Vec<mcp::ServerStatus> {
        self.mcp.status().await
    }

    pub async fn reconnect_mcp(&self, name: &str) -> anyhow::Result<()> {
        self.mcp.reconnect(name).await
    }

    pub(crate) fn mcp_config_for_agents(&self) -> mcp::ConfigFile {
        self.mcp_config()
            .unwrap_or_else(|_| self.builtin_mcp.clone())
    }

    // ---- model previews ------------------------------------------------
    // The preview viewer asks the vpkmerge server for animations itself, so
    // playing one never goes through a model.

    pub async fn preview_animations(&self, hero: &str, vpk: Option<&str>) -> anyhow::Result<PreviewAnimations> {
        let args = serde_json::json!({ "hero": hero, "vpk": vpk });
        let out = self.call_tool("vpkmerge", "list_hero_animations", &args).await?;
        serde_json::from_value(out).context("list_hero_animations returned an unexpected shape")
    }

    /// Path of a skeleton-only GLB carrying one animation.
    pub async fn preview_animation(&self, hero: &str, vpk: Option<&str>, animation: &str) -> anyhow::Result<PathBuf> {
        let args = serde_json::json!({ "hero": hero, "vpk": vpk, "animation": animation });
        let out = self.call_tool("vpkmerge", "preview_hero_animation", &args).await?;
        out["animationGlb"]
            .as_str()
            .map(PathBuf::from)
            .context("preview_hero_animation returned no file")
    }

    async fn call_tool(&self, server: &str, tool: &str, args: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let (output, is_error) = self.mcp.call(&mcp::qualified(server, tool), &args.to_string()).await?;
        if is_error {
            bail!("{output}");
        }
        serde_json::from_str(&output).with_context(|| format!("{tool} returned {output}"))
    }

    // ---- ACP agents ----------------------------------------------------

    /// Running process for `agent`, started on first use and after a crash.
    pub(crate) async fn agent(&self, agent: AgentId) -> anyhow::Result<Arc<AgentProcess>> {
        let mut agents = self.agents.lock().await;
        if let Some(p) = agents.get(&agent).filter(|p| p.is_alive()) {
            return Ok(Arc::clone(p));
        }
        match AgentProcess::spawn(agent).await {
            Ok(p) => {
                let p = Arc::new(p);
                agents.insert(agent, Arc::clone(&p));
                self.agent_errors.lock().unwrap().remove(&agent);
                Ok(p)
            }
            Err(e) => {
                self.agent_errors.lock().unwrap().insert(agent, format!("{e:#}"));
                Err(e)
            }
        }
    }

    pub async fn agents(&self) -> Vec<AgentStatus> {
        let running = self.agents.lock().await;
        let errors = self.agent_errors.lock().unwrap();
        AgentId::ALL
            .iter()
            .map(|&id| AgentStatus {
                id,
                name: id.name(),
                description: id.description(),
                installed: id.command().is_some(),
                command: id.command_label(),
                auth_methods: running
                    .get(&id)
                    .map(|p| p.auth_methods().to_vec())
                    .unwrap_or_default(),
                error: errors.get(&id).cloned(),
            })
            .collect()
    }

    pub async fn agent_sign_in(&self, agent: AgentId, method_id: &str) -> anyhow::Result<()> {
        self.agent(agent).await?.authenticate(method_id).await
    }

    pub async fn respond_permission(&self, request_id: &str, option_id: Option<&str>) -> anyhow::Result<()> {
        let pending = self
            .permissions
            .lock()
            .unwrap()
            .remove(request_id)
            .context("that request was already answered or has expired")?;
        match pending {
            PendingPermission::Agent { process, rpc_id } => process.answer_permission(rpc_id, option_id).await,
            PendingPermission::Tool(answer) => {
                let _ = answer.send(option_id.map(str::to_owned));
            }
        }
        Ok(())
    }

    pub async fn shutdown(&self) {
        for (_, token) in self.turns.lock().unwrap().values() {
            token.cancel();
        }
        self.agents.lock().await.clear();
        self.mcp.shutdown_all().await;
    }
}
