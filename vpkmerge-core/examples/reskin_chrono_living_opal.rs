// Paradox "Living Opal" -- the magnum-opus reactive skin.
//
// It fuses every animation/reactivity lever we have proven, into ONE coherent
// identity on the chrono (Paradox) body:
//
//   base art   : a black-opal substrate albedo (dark gem matrix) + a flowing
//                ripple normal-roughness override (the liquid-metal mechanism:
//                animate the SURFACE, reflections crawl).
//   scroll     : slow normal + albedo + self-illum scroll so the substrate and
//                its micro-structure drift (g_v*ScrollSpeed1, no feature flag).
//   gloss      : uniform low-ish roughness = gem sheen (the Vindicta-dress lesson).
//   REACTIVITY : three dynamic expressions (compiled to engine bytecode and
//                injected LZ4-native, the proven `vmat --set-expr` container):
//                  1. g_vSelfIllumFresnelMaskTint1  <- camera-orbit IRIDESCENCE.
//                     The glowing Fresnel rim sweeps hue as the camera orbits
//                     (real thin-film play-of-color via $camera_origin/$ent_origin,
//                     not a faked static gradient). This is the showstopper.
//                  2. g_vColorTint1  <- a second, phase-offset iridescence on the
//                     body, lerped toward crimson as $ent_health drops (a flush).
//                  3. g_flSelfIllumScale1  <- base glow + low-HP surge. Also the
//                     required non-empty self-illum expression (Yearlu's gotcha:
//                     no expression evaluates unless a self-illum one is present).
//
// Per-draw uniform only (no per-pixel world projection in pbr.vfx, no atan2 in
// the function set), so reactivity drives tint / scroll / scale, layered over
// the flowing texture -- which is exactly what the opal look wants.
//
// chrono's g_tNormalRoughness is a flat 4x4 placeholder, so we override it full
// res out of the body's 4096 BC7 container (same trick liquid-metal used).
//
// All scroll/roughness/Fresnel edits are byte-faithful in-place double patches;
// the expressions go through `patch_vmat_params` (blob-aware LZ4-native insert).
// A full KV3 re-encode of a .vmat_c renders as the engine error shader in-game,
// so neither path ever re-serializes the material.
//
// usage:
//   # preview the base art (no game needed):
//   cargo run --release --example reskin_chrono_living_opal -- <pak01_dir.vpk> --png <prefix>
//   # de-risk: expr-only on STOCK chrono materials (smallest in-game test of the
//   # multi-expression container on this hero):
//   cargo run --release --example reskin_chrono_living_opal -- <pak01_dir.vpk> <out_dir.vpk> --probe
//   # full magnum-opus bake:
//   cargo run --release --example reskin_chrono_living_opal -- <pak01_dir.vpk> <out_dir.vpk>
use morphic::kv3::{Seg, Value};
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

// UV units/sec. Slow + diagonal = a gentle opal shimmer (not a racing scroll).
const FLOW_NORMAL: [f64; 2] = [0.03, 0.022]; // surface ripple crawl
const FLOW_ALBEDO: [f64; 2] = [0.011, 0.008]; // substrate micro-structure drift
const FLOW_SELFILLUM: [f64; 2] = [0.018, 0.013]; // emissive mask drift

// Gem sheen: low + uniform, but not a perfect mirror -- opal has body.
const ROUGHNESS_LOW: f64 = 0.22;
// Broaden the Fresnel rim so the iridescence catches across more of the silhouette.
const FRESNEL_EXP: f64 = 2.0;
// Self-illum mask range, applied to the FLECKS ONLY (LO = dark matrix glow ~0,
// HI = fleck glow). Kept gentle so the flecks get a soft inner fire without
// flooding the body to a fullbright blob (the v2 mistake + the Viscous lesson).
const EMISSIVE_LO: f32 = 0.0;
const EMISSIVE_HI: f32 = 0.55;

