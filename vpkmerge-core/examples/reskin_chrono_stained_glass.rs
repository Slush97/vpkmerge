// Paradox "Stained Glass" -- a leaded cathedral-window skin, and the FIRST
// reskin builder driven by the native UV-mask tool (`vpkmerge model mask`)
// instead of the AO-contrast heuristic.
//
// THE MASK IS LOAD-BEARING. chrono's body and headbase mesh parts share ONE
// material (chrono_v2.vmat) and ONE texture (chrono_v2_color). The AO heuristic
// (and any image-space trick) cannot tell the face from the torso when they live
// in the same texture -- there is no per-region signal in the pixels. The part
// mask CAN: it is union-find over the mesh's own UV index graph, so it knows
// exactly which texels belong to the `body` part vs the `headbase` part. We bake
// both masks in-process (the same `morphic::model::{segments, mask_png}` the CLI
// calls) and use them to:
//   1. paint the torso/coat as jewel-toned LEADED GLASS (saturated Voronoi panes
//      separated by near-black came lines, backlit so the glass glows), and
//   2. paint the headbase (face/neck base) as pale CLEAR/FROSTED glass so the
//      face reads distinct from the colored body -- only possible because the
//      mask separates two parts that share the texture, and
//   3. lead-black the DEAD texels (no part covers them) so any UV-border mip
//      bleed reads as came, never as a stray colored fringe (stained glass wants
//      black borders -- the mask's true-coverage edge is exactly right here).
//
// Everything else follows the proven chrono pipeline: full-res texture overrides
// via replace_mip_chain (the body's 4096 BC7 container is the template), glassy
// low roughness in the panes, and ONE dynamic self-illum expression (a gentle
// candle-flicker on the backlight) injected with the blob-aware LZ4-native
// `patch_vmat_params`. No KV3 re-encode of any .vmat_c (that renders the engine
// error shader on hero materials), and no scroll -- a window holds still.
//
// usage:
//   # preview the art, no game needed (writes <prefix>_*.png):
//   cargo run --release --example reskin_chrono_stained_glass -- <pak01_dir.vpk> --png <prefix>
//   # full addon bake:
//   cargo run --release --example reskin_chrono_stained_glass -- <pak01_dir.vpk> <out_dir.vpk>
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

// The hero codename whose part masks define the body/headbase regions.
const HERO: &str = "chrono";

// Stained-glass tuning (cathedral, not candy) ------------------------------
const PANE_GRID: i32 = 15; // big leaded panes across the body texture
const MICRO_GRID: i32 = 46; // fine crackle/seedy-glass within each pane
const LEAD_WIDTH: f32 = 0.15; // came thickness (fraction of cell radius) -- BOLD
const PANE_ROUGH: f32 = 0.07; // glass: wet, near-mirror specular
const LEAD_ROUGH: f32 = 0.55; // came/lead: matte dark metal
const GLASS_VALUE: f32 = 0.74; // deep, rich glass body (was a candy-bright 0.92)
const CAME_RELIEF: f32 = 0.85; // raised lead cames in the normal map (panes stay FLAT)
                               // Backlight: panes glow (self-illum samples albedo), came stays dark. Calmer than
                               // the candy build so the glass reads as lit, not fullbright plastic.
const GLOW_PANE: f32 = 0.55;
const GLOW_LEAD: f32 = 0.0;
const GLOW_CLEAR: f32 = 0.16; // clear-glass face glows faintly

// ---------------------------------------------------------------------------
// Period-1 value noise + periodic Voronoi (tiles seamlessly so mips stay clean).
// Same generator family as the other chrono reskins.
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

fn cell_hash(cx: i32, cy: i32, salt: i64) -> f32 {
    hash2(cx as i64 + salt * 7919, cy as i64 + salt * 104_729)
}

