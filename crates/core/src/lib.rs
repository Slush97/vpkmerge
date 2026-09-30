//! Workbench core: sessions, model providers, sign-in, MCP, skills and the
//! agent loop. Independent of Tauri so a CLI or MCP server can drive it too.

pub mod acp;
mod acp_turn;
mod agent;
mod app;
pub mod auth;
pub mod config;
pub mod mcp;
pub mod providers;
pub mod secrets;
pub mod skills;
pub mod store;

pub use agent::AgentEvent;
pub use app::{AgentStatus, App, AppInfo, ProviderStatus};

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

pub(crate) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Writes through a sibling temp file so a crash never leaves a torn file.
pub(crate) fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