// ---------------------------------------------------------------------------
// Reactive expressions. Compiled by morphic::vfx_expr to engine stack bytecode.
// $camera_origin (scene-view) + $ent_origin give a per-draw view vector; orbit
// the camera and dot3(viewdir, axis) sweeps -> the cosine triplet sweeps hue.
// A slow time() term keeps a shimmer alive even when nobody moves.
// ---------------------------------------------------------------------------
fn view_phase(speed: f64, drift: f64) -> String {
    // normalize($camera_origin - $ent_origin) projected onto a fixed axis, plus
    // a slow clock so the play-of-color never fully freezes. Inlined (the
    // compiler has no locals); the subtree is small.
    format!(
        "(dot3(normalize($camera_origin-$ent_origin),float3(2.6,1.5,0.4))*{speed}+time()*{drift})*3.14159265"
    )
}

// An iridescent float3 from a phase: three cosines 120deg apart = a hue wheel.
fn iridescent(phase: &str) -> String {
    format!(
        "float3(0.5+0.5*cos({phase}),0.5+0.5*cos(({phase})+2.0944),0.5+0.5*cos(({phase})+4.1888))"
    )
}

// The opal COLOUR is baked (spatial). The camera expressions only add a SUBTLE
// living shimmer on top -- v2's mistake was letting a uniform tint supply the
// colour, which floods the whole body one mood-ring hue. So every reactive tint
// here is a GENTLE wash: mostly neutral, blended only `amt` toward the
// iridescence so the baked play-of-color stays dominant.
fn gentle(irid: &str, neutral: &str, amt: f64) -> String {
    format!("lerp({neutral},{irid},{amt})")
}

// 1. Albedo tint (g_vColorTint1): near-neutral, a faint hue wash as you orbit,
//    leaning a touch crimson at low HP. Keeps the baked opal colours, adds life.
fn expr_color_tint() -> String {
    let shimmer = gentle(
        &iridescent(&view_phase(0.7, 0.18)),
        "float3(0.95,0.93,1.0)",
        0.22,
    );
    // blend toward a soft crimson wash only as health drops
    format!("lerp(float3(0.95,0.55,0.55),{shimmer},smoothstep(0.20,0.55,$ent_health))")
}

// 2. Fresnel rim (g_vSelfIllumFresnelMaskTint1): the tasteful camera hook -- the
//    grazing rim sheen shifts hue as you orbit. Moderate (rim only, small area).
fn expr_fresnel_tint() -> String {
    gentle(
        &iridescent(&view_phase(1.2, 0.22)),
        "float3(0.8,0.8,0.9)",
        0.6,
    )
}

// 3. Self-illum scale: LOW base (flecks get a soft fire, not a flood) + a gentle
//    surge as health drops. Also the required non-empty self-illum expression.
fn expr_selfillum_scale() -> &'static str {
    "1.1+(1.0-$ent_health)*1.6"
}

fn reactive_edits() -> anyhow::Result<Vec<VmatEdit>> {
    Ok(vec![
        VmatEdit::expr("g_vSelfIllumFresnelMaskTint1", &expr_fresnel_tint())?,
        VmatEdit::expr("g_vColorTint1", &expr_color_tint())?,
        VmatEdit::expr("g_flSelfIllumScale1", expr_selfillum_scale())?,
    ])
}

// ---------------------------------------------------------------------------
// Period-1 value noise (tiles seamlessly so the scroll wraps). Same family as
// the liquid-metal / trippy generators.
// ---------------------------------------------------------------------------
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

// Flowing opal height field: domain-warped low-freq blobs (the gem matrix) plus
// a couple of integer-frequency ripples (a directional current to the shimmer).
fn height(u: f32, v: f32) -> f32 {
    let wx = fbm(u, v, 2, 3);
    let wy = fbm(u + 3.3, v + 1.7, 2, 3);
    let warp = 0.45;
    let blob = fbm(u + warp * wx, v + warp * wy, 3, 4);
    let ripple = 0.26 * (TAU * (2.0 * u + v)).sin() + 0.18 * (TAU * (u + 2.0 * v + 0.3)).sin();
    blob * 0.78 + (ripple * 0.5 + 0.5) * 0.22
}

fn surface_normal(u: f32, v: f32, eps: f32, bump: f32) -> [f32; 3] {
    let hl = height(u - eps, v);
    let hr = height(u + eps, v);
    let hd = height(u, v - eps);
    let hu = height(u, v + eps);
    let nx = -(hr - hl) / (2.0 * eps) * bump;
    let ny = -(hu - hd) / (2.0 * eps) * bump;
    let nz = 1.0;
    let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
    [nx * inv, ny * inv, nz * inv]
}

