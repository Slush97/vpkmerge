// Render an exported (posed) GLB's upper body in its NATIVE glTF space (auto
// bounds), flagging hat verts (distinctive corner swatch UVs) red, so we see the
// hat exactly as the engine poses it.
// usage: cargo run --release --example render_posed_glb -- <glb> <out.png>
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
use anyhow::{Context, Result};
use vpkmerge_core::soul_import_clone::read_glb_primitives;
use vpkmerge_core::SoulOrient;

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let glb_path = a.next().context("arg1: glb")?;
    let out = a.next().unwrap_or_else(|| ".scratch/posed.png".into());
    let glb = std::fs::read(&glb_path)?;
    let (prims, _) = read_glb_primitives(&glb, SoulOrient::YUp, None)?;

    let corners = [
        [0.00195f32, 0.00195],
        [0.998, 0.998],
        [0.00195, 0.998],
        [0.998, 0.00195],
    ];
    let is_hat = |uv: [f32; 2]| {
        corners
            .iter()
            .any(|c| (uv[0] - c[0]).abs() < 0.02 && (uv[1] - c[1]).abs() < 0.02)
    };

    let mut pts: Vec<([f32; 3], bool)> = Vec::new();
    for p in &prims {
        let vb = &p.vertex_buffer;
        let uvs = vb.texcoords.first();
        for (i, v) in vb.positions.iter().enumerate() {
            let uv = uvs.and_then(|t| t.get(i)).copied().unwrap_or([0.5, 0.5]);
            pts.push((*v, is_hat(uv)));
        }
    }
    let hatn = pts.iter().filter(|p| p.1).count();

    // tallest native axis = vertical; the other two are the horizontal views.
    let (mut mn, mut mx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for (v, _) in &pts {
        for k in 0..3 {
            mn[k] = mn[k].min(v[k]);
            mx[k] = mx[k].max(v[k]);
        }
    }
    let span = [mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]];
    let up = (0..3).max_by(|&a, &b| span[a].total_cmp(&span[b])).unwrap();
    let h1 = (0..3).find(|&k| k != up).unwrap();
    let h2 = (0..3).rfind(|&k| k != up).unwrap();

    // hat centroid + cone axis (tip = hat vert farthest from hat centroid).
    let hp: Vec<[f32; 3]> = pts.iter().filter(|p| p.1).map(|p| p.0).collect();
    let mut hc = [0.0; 3];
    for v in &hp {
        for k in 0..3 {
            hc[k] += v[k];
        }
    }
    for k in 0..3 {
        hc[k] /= hp.len().max(1) as f32;
    }
    let tip = hp
        .iter()
        .cloned()
        .max_by(|a, b| {
            let da = (0..3).map(|k| (a[k] - hc[k]).powi(2)).sum::<f32>();
            let db = (0..3).map(|k| (b[k] - hc[k]).powi(2)).sum::<f32>();
            da.total_cmp(&db)
        })
        .unwrap_or(hc);
    let axis = [tip[0] - hc[0], tip[1] - hc[1], tip[2] - hc[2]];
    let al = (axis[0].powi(2) + axis[1].powi(2) + axis[2].powi(2))
        .sqrt()
        .max(1e-6);
    let an = [axis[0] / al, axis[1] / al, axis[2] / al];
    eprintln!(
        "native spans X{:.2} Y{:.2} Z{:.2}; up-axis={}",
        span[0],
        span[1],
        span[2],
        ["X", "Y", "Z"][up]
    );
    eprintln!(
        "{} verts, {} hat. hat cone axis (centroid->tip) native=({:+.2},{:+.2},{:+.2})",
        pts.len(),
        hatn,
        an[0],
        an[1],
        an[2]
    );
    eprintln!(
        "  -> |up-component|={:.2} (1.0=straight up, ~0=lying sideways)",
        an[up].abs()
    );

    // render two panels: (h1, up) and (h2, up). show only top 40% (head region).
    let zsplit = mn[up] + (mx[up] - mn[up]) * 0.55; // upper portion
    let (w, hh) = (560u32, 600u32);
    let pad = 24.0;
    let mut img = vec![18u8; (w * hh * 3) as usize];
    let halfw = (w / 2) as f32;
    // bounds for the upper region
    let view: Vec<&([f32; 3], bool)> = pts.iter().filter(|p| p.0[up] > zsplit).collect();
    let mut vmn = [f32::INFINITY; 3];
    let mut vmx = [f32::NEG_INFINITY; 3];
    for pr in &view {
        for k in 0..3 {
            vmn[k] = vmn[k].min(pr.0[k]);
            vmx[k] = vmx[k].max(pr.0[k]);
        }
    }
    let map =
        |val: f32, lo: f32, hi: f32, a: f32, b: f32| a + (val - lo) / (hi - lo + 1e-6) * (b - a);
    let mut plot = |hx: f32, vz: f32, haxis: usize, panel: u32, col: [u8; 3], img: &mut [u8]| {
        let x0 = panel as f32 * halfw;
        let px = map(hx, vmn[haxis], vmx[haxis], x0 + pad, x0 + halfw - pad);
        let py = map(vz, vmx[up], vmn[up], pad, hh as f32 - pad);
        let (ix, iy) = (px as i32, py as i32);
        if ix >= 0 && ix < w as i32 && iy >= 0 && iy < hh as i32 {
            let o = ((iy as u32 * w + ix as u32) * 3) as usize;
            img[o] = col[0];
            img[o + 1] = col[1];
            img[o + 2] = col[2];
        }
    };
    for pr in &view {
        let (p, hat) = **pr;
        if !hat {
            plot(p[h1], p[up], h1, 0, [120, 120, 120], &mut img);
            plot(p[h2], p[up], h2, 1, [120, 120, 120], &mut img);
        }
    }
    for pr in &view {
        let (p, hat) = **pr;
        if hat {
            plot(p[h1], p[up], h1, 0, [235, 40, 40], &mut img);
            plot(p[h2], p[up], h2, 1, [235, 40, 40], &mut img);
        }
    }
    image::save_buffer(&out, &img, w, hh, image::ColorType::Rgb8)?;
    eprintln!(
        "wrote {out}  LEFT=({},{}) RIGHT=({},{}). red=hat",
        ["X", "Y", "Z"][h1],
        ["X", "Y", "Z"][up],
        ["X", "Y", "Z"][h2],
        ["X", "Y", "Z"][up]
    );
    Ok(())
}
