// Measure the welded hat's cone-axis under each embedded pose, isolating hat
// verts by their exact swatch UVs. Tells us if any clip tips the hat sideways.
// usage: cargo run --release --example mina_hat_pose_axis -- <baked_dir.vpk>
#![allow(clippy::cast_precision_loss)]
use morphic::model::{bake_pose, Model};
use vpkmerge_core::read_vpk_entry;

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";
const GRID: f32 = 256.0;

fn swatch_uv(uv: [f32; 2]) -> bool {
    // swatch cell centers live at grid corners (0,0),(255,255),(0,255),(255,0).
    let c = [0.5 / GRID, 255.5 / GRID];
    let near = |a: f32, b: f32| (a - b).abs() < 0.6 / GRID;
    (near(uv[0], c[0]) || near(uv[0], c[1])) && (near(uv[1], c[0]) || near(uv[1], c[1]))
}

fn head_hat_indices(m: &Model) -> Vec<usize> {
    let head = m.meshes.iter().find(|p| p.name == "head").unwrap();
    let vb = &head.vertex_buffers[0];
    let uv = &vb.texcoords[0];
    (0..vb.element_count)
        .filter(|&i| swatch_uv(uv[i]))
        .collect()
}

fn cone_axis(m: &Model, idx: &[usize]) -> ([f32; 3], f32) {
    let head = m.meshes.iter().find(|p| p.name == "head").unwrap();
    let vb = &head.vertex_buffers[0];
    let pts: Vec<[f32; 3]> = idx.iter().map(|&i| vb.positions[i]).collect();
    let mut c = [0.0; 3];
    for p in &pts {
        for k in 0..3 {
            c[k] += p[k];
        }
    }
    for k in 0..3 {
        c[k] /= pts.len().max(1) as f32;
    }
    let tip = pts
        .iter()
        .cloned()
        .fold(([0.0; 3], -1.0f32), |(bt, bd), p| {
            let d = (0..3).map(|k| (p[k] - c[k]).powi(2)).sum::<f32>();
            if d > bd {
                (p, d)
            } else {
                (bt, bd)
            }
        })
        .0;
    let a = [tip[0] - c[0], tip[1] - c[1], tip[2] - c[2]];
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-6);
    ([a[0] / l, a[1] / l, a[2] / l], l)
}

fn main() -> anyhow::Result<()> {
    let vpk = std::env::args().nth(1).expect("arg1: baked_dir.vpk");
    let model = morphic::model::decode(&read_vpk_entry(&vpk, ENTRY)?)?;
    let idx = head_hat_indices(&model);
    println!("isolated {} hat verts (expect ~7671)", idx.len());

    // bind (no clip)
    let (ax, len) = cone_axis(&model, &idx);
    println!("\nBIND (no pose):");
    println!(
        "  cone axis = ({:+.2},{:+.2},{:+.2})  up|Z|={:.2}  len={:.1}",
        ax[0],
        ax[1],
        ax[2],
        ax[2].abs(),
        len
    );

    println!("\nembedded clips ({}):", model.animations.len());
    let names: Vec<String> = model.animations.iter().map(|c| c.name.clone()).collect();
    for n in &names {
        print!("  {n}");
    }
    println!();

    // candidate gameplay/idle clips by name substring
    let wanted = ["stand", "idle", "ready", "run", "walk", "pose", "spawn"];
    println!("\nper-clip head-hat cone axis (frame 0):");
    for c in &model.animations {
        let lname = c.name.to_ascii_lowercase();
        if !wanted.iter().any(|w| lname.contains(w)) {
            continue;
        }
        let posed = bake_pose(&model, &[c.name.as_str()], 0);
        let (ax, _) = cone_axis(&posed, &idx);
        println!(
            "  {:32} axis=({:+.2},{:+.2},{:+.2})  up|Z|={:.2}",
            c.name,
            ax[0],
            ax[1],
            ax[2],
            ax[2].abs()
        );
    }
    Ok(())
}