// ---------------------------------------------------------------------------
// Black-opal albedo. A DARK gem matrix: the dynamic tint/Fresnel expressions
// supply the bright play-of-color, so the texture stays a deep, faintly violet
// substrate with cool veins on the wave crests (which then read as moving
// because the albedo scrolls). Kept mid-dark (not pure black) so g_vColorTint1
// has something to multiply and the body shows colour, not a void.
// ---------------------------------------------------------------------------
fn hsv2rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = (h.fract() + 1.0).fract() * 6.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

// The "fire" field: how bright the opal play-of-color is at (u,v). Varies the
// VALUE across the surface so valleys read as a dark hued gem matrix and crests
// blaze with colour -- broad coverage (real opal is fiery over most of the face,
// not two specks). Reused as the emissive mask so the glow tracks the fire.
fn fleck_mask(u: f32, v: f32) -> f32 {
    let h = height(u, v);
    let grain = fbm(u + 2.0, v + 5.0, 5, 3);
    smoothstep_f(0.30, 0.72, h * 0.65 + grain * 0.35)
}
fn smoothstep_f(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// Black-opal play-of-color. The KEY to the opal look: the colour is SPATIAL
// (many hues across the surface at once), not a single camera-driven tint. A
// low-frequency hue field paints coloured zones; the fire field sets how bright
// each is, so adjacent regions read different colours over a dark hued matrix =
// real opal. The camera expressions then only add a subtle living shimmer.
fn opal_pixel(u: f32, v: f32) -> [f32; 3] {
    // spatial hue field: smooth + low-frequency so colour forms broad zones
    let hue = fbm(u * 0.9 + 4.0, v * 0.9 + 7.0, 2, 3) * 1.35 + 0.5;
    let fire = fleck_mask(u, v);
    // value: dark hued matrix in the valleys -> bright fire on the crests.
    let val = 0.13 + 0.82 * fire;
    let sat = 0.62 + 0.28 * fire;
    let mut c = hsv2rgb(hue, sat, val);
    // hot near-white pinfire sparks on the very brightest grains
    let grain = fbm(u + 2.0, v + 5.0, 5, 3);
    let spark = smoothstep_f(0.82, 0.97, height(u, v) * 0.5 + grain * 0.5);
    for ci in c.iter_mut() {
        *ci += spark * (0.97 - *ci) * 0.55;
    }
    c
}

// Emissive mask = ONLY the fire flecks (not the whole body), and gentle. So the
// flecks carry a soft inner glow while the dark matrix stays dark. Single-channel
// ATI1N/BC4; set all RGBA the same and let replace_mip_chain re-encode BC4.
fn paint_emissive_mask(img: &mut Image) -> anyhow::Result<()> {
    let (w, h) = (img.width, img.height);
    let px = rgba8_mut(img)?;
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let m = EMISSIVE_LO + (EMISSIVE_HI - EMISSIVE_LO) * fleck_mask(u, v);
            let b = byte(m);
            let i = ((y * w + x) * 4) as usize;
            px[i] = b;
            px[i + 1] = b;
            px[i + 2] = b;
            px[i + 3] = b;
        }
    }
    Ok(())
}

// A library thumbnail that SHOWS the identity: a dark-opal gem mass with an
// iridescent Fresnel rim that sweeps the spectrum (the in-game play-of-color the
// flat albedo can't convey). 3 cosines 120deg apart = the same hue wheel the
// dynamic expressions emit, here mapped over the rim angle.
fn paint_thumbnail(img: &mut Image) -> anyhow::Result<()> {
    let (w, h) = (img.width, img.height);
    let px = rgba8_mut(img)?;
    let cx = w as f32 * 0.5;
    let cy = h as f32 * 0.5;
    let rmax = w.min(h) as f32 * 0.5;
    for y in 0..h {
        for x in 0..w {
            let dx = (x as f32 - cx) / rmax;
            let dy = (y as f32 - cy) / rmax;
            let r = (dx * dx + dy * dy).sqrt();
            let theta = dy.atan2(dx);
            // dark opal substrate with the same spatial play-of-color flecks
            let mut c = opal_pixel((x as f32 / w as f32) * 1.3, (y as f32 / h as f32) * 1.3);
            // iridescent Fresnel rim: brightest in a ring near the silhouette edge
            let rim = (-(r - 0.86).powi(2) / (2.0 * 0.10 * 0.10)).exp();
            // integer angular multiplier so the hue wheel is seamless at +/-pi
            let phase = theta * 2.0 + r * 3.0;
            let irid = [
                0.5 + 0.5 * (phase).cos(),
                0.5 + 0.5 * (phase + 2.0944).cos(),
                0.5 + 0.5 * (phase + 4.1888).cos(),
            ];
            for k in 0..3 {
                c[k] += rim * irid[k] * 1.15;
            }
            // soft vignette + circular mask so it reads as a gem, not a square
            let vignette = (1.0 - (r * 0.9).powi(3)).clamp(0.0, 1.0);
            let mask = (1.0 - ((r - 1.0) / 0.06)).clamp(0.0, 1.0);
            let i = ((y * w + x) * 4) as usize;
            px[i] = byte(c[0] * vignette * mask);
            px[i + 1] = byte(c[1] * vignette * mask);
            px[i + 2] = byte(c[2] * vignette * mask);
            px[i + 3] = 255;
        }
    }
    Ok(())
}