// Periodic Voronoi over a grid x grid torus. Returns (f1, f2, winning cell xy).
fn voronoi(u: f32, v: f32, grid: i32) -> (f32, f32, i32, i32) {
    let g = grid as f32;
    let gx = (u * g).floor() as i32;
    let gy = (v * g).floor() as i32;
    let fx = u * g - gx as f32;
    let fy = v * g - gy as f32;
    let (mut f1, mut f2) = (9.0f32, 9.0f32);
    let (mut cwx, mut cwy) = (0, 0);
    for oy in -1..=1 {
        for ox in -1..=1 {
            let (wx, wy) = ((gx + ox).rem_euclid(grid), (gy + oy).rem_euclid(grid));
            let px = ox as f32 + cell_hash(wx, wy, 1) - fx;
            let py = oy as f32 + cell_hash(wx, wy, 2) - fy;
            let d = (px * px + py * py).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
                cwx = wx;
                cwy = wy;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2, cwx, cwy)
}

fn smoothstep_f(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

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

// ---------------------------------------------------------------------------
// One stained-glass sample. `clear` morphs the pane from a saturated jewel to
// pale frosted glass (the masked headbase/face treatment).
// ---------------------------------------------------------------------------
struct GlassSample {
    rgb: [f32; 3],
    glow: f32,
    lead: f32,   // 1 on a came line, 0 in a pane interior
    height: f32, // surface relief: raised on the cames, flat across the glass
}

fn glass_sample(u: f32, v: f32, clear: f32) -> GlassSample {
    let (f1, f2, cwx, cwy) = voronoi(u, v, PANE_GRID);
    // came lines live where the two nearest seeds are equidistant.
    let lead_c = 1.0 - smoothstep_f(0.0, LEAD_WIDTH, f2 - f1);
    // a fine micro-fracture so big panes read as hand-blown seedy glass.
    let (g1, g2, _, _) = voronoi(u + 0.17, v + 0.41, MICRO_GRID);
    let lead_f = 1.0 - smoothstep_f(0.0, 0.018, g2 - g1);
    let lead = lead_c.max(lead_f * 0.35).min(1.0);

    // per-pane hue snapped to a curated CATHEDRAL palette (ruby, amber, gold,
    // emerald, teal, sapphire, royal, violet, rose) so adjacent panes harmonize
    // instead of clashing across the full neon wheel. A little intra-pane drift
    // keeps each pane from reading as flat poster paint.
    const JEWEL: [f32; 9] = [0.99, 0.07, 0.13, 0.34, 0.49, 0.60, 0.67, 0.78, 0.90];
    let pick = (cell_hash(cwx, cwy, 8) * JEWEL.len() as f32) as usize % JEWEL.len();
    let hue = (JEWEL[pick] + fbm(u * 1.3 + 2.0, v * 1.3 + 6.0, 3, 2) * 0.025 - 0.0125).fract();
    let drift = fbm(u * 4.0, v * 4.0, 5, 2);
    // light pools toward the pane centre (thin glass at the middle of a blown
    // pane), so each pane has a luminous heart. Deep, saturated jewel body.
    // light pools gently toward the pane centre, but the body stays deep and
    // saturated (cathedral glass is rich and a little dark, not fullbright).
    let centre = 1.0 - smoothstep_f(0.0, 0.55, f1);
    let val = (GLASS_VALUE * (0.62 + 0.30 * centre) + 0.04 * drift).min(1.0);
    let sat = 0.9 + 0.1 * centre;
    let jewel = hsv2rgb(hue, sat, val);

    // clear/frosted glass: low saturation, cool pale tint, still luminous.
    let frost_v = (0.82 + 0.1 * centre).min(1.0);
    let frosted = [frost_v * 0.97, frost_v * 0.99, frost_v];

    let mut rgb = [
        jewel[0] + (frosted[0] - jewel[0]) * clear,
        jewel[1] + (frosted[1] - jewel[1]) * clear,
        jewel[2] + (frosted[2] - jewel[2]) * clear,
    ];
    // collapse to near-black OPAQUE lead where the cames run (the defining line
    // of stained glass -- must read black even under the self-illum backlight).
    let lead_rgb = 0.02;
    for c in &mut rgb {
        *c = lead_rgb + (*c - lead_rgb) * (1.0 - lead);
    }

    let pane_glow = GLOW_PANE + (GLOW_CLEAR - GLOW_PANE) * clear;
    let glow = (pane_glow * (1.0 - lead) * (0.6 + 0.4 * centre) + GLOW_LEAD * 0.0).max(0.0);
    GlassSample {
        rgb,
        glow,
        lead,
        // raised lead came, flat glass: the normal map derives from this so the
        // cames read as proud H-profile leading and the panes stay flat sheets.
        height: lead,
    }
}

// ---------------------------------------------------------------------------
// Region mask: a baked white-on-black UV mask sampled as a 0..1 selector.
// ---------------------------------------------------------------------------
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

// Bake the body + headbase part masks IN PROCESS, exactly as `vpkmerge model
// mask --by part --select <id>` would, and return them plus a coverage report.
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

fn byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn rgba8_mut(img: &mut Image) -> anyhow::Result<&mut Vec<u8>> {
    match &mut img.data {
        ImageData::Rgba8(v) => Ok(v),
        ImageData::Rgba16F(_) => anyhow::bail!("unexpected HDR texture"),
    }
}

// Albedo: leaded jewel glass over the body region, frosted clear glass over the
// headbase region, near-black lead over dead texels (so mip bleed reads as came).
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
            // gun UVs collapse onto small patches; smooth/rotate the sampling
            // coordinate there so the panes don't block up (the chrono lesson).
            let (su, sv) = if smooth {
                ((u * 1.25 + v * 0.2).fract(), (v * 1.25 + u * 0.2).fract())
            } else {
                (u, v)
            };
            let (clear, live) = match masks {
                Some((body, head)) => {
                    let h = head.at(u, v);
                    let b = body.at(u, v).max(h);
                    (h, b)
                }
                None => (0.0, 1.0),
            };
            let s = glass_sample(su, sv, clear);
            // dead texels (outside every part) collapse to lead so a UV-edge
            // bleed never paints a stray colored fringe on the silhouette.
            let lead_rgb = [0.04, 0.04, 0.05];
            let i = ((y * w + x) * 4) as usize;
            for k in 0..3 {
                px[i + k] = byte(lead_rgb[k] + (s.rgb[k] - lead_rgb[k]) * live);
            }
        }
    }
    Ok(())
}

