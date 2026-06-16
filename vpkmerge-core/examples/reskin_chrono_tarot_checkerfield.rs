// Paradox "Tarot Foil Checkerfield" -- occult engraved foil cut by impossible
// checker geometry.
//
// Concept:
//   * body art: warped black/ivory checkerfield, tiled tarot-card panels,
//     engraved gold glyphs, and raised foil linework.
//   * material: wet lacquer + metallic foil illusion through roughness/normal
//     only; no static-combo shader feature flips.
//   * reactivity: existing chrono dynamic params drive camera-orbit foil shimmer,
//     a low-HP crimson curse wash, and self-illum glow surge.
//
// This intentionally follows the proven Living Opal / Stained Glass constraints:
// replace texture mip chains, in-place double patch existing VMat params, inject
// dynamic expressions via patch_vmat_params, never full-reencode hero VMATs.
//
// usage:
//   cargo run --release --example reskin_chrono_tarot_checkerfield -- <pak01_dir.vpk> --png <prefix>
//   cargo run --release --example reskin_chrono_tarot_checkerfield -- <pak01_dir.vpk> <out_dir.vpk>
use morphic::kv3::{Seg, Value};
use morphic::model::SegmentBy;
use morphic::{Image, ImageData, TextureFormat};
use std::f32::consts::TAU;
use vpkmerge_core::{patch_vmat_params, VmatEdit};

const BODY_COLOR: &str =
    "models/heroes_staging/chrono/materials/chrono_v2_color_png_d1d22ba7.vtex_c";
const BODY_NORMAL: &str =
    "models/heroes_staging/chrono/materials/chrono_v2_vmat_g_tnormalroughness_ce38f34.vtex_c";
const BODY_EMISSIVE: &str =
    "models/heroes_staging/chrono/materials/chrono_v2_emissive_png_718bd18c.vtex_c";
const BODY_VMAT: &str = "models/heroes_staging/chrono/materials/chrono_v2.vmat_c";
const GUN_COLOR: &str =
    "models/heroes_staging/chrono/chrono_gun/materials/chrono_gun_vmat_g_tcolor_7d4419c1.vtex_c";
const GUN_NORMAL: &str =
    "models/heroes_staging/chrono/chrono_gun/materials/chrono_gun_vmat_g_tnormalroughness_7cd9ceac.vtex_c";
const GUN_VMAT: &str = "models/heroes_staging/chrono/chrono_gun/materials/chrono_gun.vmat_c";
const HEADGLASS_VMAT: &str = "models/heroes_staging/chrono/materials/chrono_v2_headglass.vmat_c";
const HOURGLASS_VMAT: &str =
    "models/heroes_staging/chrono/materials/chrono_v2_head_hourglass.vmat_c";
const SHOULDER_VMAT: &str = "models/heroes_staging/chrono/materials/chrono_shoulder.vmat_c";

const HERO: &str = "chrono";

const CHECK_SCALE: f32 = 18.0;
const CARD_COLS: f32 = 4.0;
const CARD_ROWS: f32 = 6.0;
const FOIL_ROUGH: f32 = 0.055;
const LACQUER_ROUGH: f32 = 0.18;
const IVORY_ROUGH: f32 = 0.31;
const INK_ROUGH: f32 = 0.42;
const RELIEF: f32 = 0.78;
const FRESNEL_EXP: f64 = 2.2;

fn view_phase(speed: f64, drift: f64) -> String {
    format!(
        "(dot3(normalize($camera_origin-$ent_origin),float3(2.2,-1.1,0.7))*{speed}+time()*{drift})*3.14159265"
    )
}

fn iridescent(phase: &str) -> String {
    format!(
        "float3(0.5+0.5*cos({phase}),0.5+0.5*cos(({phase})+2.0944),0.5+0.5*cos(({phase})+4.1888))"
    )
}

fn expr_color_tint() -> String {
    let shimmer = iridescent(&view_phase(0.82, 0.11));
    format!(
        "lerp(float3(1.0,0.34,0.22),lerp(float3(0.95,0.82,0.52),{shimmer},0.22),smoothstep(0.18,0.55,$ent_health))"
    )
}

fn expr_fresnel_tint() -> String {
    let shimmer = iridescent(&view_phase(1.35, 0.18));
    format!("lerp(float3(1.0,0.72,0.18),{shimmer},0.48)")
}