fn srgb_encode(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn rgba8_mut(img: &mut Image) -> anyhow::Result<&mut Vec<u8>> {
    match &mut img.data {
        ImageData::Rgba8(v) => Ok(v),
        ImageData::Rgba16F(_) => anyhow::bail!("unexpected HDR texture"),
    }
}

fn paint_albedo(img: &mut Image, smooth: bool) -> anyhow::Result<()> {
    let (w, h) = (img.width, img.height);
    let px = rgba8_mut(img)?;
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            // The gun mesh has collapsed-UV patches; sample a smoothed/rotated
            // coordinate there so they don't block up (the liquid-metal lesson),
            // but keep the same spatial opal colour.
            let (su, sv) = if smooth {
                (((u * 1.3 + v * 0.2).fract()), ((v * 1.3 + u * 0.2).fract()))
            } else {
                (u, v)
            };
            let rgb = opal_pixel(su, sv);
            let i = ((y * w + x) * 4) as usize;
            px[i] = byte(rgb[0]);
            px[i + 1] = byte(rgb[1]);
            px[i + 2] = byte(rgb[2]);
        }
    }
    Ok(())
}

// Packed normal-roughness: R,G = tangent normal.xy remapped, B = roughness,
// A = 255. The slot is sampled LINEAR, so store linear directly (--gamma-precomp
// flips to inverse-sRGB if an in-game test shows a constant surface tilt).
fn paint_normal_roughness(img: &mut Image, gamma_precomp: bool) -> anyhow::Result<()> {
    let (w, h) = (img.width, img.height);
    let eps = 2.0 / w as f32;
    let bump = 0.05;
    let px = rgba8_mut(img)?;
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let n = surface_normal(u, v, eps, bump);
            let nrx = n[0] * 0.5 + 0.5;
            let nry = n[1] * 0.5 + 0.5;
            let slope = (1.0 - n[2]).clamp(0.0, 1.0);
            let rough = (ROUGHNESS_LOW as f32 + 0.14 * slope).clamp(0.0, 0.6);
            let (rr, gg, bb) = if gamma_precomp {
                (srgb_encode(nrx), srgb_encode(nry), srgb_encode(rough))
            } else {
                (nrx, nry, rough)
            };
            let i = ((y * w + x) * 4) as usize;
            px[i] = byte(rr);
            px[i + 1] = byte(gg);
            px[i + 2] = byte(bb);
            px[i + 3] = 255;
        }
    }
    Ok(())
}

// ---- in-place vmat double edits (NOT a re-encode; preserves material framing) ----
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

// Only set X,Y (leave Z,W untouched in case they are tagless defaults).
fn scroll_xy_edits(v: &Value, name: &str, xy: [f64; 2]) -> Vec<(Vec<Seg>, f64)> {
    vcomp_edits(v, name, &[(0, xy[0]), (1, xy[1])])
}