// Emissive (self-illum) mask: panes glow as if backlit, cames stay dark, dead
// texels dark. chrono routes self-illum through the albedo, so this lights the
// glass colors from within = a lit cathedral window.
fn paint_emissive(img: &mut Image, masks: Option<(&Mask, &Mask)>) -> anyhow::Result<()> {
    let (w, h) = (img.width, img.height);
    let px = rgba8_mut(img)?;
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let (clear, live) = match masks {
                Some((body, head)) => {
                    let hh = head.at(u, v);
                    (hh, body.at(u, v).max(hh))
                }
                None => (0.0, 1.0),
            };
            let g = glass_sample(u, v, clear).glow * live;
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

// Packed normal-roughness: FLAT glass-smooth panes with RAISED matte lead cames
// (the H-profile leading sits proud of the glass). R,G = tangent normal.xy
// remapped, B = roughness, A = 255.
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
            let (su, sv) = if smooth {
                ((u * 1.25 + v * 0.2).fract(), (v * 1.25 + u * 0.2).fract())
            } else {
                (u, v)
            };
            let clear = masks.map_or(0.0, |(_, head)| head.at(u, v));
            // raised-came relief from the lead-height field's gradient (flat in
            // the pane interiors, a proud ridge along each came).
            let hl = glass_sample(su - eps, sv, clear).height;
            let hr = glass_sample(su + eps, sv, clear).height;
            let hd = glass_sample(su, sv - eps, clear).height;
            let hu = glass_sample(su, sv + eps, clear).height;
            let s = glass_sample(su, sv, clear);
            let nx = -(hr - hl) / (2.0 * eps) * CAME_RELIEF;
            let ny = -(hu - hd) / (2.0 * eps) * CAME_RELIEF;
            let inv = 1.0 / (nx * nx + ny * ny + 1.0).sqrt();
            // roughness: glass-smooth in panes, matte on the lead cames.
            let rough = (PANE_ROUGH + (LEAD_ROUGH - PANE_ROUGH) * s.lead).clamp(0.04, 0.6);
            let i = ((y * w + x) * 4) as usize;
            px[i] = byte(nx * inv * 0.5 + 0.5);
            px[i + 1] = byte(ny * inv * 0.5 + 0.5);
            px[i + 2] = byte(rough);
            px[i + 3] = 255;
        }
    }
    Ok(())
}

