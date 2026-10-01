use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::acp::AgentId;
use crate::permissions::PermissionLevel;
use crate::providers::ProviderId;

/// What answers the next message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Choice {
    /// A provider model driven by our own harness loop.
    Model { provider: ProviderId, model: String },
    /// An external coding agent spoken to over ACP.
    Agent { agent: AgentId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub model: Option<Choice>,
    pub local_base_url: String,
    pub skill_dirs: Vec<PathBuf>,
    pub disabled_skills: BTreeSet<String>,
    pub max_tool_rounds: u32,
    /// `ext_agent_host_id` for Sign in with ChatGPT. Chosen once per install.
    pub openai_host_id: Option<String>,
    /// Folder ACP agents work in.
    pub agent_cwd: PathBuf,
    /// What tools may do without asking, for every model and agent.
    pub permission_level: PermissionLevel,
    pub theme: Theme,
    pub accent: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: None,
            local_base_url: "http://127.0.0.1:11434/v1".into(),
            skill_dirs: Vec::new(),
            disabled_skills: BTreeSet::new(),
            max_tool_rounds: 24,
            openai_host_id: None,
            agent_cwd: home_dir(),
            permission_level: PermissionLevel::default(),
            theme: Theme::System,
            accent: "#c9a24d".into(),
        }
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

impl Settings {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("{} is not valid settings JSON", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        crate::write_atomic(path, &serde_json::to_vec_pretty(self)?)
    }
}

/// Partial update from the UI. Absent fields stay as they are.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub model: Option<Choice>,
    pub local_base_url: Option<String>,
    pub max_tool_rounds: Option<u32>,
    pub agent_cwd: Option<PathBuf>,
    pub permission_level: Option<PermissionLevel>,
    pub theme: Option<Theme>,
    pub accent: Option<String>,
}