fn rough_low_edits(v: &Value) -> Vec<(Vec<Seg>, f64)> {
    vcomp_edits(
        v,
        "TextureRoughness1",
        &[(0, ROUGHNESS_LOW), (1, ROUGHNESS_LOW), (2, ROUGHNESS_LOW)],
    )
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

fn patch_optional(bytes: Vec<u8>, edits: &[(Vec<Seg>, f64)]) -> (Vec<u8>, usize) {
    if edits.is_empty() {
        return (bytes, 0);
    }
    match morphic::patch_kv3_resource_doubles(&bytes, edits) {
        Ok(b) => (b, edits.len()),
        Err(_) => (bytes, 0),
    }
}

// Apply all the in-place double edits the body wants (scroll + gloss + Fresnel
// broaden), each best-effort so a renamed param can't sink the bake.
fn body_double_patches(bytes: &[u8]) -> anyhow::Result<(Vec<u8>, usize)> {
    let v = morphic::decode_kv3_resource(bytes)?;
    let mut all = Vec::new();
    all.extend(scroll_xy_edits(
        &v,
        "g_vNormalAndRoughnessScrollSpeed1",
        FLOW_NORMAL,
    ));
    all.extend(scroll_xy_edits(&v, "g_vAlbedoScrollSpeed1", FLOW_ALBEDO));
    all.extend(scroll_xy_edits(
        &v,
        "g_vSelfIllumScrollSpeed1",
        FLOW_SELFILLUM,
    ));
    all.extend(rough_low_edits(&v));
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
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut gamma_precomp = false;
    let mut probe = false;
    let mut pos: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--gamma-precomp" => {
                gamma_precomp = true;
                i += 1;
            }
            "--probe" => {
                probe = true;
                i += 1;
            }
            _ => {
                pos.push(raw[i].clone());
                i += 1;
            }
        }
    }
    let pak = pos.first().cloned().expect(
        "usage: reskin_chrono_living_opal <pak01_dir.vpk> <out_dir.vpk|--png prefix> \
         [--probe] [--gamma-precomp]",
    );
    let arg2 = pos
        .get(1)
        .cloned()
        .expect("second arg: <out_dir.vpk> or --png <prefix>");

    eprintln!("Living Opal (Paradox/chrono)  probe={probe}  gamma_precomp={gamma_precomp}");
    eprintln!("reactive expressions:");
    eprintln!(
        "  fresnel rim  g_vSelfIllumFresnelMaskTint1 = {}",
        expr_fresnel_tint()
    );
    eprintln!(
        "  body tint    g_vColorTint1                = {}",
        expr_color_tint()
    );
    eprintln!(
        "  selfillum    g_flSelfIllumScale1          = {}",
        expr_selfillum_scale()
    );

    // --- preview mode: render the opal albedo + ripple normal, no game needed.
    if arg2 == "--png" {
        let prefix = pos.get(2).cloned().expect("--png needs an output prefix");
        let blank = || Image {
            width: 768,
            height: 768,
            data: ImageData::Rgba8(vec![255u8; 768 * 768 * 4]),
        };
        let mut pa = blank();
        paint_albedo(&mut pa, false)?;
        let mut pn = blank();
        paint_normal_roughness(&mut pn, gamma_precomp)?;
        let mut pt = Image {
            width: 512,
            height: 512,
            data: ImageData::Rgba8(vec![255u8; 512 * 512 * 4]),
        };
        paint_thumbnail(&mut pt)?;
        for (img, suffix) in [
            (&pa, "opal_albedo"),
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

    // --- probe mode: ONLY the three expressions, on the STOCK chrono body
    //     material. Smallest possible in-game test of whether a multi-expression
    //     LZ4-native container loads on this hero (the one unproven risk).
    if probe {
        let body_vmat_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_VMAT)?;
        let (patched, stats) = patch_vmat_params(&body_vmat_bytes, &reactive_edits()?)?;
        report_stats("body expressions", &stats);
        anyhow::ensure!(
            stats.failed.is_empty(),
            "an expression failed to inject -- aborting probe"
        );
        let readme = b"Living Opal PROBE -- chrono body, expressions ONLY (stock textures).\n\
            In-game test: orbit the camera -> the Fresnel rim should sweep hue;\n\
            take damage -> the body should flush crimson and glow harder.\n\
            If the body renders red wireframe / error shader, the multi-expression\n\
            container did not load on chrono -- fall back to one expression.\n";
        vpkmerge_core::pack(
            &[
                (BODY_VMAT, patched.as_slice()),
                ("README.txt", readme.as_slice()),
            ],
            &out,
        )?;
        println!("wrote probe VPK: {out}");
        return Ok(());
    }

    // --- full bake -------------------------------------------------------------
    // The body's 4096 BC7 container is the template for every texture override.
    let body_color_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_COLOR)?;

    let mut body_albedo = morphic::decode(&body_color_bytes)?;
    paint_albedo(&mut body_albedo, false)?;
    let new_body_color = morphic::replace_mip_chain(&body_color_bytes, &body_albedo)?;

    let mut body_nr = morphic::decode(&body_color_bytes)?;
    paint_normal_roughness(&mut body_nr, gamma_precomp)?;
    let new_body_nr = morphic::replace_mip_chain(&body_color_bytes, &body_nr)?;

    // Emissive mask override: use the emissive texture's OWN container (4096 ATI1N)
    // so dims/format match (replace_mip_chain cannot resize/reformat).
    let emissive_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_EMISSIVE)?;
    let mut body_emissive = morphic::decode(&emissive_bytes)?;
    paint_emissive_mask(&mut body_emissive)?;
    let new_body_emissive = morphic::replace_mip_chain(&emissive_bytes, &body_emissive)?;
    eprintln!("emissive mask overridden (whole body now self-illum-eligible)");

    let mut gun_albedo = morphic::decode(&body_color_bytes)?;
    paint_albedo(&mut gun_albedo, true)?;
    let new_gun_color = morphic::replace_mip_chain(&body_color_bytes, &gun_albedo)?;
    let mut gun_nr = morphic::decode(&body_color_bytes)?;
    paint_normal_roughness(&mut gun_nr, gamma_precomp)?;
    let new_gun_nr = morphic::replace_mip_chain(&body_color_bytes, &gun_nr)?;
    eprintln!("textures: body + gun opal albedo + ripple normal-roughness re-encoded");

    // body vmat: double-patches first (preserve framing), then inject expressions.
    let body_vmat_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_VMAT)?;
    let (body_doubled, n_doubles) = body_double_patches(&body_vmat_bytes)?;
    eprintln!("body vmat: {n_doubles} in-place double edits (scroll + gloss + Fresnel)");
    let (new_body_vmat, body_stats) = patch_vmat_params(&body_doubled, &reactive_edits()?)?;
    report_stats("body expressions", &body_stats);
    anyhow::ensure!(
        body_stats.failed.is_empty(),
        "a body expression failed to inject -- aborting"
    );

    // gun vmat: no native self-illum, so no expressions -- just flow + gloss.
    let gun_vmat_bytes = vpkmerge_core::read_vpk_entry(&pak, GUN_VMAT)?;
    let gv = morphic::decode_kv3_resource(&gun_vmat_bytes)?;
    let mut gun_edits = scroll_xy_edits(&gv, "g_vNormalAndRoughnessScrollSpeed1", FLOW_NORMAL);
    gun_edits.extend(scroll_xy_edits(&gv, "g_vAlbedoScrollSpeed1", FLOW_ALBEDO));
    gun_edits.extend(rough_low_edits(&gv));
    let (new_gun_vmat, n_gun) = patch_optional(gun_vmat_bytes.clone(), &gun_edits);
    eprintln!("gun vmat: {n_gun} in-place double edits (flow + gloss)");

    let readme = format!(
        "Paradox \"Living Opal\" -- reactive magnum-opus skin\n\
        ================================================\n\
        vpkmerge test build. Hero: Paradox (chrono).\n\n\
        Black-opal substrate albedo + flowing ripple normal (surface crawls) +\n\
        slow scroll + gem-gloss roughness, with THREE dynamic expressions:\n\
          1. Fresnel rim tint  -> sweeps hue as you ORBIT the camera (real\n\
             thin-film play-of-color via $camera_origin/$ent_origin).\n\
          2. Body color tint   -> phase-offset iridescence, flushes crimson at\n\
             low $ent_health.\n\
          3. Self-illum scale  -> base glow + low-HP surge.\n\n\
        In-game checks: orbit -> rim hue sweeps; take damage -> crimson flush +\n\
        glow surge. gamma_precomp={gamma_precomp}.\n"
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
            ("README.txt", readme.as_bytes()),
        ],
        &out,
    )?;
    println!("wrote addon VPK: {out}");
    Ok(())
}
