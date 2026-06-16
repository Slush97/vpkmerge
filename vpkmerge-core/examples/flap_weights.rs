//! Count how many model vertices are actually skinned to Holliday's tassel
//! bones (`flaps_0_L/R`) vs the shin (`leg_lower_L/R`), straight from the raw
//! VBIB (post bone-remap). Confirms whether driving the flap bones can move any
//! geometry at all.
//!
//! Usage: cargo run -p vpkmerge-core --example flap_weights -- <pak.vpk>

use morphic::model::decode;

const ENTRY: &str = "models/heroes_staging/astro/astro.vmdl_c";

fn main() -> anyhow::Result<()> {
    let pak = std::env::args().nth(1).expect("pak.vpk");
    let model = vpkmerge_core::read_vpk_entry(&pak, ENTRY)?;
    let m = decode(&model).map_err(|e| anyhow::anyhow!("{e:?}"))?;

    let names: Vec<&str> = m.skeleton.bones.iter().map(|b| b.name.as_str()).collect();
    let idx = |n: &str| names.iter().position(|x| *x == n);
    let targets = ["flaps_0_L", "flaps_0_R", "leg_lower_L", "leg_lower_R"];
    for t in targets {
        let bi = match idx(t) {
            Some(i) => i as u16,
            None => {
                println!("{t}: NOT IN SKELETON");
                continue;
            }
        };
        let mut verts = 0usize;
        let mut wsum = 0.0f32;
        for mesh in &m.meshes {
            for vb in &mesh.vertex_buffers {
                for (j4, w4) in vb.joints.iter().zip(&vb.weights) {
                    for k in 0..4 {
                        if j4[k] == bi && w4[k] > 0.0 {
                            verts += 1;
                            wsum += w4[k];
                        }
                    }
                }
            }
        }
        println!("{t}: bone#{bi}  verts_referencing={verts}  weight_sum={wsum:.1}");
    }
    let total: usize = m
        .meshes
        .iter()
        .flat_map(|mesh| &mesh.vertex_buffers)
        .map(|vb| vb.joints.len())
        .sum();
    println!("(total vertices across {} meshes: {total})", m.meshes.len());
    Ok(())
}