fn expr_selfillum_scale() -> &'static str {
    "1.15+0.10*sin(time()*1.9)+(1.0-$ent_health)*1.85"
}

fn reactive_edits() -> anyhow::Result<Vec<VmatEdit>> {
    Ok(vec![
        VmatEdit::expr("g_vSelfIllumFresnelMaskTint1", &expr_fresnel_tint())?,
        VmatEdit::expr("g_vColorTint1", &expr_color_tint())?,
        VmatEdit::expr("g_flSelfIllumScale1", expr_selfillum_scale())?,
    ])
}

fn expr_hourglass_tint() -> String {
    let shimmer = iridescent(&view_phase(1.05, 0.16));
    format!(
        "lerp(float3(1.0,0.24,0.18),lerp(float3(0.95,0.58,0.16),{shimmer},0.38),smoothstep(0.18,0.55,$ent_health))"
    )
}

fn expr_hourglass_scale() -> &'static str {
    "3.6+0.18*sin(time()*2.7)+(1.0-$ent_health)*2.8"
}

fn hourglass_edits() -> anyhow::Result<Vec<VmatEdit>> {
    Ok(vec![
        VmatEdit::expr("g_vSelfIllumTint1", &expr_hourglass_tint())?,
        VmatEdit::expr("g_flSelfIllumScale1", expr_hourglass_scale())?,
    ])
}

fn shoulder_edits() -> anyhow::Result<Vec<VmatEdit>> {
    Ok(vec![
        VmatEdit::expr("g_vSelfIllumTint1", &expr_hourglass_tint())?,
        VmatEdit::expr("g_flSelfIllumScale1", "7.0+(1.0-$ent_health)*4.5")?,
    ])
}

fn hash2(i: i64, j: i64) -> f32 {
    let mut h = (i
        .wrapping_mul(374_761_393)
        .wrapping_add(j.wrapping_mul(668_265_263))) as u64;
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xff_ffff) as f32 / 16_777_216.0
}

fn vnoise(x: f32, y: f32, p: i64) -> f32 {
    let gx = x * p as f32;
    let gy = y * p as f32;
    let x0 = gx.floor() as i64;
    let y0 = gy.floor() as i64;
    let fx = gx - x0 as f32;
    let fy = gy - y0 as f32;
    let wrap = |a: i64| ((a % p) + p) % p;
    let (x0w, y0w) = (wrap(x0), wrap(y0));
    let (x1w, y1w) = (wrap(x0 + 1), wrap(y0 + 1));
    let s = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (ux, uy) = (s(fx), s(fy));
    let v00 = hash2(x0w, y0w);
    let v10 = hash2(x1w, y0w);
    let v01 = hash2(x0w, y1w);
    let v11 = hash2(x1w, y1w);
    let a = v00 + (v10 - v00) * ux;
    let b = v01 + (v11 - v01) * ux;
    a + (b - a) * uy
}

fn fbm(x: f32, y: f32, p0: i64, oct: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut p = p0;
    let mut norm = 0.0;
    for _ in 0..oct {
        sum += amp * vnoise(x, y, p);
        norm += amp;
        amp *= 0.5;
        p *= 2;
    }
    sum / norm
}

fn cell_hash(cx: i32, cy: i32, salt: i64) -> f32 {
    hash2(cx as i64 + salt * 7919, cy as i64 + salt * 104_729)
}

