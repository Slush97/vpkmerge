// Throwaway: dump Mina's head mesh geometry bounds so we can seat a hat correctly.
// usage: cargo run --release --example mina_head_bbox -- <pak01_dir.vpk>
use vpkmerge_core::read_vpk_entry;

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";
const HEAD_BONE: u16 = 16;

fn main() -> anyhow::Result<()> {
    let pak = std::env::args().nth(1).expect("arg1: pak01_dir.vpk");
    let bytes = read_vpk_entry(&pak, ENTRY)?;
    let model = morphic::model::decode(&bytes)?;
    let b = &model.skeleton.bones[HEAD_BONE as usize].global_bind.m;
    println!("head bone (16): ({:.2}, {:.2}, {:.2})", b[12], b[13], b[14]);

    let head = model.meshes.iter().find(|m| m.name == "head").unwrap();
    let shared = &head.vertex_buffers[0];

    // Full head part bbox.
    bbox(
        "head part (all 3 draw calls, shared VB)",
        shared.positions.iter(),
    );

    // Per primitive bbox (which is eyes/hair/face).
    for (pi, p) in head.primitives.iter().enumerate() {
        let mat = p.material.rsplit('/').next().unwrap_or(&p.material);
        let verts: Vec<[f32; 3]> = p
            .indices
            .iter()
            .map(|&i| shared.positions[i as usize])
            .collect();
        bbox(
            &format!("  prim[{pi}] {mat} ({} idx)", p.indices.len()),
            verts.iter(),
        );
    }

    // Where is the head geometry vs the head bone? Look at verts skinned mostly to bone 16.
    let mut hx = (0.0f32, 0.0f32, 0usize, f32::INFINITY, f32::NEG_INFINITY);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    for (i, p) in shared.positions.iter().enumerate() {
        let j = shared.joints[i];
        let w = shared.weights[i];
        let head_w: f32 = (0..4).filter(|&k| j[k] == HEAD_BONE).map(|k| w[k]).sum();
        if head_w > 0.5 {
            sx += p[0];
            sy += p[1];
            hx.2 += 1;
            hx.3 = hx.3.min(p[2]);
            hx.4 = hx.4.max(p[2]);
        }
    }
    if hx.2 > 0 {
        println!(
            "verts >50% head-skinned: {} | XY centroid=({:.2},{:.2}) | z {:.2}..{:.2}",
            hx.2,
            sx / hx.2 as f32,
            sy / hx.2 as f32,
            hx.3,
            hx.4
        );
    }
    Ok(())
}

fn bbox<'a>(label: &str, it: impl Iterator<Item = &'a [f32; 3]>) {
    let (mut mn, mut mx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    let mut n = 0;
    for p in it {
        for k in 0..3 {
            mn[k] = mn[k].min(p[k]);
            mx[k] = mx[k].max(p[k]);
        }
        n += 1;
    }
    println!(
        "{label}: n={n} x[{:.2},{:.2}] y[{:.2},{:.2}] z[{:.2},{:.2}] center=({:.2},{:.2},{:.2})",
        mn[0],
        mx[0],
        mn[1],
        mx[1],
        mn[2],
        mx[2],
        (mn[0] + mx[0]) * 0.5,
        (mn[1] + mx[1]) * 0.5,
        (mn[2] + mx[2]) * 0.5
    );
}
