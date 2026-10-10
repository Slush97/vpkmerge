//! Which bones drive the duster-material draw calls, and where do they sit?
//! usage: probe_duster_mats <vpk>
use std::collections::BTreeMap;

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";

fn main() -> anyhow::Result<()> {
    let pak = std::env::args().nth(1).expect("vpk");
    let bytes = vpkmerge_core::read_vpk_entry(&pak, ENTRY)?;
    let m = morphic::model::decode(&bytes).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let sk = morphic::model::decode_skeleton(&bytes).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let mesh = &m.meshes[0];
    let vb = &mesh.vertex_buffers[0];
    for p in &mesh.primitives {
        if !(p.material.contains("feather") || p.material.contains("duster")) {
            continue;
        }
        let mut verts: Vec<usize> = p.indices.iter().map(|&i| i as usize).collect();
        verts.sort_unstable();
        verts.dedup();
        let mut by_bone: BTreeMap<u16, usize> = BTreeMap::new();
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for &v in &verts {
            for k in 0..4 {
                if vb.weights[v][k] > 0.0 {
                    *by_bone.entry(vb.joints[v][k]).or_default() += 1;
                }
            }
            for a in 0..3 {
                lo[a] = lo[a].min(vb.positions[v][a]);
                hi[a] = hi[a].max(vb.positions[v][a]);
            }
        }
        println!(
            "prim mat={} verts={} bbox z {:.1}..{:.1} x {:.1}..{:.1} y {:.1}..{:.1}",
            p.material,
            verts.len(),
            lo[2],
            hi[2],
            lo[0],
            hi[0],
            lo[1],
            hi[1]
        );
        for (b, c) in &by_bone {
            println!("   bone #{b} {} -> {c} verts", sk.bones[*b as usize].name);
        }
    }
    Ok(())
}
