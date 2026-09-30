//! Build an A/B test VPK disabling only Bunnydicta's body outline draw call.
//! Usage: bunnydicta_outline_test <input_dir.vpk> <output_dir.vpk>
use anyhow::{ensure, Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 3,
        "usage: bunnydicta_outline_test <input_dir.vpk> <output_dir.vpk>"
    );
    let (input, output) = (&args[1], &args[2]);
    ensure!(
        !std::path::Path::new(output).exists(),
        "output already exists; choose a new path"
    );
    let entry = "models/heroes_staging/hornet_v3/hornet.vmdl_c";
    let original = vpkmerge_core::read_vpk_entry(input, entry)?;
    let before = morphic::model::draw_call_targets(&original)?;
    let targets: Vec<usize> = before
        .iter()
        .filter(|dc| dc.mesh_name == "body" && dc.material.contains("vindicta_outline"))
        .map(|dc| dc.primitive_index)
        .collect();
    ensure!(
        targets.len() == 1,
        "expected exactly one body outline draw call"
    );
    let edited = morphic::model::remove_part_draw_calls(&original, "body", &targets)?;
    let after = morphic::model::draw_call_targets(&edited)?;
    ensure!(before.len() == after.len(), "draw call count changed");
    for (old, new) in before.iter().zip(&after) {
        ensure!(
            old.mesh_name == new.mesh_name
                && old.primitive_index == new.primitive_index
                && old.material == new.material,
            "draw call identity changed"
        );
        if old.mesh_name == "body" && targets.contains(&old.primitive_index) {
            ensure!(new.index_count == 0, "body outline is still enabled");
            println!(
                "Disabled body primitive {}: {} -> 0 indices",
                old.primitive_index, old.index_count
            );
        } else {
            ensure!(
                old.index_count == new.index_count,
                "unrelated draw call changed"
            );
        }
    }
    let vpk = valve_pak::open(input).context("open source VPK")?;
    let mut paths: Vec<String> = vpk.file_paths().cloned().collect();
    paths.sort();
    let mut entries = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes = if path == entry {
            edited.clone()
        } else {
            vpkmerge_core::read_vpk_entry(input, &path)?
        };
        entries.push((path, bytes));
    }
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, output)?;
    // Read back every entry to verify the packed test is complete and unchanged.
    for (path, expected) in &entries {
        ensure!(
            vpkmerge_core::read_vpk_entry(output, path)? == *expected,
            "readback mismatch: {path}"
        );
    }
    println!("Verified {} entries; separate outline and all other draw calls preserved.\nTest VPK: {output}", entries.len());
    Ok(())
}
