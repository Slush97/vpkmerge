// Render an orthographic side (X-Z) + front (Y-Z) profile of Mina's head with a
// fitted hat overlaid, so hat placement can be judged without Blender.
// Head = gray points, hat = red points. Mina faces +X.
// usage: cargo run --release --example mina_hat_render -- <pak> <hat.glb> <out.png>
//   HAT_WIDTH HAT_RAISE HAT_YAW HAT_PITCH HAT_ROLL env vars (same as the bake).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
use anyhow::{anyhow, Context, Result};
use morphic::model::VertexBuffer;
use vpkmerge_core::read_vpk_entry;
use vpkmerge_core::soul_import_clone::read_glb_primitives;
use vpkmerge_core::SoulOrient;

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";
const HEAD_BONE: u16 = 16;

fn envf(k: &str, d: f32) -> f32 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(d)
}

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let pak = a.next().context("arg1: pak")?;
    let glb_path = a.next().context("arg2: hat.glb")?;
    let out = a
        .next()
        .unwrap_or_else(|| ".scratch/mina_hat_render.png".into());
    let width = envf("HAT_WIDTH", 18.0);
    let raise = envf("HAT_RAISE", -7.0);
    let yaw = envf("HAT_YAW", 0.0);
    let pitch = envf("HAT_PITCH", 0.0);
    let roll = envf("HAT_ROLL", 0.0);

    let model = morphic::model::decode(&read_vpk_entry(&pak, ENTRY)?)?;
    let bb = &model.skeleton.bones[HEAD_BONE as usize].global_bind.m;
    let anchor = [bb[12], bb[13], bb[14]];
    let head = model.meshes.iter().find(|m| m.name == "head").unwrap();
    let shared = &head.vertex_buffers[0];
    let crown_z = shared
        .positions
        .iter()
        .map(|p| p[2])
        .fold(f32::NEG_INFINITY, f32::max);

    // --- replicate the bake's fit exactly ---
    let glb = std::fs::read(&glb_path)?;
    let (prims, _) = read_glb_primitives(&glb, SoulOrient::YUp, None)?;
    let mut hat: Vec<[f32; 3]> = Vec::new();
    for p in &prims {
        for v in &p.vertex_buffer.positions {
            hat.push([v[0], v[2], -v[1]]); // YUp -> Source swizzle
        }
    }
    let (mut mn, mut mx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for p in &hat {
        for k in 0..3 {
            mn[k] = mn[k].min(p[k]);
            mx[k] = mx[k].max(p[k]);
        }
    }
    let span = (mx[0] - mn[0]).max(mx[1] - mn[1]).max(1e-4);
    let scale = width / span;
    let c = [(mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5];
    for p in &mut hat {
        p[0] = (p[0] - c[0]) * scale + anchor[0];
        p[1] = (p[1] - c[1]) * scale + anchor[1];
        p[2] *= scale;
    }
    let rot = euler_zyx(pitch, roll, yaw);
    for p in &mut hat {
        let v = [p[0] - anchor[0], p[1] - anchor[1], p[2] - anchor[2]];
        let r = mat3_mul(&rot, &v);
        *p = [r[0] + anchor[0], r[1] + anchor[1], r[2] + anchor[2]];
    }
    let (mut cx, mut cy, mut lo) = (0.0f32, 0.0f32, f32::INFINITY);
    for p in &hat {
        cx += p[0];
        cy += p[1];
        lo = lo.min(p[2]);
    }
    cx /= hat.len() as f32;
    cy /= hat.len() as f32;
    let (sx, sy, sz) = (anchor[0] - cx, anchor[1] - cy, crown_z + raise - lo);
    for p in &mut hat {
        p[0] += sx;
        p[1] += sy;
        p[2] += sz;
    }

    // head points to draw (face + hair prims), in model space.
    let mut head_pts: Vec<[f32; 3]> = Vec::new();
    for p in &head.primitives {
        for &i in &p.indices {
            head_pts.push(shared.positions[i as usize]);
        }
    }

    // --- render two ortho views side by side ---
    // View A: side  (project X horizontal, Z vertical) -- profile, Mina faces +X (right)
    // View B: front (project Y horizontal, Z vertical)
    let (w, h) = (520u32, 640u32);
    let pad = 30.0f32;
    // common z range
    let zlo = 70.0;
    let zhi = 110.0;
    let xlo = -25.0;
    let xhi = 25.0; // covers head (-9..5) + hat
    let mut img = vec![20u8; (w * h * 3) as usize];

    let halfw = (w / 2) as f32;
    let to_px = |val: f32, lo: f32, hi: f32, lo_px: f32, hi_px: f32| {
        lo_px + (val - lo) / (hi - lo) * (hi_px - lo_px)
    };
    let mut plot = |hx: f32, vz: f32, panel: u32, col: [u8; 3], img: &mut [u8]| {
        let x0 = panel as f32 * halfw;
        let px = to_px(hx, xlo, xhi, x0 + pad, x0 + halfw - pad);
        let py = to_px(vz, zhi, zlo, pad, h as f32 - pad); // z up => invert
        let (ix, iy) = (px as i32, py as i32);
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (xx, yy) = (ix + dx, iy + dy);
                if xx >= 0 && xx < w as i32 && yy >= 0 && yy < h as i32 {
                    let o = ((yy as u32 * w + xx as u32) * 3) as usize;
                    img[o] = col[0];
                    img[o + 1] = col[1];
                    img[o + 2] = col[2];
                }
            }
        }
    };
    // gridline at crown_z and anchor_z
    for gx in 0..w {
        for &(zz, cc) in &[(crown_z, [60u8, 60, 90]), (anchor[2], [40, 70, 40])] {
            let py = to_px(zz, zhi, zlo, pad, h as f32 - pad) as i32;
            if py >= 0 && py < h as i32 {
                let o = ((py as u32 * w + gx) * 3) as usize;
                img[o] = cc[0];
                img[o + 1] = cc[1];
                img[o + 2] = cc[2];
            }
        }
    }
    // panel 0 = side (X horiz). panel1 = front (Y horiz).
    for p in &head_pts {
        plot(p[0], p[2], 0, [130, 130, 130], &mut img);
        plot(p[1], p[2], 1, [130, 130, 130], &mut img);
    }
    for p in &hat {
        plot(p[0], p[2], 0, [230, 40, 40], &mut img);
        plot(p[1], p[2], 1, [230, 40, 40], &mut img);
    }

    image::save_buffer(&out, &img, w, h, image::ColorType::Rgb8)?;
    // numeric summary
    let (mut hn, mut hxb) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for p in &hat {
        for k in 0..3 {
            hn[k] = hn[k].min(p[k]);
            hxb[k] = hxb[k].max(p[k]);
        }
    }
    eprintln!(
        "anchor head bone z={:.1}, crown(hair top) z={:.1}",
        anchor[2], crown_z
    );
    eprintln!(
        "HAT final bbox: x[{:.1},{:.1}] y[{:.1},{:.1}] z[{:.1},{:.1}]  (w/d/h {:.1}/{:.1}/{:.1})",
        hn[0],
        hxb[0],
        hn[1],
        hxb[1],
        hn[2],
        hxb[2],
        hxb[0] - hn[0],
        hxb[1] - hn[1],
        hxb[2] - hn[2]
    );
    eprintln!("wrote {out} (LEFT=side X-Z, Mina faces +X=right; RIGHT=front Y-Z). blue line=crown, green=head bone");
    Ok(())
}

fn euler_zyx(pitch: f32, roll: f32, yaw: f32) -> [[f32; 3]; 3] {
    let (sx, cx) = pitch.to_radians().sin_cos();
    let (sy, cy) = roll.to_radians().sin_cos();
    let (sz, cz) = yaw.to_radians().sin_cos();
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}
fn mat3_mul(m: &[[f32; 3]; 3], v: &[f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}
#[allow(dead_code)]
fn unused(_: VertexBuffer) {}