// A library thumbnail: a rose window. Radial leaded panes around a bright heart,
// jewel-toned and backlit, on a dark cathedral ground.
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
            // radial tracery: petals around the centre + concentric rings.
            let petals = 12.0;
            let pane_u = (theta / TAU * petals).fract();
            let ring = (r * 4.0).fract();
            let lead = (1.0 - smoothstep_f(0.0, 0.14, (pane_u - 0.5).abs()))
                .max(1.0 - smoothstep_f(0.0, 0.14, (ring - 0.5).abs()))
                .min(1.0);
            let seg = (theta / TAU * petals).floor() as i32;
            let band = (r * 4.0).floor() as i32;
            const JEWEL: [f32; 9] = [0.99, 0.07, 0.13, 0.34, 0.49, 0.60, 0.67, 0.78, 0.90];
            let pick = (cell_hash(seg, band, 8) * JEWEL.len() as f32) as usize % JEWEL.len();
            let hue = JEWEL[pick];
            let centre = (1.0 - smoothstep_f(0.0, 0.16, r)).max(0.0);
            let val = (0.86 * (0.62 + 0.38 * (1.0 - ring))).min(1.0);
            let mut c = hsv2rgb(hue, 0.92, val);
            // bright heart
            for k in 0..3 {
                c[k] += centre * (1.0 - c[k]) * 0.8;
            }
            // bold near-black lead cames
            for k in 0..3 {
                c[k] = 0.02 + (c[k] - 0.02) * (1.0 - lead);
            }
            // circular vignette mask
            let mask = (1.0 - ((r - 1.0) / 0.05)).clamp(0.0, 1.0);
            let i = ((y * w + x) * 4) as usize;
            px[i] = byte(c[0] * mask);
            px[i + 1] = byte(c[1] * mask);
            px[i + 2] = byte(c[2] * mask);
            px[i + 3] = 255;
        }
    }
    Ok(())
}

// ---- in-place vmat double edits (NOT a re-encode; preserves material framing) ----
fn vparam_index(v: &morphic::kv3::Value, name: &str) -> Option<usize> {
    v.get("m_vectorParams")?
        .as_array()?
        .iter()
        .position(|p| p.get("m_name").and_then(morphic::kv3::Value::as_str) == Some(name))
}

