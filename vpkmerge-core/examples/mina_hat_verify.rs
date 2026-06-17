// Verify the hat box landed correctly in the edited Mina model: find vertices
// inside the box AABB and confirm they are skinned to the head bone (model 16).
// usage: cargo run --release --example mina_hat_verify -- <mina_hat_dir.vpk>
use vpkmerge_core::read_vpk_entry;

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";
const HEAD_BONE: u16 = 16;

fn main() -> anyhow::Result<()> {
    let vpk = std::env::args().nth(1).expect("arg1: mina_hat_dir.vpk");
    let bytes = read_vpk_entry(&vpk, ENTRY)?;
    let model = morphic::model::decode(&bytes)?;
    let head = model.meshes.iter().find(|m| m.name == "head").unwrap();
    let vb = &head.vertex_buffers[0];

    // Box was centered near head_end+4 (~z 97) with half-extents (6,6,7).
    let mut box_verts = 0;
    let mut head_skinned = 0;
    let mut zmin = f32::INFINITY;
    let mut zmax = f32::NEG_INFINITY;
    for (i, p) in vb.positions.iter().enumerate() {
        let in_box = p[0].abs() < 12.0 && p[1].abs() < 12.0 && p[2] > 88.0 && p[2] < 106.0;
        // box is rigid: exactly +-6/+-7 around (-1.37,0,~97). Tighten on z>90 + far from body.
        if in_box && p[2] > 89.0 {
            // dominant joint
            let j = vb.joints.get(i).copied().unwrap_or([0; 4]);
            let w = vb.weights.get(i).copied().unwrap_or([0.0; 4]);
            let dom = (0..4).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap();
            if j.contains(&HEAD_BONE) {
                head_skinned += 1;
            }
            // Only count tight box corners (skin verts here are denser; we want the 8 z-levels)
            let _ = (dom, j);
        }
    }
    // Count verts that are EXACTLY at box corner offsets from the anchor.
    let anchor = model
        .skeleton
        .bones
        .iter()
        .find(|b| b.name.eq_ignore_ascii_case("head_end"))
        .map(|b| {
            let t = b.global_bind.m;
            [t[12], t[13], t[14] + 4.0]
        })
        .unwrap();
    for (i, p) in vb.positions.iter().enumerate() {
        let dx = (p[0] - anchor[0]).abs();
        let dy = (p[1] - anchor[1]).abs();
        let dz = (p[2] - anchor[2]).abs();
        if (dx - 6.0).abs() < 0.01 && (dy - 6.0).abs() < 0.01 && (dz - 7.0).abs() < 0.01 {
            box_verts += 1;
            let j = vb.joints.get(i).copied().unwrap_or([0; 4]);
            zmin = zmin.min(p[2]);
            zmax = zmax.max(p[2]);
            assert!(
                j.contains(&HEAD_BONE),
                "box vert {i} not skinned to head: {j:?}"
            );
        }
    }
    println!(
        "anchor (model): ({:.2}, {:.2}, {:.2})",
        anchor[0], anchor[1], anchor[2]
    );
    println!("box corner verts found (|off|=6,6,7): {box_verts} (expect 24, all head-skinned)");
    println!("box z-range: {zmin:.2} .. {zmax:.2}");
    println!("verts in loose box region also head-skinned: {head_skinned}");
    Ok(())
}
