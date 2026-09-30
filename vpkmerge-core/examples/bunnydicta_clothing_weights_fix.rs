//! Repair clothing accidentally weighted to Bunnydicta's hair bones.
//! Usage: bunnydicta_clothing_weights_fix <input_dir.vpk> <output_dir.vpk>
use anyhow::{ensure, Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 3,
        "usage: bunnydicta_clothing_weights_fix <input_dir.vpk> <output_dir.vpk>"
    );
    let (input, output) = (&args[1], &args[2]);
    ensure!(
        !std::path::Path::new(output).exists(),
        "output already exists; choose a new path"
    );
    let entry = "models/heroes_staging/hornet_v3/hornet.vmdl_c";
    let original = vpkmerge_core::read_vpk_entry(input, entry)?;
    let skel = morphic::model::decode_skeleton(&original)?;
    let is_hair = |j: u16| {
        let name = skel.bones[usize::from(j)].name.to_ascii_lowercase();
        name.contains("hair") || name.contains("bunnyear")
    };
    let model = morphic::model::decode(&original)?;
    let body = model
        .meshes
        .iter()
        .find(|m| m.name == "body")
        .context("body mesh")?;
    let mut donors = Vec::new();
    for prim in &body.primitives {
        if !prim.material.contains("skinmaterial") {
            continue;
        }
        let vb = &body.vertex_buffers[prim.vertex_buffer];
        let mut used = prim.indices.clone();
        used.sort_unstable();
        used.dedup();
        for i in used {
            let i = i as usize;
            if (0..4).any(|k| vb.weights[i][k] > 0.0001 && is_hair(vb.joints[i][k])) {
                continue;
            }
            donors.push((vb.positions[i], vb.joints[i], vb.weights[i]));
        }
    }
    ensure!(!donors.is_empty(), "no skin donors");
    let clothes = model
        .meshes
        .iter()
        .find(|m| m.name == "clothes")
        .context("clothes mesh")?;
    let prim = &clothes.primitives[0];
    let mut vb = clothes.vertex_buffers[prim.vertex_buffer].clone();
    let mut fixed = 0;
    for i in 0..vb.positions.len() {
        if !(0..4).any(|k| vb.weights[i][k] > 0.0001 && is_hair(vb.joints[i][k])) {
            continue;
        }
        let p = vb.positions[i];
        let distance = |d: &([f32; 3], [u16; 4], [f32; 4])| {
            (0..3).map(|k| (p[k] - d.0[k]).powi(2)).sum::<f32>()
        };
        let donor = donors
            .iter()
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))
            .unwrap();
        vb.joints[i] = donor.1;
        let sum: f32 = donor.2.iter().sum();
        ensure!(sum > 0.0, "unweighted donor");
        vb.weights[i] = donor.2.map(|w| w / sum);
        fixed += 1;
    }
    ensure!(fixed > 0, "no clothing hair weights found");
    vb.tangents.clear();
    let (edited, _) = morphic::model::replace_draw_call_uncompressed(
        &original,
        "clothes",
        0,
        &vb,
        &prim.indices,
    )?;
    let decoded = morphic::model::decode(&edited)?;
    let mesh = decoded.meshes.iter().find(|m| m.name == "clothes").unwrap();
    let result = &mesh.vertex_buffers[mesh.primitives[0].vertex_buffer];
    for &i in &mesh.primitives[0].indices {
        let i = i as usize;
        ensure!(
            !(0..4).any(|k| result.weights[i][k] > 0.0001 && is_hair(result.joints[i][k])),
            "hair weights survived roundtrip"
        );
        ensure!(
            (result.weights[i].iter().sum::<f32>() - 1.0).abs() < 0.02,
            "weights not normalized"
        );
    }
    let before = morphic::model::draw_call_targets(&original)?;
    let after = morphic::model::draw_call_targets(&edited)?;
    ensure!(before.len() == after.len(), "draw call count changed");
    for (a, b) in before.iter().zip(&after) {
        ensure!(
            a.mesh_name == b.mesh_name
                && a.material == b.material
                && a.index_count == b.index_count,
            "draw call changed"
        );
    }
    println!("Rebound {fixed} clothing vertices from braid bones to nearest skin weights; zero remaining hair influences.");
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
    println!(
        "Verified {} entries; all draw calls preserved.\nTest VPK: {output}",
        entries.len()
    );
    Ok(())
}
