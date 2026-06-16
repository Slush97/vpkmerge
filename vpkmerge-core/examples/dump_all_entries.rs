//! Dump every entry of a VPK to a directory, preserving path structure.
//! Usage: cargo run -p vpkmerge-core --example dump_all_entries -- <vpk> <outdir>
use std::fs;
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let vpk_path = &args[1];
    let out = Path::new(&args[2]);
    let vpk = valve_pak::open(vpk_path)?;
    let paths: Vec<String> = vpk.file_paths().cloned().collect();
    for entry in &paths {
        let bytes = vpkmerge_core::read_vpk_entry(vpk_path, entry)?;
        let dest = out.join(entry);
        if let Some(p) = dest.parent() {
            fs::create_dir_all(p)?;
        }
        fs::write(&dest, &bytes)?;
    }
    println!("dumped {} entries to {}", paths.len(), out.display());
    Ok(())
}
