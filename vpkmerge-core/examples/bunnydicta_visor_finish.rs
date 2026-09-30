//! Transfer visor hair fitting to game meshes and validate the finished outfit.
//! Usage: bunnydicta_visor_finish <input_dir.vpk> <output_dir.vpk> <hair_tuck.json>
use anyhow::{ensure, Context, Result};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    ensure!(
        args.len() == 4,
        "usage: bunnydicta_visor_finish <input_dir.vpk> <output_dir.vpk> <hair_tuck.json>"
    );
    let (input, output) = (&args[1], &args[2]);
    ensure!(
        !std::path::Path::new(output).exists(),
        "output already exists; choose a new path"
    );
    let entry = "models/heroes_staging/hornet_v3/hornet.vmdl_c";
    let original = vpkmerge_core::read_vpk_entry(input, entry)?;
    let records: Vec<serde_json::Value> = serde_json::from_slice(&std::fs::read(&args[3])?)?;
    let points: Vec<([f32; 3], [f32; 3])> = records
        .iter()
        .map(|r| {
            let old = std::array::from_fn(|k| r["old"][k].as_f64().unwrap() as f32);
            let new = std::array::from_fn(|k| r["new"][k].as_f64().unwrap() as f32);
            (old, new)
        })
        .collect();
    let mut edited = original.clone();
    let mut hair_moved = 0;
    for (mesh_name, prim_index, is_outline) in
        [("body", 2, false), ("body", 1, true), ("outline", 0, true)]
    {
        let model = morphic::model::decode(&edited)?;
        let mesh = model
            .meshes
            .iter()
            .find(|m| m.name == mesh_name)
            .context("missing mesh")?;
        let prim = &mesh.primitives[prim_index];
        let mut vb = mesh.vertex_buffers[prim.vertex_buffer].clone();
        let mut used = prim.indices.clone();
        used.sort_unstable();
        used.dedup();
        let mut count = 0;
        for i in used {
            let p = vb.positions[i as usize];
            if p[2] < 2.24 / 0.0254 {
                continue;
            }
            let distance = |a: [f32; 3]| (0..3).map(|k| (a[k] - p[k]).powi(2)).sum::<f32>();
            let (old, new) = points
                .iter()
                .min_by(|a, b| distance(a.0).total_cmp(&distance(b.0)))
                .unwrap();
            let limit = if is_outline { 1.0 } else { 0.0001 };
            if distance(*old) > limit * limit {
                continue;
            }
            let delta: [f32; 3] = std::array::from_fn(|k| new[k] - old[k]);
            if delta.iter().map(|x| x * x).sum::<f32>() < 1e-10 {
                continue;
            }
            vb.positions[i as usize] = std::array::from_fn(|k| p[k] + delta[k]);
            count += 1;
        }
        if !is_outline {
            hair_moved = count;
        }
        if count > 0 {
            vb.tangents.clear();
            if mesh_name == "body" {
                anyhow::bail!("Refusing compressed body-buffer rewrite: this path produced exploded geometry in Source 2 despite passing self-roundtrip checks. Use the untouched-body outfit build until a validated uncompressed shared-buffer path exists.");
            } else {
                edited = morphic::model::replace_draw_call_uncompressed(
                    &edited,
                    mesh_name,
                    prim_index,
                    &vb,
                    &prim.indices,
                )?
                .0;
            }
        }
        println!("{mesh_name} primitive {prim_index}: transferred {count} hair-fit vertices");
    }
    ensure!(
        hair_moved >= 300,
        "hair correspondence failed: {hair_moved}"
    );
    let model = morphic::model::decode(&edited)?;
    let skel = morphic::model::decode_skeleton(&edited)?;
    let clothes = model.meshes.iter().find(|m| m.name == "clothes").unwrap();
    let prim = &clothes.primitives[0];
    let vb = &clothes.vertex_buffers[prim.vertex_buffer];
    for &i in &prim.indices {
        let i = i as usize;
        ensure!(
            (vb.weights[i].iter().sum::<f32>() - 1.0).abs() < 0.02,
            "invalid weights"
        );
        for k in 0..4 {
            if vb.weights[i][k] > 0.0001 {
                let name = skel.bones[vb.joints[i][k] as usize]
                    .name
                    .to_ascii_lowercase();
                ensure!(
                    !name.contains("hair") && !name.contains("bunnyear"),
                    "clothing contains hair weights"
                );
            }
        }
    }
    let before = morphic::model::draw_call_targets(&original)?;
    let after = morphic::model::draw_call_targets(&edited)?;
    ensure!(before.len() == after.len(), "draw call count changed");
    for (a, b) in before.iter().zip(&after) {
        ensure!(
            a.index_count == b.index_count && a.material == b.material,
            "draw call changed"
        );
    }
    println!("Validated clothing weights and preserved all draw calls");
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