fn vcomp_edits(
    v: &morphic::kv3::Value,
    name: &str,
    comps: &[(usize, f64)],
) -> Vec<(Vec<morphic::kv3::Seg>, f64)> {
    use morphic::kv3::Seg;
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

fn rough_low_edits(v: &morphic::kv3::Value) -> Vec<(Vec<morphic::kv3::Seg>, f64)> {
    vcomp_edits(
        v,
        "TextureRoughness1",
        &[
            (0, PANE_ROUGH as f64),
            (1, PANE_ROUGH as f64),
            (2, PANE_ROUGH as f64),
        ],
    )
}

fn patch_optional(bytes: Vec<u8>, edits: &[(Vec<morphic::kv3::Seg>, f64)]) -> (Vec<u8>, usize) {
    if edits.is_empty() {
        return (bytes, 0);
    }
    match morphic::patch_kv3_resource_doubles(&bytes, edits) {
        Ok(b) => (b, edits.len()),
        Err(_) => (bytes, 0),
    }
}

// The self-illum backlight: a steady glow with a faint candle-flicker so the
// window reads as lit, not painted. ONE expression -- and it is itself a
// self-illum expression, satisfying the "a self-illum expr must be present for
// any expression to evaluate" gotcha. Proven container on chrono.
fn selfillum_expr() -> &'static str {
    "1.25+0.16*sin(time()*1.7)+0.08*sin(time()*4.3)"
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
    let pak = pos
        .first()
        .cloned()
        .expect("usage: reskin_chrono_stained_glass <pak01_dir.vpk> <out_dir.vpk|--png prefix>");
    let arg2 = pos
        .get(1)
        .cloned()
        .expect("second arg: <out_dir.vpk> or --png <prefix>");

    eprintln!("Paradox \"Stained Glass\" (chrono) -- UV-mask-driven reskin");
    eprintln!("  self-illum backlight = {}", selfillum_expr());

    // --- preview mode: render the art at 768 with masks, no game needed.
    if arg2 == "--png" {
        let prefix = pos.get(2).cloned().expect("--png needs an output prefix");
        let (body_mask, head_mask) = bake_part_masks(&pak, 1024)?;
        let masks = Some((&body_mask, &head_mask));
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

    // --- full bake ------------------------------------------------------------
    // Mask at the body texture's resolution so the part edges land on real texels.
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
    eprintln!("textures: body albedo + emissive + normal-roughness re-encoded (mask-confined)");

    // Gun: no mask (its UVs don't share the body texture); paint with the smooth
    // coordinate so the panes stay coherent across the collapsed gun patches.
    let mut gun_albedo = morphic::decode(&body_color_bytes)?;
    paint_albedo(&mut gun_albedo, None, true)?;
    let new_gun_color = morphic::replace_mip_chain(&body_color_bytes, &gun_albedo)?;
    let mut gun_nr = morphic::decode(&body_color_bytes)?;
    paint_normal_roughness(&mut gun_nr, None, true)?;
    let new_gun_nr = morphic::replace_mip_chain(&body_color_bytes, &gun_nr)?;
    eprintln!("gun: stained-glass albedo + normal-roughness re-encoded");

    // body vmat: glassy roughness fallback (in-place double) + the backlight
    // self-illum expression (blob-aware insert). No scroll: a window holds still.
    let body_vmat_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_VMAT)?;
    let bv = morphic::decode_kv3_resource(&body_vmat_bytes)?;
    let (body_doubled, n_doubles) = patch_optional(body_vmat_bytes.clone(), &rough_low_edits(&bv));
    eprintln!("body vmat: {n_doubles} in-place roughness edit(s)");
    let (new_body_vmat, body_stats) = patch_vmat_params(
        &body_doubled,
        &[VmatEdit::expr("g_flSelfIllumScale1", selfillum_expr())?],
    )?;
    report_stats("body backlight expression", &body_stats);
    anyhow::ensure!(
        body_stats.failed.is_empty(),
        "the body self-illum expression failed to inject -- aborting"
    );

    // gun vmat: glassy roughness only (no native self-illum to drive).
    let gun_vmat_bytes = vpkmerge_core::read_vpk_entry(&pak, GUN_VMAT)?;
    let gv = morphic::decode_kv3_resource(&gun_vmat_bytes)?;
    let (new_gun_vmat, n_gun) = patch_optional(gun_vmat_bytes.clone(), &rough_low_edits(&gv));
    eprintln!("gun vmat: {n_gun} in-place roughness edit(s)");

    // Head glass dome: a deep cathedral-glass tint + matching solid outline so the
    // dome reads as leaded glass too (best-effort in-place doubles).
    let headglass_bytes = vpkmerge_core::read_vpk_entry(&pak, HEADGLASS_VMAT)?;
    let hgl = morphic::decode_kv3_resource(&headglass_bytes)?;
    let mut dome = vcomp_edits(&hgl, "TextureColor1", &[(0, 0.05), (1, 0.07), (2, 0.26)]);
    dome.extend(vcomp_edits(
        &hgl,
        "g_vSolidOutlineTint",
        &[(0, 0.16), (1, 0.05), (2, 0.02)],
    ));
    let (new_headglass_vmat, n_dome) = patch_optional(headglass_bytes.clone(), &dome);
    eprintln!("head glass dome: {n_dome} cathedral-glass tint/outline edit(s)");

    // Hourglass: a warm amber self-illum tint (best-effort), matching the lit-glass
    // identity. If the param isn't present the override is simply inert.
    let hourglass_bytes = vpkmerge_core::read_vpk_entry(&pak, HOURGLASS_VMAT)?;
    let hg = morphic::decode_kv3_resource(&hourglass_bytes)?;
    let amber = vcomp_edits(&hg, "g_vSelfIllumTint1", &[(0, 1.0), (1, 0.62), (2, 0.18)]);
    let (new_hourglass_vmat, n_hg) = patch_optional(hourglass_bytes.clone(), &amber);
    eprintln!("hourglass: {n_hg} amber self-illum tint edit(s)");

    let readme = "Paradox \"Stained Glass\" -- UV-mask-driven reskin\n\
        ================================================\n\
        vpkmerge test build. Hero: Paradox (chrono).\n\n\
        FIRST reskin built on the native UV-mask tool (vpkmerge model mask). The\n\
        body and headbase mesh parts share one material/texture, so the part mask\n\
        is the ONLY way to tell the face from the torso in image space:\n\
          * body region   -> jewel-toned LEADED GLASS (Voronoi panes + came lines),\n\
                             backlit so the colors glow.\n\
          * headbase/face -> pale CLEAR/FROSTED glass (reads distinct from body).\n\
          * dead texels   -> lead-black (UV-edge mip bleed reads as came).\n\n\
        Glassy low roughness, a steady candle-flicker self-illum backlight\n\
        (one dynamic expression), and a cathedral-glass head dome. Stays on the\n\
        toon lighting path; no KV3 re-encode of any .vmat_c.\n";

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
            ("README.txt", readme.as_bytes()),
        ],
        &out,
    )?;
    println!("wrote addon VPK: {out}");
    Ok(())
}
