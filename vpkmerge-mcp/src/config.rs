//! Where the server finds the game and where it writes.
//!
//! Precedence for the game pak: `DEADLOCK_PAK` (the `pak01_dir.vpk` itself), then
//! `DEADLOCK_GAME_DIR` (the Deadlock install), then every Steam library listed in
//! `libraryfolders.vdf`. Output goes to `VPKMERGE_STAGING_DIR`, never the game.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

const PAK_NAME: &str = "pak01_dir.vpk";

pub struct Config {
    pak: std::result::Result<PathBuf, String>,
    /// Every tool that builds a mod writes here.
    pub staging: PathBuf,
    /// Extra markdown directories for the docs tools (`VPKMERGE_DOCS_DIRS`).
    pub docs_dirs: Vec<PathBuf>,
}

impl Config {
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            pak: discover_pak(),
            staging: env_path("VPKMERGE_STAGING_DIR").unwrap_or_else(default_staging),
            docs_dirs: std::env::var_os("VPKMERGE_DOCS_DIRS")
                .map(|v| std::env::split_paths(&v).collect())
                .unwrap_or_default(),
        }
    }

    /// A config with a known pak (or none) and staging dir, skipping discovery.
    #[must_use]
    pub fn new(pak: Option<PathBuf>, staging: PathBuf) -> Self {
        Self {
            pak: pak.ok_or_else(|| NOT_FOUND.to_owned()),
            staging,
            docs_dirs: Vec::new(),
        }
    }

    /// The base `citadel/pak01_dir.vpk`, or an error that says how to configure it.
    pub fn pak(&self) -> Result<&Path> {
        self.pak.as_deref().map_err(|e| anyhow!("{e}"))
    }

    #[must_use]
    pub fn cache_dir(&self) -> PathBuf {
        self.staging.join(".catalog-cache")
    }

    /// `citadel/addons`, where installed mods live as `pakNN_dir.vpk`.
    #[must_use]
    pub fn addons_dir(&self) -> Option<PathBuf> {
        Some(self.pak.as_ref().ok()?.parent()?.join("addons"))
    }
}

const NOT_FOUND: &str = "Deadlock install not found. Set DEADLOCK_GAME_DIR to the Deadlock \
     install directory (the one containing game/citadel/pak01_dir.vpk) in this MCP server's \
     env, then restart it.";

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn discover_pak() -> std::result::Result<PathBuf, String> {
    if let Some(pak) = env_path("DEADLOCK_PAK") {
        return if pak.is_file() {
            Ok(pak)
        } else {
            Err(format!(
                "DEADLOCK_PAK is set to {} but no file is there.",
                pak.display()
            ))
        };
    }
    if let Some(dir) = env_path("DEADLOCK_GAME_DIR") {
        return pak_under(&dir).ok_or_else(|| {
            format!(
                "DEADLOCK_GAME_DIR is set to {} but game/citadel/{PAK_NAME} is not under it.",
                dir.display()
            )
        });
    }
    steam_roots()
        .iter()
        .flat_map(|root| library_dirs(root))
        .find_map(|lib| pak_under(&lib.join("steamapps/common/Deadlock")))
        .ok_or_else(|| NOT_FOUND.to_owned())
}

/// Accepts the install root, its `game` dir, or `game/citadel`.
fn pak_under(dir: &Path) -> Option<PathBuf> {
    ["game/citadel", "citadel", ""]
        .iter()
        .map(|rel| dir.join(rel).join(PAK_NAME))
        .find(|p| p.is_file())
}

fn steam_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(windows) {
        for var in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(dir) = env_path(var) {
                roots.push(dir.join("Steam"));
            }
        }
    } else if let Some(home) = env_path("HOME") {
        roots.push(home.join(".steam/steam"));
        roots.push(home.join(".local/share/Steam"));
        roots.push(home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"));
    }
    roots
}

/// A Steam root plus every extra library its `libraryfolders.vdf` lists.
fn library_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.to_path_buf()];
    if let Ok(text) = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
        dirs.extend(parse_library_paths(&text));
    }
    dirs
}

fn parse_library_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|line| {
            let mut quoted = line.trim().split('"');
            (quoted.nth(1)? == "path").then_some(())?;
            Some(PathBuf::from(quoted.nth(1)?.replace("\\\\", "\\")))
        })
        .collect()
}

fn default_staging() -> PathBuf {
    let data = if cfg!(windows) {
        env_path("LOCALAPPDATA")
    } else {
        env_path("XDG_DATA_HOME").or_else(|| env_path("HOME").map(|h| h.join(".local/share")))
    };
    data.unwrap_or_else(std::env::temp_dir)
        .join("vpkmerge")
        .join("staging")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_library_path() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/me/.local/share/Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(
            parse_library_paths(vdf),
            [
                PathBuf::from("/home/me/.local/share/Steam"),
                PathBuf::from("D:\\SteamLibrary")
            ]
        );
    }

    #[test]
    fn finds_the_pak_from_any_install_level() {
        let tmp = tempfile::tempdir().unwrap();
        let citadel = tmp.path().join("game/citadel");
        std::fs::create_dir_all(&citadel).unwrap();
        std::fs::write(citadel.join(PAK_NAME), b"").unwrap();
        for dir in [
            tmp.path().to_path_buf(),
            tmp.path().join("game"),
            citadel.clone(),
        ] {
            assert_eq!(pak_under(&dir), Some(citadel.join(PAK_NAME)));
        }
        assert_eq!(pak_under(&tmp.path().join("nope")), None);
    }

    #[test]
    fn missing_pak_error_says_how_to_fix_it() {
        let config = Config::new(None, PathBuf::from("/tmp/staging"));
        assert!(config
            .pak()
            .unwrap_err()
            .to_string()
            .contains("DEADLOCK_GAME_DIR"));
    }
}
