//! Reduce oversized outline-shell offsets using spatial and bone-weight matching.
//! Usage: bunnydicta_outline_refine <input_dir.vpk> <output_dir.vpk>
use anyhow::{ensure, Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 3,
        "usage: bunnydicta_outline_refine <input_dir.vpk> <output_dir.vpk>"
    );
    let (input, output) = (&args[1], &args[2]);
    ensure!(
        !std::path::Path::new(output).exists(),
        "output already exists; choose a new path"
    );
    let entry = "models/heroes_staging/hornet_v3/hornet.vmdl_c";
    let original = vpkmerge_core::read_vpk_entry(input, entry)?;
    let model = morphic::model::decode(&original)?;
    let outline = model
        .meshes
        .iter()
        .find(|m| m.name == "outline")
        .context("outline")?;
    let mut vb = outline.vertex_buffers[0].clone();
    let mut donors = Vec::new();
    for mesh in model.meshes.iter().filter(|m| m.name == "body") {
        for prim in &mesh.primitives {
            if prim.material.contains("outline") {
                continue;
            }
            let src = &mesh.vertex_buffers[prim.vertex_buffer];
            let mut used = prim.indices.clone();
            used.sort_unstable();
            used.dedup();
            for i in used {
                let i = i as usize;
                donors.push((src.positions[i], src.joints[i], src.weights[i]));
            }
        }
    }
    let mut distances = Vec::new();
    let mut changed = 0;
    let mut skipped = 0;
    for i in 0..vb.positions.len() {
        let p = vb.positions[i];
        // Match both location and skinning, so the nearby braid cannot attract a torso vertex.
        let weight_diff = |j: [u16; 4], w: [f32; 4]| {
            let mut result = 0.0f32;
            for k in 0..4 {
                if vb.weights[i][k] <= 0.0 {
                    continue;
                }
                let other: f32 = (0..4)
                    .filter(|&l| j[l] == vb.joints[i][k])
                    .map(|l| w[l])
                    .sum();
                result += (vb.weights[i][k] - other).abs();
            }
            result
        };
        let dist = |q: [f32; 3]| (0..3).map(|k| (p[k] - q[k]).powi(2)).sum::<f32>();
        let nearest = donors
            .iter()
            .filter(|d| weight_diff(d.1, d.2) < 0.08)
            .min_by(|a, b| dist(a.0).total_cmp(&dist(b.0)));
        let Some(d) = nearest else {
            skipped += 1;
            continue;
        };
        let gap = dist(d.0).sqrt();
        if gap > 1.2 || gap < 0.15 {
            skipped += 1;
            continue;
        }
        distances.push(gap);
        vb.positions[i] = std::array::from_fn(|k| d.0[k] + (p[k] - d.0[k]) * (0.15 / gap));
        changed += 1;
    }
    ensure!(changed > 0, "no valid outline matches");
    distances.sort_by(f32::total_cmp);
    println!("Reduced {changed} outline vertices from median gap {:.3} inches to 0.15 inches; {skipped} unmatched/already close vertices preserved. Bone weights unchanged.",distances[distances.len()/2]);
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
