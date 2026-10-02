//! Handing a built mod to the Grimoire mod manager. Grimoire takes mod files as
//! command-line arguments and opens its import dialog with them, where the user
//! confirms. A running Grimoire receives them through its single-instance lock.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

const NOT_FOUND: &str = "Grimoire was not found. Set GRIMOIRE_BIN to the Grimoire executable \
     in this MCP server's env, or install the mod by copying it into the addons folder \
     (see game_status).";

/// `GRIMOIRE_BIN`, then `grimoire` on PATH, then each platform's install location.
pub fn find() -> Result<PathBuf> {
    if let Some(bin) = std::env::var_os("GRIMOIRE_BIN").filter(|v| !v.is_empty()) {
        let bin = PathBuf::from(bin);
        if bin.is_file() {
            return Ok(bin);
        }
        bail!(
            "GRIMOIRE_BIN is set to {} but no file is there.",
            bin.display()
        );
    }
    on_path()
        .into_iter()
        .chain(install_locations())
        .find(|p| p.is_file())
        .context(NOT_FOUND)
}

fn on_path() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "Grimoire.exe"
    } else {
        "grimoire"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

fn install_locations() -> Vec<PathBuf> {
    if cfg!(windows) {
        // The NSIS installer's per-user default.
        std::env::var_os("LOCALAPPDATA")
            .map(|d| PathBuf::from(d).join("Programs/grimoire/Grimoire.exe"))
            .into_iter()
            .collect()
    } else if cfg!(target_os = "macos") {
        vec![PathBuf::from(
            "/Applications/Grimoire.app/Contents/MacOS/Grimoire",
        )]
    } else {
        vec![PathBuf::from("/opt/Grimoire/grimoire")]
    }
}

/// Starts `bin` with `vpk` and returns without waiting for it. The child is reaped
/// on a thread: a second instance exits at once, a first one runs until closed.
pub fn open(bin: &Path, vpk: &Path) -> Result<()> {
    if !vpk.is_file() {
        bail!("{} does not exist.", vpk.display());
    }
    let mut child = Command::new(bin)
        .arg(vpk)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {}", bin.display()))?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grimoire_bin_wins_and_must_exist() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("grimoire");
        std::fs::write(&bin, "").unwrap();

        std::env::set_var("GRIMOIRE_BIN", &bin);
        assert_eq!(find().unwrap(), bin);

        std::env::set_var("GRIMOIRE_BIN", dir.path().join("missing"));
        assert!(find().unwrap_err().to_string().contains("GRIMOIRE_BIN"));
        std::env::remove_var("GRIMOIRE_BIN");
    }

    #[test]
    fn a_missing_mod_is_refused_before_launching() {
        let err = open(Path::new("/bin/true"), Path::new("/no/such_dir.vpk")).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }
}