fn smoothstep_f(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn add_glow(rgb: &mut [f32; 3], target: [f32; 3], amt: f32) {
    for (c, t) in rgb.iter_mut().zip(target) {
        *c += (t - *c) * amt;
    }
}

fn line(dist: f32, width: f32) -> f32 {
    1.0 - smoothstep_f(width, width * 1.7, dist.abs())
}

fn sd_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let vx = bx - ax;
    let vy = by - ay;
    let wx = px - ax;
    let wy = py - ay;
    let c = ((wx * vx + wy * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
    let dx = px - (ax + vx * c);
    let dy = py - (ay + vy * c);
    (dx * dx + dy * dy).sqrt()
}

fn glyph_lines(x: f32, y: f32, glyph: i32) -> f32 {
    let px = x - 0.5;
    let py = y - 0.5;
    let r = (px * px + py * py).sqrt();
    let theta = py.atan2(px);
    let ring = line(r - 0.24, 0.012).max(line(r - 0.34, 0.008));
    match glyph.rem_euclid(6) {
        0 => {
            let rays = line(((theta / TAU * 12.0).fract() - 0.5).abs(), 0.035)
                * smoothstep_f(0.20, 0.36, r)
                * (1.0 - smoothstep_f(0.40, 0.48, r));
            ring.max(rays).max(line(r - 0.08, 0.018))
        }
        1 => {
            let moon_outer = line(((px + 0.03).powi(2) + py.powi(2)).sqrt() - 0.23, 0.018);
            let moon_inner = smoothstep_f(0.17, 0.20, ((px - 0.08).powi(2) + py.powi(2)).sqrt());
            ring.max(moon_outer * moon_inner)
        }
        2 => {
            let eye = line((px / 0.34).powi(2) + (py / 0.13).powi(2) - 1.0, 0.10)
                .max(line(r - 0.06, 0.018));
            ring.max(eye)
        }
        3 => {
            let a = line(sd_segment(x, y, 0.30, 0.70, 0.50, 0.24), 0.012);
            let b = line(sd_segment(x, y, 0.70, 0.70, 0.50, 0.24), 0.012);
            let c = line(sd_segment(x, y, 0.34, 0.60, 0.66, 0.60), 0.012);
            ring.max(a.max(b).max(c))
        }
        4 => {
            let sword = line(sd_segment(x, y, 0.50, 0.20, 0.50, 0.76), 0.010)
                .max(line(sd_segment(x, y, 0.38, 0.58, 0.62, 0.58), 0.012));
            ring.max(sword)
        }
        _ => {
            let hour = line(sd_segment(x, y, 0.35, 0.25, 0.65, 0.75), 0.012)
                .max(line(sd_segment(x, y, 0.65, 0.25, 0.35, 0.75), 0.012))
                .max(line(sd_segment(x, y, 0.34, 0.25, 0.66, 0.25), 0.012))
                .max(line(sd_segment(x, y, 0.34, 0.75, 0.66, 0.75), 0.012));
            ring.max(hour)
        }
    }
}

fn warped_uv(u: f32, v: f32, smooth: bool) -> (f32, f32) {
    let scale = if smooth { 0.82 } else { 1.0 };
    let n = fbm(u * 1.2 + 2.0, v * 1.2 + 8.0, 3, 3);
    let w = 0.075 * scale;
    let wu = u + w * (TAU * (v * 2.0 + n)).sin() + 0.035 * (TAU * (u + v * 1.7)).sin();
    let wv = v + w * (TAU * (u * 1.6 - n)).cos() + 0.035 * (TAU * (v - u * 1.3)).sin();
    (wu.fract(), wv.fract())
}

fn checker(wu: f32, wv: f32) -> (f32, f32) {
    let cu = wu * CHECK_SCALE;
    let cv = wv * CHECK_SCALE;
    let ix = cu.floor() as i32;
    let iy = cv.floor() as i32;
    let parity = ((ix + iy) & 1) as f32;
    let fx = (cu.fract() - 0.5).abs();
    let fy = (cv.fract() - 0.5).abs();
    let edge = 1.0 - smoothstep_f(0.455, 0.495, fx.max(fy));
    (parity, edge)
}

fn tarot_panel(u: f32, v: f32) -> (f32, f32, f32, i32, f32, f32) {
    let gu = u * CARD_COLS;
    let gv = v * CARD_ROWS;
    let cx = gu.floor() as i32;
    let cy = gv.floor() as i32;
    let lx = gu.fract();
    let ly = gv.fract();
    let margin = 0.085 + 0.025 * cell_hash(cx, cy, 13);
    let inside = smoothstep_f(margin, margin + 0.025, lx)
        * smoothstep_f(margin, margin + 0.025, ly)
        * (1.0 - smoothstep_f(1.0 - margin - 0.025, 1.0 - margin, lx))
        * (1.0 - smoothstep_f(1.0 - margin - 0.025, 1.0 - margin, ly));
    let border_dist = (lx - margin)
        .abs()
        .min((lx - (1.0 - margin)).abs())
        .min((ly - margin).abs())
        .min((ly - (1.0 - margin)).abs());
    let border = line(border_dist, 0.012) * inside.max(0.35);
    let glyph = (cell_hash(cx, cy, 21) * 6.0) as i32;
    let symbol = glyph_lines(lx, ly, glyph) * inside;
    (inside, border, symbol, glyph, lx, ly)
}

struct TarotSample {
    rgb: [f32; 3],
    glow: f32,
    rough: f32,
    height: f32,
}

fn tarot_sample(u: f32, v: f32, clear: f32, smooth: bool) -> TarotSample {
    let (wu, wv) = warped_uv(u, v, smooth);
    let (check, check_edge) = checker(wu, wv);
    let (inside, border, symbol, glyph, lx, ly) = tarot_panel(u, v);

    let occult_grid = line(((u * 32.0 + v * 9.0).fract() - 0.5).abs(), 0.018) * 0.25
        + line(((u * -7.0 + v * 42.0).fract() - 0.5).abs(), 0.014) * 0.18;
    let hatch = line(((u * 84.0 + v * 41.0).fract() - 0.5).abs(), 0.020)
        * line(((u * -37.0 + v * 91.0).fract() - 0.5).abs(), 0.035)
        * 0.55;
    let card_ticks = line((lx * 8.0).fract() - 0.5, 0.035)
        .max(line((ly * 8.0).fract() - 0.5, 0.035))
        * inside
        * 0.16;

    let foil = (border * 0.95
        + symbol * 1.15
        + check_edge * 0.32
        + occult_grid
        + card_ticks
        + hatch * (0.55 + 0.45 * inside))
        .clamp(0.0, 1.0);
    let ink = ((1.0 - check) * 0.30 + check_edge * 0.18 + hatch * 0.25).clamp(0.0, 1.0);

    let obsidian = [0.018, 0.014, 0.026];
    let ivory = [0.72, 0.65, 0.49];
    let oxblood = [0.085, 0.020, 0.040];
    let midnight = [0.025, 0.030, 0.080];
    let parchment = [0.80, 0.75, 0.62];
    let card_hue = cell_hash(glyph, glyph * 13, 5);
    let card = if card_hue < 0.33 {
        oxblood
    } else if card_hue < 0.66 {
        midnight
    } else {
        [0.055, 0.033, 0.070]
    };

    let check_base = mix(obsidian, ivory, check * (0.78 - 0.45 * clear));
    let mut rgb = mix(check_base, card, inside * (0.58 - 0.28 * clear));
    rgb = mix(rgb, parchment, clear * 0.72);

    let age = fbm(u * 5.0 + 0.3, v * 5.0 + 9.1, 5, 3);
    let tarnish = [0.34, 0.22, 0.11];
    let gold = [1.00, 0.68, 0.20];
    let pale_gold = [1.0, 0.86, 0.46];
    let foil_col = mix(
        mix(tarnish, gold, 0.74 + 0.22 * age),
        pale_gold,
        symbol * 0.38,
    );
    add_glow(&mut rgb, foil_col, foil * (0.78 + 0.12 * clear));

    let curse = smoothstep_f(0.74, 0.98, fbm(u * 2.1 + 8.0, v * 2.1 + 1.0, 3, 4));
    add_glow(&mut rgb, [0.50, 0.04, 0.10], curse * 0.10 * (1.0 - clear));

    let rough = (LACQUER_ROUGH * (1.0 - check) + IVORY_ROUGH * check + INK_ROUGH * ink * 0.30
        - foil * 0.16)
        .clamp(FOIL_ROUGH, 0.48);
    let glow = (foil * (0.34 + 0.44 * symbol) + check_edge * 0.08).clamp(0.0, 0.72);
    let height = (foil * 0.78 + border * 0.28 + check_edge * 0.16 - ink * 0.10).clamp(0.0, 1.0);

    TarotSample {
        rgb,
        glow,
        rough,
        height,
    }
}

struct Mask {
    w: usize,
    h: usize,
    g: Vec<f32>,
}

impl Mask {
    fn from_png(bytes: &[u8]) -> anyhow::Result<Self> {
        let img = image::load_from_memory(bytes)?.to_luma8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let g = img.into_raw().iter().map(|&p| p as f32 / 255.0).collect();
        Ok(Self { w, h, g })
    }

    fn at(&self, u: f32, v: f32) -> f32 {
        let x = ((u.fract() + 1.0).fract() * self.w as f32) as usize % self.w;
        let y = ((v.fract() + 1.0).fract() * self.h as f32) as usize % self.h;
        self.g[y * self.w + x]
    }
}

fn bake_part_masks(pak: &str, res: u32) -> anyhow::Result<(Mask, Mask)> {
    let entry = vpkmerge_core::hero_model_entry(pak, None, HERO)?;
    let bytes = vpkmerge_core::read_vpk_entry(pak, &entry)?;
    let model = morphic::model::decode(&bytes)?;
    let segs = morphic::model::segments(&model, SegmentBy::Part, None);
    let find = |label: &str| -> anyhow::Result<usize> {
        segs.iter()
            .position(|s| s.label == label)
            .ok_or_else(|| anyhow::anyhow!("part '{label}' not found in {entry}"))
    };
    let body_id = find("body")?;
    let head_id = find("headbase")?;
    let body_png = morphic::model::mask_png(&segs, &[body_id], res)?;
    let head_png = morphic::model::mask_png(&segs, &[head_id], res)?;
    eprintln!(
        "UV part masks baked from {entry} (res {res}): body=id{body_id}, headbase=id{head_id}"
    );
    Ok((Mask::from_png(&body_png)?, Mask::from_png(&head_png)?))
}

fn rgba8_mut(img: &mut Image) -> anyhow::Result<&mut Vec<u8>> {
    match &mut img.data {
        ImageData::Rgba8(v) => Ok(v),
        ImageData::Rgba16F(_) => anyhow::bail!("unexpected HDR texture"),
    }
}

fn mask_weights(masks: Option<(&Mask, &Mask)>, u: f32, v: f32) -> (f32, f32) {
    match masks {
        Some((body, head)) => {
            let h = head.at(u, v);
            let live = body.at(u, v).max(h);
            (h, live)
        }
        None => (0.0, 1.0),
    }
}

fn paint_albedo(
    img: &mut Image,
    masks: Option<(&Mask, &Mask)>,
    smooth: bool,
) -> anyhow::Result<()> {
    let (w, h) = (img.width, img.height);
    let px = rgba8_mut(img)?;
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let (clear, live) = mask_weights(masks, u, v);
            let s = tarot_sample(u, v, clear, smooth);
            let dead = [0.018, 0.016, 0.022];
            let rgb = mix(dead, s.rgb, live);
            let i = ((y * w + x) * 4) as usize;
            px[i] = byte(rgb[0]);
            px[i + 1] = byte(rgb[1]);
            px[i + 2] = byte(rgb[2]);
            px[i + 3] = 255;
        }
    }
    Ok(())
}

