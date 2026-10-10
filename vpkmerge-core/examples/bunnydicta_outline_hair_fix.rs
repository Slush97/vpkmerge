//! Restore hair-bound outline vertices mistakenly reseated onto body skin.
//! Usage: bunnydicta_outline_hair_fix <input_dir.vpk> <output_dir.vpk> <original_mod.vpk>
use anyhow::{ensure, Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 4,
        "usage: bunnydicta_outline_hair_fix <input_dir.vpk> <output_dir.vpk> <original_mod.vpk>"
    );
    let (input, output) = (&args[1], &args[2]);
    ensure!(
        !std::path::Path::new(output).exists(),
        "output already exists; choose a new path"
    );
    let entry = "models/heroes_staging/hornet_v3/hornet.vmdl_c";
    let original = vpkmerge_core::read_vpk_entry(input, entry)?;
    let donor_bytes = vpkmerge_core::read_vpk_entry(&args[3], entry)?;
    let source = morphic::model::decode(&donor_bytes)?;
    let model = morphic::model::decode(&original)?;
    let skel = morphic::model::decode_skeleton(&original)?;
    let outline = model
        .meshes
        .iter()
        .find(|m| m.name == "outline")
        .context("outline")?;
    let donor = source
        .meshes
        .iter()
        .find(|m| m.name == "outline")
        .context("donor outline")?;
    let mut vb = outline.vertex_buffers[0].clone();
    let orig = &donor.vertex_buffers[0];
    ensure!(
        vb.positions.len() == orig.positions.len(),
        "outline topology changed"
    );
    ensure!(
        outline.primitives[0].indices == donor.primitives[0].indices,
        "outline indices changed"
    );
    let mut restored = 0;
    for i in 0..vb.positions.len() {
        let follows_hair = (0..4).any(|k| {
            orig.weights[i][k] > 0.001
                && skel.bones[orig.joints[i][k] as usize].name.contains("hair")
        });
        if follows_hair
            && (vb.positions[i] != orig.positions[i]
                || vb.joints[i] != orig.joints[i]
                || vb.weights[i] != orig.weights[i])
        {
            vb.positions[i] = orig.positions[i];
            vb.normals[i] = orig.normals[i];
            vb.joints[i] = orig.joints[i];
            vb.weights[i] = orig.weights[i];
            restored += 1;
        }
    }
    ensure!(restored > 0, "no damaged hair-outline vertices found");
    vb.tangents.clear();
    let edited = morphic::model::replace_draw_call_uncompressed(
        &original,
        "outline",
        0,
        &vb,
        &outline.primitives[0].indices,
    )?
    .0;
    let decoded = morphic::model::decode(&edited)?;
    let out = &decoded
        .meshes
        .iter()
        .find(|m| m.name == "outline")
        .unwrap()
        .vertex_buffers[0];
    for i in 0..vb.positions.len() {
        ensure!(out.positions[i] == vb.positions[i], "position mismatch");
        for k in 0..4 {
            ensure!(
                (out.weights[i][k] - vb.weights[i][k]).abs() < 0.01,
                "weight mismatch"
            );
            if vb.weights[i][k] > 0.001 {
                ensure!(out.joints[i][k] == vb.joints[i][k], "joint mismatch");
            }
        }
    }
    println!("Restored {restored} hair-outline vertices to their original positions and bone weights using uncompressed output");
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
