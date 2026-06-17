// Throwaway: inspect Mina's skeleton + per-mesh-part bone palettes.
// Goal: find the head bone (index + model-space position) and learn which mesh
// parts actually skin to it, so we know whether a single-draw-call part can host
// a head-attached hat via the proven uncompressed wedge, or whether we must edit
// the multi-draw-call head part.
//
// usage: cargo run --release --example mina_bone_probe -- <pak01_dir.vpk>
use std::collections::BTreeSet;
use vpkmerge_core::read_vpk_entry;

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";

fn main() -> anyhow::Result<()> {
    let pak = std::env::args().nth(1).expect("arg1: pak01_dir.vpk");
    let bytes = read_vpk_entry(&pak, ENTRY)?;
    let model = morphic::model::decode(&bytes)?;

    // Bones whose name mentions head/skull/neck (candidate anchors).
    println!("== candidate head bones ==");
    for (i, b) in model.skeleton.bones.iter().enumerate() {
        let n = b.name.to_ascii_lowercase();
        if n.contains("head") || n.contains("skull") || n.contains("neck") || n.contains("face") {
            let t = b.global_bind.m;
            // column-major mat4: translation is m[12],m[13],m[14]
            println!(
                "  [{i:3}] {:32} pos=({:.2}, {:.2}, {:.2})",
                b.name, t[12], t[13], t[14]
            );
        }
    }

    println!("\n== mesh parts: bone palette (model bone indices actually weighted) ==");
    for part in &model.meshes {
        let mut used: BTreeSet<u16> = BTreeSet::new();
        for vb in &part.vertex_buffers {
            for (j, w) in vb.joints.iter().zip(vb.weights.iter()) {
                for k in 0..4 {
                    if w[k] > 0.0 {
                        used.insert(j[k]);
                    }
                }
            }
        }
        let head_idx = model
            .skeleton
            .bones
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case("head"))
            .or_else(|| {
                model
                    .skeleton
                    .bones
                    .iter()
                    .position(|b| b.name.to_ascii_lowercase().contains("head"))
            });
        let has_head = head_idx.map_or(false, |h| used.contains(&(h as u16)));
        let names: Vec<String> = used
            .iter()
            .filter_map(|&bi| {
                model
                    .skeleton
                    .bones
                    .get(bi as usize)
                    .map(|b| b.name.clone())
            })
            .collect();
        println!(
            "  {:14} {:3} draw-call(s)  {:3} bones used  head?={}",
            part.name,
            part.primitives.len(),
            used.len(),
            if has_head { "YES" } else { "no" }
        );
        if used.len() <= 12 {
            println!("        {names:?}");
        }
    }
    Ok(())
}
