// Bake the full model in a named NM gameplay clip and render the head region:
// head mesh gray, welded hat red (swatch UV). Shows if the HEAD itself swings or
// only the hat. usage:
//   cargo run --release --example mina_nm_pose_render -- <baked_dir.vpk> <pak01> <clip> <out.png>
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
use morphic::model::{bake_pose, decode_nm_clip, decode_nm_skeleton, nm_clip_to_clip, Model};
use vpkmerge_core::read_vpk_entry;

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";
const SKEL: &str = "models/heroes_wip/vampirebat/vampirebat.vnmskel_c";
const GRID: f32 = 256.0;

fn swatch_uv(uv: [f32; 2]) -> bool {
    let c = [0.5 / GRID, 255.5 / GRID];
    let near = |a: f32, b: f32| (a - b).abs() < 0.6 / GRID;
    (near(uv[0], c[0]) || near(uv[0], c[1])) && (near(uv[1], c[0]) || near(uv[1], c[1]))
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let bake = a.next().expect("baked vpk");
    let pak = a.next().expect("pak01");
    let clip = a.next().expect("clip name");
    let out = a.next().unwrap_or_else(|| ".scratch/nm_pose.png".into());

    let mut model = morphic::model::decode(&read_vpk_entry(&bake, ENTRY)?)?;
    let skel = decode_nm_skeleton(&read_vpk_entry(&pak, SKEL)?)?;
    let nmclip = decode_nm_clip(&read_vpk_entry(
        &pak,
        &format!("models/heroes_wip/vampirebat/clips/{clip}.vnmclip_c"),
    )?)?;
    let c = nm_clip_to_clip(&nmclip, &skel, &model.skeleton, "g");
    let frame = c.frame_count / 2;
    model.animations = vec![c];
    let posed = bake_pose(&model, &["g"], frame);

    render(&posed, &out)?;
    eprintln!("wrote {out} for clip {clip}");
    Ok(())
}

fn render(m: &Model, out: &str) -> anyhow::Result<()> {
    let head = m.meshes.iter().find(|p| p.name == "head").unwrap();
    let vb = &head.vertex_buffers[0];
    let uv = &vb.texcoords[0];
    let pts: Vec<([f32; 3], bool)> = (0..vb.element_count)
        .map(|i| (vb.positions[i], swatch_uv(uv[i])))
        .collect();
    // bounds of head region
    let (mut mn, mut mx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for (p, _) in &pts {
        for k in 0..3 {
            mn[k] = mn[k].min(p[k]);
            mx[k] = mx[k].max(p[k]);
        }
    }
    let (w, h) = (520u32, 420u32);
    let pad = 22.0;
    let halfw = (w / 2) as f32;
    let mut img = vec![18u8; (w * h * 3) as usize];
    let map = |v: f32, lo: f32, hi: f32, a: f32, b: f32| a + (v - lo) / (hi - lo + 1e-6) * (b - a);
    let mut plot = |hx: f32, vz: f32, haxis: usize, panel: u32, col: [u8; 3], img: &mut [u8]| {
        let x0 = panel as f32 * halfw;
        let px = map(hx, mn[haxis], mx[haxis], x0 + pad, x0 + halfw - pad);
        let py = map(vz, mx[2], mn[2], pad, h as f32 - pad);
        let (ix, iy) = (px as i32, py as i32);
        if ix >= 0 && ix < w as i32 && iy >= 0 && iy < h as i32 {
            let o = ((iy as u32 * w + ix as u32) * 3) as usize;
            img[o] = col[0];
            img[o + 1] = col[1];
            img[o + 2] = col[2];
        }
    };
    // LEFT = side (X horiz), RIGHT = front/back (Y horiz). Vertical = Z.
    for (p, hat) in &pts {
        if !hat {
            plot(p[0], p[2], 0, 0, [120, 120, 120], &mut img);
            plot(p[1], p[2], 1, 1, [120, 120, 120], &mut img);
        }
    }
    for (p, hat) in &pts {
        if *hat {
            plot(p[0], p[2], 0, 0, [235, 40, 40], &mut img);
            plot(p[1], p[2], 1, 1, [235, 40, 40], &mut img);
        }
    }
    image::save_buffer(out, &img, w, h, image::ColorType::Rgb8)?;
    Ok(())
}