fn paint_emissive(img: &mut Image, masks: Option<(&Mask, &Mask)>) -> anyhow::Result<()> {
    let (w, h) = (img.width, img.height);
    let px = rgba8_mut(img)?;
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let (clear, live) = mask_weights(masks, u, v);
            let s = tarot_sample(u, v, clear, false);
            let g = (s.glow * live * (0.85 + 0.15 * clear)).clamp(0.0, 0.76);
            let b = byte(g);
            let i = ((y * w + x) * 4) as usize;
            px[i] = b;
            px[i + 1] = b;
            px[i + 2] = b;
            px[i + 3] = b;
        }
    }
    Ok(())
}

fn paint_normal_roughness(
    img: &mut Image,
    masks: Option<(&Mask, &Mask)>,
    smooth: bool,
) -> anyhow::Result<()> {
    let (w, h) = (img.width, img.height);
    let eps = 2.0 / w as f32;
    let px = rgba8_mut(img)?;
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let (clear, live) = mask_weights(masks, u, v);
            let hl = tarot_sample(u - eps, v, clear, smooth).height;
            let hr = tarot_sample(u + eps, v, clear, smooth).height;
            let hd = tarot_sample(u, v - eps, clear, smooth).height;
            let hu = tarot_sample(u, v + eps, clear, smooth).height;
            let s = tarot_sample(u, v, clear, smooth);
            let nx = -(hr - hl) / (2.0 * eps) * RELIEF;
            let ny = -(hu - hd) / (2.0 * eps) * RELIEF;
            let inv = 1.0 / (nx * nx + ny * ny + 1.0).sqrt();
            let rough = (s.rough + (0.46 - s.rough) * (1.0 - live)).clamp(0.04, 0.62);
            let i = ((y * w + x) * 4) as usize;
            px[i] = byte(nx * inv * 0.5 + 0.5);
            px[i + 1] = byte(ny * inv * 0.5 + 0.5);
            px[i + 2] = byte(rough);
            px[i + 3] = 255;
        }
    }
    Ok(())
}

