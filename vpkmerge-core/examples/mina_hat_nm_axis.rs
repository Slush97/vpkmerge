// Where does the welded hat's crown point under Mina's REAL gameplay clips,
// using the ANIMATED NM decoder (decode_nm_clip + nm_clip_to_clip + bake_pose)
// so animated channels are sampled correctly (decode_nm_pose reads only the
// constant subset and fakes a pose on animated clips).
// usage: cargo run --release --example mina_hat_nm_axis -- <baked_dir.vpk> <pak01_dir.vpk> [clip...]
#![allow(clippy::cast_precision_loss)]
use morphic::model::{bake_pose, decode_nm_clip, decode_nm_skeleton, nm_clip_to_clip, Model};
use vpkmerge_core::read_vpk_entry;

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";
const SKEL: &str = "models/heroes_wip/vampirebat/vampirebat.vnmskel_c";
const CLIPDIR: &str = "models/heroes_wip/vampirebat/clips";
const GRID: f32 = 256.0;

fn swatch_uv(uv: [f32; 2]) -> bool {
    let c = [0.5 / GRID, 255.5 / GRID];
    let near = |a: f32, b: f32| (a - b).abs() < 0.6 / GRID;
    (near(uv[0], c[0]) || near(uv[0], c[1])) && (near(uv[1], c[0]) || near(uv[1], c[1]))
}
fn hat_idx(m: &Model) -> Vec<usize> {
    let h = m.meshes.iter().find(|p| p.name == "head").unwrap();
    let uv = &h.vertex_buffers[0].texcoords[0];
    (0..h.vertex_buffers[0].element_count)
        .filter(|&i| swatch_uv(uv[i]))
        .collect()
}
fn hat_pos(m: &Model, idx: &[usize]) -> Vec<[f32; 3]> {
    let h = m.meshes.iter().find(|p| p.name == "head").unwrap();
    idx.iter()
        .map(|&i| h.vertex_buffers[0].positions[i])
        .collect()
}
fn centroid(p: &[[f32; 3]]) -> [f32; 3] {
    let mut c = [0.0; 3];
    for v in p {
        for k in 0..3 {
            c[k] += v[k];
        }
    }
    for k in 0..3 {
        c[k] /= p.len().max(1) as f32;
    }
    c
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let bake = a.next().expect("arg1 baked vpk");
    let pak = a.next().expect("arg2 pak01");
    let clips: Vec<String> = a.collect();
    let clips = if clips.is_empty() {
        vec![
            "vampirebat_primary_stand_idle".into(),
            "vampirebat_weapon_stand_idle".into(),
            "vampirebat_outofcombat_stand_idle".into(),
            "vampirebat_weapon_crouch_run_n".into(),
        ]
    } else {
        clips
    };

    let model = morphic::model::decode(&read_vpk_entry(&bake, ENTRY)?)?;
    let idx = hat_idx(&model);
    let nm_skel = decode_nm_skeleton(&read_vpk_entry(&pak, SKEL)?)?;
    println!("isolated {} hat verts", idx.len());

    let bind = hat_pos(&model, &idx);
    let cb = centroid(&bind);
    // crown reference vertex = max-z hat vert in bind (cone tip points +Z).
    let apex = (0..idx.len())
        .max_by(|&i, &j| bind[i][2].total_cmp(&bind[j][2]))
        .unwrap();
    println!("bind crown dir ~ +Z (apex vert #{apex})\n");

    for clip in &clips {
        let entry = format!("{CLIPDIR}/{clip}.vnmclip_c");
        let bytes = match read_vpk_entry(&pak, &entry) {
            Ok(b) => b,
            Err(_) => {
                println!("{clip}: <missing>");
                continue;
            }
        };
        let nmclip = match decode_nm_clip(&bytes) {
            Ok(c) => c,
            Err(e) => {
                println!("{clip}: <decode err: {e}>");
                continue;
            }
        };
        let clip_obj = nm_clip_to_clip(&nmclip, &nm_skel, &model.skeleton, "g");
        let frames = clip_obj.frame_count.max(1);
        let mut m2 = model.clone();
        m2.animations = vec![clip_obj];
        print!("{clip:40} frames={frames:3} | ");
        for &f in &[0usize, frames / 2] {
            let posed = bake_pose(&m2, &["g"], f);
            let q = hat_pos(&posed, &idx);
            let ci = centroid(&q);
            let d = [q[apex][0] - ci[0], q[apex][1] - ci[1], q[apex][2] - ci[2]];
            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
            let dir = [d[0] / l, d[1] / l, d[2] / l];
            let tilt = dir[2].clamp(-1.0, 1.0).acos().to_degrees();
            print!(
                "f{f}: ({:+.2},{:+.2},{:+.2}) tilt={:.0}deg   ",
                dir[0], dir[1], dir[2], tilt
            );
        }
        println!();
    }
    Ok(())
}
