//! Pack a loose directory tree into an addon VPK, preserving relative paths.
//!
//! Usage:
//!   cargo run --release -p vpkmerge-core --example pack_tree -- <root_dir> <out_dir.vpk>

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            collect_files(root, &path, out)?;
        } else if ty.is_file() {
            let rel = path
                .strip_prefix(root)
                .with_context(|| format!("stripping root from {}", path.display()))?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            out.push((rel, bytes));
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let mut a = std::env::args_os().skip(1);
    let root = PathBuf::from(
        a.next()
            .context("usage: pack_tree <root_dir> <out_dir.vpk>")?,
    );
    let out = PathBuf::from(a.next().context("out_dir.vpk")?);

    let mut files = Vec::new();
    collect_files(&root, &root, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    anyhow::ensure!(!files.is_empty(), "no files under {}", root.display());

    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &out)?;
    println!(
        "packed {} files from {} -> {}",
        refs.len(),
        root.display(),
        out.display()
    );
    Ok(())
}