fn paint_thumbnail(img: &mut Image) -> anyhow::Result<()> {
    let (w, h) = (img.width, img.height);
    let px = rgba8_mut(img)?;
    let cx = w as f32 * 0.5;
    let cy = h as f32 * 0.5;
    let scale = w.min(h) as f32;
    for y in 0..h {
        for x in 0..w {
            let pxn = (x as f32 - cx) / scale + 0.5;
            let pyn = (y as f32 - cy) / scale + 0.5;
            let card = smoothstep_f(0.06, 0.08, pxn)
                * smoothstep_f(0.08, 0.10, pyn)
                * (1.0 - smoothstep_f(0.90, 0.92, pxn))
                * (1.0 - smoothstep_f(0.90, 0.92, pyn));
            let s = tarot_sample(pxn, pyn, 0.0, false);
            let border = line((pxn - 0.08).abs().min((pxn - 0.92).abs()), 0.01)
                .max(line((pyn - 0.10).abs().min((pyn - 0.90).abs()), 0.01));
            let mut rgb = s.rgb;
            add_glow(&mut rgb, [1.0, 0.76, 0.25], border * 0.85);
            let vignette = 0.55 + 0.45 * (1.0 - ((pxn - 0.5).powi(2) + (pyn - 0.5).powi(2)));
            let i = ((y * w + x) * 4) as usize;
            px[i] = byte(rgb[0] * card * vignette);
            px[i + 1] = byte(rgb[1] * card * vignette);
            px[i + 2] = byte(rgb[2] * card * vignette);
            px[i + 3] = 255;
        }
    }
    Ok(())
}

