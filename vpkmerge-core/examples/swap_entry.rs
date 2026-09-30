//! Repack a VPK with one entry's bytes replaced by a loose file, preserving every
//! other entry. Used to drop a post-compile-reordered wraith.vmdl_c back into the
//! working gun build's VPK without losing its custom gun materials/textures.
//! Usage: swap_entry <in.vpk> <entry/path> <replacement_file> <out_dir.vpk>
use anyhow::Result;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (in_vpk, entry, repl, out) = (&a[1], &a[2], &a[3], &a[4]);

    let vpk = valve_pak::open(in_vpk)?;
    let paths: Vec<String> = vpk.file_paths().cloned().collect();
    let repl_bytes = std::fs::read(repl)?;

    let mut owned: Vec<(String, Vec<u8>)> = Vec::with_capacity(paths.len());
    let mut swapped = false;
    for p in &paths {
        if p == entry {
            owned.push((p.clone(), repl_bytes.clone()));
            swapped = true;
        } else {
            owned.push((p.clone(), vpkmerge_core::read_vpk_entry(in_vpk, p)?));
        }
    }
    anyhow::ensure!(swapped, "entry {entry} not found in {in_vpk}");

    let refs: Vec<(&str, &[u8])> = owned
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, out)?;
    eprintln!(
        "packed {} entries -> {out} ({} replaced)",
        refs.len(),
        entry
    );
    Ok(())
}
