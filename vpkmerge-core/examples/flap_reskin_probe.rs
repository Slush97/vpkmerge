//! Probe everything the fringe re-skin needs: is `flaps_0_*` in the body mesh's
//! bone palette (remap table)? what BLENDINDICES format? where (model space) do
//! the leg_lower-weighted verts that we'd reskin live, relative to the flap bone?
//!
//! Usage: cargo run -p vpkmerge-core --example flap_reskin_probe -- <pak.vpk>

use morphic::kv3::Value;
use morphic::model::{decode_skeleton, invert_remap};

const ENTRY: &str = "models/heroes_staging/astro/astro.vmdl_c";

fn blocks(b: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let bo = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    let c = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
    let base = 8 + bo;
    (0..c)
        .map(|i| {
            let e = base + i * 12;
            let mut k = [0u8; 4];
            k.copy_from_slice(&b[e..e + 4]);
            let r = u32::from_le_bytes(b[e + 4..e + 8].try_into().unwrap()) as usize;
            let l = u32::from_le_bytes(b[e + 8..e + 12].try_into().unwrap()) as usize;
            (k, (e + 4) + r, l)
        })
        .collect()
}

fn main() -> anyhow::Result<()> {
    let pak = std::env::args().nth(1).expect("pak.vpk");
    let model = vpkmerge_core::read_vpk_entry(&pak, ENTRY)?;
    let sk = decode_skeleton(&model).map_err(|e| anyhow::anyhow!("{e:?}"))?;

    let names: Vec<&str> = sk.bones.iter().map(|b| b.name.as_str()).collect();
    let bidx = |n: &str| names.iter().position(|x| *x == n).map(|i| i as usize);
    let bpos = |n: &str| -> [f32; 3] {
        let m = &sk.bones[bidx(n).unwrap()].global_bind.m;
        [m[12], m[13], m[14]]
    };
    for n in [
        "flaps_0_L",
        "flaps_0_R",
        "leg_lower_L",
        "leg_lower_R",
        "ankle_L",
    ] {
        println!("bone {n}: model#{:?} pos(model)={:?}", bidx(n), bpos(n));
    }

    // remap table for each mesh + palette membership of our bones
    let (_, off, len) = blocks(&model)
        .into_iter()
        .find(|(k, _, _)| k == b"DATA")
        .unwrap();
    let data =
        morphic::kv3::decode(&model[off..off + len]).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let starts = data
        .get("m_remappingTableStarts")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    println!(
        "\nremap tables: {} mesh entries",
        starts.saturating_sub(1).max(0)
    );
    for mi in 0..starts.saturating_sub(1) {
        if let Some(rt) = morphic::model::remap_table(&data, mi) {
            let inv = invert_remap(&rt);
            let has = |n: &str| bidx(n).and_then(|m| inv.get(&m).copied());
            println!(
                "  mesh#{mi}: palette={} bones; flaps_0_L local={:?} flaps_0_R local={:?} leg_lower_L local={:?} leg_lower_R local={:?}",
                rt.len(),
                has("flaps_0_L"),
                has("flaps_0_R"),
                has("leg_lower_L"),
                has("leg_lower_R"),
            );
        }
    }

    // model-space spread of leg_lower_L-weighted verts vs the flap bone
    let m = morphic::model::decode(&model).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let ll = bidx("leg_lower_L").unwrap() as u16;
    let flap = bpos("flaps_0_L");
    let mut near = vec![];
    for mesh in &m.meshes {
        for vb in &mesh.vertex_buffers {
            for (vi, (j4, w4)) in vb.joints.iter().zip(&vb.weights).enumerate() {
                for k in 0..4 {
                    if j4[k] == ll && w4[k] > 0.3 {
                        let p = vb.positions[vi];
                        let d = ((p[0] - flap[0]).powi(2)
                            + (p[1] - flap[1]).powi(2)
                            + (p[2] - flap[2]).powi(2))
                        .sqrt();
                        near.push((d, p, w4[k]));
                    }
                }
            }
        }
    }
    near.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    println!(
        "\nleg_lower_L-dominant verts: {} total; flap bone(model) = {:?}",
        near.len(),
        flap
    );
    for r in [0.5f32, 1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0] {
        let c = near.iter().filter(|(d, _, _)| *d <= r).count();
        println!("  within {r:>5} model-units of flap: {c} verts");
    }
    // show the bbox of the closest 300 (likely the fringe attachment cluster)
    let take = near.iter().take(300);
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (_, p, _) in take {
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    println!("  bbox of closest 300 verts: lo={lo:?} hi={hi:?}");
    Ok(())
}