fn vparam_index(v: &Value, name: &str) -> Option<usize> {
    v.get("m_vectorParams")?
        .as_array()?
        .iter()
        .position(|p| p.get("m_name").and_then(Value::as_str) == Some(name))
}

fn vcomp_edits(v: &Value, name: &str, comps: &[(usize, f64)]) -> Vec<(Vec<Seg>, f64)> {
    let Some(i) = vparam_index(v, name) else {
        return Vec::new();
    };
    comps
        .iter()
        .map(|&(k, val)| {
            (
                vec![
                    Seg::Key("m_vectorParams".to_string()),
                    Seg::Index(i),
                    Seg::Key("m_value".to_string()),
                    Seg::Index(k),
                ],
                val,
            )
        })
        .collect()
}

fn fparam_index(v: &Value, name: &str) -> Option<usize> {
    v.get("m_floatParams")?
        .as_array()?
        .iter()
        .position(|p| p.get("m_name").and_then(Value::as_str) == Some(name))
}

fn fscalar_edit(v: &Value, name: &str, val: f64) -> Vec<(Vec<Seg>, f64)> {
    match fparam_index(v, name) {
        Some(i) => vec![(
            vec![
                Seg::Key("m_floatParams".to_string()),
                Seg::Index(i),
                Seg::Key("m_flValue".to_string()),
            ],
            val,
        )],
        None => Vec::new(),
    }
}

fn rough_low_edits(v: &Value) -> Vec<(Vec<Seg>, f64)> {
    vcomp_edits(
        v,
        "TextureRoughness1",
        &[
            (0, FOIL_ROUGH as f64),
            (1, FOIL_ROUGH as f64),
            (2, FOIL_ROUGH as f64),
        ],
    )
}

fn patch_optional(bytes: Vec<u8>, edits: &[(Vec<Seg>, f64)]) -> (Vec<u8>, usize) {
    if edits.is_empty() {
        return (bytes, 0);
    }
    match morphic::patch_kv3_resource_doubles(&bytes, edits) {
        Ok(b) => (b, edits.len()),
        Err(_) => (bytes, 0),
    }
}

fn body_double_patches(bytes: &[u8]) -> anyhow::Result<(Vec<u8>, usize)> {
    let v = morphic::decode_kv3_resource(bytes)?;
    let mut all = rough_low_edits(&v);
    all.extend(fscalar_edit(
        &v,
        "g_flSelfIllumFresnelMaskExponent",
        FRESNEL_EXP,
    ));
    let (out, n) = patch_optional(bytes.to_vec(), &all);
    Ok((out, n))
}

fn report_stats(label: &str, stats: &vpkmerge_core::VmatPatchStats) {
    eprintln!(
        "  {label}: {} set, {} inserted, {} failed{}",
        stats.set,
        stats.inserted,
        stats.failed.len(),
        if stats.failed.is_empty() {
            String::new()
        } else {
            format!(" ({})", stats.failed.join(", "))
        }
    );
}

