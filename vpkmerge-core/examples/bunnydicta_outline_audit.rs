use anyhow::{Context, Result};
fn main() -> Result<()> {
    let path = std::env::args().nth(1).context("VPK argument")?;
    let bytes =
        vpkmerge_core::read_vpk_entry(&path, "models/heroes_staging/hornet_v3/hornet.vmdl_c")?;
    let model = morphic::model::decode(&bytes)?;
    let skel = morphic::model::decode_skeleton(&bytes)?;
    let hairbone = |j: u16| skel.bones[j as usize].name.contains("hair");
    let body = model.meshes.iter().find(|m| m.name == "body").unwrap();
    let mut hair = Vec::new();
    let mut skin = Vec::new();
    for p in &body.primitives {
        if p.material.contains("outline") {
            continue;
        }
        let vb = &body.vertex_buffers[p.vertex_buffer];
        let dst = if p.material.contains("hair") {
            &mut hair
        } else {
            &mut skin
        };
        let mut ids = p.indices.clone();
        ids.sort_unstable();
        ids.dedup();
        for i in ids {
            let i = i as usize;
            dst.push((vb.positions[i], vb.joints[i], vb.weights[i]));
        }
    }
    for mesh in &model.meshes {
        for p in &mesh.primitives {
            if !p.material.contains("outline") {
                continue;
            }
            let vb = &mesh.vertex_buffers[p.vertex_buffer];
            let mut ids = p.indices.clone();
            ids.sort_unstable();
            ids.dedup();
            let mut weighted = 0;
            let mut near_skin = 0;
            let mut mismatch = 0;
            let mut samples = Vec::new();
            for i in &ids {
                let i = *i as usize;
                let pos = vb.positions[i];
                let d = |q: [f32; 3]| (0..3).map(|k| (q[k] - pos[k]).powi(2)).sum::<f32>();
                let h = hair
                    .iter()
                    .min_by(|a, b| d(a.0).total_cmp(&d(b.0)))
                    .unwrap();
                let s = skin
                    .iter()
                    .min_by(|a, b| d(a.0).total_cmp(&d(b.0)))
                    .unwrap();
                let hw: f32 = (0..4)
                    .filter(|&k| hairbone(vb.joints[i][k]))
                    .map(|k| vb.weights[i][k])
                    .sum();
                if hw > 0.001 {
                    weighted += 1;
                    if d(s.0) < d(h.0) {
                        near_skin += 1;
                        if samples.len() < 8 {
                            samples.push((i, pos, hw, d(s.0).sqrt(), d(h.0).sqrt()));
                        }
                    }
                }
                let donor = if d(s.0) < d(h.0) { s } else { h };
                let dhw: f32 = (0..4)
                    .filter(|&k| hairbone(donor.1[k]))
                    .map(|k| donor.2[k])
                    .sum();
                if (hw - dhw).abs() > 0.3 {
                    mismatch += 1;
                }
            }
            let mixed = p
                .indices
                .chunks_exact(3)
                .filter(|t| {
                    let flags: Vec<bool> = t
                        .iter()
                        .map(|&i| {
                            (0..4).any(|k| {
                                vb.weights[i as usize][k] > 0.05
                                    && hairbone(vb.joints[i as usize][k])
                            })
                        })
                        .collect();
                    flags.iter().any(|x| *x) && flags.iter().any(|x| !*x)
                })
                .count();
            println!("{} {}: {} used verts, {} hair-weighted, {} closer to skin, {} nearest-surface weight mismatches, {} mixed triangles; samples {:?}",mesh.name,p.material,ids.len(),weighted,near_skin,mismatch,mixed,samples);
        }
    }
    Ok(())
}
