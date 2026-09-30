//! UI-independent contracts and read-only tools for local modding workflows.

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub mod blender;

pub const SCHEMA_VERSION: u32 = 1;

/// Paths are absolute so a saved project behaves identically from any caller.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub schema_version: u32,
    pub name: String,
    pub workspace: PathBuf,
    pub game_pak: Option<PathBuf>,
    pub blender: Option<BlenderConfig>,
    pub csdk: Option<CsdkConfig>,
}

/// An existing host-managed MCP server, not the Blender addon's socket endpoint.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlenderConfig {
    pub mcp_server_name: String,
    pub blend_file: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CsdkConfig {
    pub root: PathBuf,
    pub proton: PathBuf,
}

impl Project {
    pub fn load(path: &Path) -> Result<Self> {
        let project: Self = serde_json::from_slice(
            &std::fs::read(path).with_context(|| format!("reading {}", path.display()))?,
        )?;
        project.validate()?;
        Ok(project)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == SCHEMA_VERSION,
            "unsupported project schema version"
        );
        ensure!(!self.name.trim().is_empty(), "project name cannot be empty");
        ensure!(
            self.workspace.is_absolute(),
            "workspace must be an absolute path"
        );
        if let Some(path) = &self.game_pak {
            ensure!(path.is_absolute(), "game_pak must be an absolute path");
        }
        if let Some(config) = &self.blender {
            ensure!(
                !config.mcp_server_name.trim().is_empty(),
                "MCP server name cannot be empty"
            );
            if let Some(path) = &config.blend_file {
                ensure!(path.is_absolute(), "blend_file must be an absolute path");
            }
        }
        if let Some(config) = &self.csdk {
            ensure!(
                config.root.is_absolute() && config.proton.is_absolute(),
                "CSDK and Proton paths must be absolute"
            );
        }
        Ok(())
    }
}

/// Create a project without replacing an existing manifest.
pub fn init_project(directory: &Path, name: String) -> Result<PathBuf> {
    ensure!(!name.trim().is_empty(), "project name cannot be empty");
    std::fs::create_dir_all(directory)?;
    let workspace = directory.canonicalize()?;
    let path = workspace.join("mod-project.json");
    let project = Project {
        schema_version: SCHEMA_VERSION,
        name,
        workspace,
        game_pak: None,
        blender: None,
        csdk: None,
    };
    let bytes = serde_json::to_vec_pretty(&project)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| {
            format!(
                "creating {} (existing projects are never overwritten)",
                path.display()
            )
        })?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    Ok(path)
}

/// Serializable calls shared by the CLI and future Tauri/MCP transports.
/// These tools accept local paths. A future remote transport must add path authorization.
#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "tool",
    content = "arguments",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ToolCall {
    GetCapabilities {},
    InspectMod {
        path: PathBuf,
        offset: usize,
        limit: usize,
    },
    DetectConflicts {
        inputs: Vec<PathBuf>,
        offset: usize,
        limit: usize,
    },
}

fn page_end(offset: usize, limit: usize, total: usize) -> Result<(usize, usize)> {
    ensure!(
        (1..=500).contains(&limit),
        "limit must be between 1 and 500"
    );
    let start = offset.min(total);
    Ok((start, start.saturating_add(limit).min(total)))
}

pub fn execute(project: &Project, call: ToolCall) -> Result<Value> {
    project.validate()?;
    match call {
        ToolCall::GetCapabilities {} => Ok(json!({
            "schema_version": SCHEMA_VERSION,
            "tools": ["get_capabilities", "inspect_mod", "detect_conflicts"],
            "workspace_exists": project.workspace.is_dir(),
            "game_pak_exists": project.game_pak.as_ref().is_some_and(|p| p.is_file()),
            "blender": {
                "configured": project.blender.is_some(),
                "adapter_implemented": false,
                "addon_snapshot_adapter_implemented": true,
                "live_view_implemented": false,
                "connection_tested": false
            },
            "csdk": {
                "configured": project.csdk.is_some(),
                "compiler_exists": project.csdk.as_ref().is_some_and(|c| c.root.join("game/bin_tools/win64/resourcecompiler.exe").is_file()),
                "proton_exists": project.csdk.as_ref().is_some_and(|c| c.proton.is_file()),
                "adapter_implemented": false,
                "execution_tested": false
            },
            "model_provider_implemented": false,
            "mcp_transport_implemented": false
        })),
        ToolCall::InspectMod {
            path,
            offset,
            limit,
        } => {
            page_end(0, limit, 0)?;
            let info = vpkmerge_core::inspect(&path)?;
            let mut paths = info.file_paths;
            paths.sort();
            let (start, end) = page_end(offset, limit, paths.len())?;
            Ok(json!({
                "path": path, "size_bytes": info.size_bytes,
                "entry_count": paths.len(), "entries": &paths[start..end],
                "next_offset": (end < paths.len()).then_some(end),
                "verification": "inventory_only"
            }))
        }
        ToolCall::DetectConflicts {
            inputs,
            offset,
            limit,
        } => {
            ensure!(inputs.len() >= 2, "at least two input VPKs are required");
            page_end(0, limit, 0)?;
            let mut conflicts = vpkmerge_core::detect_conflicts(&inputs)?;
            conflicts.sort_by(|a, b| a.path.cmp(&b.path));
            let (start, end) = page_end(offset, limit, conflicts.len())?;
            let entries: Vec<_> = conflicts[start..end]
                .iter()
                .map(|c| {
                    json!({
                        "path": c.path, "owner_indices": c.owner_indices
                    })
                })
                .collect();
            Ok(json!({"inputs": inputs, "conflict_count": conflicts.len(),
                "conflicts": entries, "next_offset": (end < conflicts.len()).then_some(end)
            }))
        }
    }
}