fn main() -> anyhow::Result<()> {
    let pos: Vec<String> = std::env::args().skip(1).collect();
    let pak = pos.first().cloned().expect(
        "usage: reskin_chrono_tarot_checkerfield <pak01_dir.vpk> <out_dir.vpk|--png prefix>",
    );
    let arg2 = pos
        .get(1)
        .cloned()
        .expect("second arg: <out_dir.vpk> or --png <prefix>");

    eprintln!("Paradox \"Tarot Foil Checkerfield\" (chrono)");
    eprintln!("  body tint    = {}", expr_color_tint());
    eprintln!("  fresnel tint = {}", expr_fresnel_tint());
    eprintln!("  selfillum    = {}", expr_selfillum_scale());

    if arg2 == "--png" {
        let prefix = pos.get(2).cloned().expect("--png needs an output prefix");
        let masks_owned = match bake_part_masks(&pak, 1024) {
            Ok(masks) => Some(masks),
            Err(e) => {
                eprintln!(
                    "note: could not bake chrono UV masks for preview ({e:#}); rendering no-mask sheet"
                );
                None
            }
        };
        let masks = masks_owned
            .as_ref()
            .map(|(body_mask, head_mask)| (body_mask, head_mask));
        let blank = || Image {
            width: 768,
            height: 768,
            data: ImageData::Rgba8(vec![255u8; 768 * 768 * 4]),
        };
        let mut pa = blank();
        paint_albedo(&mut pa, masks, false)?;
        let mut pe = blank();
        paint_emissive(&mut pe, masks)?;
        let mut pn = blank();
        paint_normal_roughness(&mut pn, masks, false)?;
        let mut pt = Image {
            width: 512,
            height: 512,
            data: ImageData::Rgba8(vec![255u8; 512 * 512 * 4]),
        };
        paint_thumbnail(&mut pt)?;
        for (img, suffix) in [
            (&pa, "albedo"),
            (&pe, "emissive"),
            (&pn, "normalroughness"),
            (&pt, "thumb"),
        ] {
            let png = morphic::encode_image(img, TextureFormat::PngRgba8888)?;
            let path = format!("{prefix}_{suffix}.png");
            std::fs::write(&path, &png)?;
            println!("wrote {path} ({}x{})", img.width, img.height);
        }
        return Ok(());
    }
    let out = arg2;

    let body_color_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_COLOR)?;
    let template = morphic::decode(&body_color_bytes)?;
    let (body_mask, head_mask) = bake_part_masks(&pak, template.width.max(2048))?;
    let masks = Some((&body_mask, &head_mask));

    let mut body_albedo = morphic::decode(&body_color_bytes)?;
    paint_albedo(&mut body_albedo, masks, false)?;
    let new_body_color = morphic::replace_mip_chain(&body_color_bytes, &body_albedo)?;

    let mut body_nr = morphic::decode(&body_color_bytes)?;
    paint_normal_roughness(&mut body_nr, masks, false)?;
    let new_body_nr = morphic::replace_mip_chain(&body_color_bytes, &body_nr)?;

    let emissive_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_EMISSIVE)?;
    let mut body_emissive = morphic::decode(&emissive_bytes)?;
    paint_emissive(&mut body_emissive, masks)?;
    let new_body_emissive = morphic::replace_mip_chain(&emissive_bytes, &body_emissive)?;
    eprintln!("textures: body albedo + emissive + normal-roughness re-encoded");

    let mut gun_albedo = morphic::decode(&body_color_bytes)?;
    paint_albedo(&mut gun_albedo, None, true)?;
    let new_gun_color = morphic::replace_mip_chain(&body_color_bytes, &gun_albedo)?;
    let mut gun_nr = morphic::decode(&body_color_bytes)?;
    paint_normal_roughness(&mut gun_nr, None, true)?;
    let new_gun_nr = morphic::replace_mip_chain(&body_color_bytes, &gun_nr)?;
    eprintln!("gun: tarot checkerfield albedo + normal-roughness re-encoded");

    let body_vmat_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_VMAT)?;
    let (body_doubled, n_doubles) = body_double_patches(&body_vmat_bytes)?;
    eprintln!("body vmat: {n_doubles} in-place roughness/Fresnel edit(s)");
    let (new_body_vmat, body_stats) = patch_vmat_params(&body_doubled, &reactive_edits()?)?;
    report_stats("body expressions", &body_stats);
    anyhow::ensure!(
        body_stats.failed.is_empty(),
        "a body expression failed to inject -- aborting"
    );

    let gun_vmat_bytes = vpkmerge_core::read_vpk_entry(&pak, GUN_VMAT)?;
    let gv = morphic::decode_kv3_resource(&gun_vmat_bytes)?;
    let (new_gun_vmat, n_gun) = patch_optional(gun_vmat_bytes.clone(), &rough_low_edits(&gv));
    eprintln!("gun vmat: {n_gun} in-place roughness edit(s)");

    let headglass_bytes = vpkmerge_core::read_vpk_entry(&pak, HEADGLASS_VMAT)?;
    let hgl = morphic::decode_kv3_resource(&headglass_bytes)?;
    let mut dome = vcomp_edits(&hgl, "TextureColor1", &[(0, 0.08), (1, 0.055), (2, 0.12)]);
    dome.extend(vcomp_edits(
        &hgl,
        "g_vSolidOutlineTint",
        &[(0, 0.95), (1, 0.58), (2, 0.12)],
    ));
    let (new_headglass_vmat, n_dome) = patch_optional(headglass_bytes.clone(), &dome);
    eprintln!("head glass dome: {n_dome} occult glass tint/outline edit(s)");

    let hourglass_bytes = vpkmerge_core::read_vpk_entry(&pak, HOURGLASS_VMAT)?;
    let (new_hourglass_vmat, hg_stats) = patch_vmat_params(&hourglass_bytes, &hourglass_edits()?)?;
    report_stats("hourglass expressions", &hg_stats);

    let shoulder_bytes = vpkmerge_core::read_vpk_entry(&pak, SHOULDER_VMAT)?;
    let sh = morphic::decode_kv3_resource(&shoulder_bytes)?;
    let shoulder_base = vcomp_edits(&sh, "TextureColor1", &[(0, 0.82), (1, 0.68), (2, 0.38)]);
    let (shoulder_neutral, n_sh) = patch_optional(shoulder_bytes.clone(), &shoulder_base);
    let (new_shoulder_vmat, sh_stats) = patch_vmat_params(&shoulder_neutral, &shoulder_edits()?)?;
    report_stats("shoulder expressions", &sh_stats);
    eprintln!("shoulder: {n_sh} foil albedo edit(s) + dynamic glow");

    let readme = format!(
        "Paradox \"Tarot Foil Checkerfield\" -- reactive creative skin\n\
        =============================================================\n\
        vpkmerge test build. Hero: Paradox (chrono).\n\n\
        Baked art: warped impossible checkerfield, tarot-card panels, engraved\n\
        occult symbols, raised gold foil linework, ivory porcelain headbase, and\n\
        dark lacquer body fields. Normal/roughness makes foil lines raised and\n\
        mirror-wet while ink and ivory stay rougher.\n\n\
        Dynamic expressions:\n\
          1. Fresnel rim tint  -> camera-orbit holographic foil shimmer.\n\
          2. Body color tint   -> gold spectral wash, curses crimson at low HP.\n\
          3. Self-illum scale  -> engraved symbols breathe and surge at low HP.\n\n\
        No shader feature flips and no full VMAT re-encode.\n\
        selfillum_expr={}\n",
        expr_selfillum_scale()
    );

    vpkmerge_core::pack(
        &[
            (BODY_COLOR, new_body_color.as_slice()),
            (BODY_NORMAL, new_body_nr.as_slice()),
            (BODY_EMISSIVE, new_body_emissive.as_slice()),
            (BODY_VMAT, new_body_vmat.as_slice()),
            (GUN_COLOR, new_gun_color.as_slice()),
            (GUN_NORMAL, new_gun_nr.as_slice()),
            (GUN_VMAT, new_gun_vmat.as_slice()),
            (HEADGLASS_VMAT, new_headglass_vmat.as_slice()),
            (HOURGLASS_VMAT, new_hourglass_vmat.as_slice()),
            (SHOULDER_VMAT, new_shoulder_vmat.as_slice()),
            ("README.txt", readme.as_bytes()),
        ],
        &out,
    )?;
    println!("wrote addon VPK: {out}");
    Ok(())
}
