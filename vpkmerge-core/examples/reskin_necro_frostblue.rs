//! Graves ("necro") "Frostbite" -- light blue reskin of the Graves Shirt addon,
//! with the back satchel removed.
//!
//! The Graves Shirt skin paints its outfit as a near-monochrome charcoal: every
//! garment albedo sits in a narrow luminance band (shirt L p2..p90 = 0.09..0.18),
//! with red plaid pinstripes and boot laces as the only saturated accents. A hue
//! *set* (`vpkmerge texture --hue`) does nothing to that art, because setting the
//! hue of a pixel whose saturation is ~0 leaves it grey. So this reskin uses a
//! **luminance gradient map** instead: each garment's own luminance is stretched
//! out of its narrow band and mapped onto a cold ramp, which recolors *and*
//! lifts the value so the result actually reads as light blue in game while
//! keeping every painted fold, seam and emblem.
//!
//! Region separation matters because the garments share texture maps: the skirt,
//! boots and tights all sample `necro_lower_basecolor`, and the shirt and sleeves
//! split `necro_upperblack_basecolor` / `necro_upper_basecolor`. The tights get
//! their own ramp via a UV mask baked in-process from the `pantythose` material
//! (the same `morphic::model::{segments, mask_png}` the `vpkmerge model mask` CLI
//! uses), so the legs stay a cooler blue-grey than the skirt. The boots share
//! both the skirt's material and its luminance band, so no mask or ramp can
//! separate them; they are selected by model-space height instead.
//!
//! Skin is protected without a mask: the fabric never exceeds L ~= 0.23, so a
//! texel that is both brighter than that *and* warm-hued fades out of the
//! recolor, and her neck, chest and the skin through the tights' rips keep their
//! pale tone. The hue half of that test matters: brightness alone would also
//! spare the acid-green badge on her collar and the flame baked into the hand's
//! albedo, leaving islands of the old scheme behind.
//!
//! The green magic is retinted to cyan/ice from both directions: the shader
//! tints (hand flame, jar glow, and the green NPR outline on her face) through
//! `patch_vmat_params`, and the yellow-green baked into the albedos through a
//! second accent rule, so nothing is left over. No `.vmat_c` is re-encoded (that
//! renders red wireframe on blob-bearing materials); every edit is an in-place
//! param patch.
//!
//! The satchel is dropped with `remove_draw_calls_by_material`, which zeroes the
//! matching draw calls' index counts in place: no vertices move, the rig, physics
//! and every other draw call are untouched.
//!
//! Usage:
//!   cargo run --release -p vpkmerge-core --example reskin_necro_frostblue -- \
//!     <graves-shirt_dir.vpk> <out_dir.vpk>
//!   cargo run --release -p vpkmerge-core --example reskin_necro_frostblue -- \
//!     <graves-shirt_dir.vpk> --png <prefix>          # art preview, no game needed

use morphic::model::SegmentBy;
use morphic::{Image, ImageData, TextureFormat};
use vpkmerge_core::{patch_vmat_params, VmatEdit};

const MODEL: &str = "models/heroes_wip/necro/necro.vmdl_c";
const BAG_MATERIAL: &str = "necro_bag";

const DIR: &str = "models/heroes_wip/necro/materials";
const SHIRT_COLOR: &str =
    "models/heroes_wip/necro/materials/graves_new/necro_upperblack_basecolor_png_d00ed8c7.vtex_c";
const SLEEVE_COLOR: &str =
    "models/heroes_wip/necro/materials/graves_new/necro_upper_basecolor_png_3f7d0eb5.vtex_c";
const LOWER_COLOR: &str =
    "models/heroes_wip/necro/materials/graves_new/necro_lower_basecolor_png_ab3dcf3d.vtex_c";
const HAND_COLOR: &str =
    "models/heroes_wip/necro/materials/graves_new/necro_hand_basecolor_png_19300010.vtex_c";

//  ---------------------------------------------------------------------------
//  Palette. Each ramp is sampled by the source texel's own normalized luminance,
//  so painted detail survives; the stop colors are the design.

type Stop = (f32, [f32; 3]);

fn rgb(hex: u32) -> [f32; 3] {
    [
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
    ]
}

//  Shirt: pale powder blue body with a deep navy shadow and a near-white
//  highlight so folds still catch light.
fn shirt_ramp() -> Vec<Stop> {
    vec![
        (0.00, rgb(0x2C3D52)),
        (0.30, rgb(0x6A8EB4)),
        (0.55, rgb(0xA8C8E8)),
        (0.80, rgb(0xD2E6F7)),
        (1.00, rgb(0xFFFFFF)),
    ]
}

//  Skirt + boots: a mid steel blue, darker than the shirt so the two garments
//  stay visually separate. The boots sit lower in the source luminance band and
//  land on the navy end of the same ramp on their own.
fn skirt_ramp() -> Vec<Stop> {
    vec![
        (0.00, rgb(0x202B3C)),
        (0.25, rgb(0x2E3A4F)),
        (0.55, rgb(0x5A7A9E)),
        (0.80, rgb(0x8FB2D2)),
        (1.00, rgb(0xDCEDF9)),
    ]
}

//  Tights: cool blue-grey, deliberately less saturated than the skirt so the
//  legs read as hosiery rather than as more of the same fabric.
fn tights_ramp() -> Vec<Stop> {
    vec![
        (0.00, rgb(0x1E2531)),
        (0.35, rgb(0x363F52)),
        (0.60, rgb(0x46536B)),
        (0.85, rgb(0x6E7E99)),
        (1.00, rgb(0xA8B6CB)),
    ]
}

//  Boots: dark navy leather. They share the skirt's material *and* its texture
//  and sit in the same luminance band, so they are selected by model-space
//  height instead and given a ramp that keeps them the darkest thing she wears,
//  anchoring the silhouette the way the original black boots did.
fn boots_ramp() -> Vec<Stop> {
    vec![
        (0.00, rgb(0x141A26)),
        (0.30, rgb(0x222C3E)),
        (0.60, rgb(0x2E3A4F)),
        (0.85, rgb(0x475773)),
        (1.00, rgb(0x7C90AC)),
    ]
}

//  The spectral hand: colder and deeper than the clothes, so the cyan flame that
//  burns over it reads as light rather than as paint.
fn hand_ramp() -> Vec<Stop> {
    vec![
        (0.00, rgb(0x10202C)),
        (0.35, rgb(0x1C3A4E)),
        (0.60, rgb(0x2E5C74)),
        (0.85, rgb(0x63A5BE)),
        (1.00, rgb(0xC8ECF7)),
    ]
}

//  Red plaid pinstripes and boot laces become pale cyan instead of merging into
//  the fabric, keeping the garment's graphic structure.
const ACCENT: u32 = 0xBFE4F5;

//  Albedo counterpart of the cyan magic tint, for the acid-green details baked
//  into the clothing maps.
const MAGIC_ACCENT: u32 = 0x4FD6F5;

//  Where the boots end, as a fraction of the skirt material's model-space height
//  range (sole = 0.0, waist = 1.0). Tuned against the rendered turnaround.
const BOOT_TOP_FRACTION: f32 = 0.16;

fn sample(stops: &[Stop], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    let mut prev = stops[0];
    for &s in &stops[1..] {
        if t <= s.0 {
            let span = (s.0 - prev.0).max(1e-6);
            let k = (t - prev.0) / span;
            return [
                prev.1[0] + (s.1[0] - prev.1[0]) * k,
                prev.1[1] + (s.1[1] - prev.1[1]) * k,
                prev.1[2] + (s.1[2] - prev.1[2]) * k,
            ];
        }
        prev = s;
    }
    stops[stops.len() - 1].1
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn luminance(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn hue_sat(r: f32, g: f32, b: f32) -> (f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max <= f32::EPSILON { 0.0 } else { d / max };
    (h, s)
}

//  ---------------------------------------------------------------------------
//  UV mask (white-on-black PNG baked from the model's own triangles).

struct Mask {
    w: u32,
    h: u32,
    g: Vec<u8>,
}

impl Mask {
    fn from_png(png: &[u8]) -> anyhow::Result<Self> {
        let img = image::load_from_memory(png)?.to_luma8();
        Ok(Mask {
            w: img.width(),
            h: img.height(),
            g: img.into_raw(),
        })
    }

    //  Nearest-sample in normalized texture space so the mask can be baked at a
    //  different resolution than the texture it drives.
    fn at(&self, u: f32, v: f32) -> bool {
        let x = ((u * self.w as f32) as u32).min(self.w - 1);
        let y = ((v * self.h as f32) as u32).min(self.h - 1);
        self.g[(y * self.w + x) as usize] > 127
    }
}

/// Rasterize a UV triangle into `g` (white = selected).
fn fill_tri(g: &mut [u8], res: u32, uv: [[f32; 2]; 3]) {
    let to_px = |c: [f32; 2]| [c[0] * res as f32, c[1] * res as f32];
    let p = [to_px(uv[0]), to_px(uv[1]), to_px(uv[2])];
    let min_x = p
        .iter()
        .map(|q| q[0])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as u32;
    let max_x = (p
        .iter()
        .map(|q| q[0])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i64)
        .clamp(0, res as i64 - 1) as u32;
    let min_y = p
        .iter()
        .map(|q| q[1])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as u32;
    let max_y = (p
        .iter()
        .map(|q| q[1])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i64)
        .clamp(0, res as i64 - 1) as u32;
    let area =
        (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
    if area.abs() < 1e-9 {
        return;
    }
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = ((p[1][0] - p[0][0]) * (py - p[0][1]) - (px - p[0][0]) * (p[1][1] - p[0][1]))
                / area;
            let w1 = ((px - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (py - p[0][1]))
                / area;
            //  Dilate by a hair so adjacent triangles leave no seam.
            if w0 >= -0.002 && w1 >= -0.002 && w0 + w1 <= 1.002 {
                g[(y * res + x) as usize] = 255;
            }
        }
    }
}

/// A mask of one material's triangles that sit in a model-space height band,
/// given as a fraction of that material's own Z extent.
///
/// The boots and the skirt share both the `necro_lower` material and its texture,
/// and their albedo luminance overlaps, so neither a material mask nor the
/// gradient map can separate them. Height can: the boots are simply the bottom of
/// the garment. LOD copies are skipped so only the rendered mesh contributes.
fn bake_height_mask(
    model_bytes: &[u8],
    material: &str,
    z_lo: f32,
    z_hi: f32,
    res: u32,
) -> anyhow::Result<Mask> {
    let model = morphic::model::decode(model_bytes)?;
    let needle = material.to_ascii_lowercase();

    let mut tris: Vec<([[f32; 2]; 3], f32)> = Vec::new();
    let (mut zmin, mut zmax) = (f32::INFINITY, f32::NEG_INFINITY);
    for part in &model.meshes {
        if part.name.to_ascii_lowercase().contains("_lod") {
            continue;
        }
        for prim in &part.primitives {
            if !prim.material.to_ascii_lowercase().contains(&needle) {
                continue;
            }
            let vb = &part.vertex_buffers[prim.vertex_buffer];
            let uvs = vb.texcoords.first();
            let Some(uvs) = uvs else { continue };
            for tri in prim.indices.chunks_exact(3) {
                let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
                if a >= vb.positions.len() || b >= vb.positions.len() || c >= vb.positions.len() {
                    continue;
                }
                if a >= uvs.len() || b >= uvs.len() || c >= uvs.len() {
                    continue;
                }
                let z = (vb.positions[a][2] + vb.positions[b][2] + vb.positions[c][2]) / 3.0;
                zmin = zmin.min(z);
                zmax = zmax.max(z);
                tris.push(([uvs[a], uvs[b], uvs[c]], z));
            }
        }
    }
    anyhow::ensure!(!tris.is_empty(), "no triangles for material '{material}'");

    let span = (zmax - zmin).max(1e-6);
    let lo = zmin + span * z_lo;
    let hi = zmin + span * z_hi;
    let mut g = vec![0u8; (res * res) as usize];
    let mut n = 0;
    for (uv, z) in &tris {
        if *z >= lo && *z <= hi {
            //  UVs may tile outside [0,1]; wrap into the unit square.
            let w = uv.map(|c| [c[0].rem_euclid(1.0), c[1].rem_euclid(1.0)]);
            fill_tri(&mut g, res, w);
            n += 1;
        }
    }
    eprintln!(
        "  height mask for '{material}' z {z_lo:.2}..{z_hi:.2} of [{zmin:.1}, {zmax:.1}]: \
         {n}/{} triangle(s)",
        tris.len()
    );
    Ok(Mask { w: res, h: res, g })
}

fn bake_material_mask(model_bytes: &[u8], material: &str, res: u32) -> anyhow::Result<Mask> {
    let model = morphic::model::decode(model_bytes)?;
    let segs = morphic::model::segments(&model, SegmentBy::Material, None);
    let id = segs
        .iter()
        .position(|s| s.label.to_ascii_lowercase().contains(material))
        .ok_or_else(|| anyhow::anyhow!("material '{material}' not found in {MODEL}"))?;
    let png = morphic::model::mask_png(&segs, &[id], res)?;
    eprintln!(
        "  UV mask baked for {} (id {id}, res {res})",
        segs[id].label
    );
    Mask::from_png(&png)
}

fn rgba8_mut(img: &mut Image) -> anyhow::Result<&mut Vec<u8>> {
    match &mut img.data {
        ImageData::Rgba8(v) => Ok(v),
        ImageData::Rgba16F(_) => anyhow::bail!("unexpected HDR texture (LDR albedo expected)"),
    }
}

/// One garment's recolor rule.
struct Paint {
    stops: Vec<Stop>,
    /// Luminance window that the fabric occupies; mapped onto the whole ramp.
    lo: f32,
    hi: f32,
    /// Above `hi` the recolor fades out and is gone by `fade`, which is how skin
    /// (L ~= 0.6) survives inside a garment's texture without needing a mask.
    ///
    /// The fade is gated on *warm* hues, not brightness alone: her neck, chest
    /// and the skin through the tights' rips are the only bright things that
    /// should survive, while bright neutral trim and the acid-green badge on her
    /// collar are part of the recolor.
    fade: f32,
    /// Retint saturated red texels (plaid stripes, boot laces) to pale cyan.
    accent: bool,
}

/// Skin-likeness of a texel: warm hue with some, but not extreme, saturation.
/// Only this is protected from the ramp when it is brighter than the fabric.
fn skin_like(hue: f32, sat: f32) -> f32 {
    let warm = if hue <= 50.0 || hue >= 335.0 {
        1.0
    } else {
        0.0
    };
    //  A fully neutral bright texel is trim, not skin.
    warm * smoothstep(0.02, 0.06, sat) * (1.0 - smoothstep(0.45, 0.65, sat))
}

/// Apply `paint` to every texel `mask` selects (or all of them when `None`).
///
/// Every decision reads `src`, the pristine decode, never the working image: a
/// second pass over a region already recolored by a first one (the tights inside
/// the skirt's texture) would otherwise sample the lifted luminance and fall
/// straight through the skin-protection fade.
///
/// Alpha is never touched: on Deadlock albedo it is a mask channel, not opacity.
fn paint(img: &mut Image, src: &[u8], mask: Option<&Mask>, p: &Paint) -> anyhow::Result<u64> {
    let (w, h) = (img.width, img.height);
    let accent = rgb(ACCENT);
    let px = rgba8_mut(img)?;
    let mut touched = 0u64;
    for y in 0..h {
        for x in 0..w {
            if let Some(m) = mask {
                let u = (x as f32 + 0.5) / w as f32;
                let v = (y as f32 + 0.5) / h as f32;
                if !m.at(u, v) {
                    continue;
                }
            }
            let i = ((y * w + x) * 4) as usize;
            let (r0, g0, b0) = (
                src[i] as f32 / 255.0,
                src[i + 1] as f32 / 255.0,
                src[i + 2] as f32 / 255.0,
            );
            let l = luminance(r0, g0, b0);
            let (hue, sat) = hue_sat(r0, g0, b0);

            //  Fabric weight: full inside the garment's luminance band, and only
            //  surrendered to texels that are both brighter than the fabric and
            //  skin-coloured.
            let w_fabric = 1.0 - smoothstep(p.hi, p.fade, l) * skin_like(hue, sat);
            if w_fabric <= 0.0 {
                continue;
            }

            let t = ((l - p.lo) / (p.hi - p.lo)).clamp(0.0, 1.0);
            let mut col = sample(&p.stops, t);

            //  Yellow-green is the old magic accent: the badge on her collar, and
            //  most of the spectral hand, whose flame is baked into its albedo
            //  (69% of that map is saturated, 53% of it green). It follows the
            //  flame to cyan rather than dissolving into the fabric ramp.
            if sat > 0.25 && (45.0..205.0).contains(&hue) {
                let k = smoothstep(0.25, 0.40, sat);
                let scale = 0.45 + 1.1 * l;
                let g = rgb(MAGIC_ACCENT);
                for c in 0..3 {
                    col[c] += ((g[c] * scale).min(1.0) - col[c]) * k;
                }
            } else if p.accent && sat > 0.22 && (hue < 28.0 || hue > 332.0) {
                let k = smoothstep(0.22, 0.34, sat);
                //  Keep the stripe's own shading by scaling the accent with
                //  the source luminance instead of flat-filling it.
                let scale = 0.55 + 1.6 * l;
                let a = [
                    (accent[0] * scale).min(1.0),
                    (accent[1] * scale).min(1.0),
                    (accent[2] * scale).min(1.0),
                ];
                for c in 0..3 {
                    col[c] += (a[c] - col[c]) * k;
                }
            }

            for c in 0..3 {
                //  Blend against the source, so a re-ramped region replaces the
                //  earlier pass rather than compounding with it, and a faded
                //  skin texel falls back to its original tone either way.
                let old = src[i + c] as f32 / 255.0;
                px[i + c] =
                    (((old + (col[c] - old) * w_fabric) * 255.0).round()).clamp(0.0, 255.0) as u8;
            }
            touched += 1;
        }
    }
    Ok(touched)
}

fn vec3(name: &str, c: [f64; 3]) -> VmatEdit {
    VmatEdit::Vector {
        name: name.to_owned(),
        value: [c[0], c[1], c[2], 0.0],
    }
}

//  Cold palette for the shader-side tints.
const OUTLINE_COOL: [f64; 3] = [0.070, 0.200, 0.376]; // deep blue NPR outline
const OUTLINE_FACE: [f64; 3] = [0.102, 0.282, 0.463]; // was green on the head
const TRANSMISSIVE_COOL: [f64; 3] = [0.157, 0.341, 0.525]; // was a green subsurface
const MAGIC_CORE: [f64; 3] = [0.350, 0.850, 1.000]; // cyan flame
const MAGIC_RIM: [f64; 3] = [0.550, 0.920, 1.000];
const JAR_GLOW: [f64; 3] = [0.720, 0.940, 1.000]; // pale ice, was yellow-green
const JAR_FRESNEL: [f64; 3] = [0.000, 0.160, 0.280];

fn edits_for(material: &str) -> Vec<VmatEdit> {
    match material {
        //  Garments: red NPR outline -> deep blue, green subsurface -> cold.
        "necro_upper_dark" | "necro_lower" | "sleeves" | "pantythose" => vec![
            vec3("g_vSolidOutlineTint", OUTLINE_COOL),
            vec3("TextureNprTramsissiveColor1", TRANSMISSIVE_COOL),
        ],
        "necro_eye" => vec![vec3("g_vSolidOutlineTint", OUTLINE_COOL)],
        //  Her face outline is green in the source; the skin itself is untouched.
        "necro_head" => vec![vec3("g_vSolidOutlineTint", OUTLINE_FACE)],
        //  White hair stays white; only its outline is cooled.
        "necro_hair" => vec![vec3("g_vSolidOutlineTint", [0.157, 0.196, 0.259])],
        //  The flaming hand: acid green self-illum -> cyan. The pulse
        //  expressions on g_flSelfIllum* drive scalars and are left alone.
        "necro_hand" => vec![
            vec3("g_vSelfIllumTint1", MAGIC_CORE),
            vec3("g_vSelfIllumFresnelMaskTint1", MAGIC_RIM),
            vec3("g_vSolidOutlineTint", OUTLINE_COOL),
            vec3("TextureNprTramsissiveColor1", TRANSMISSIVE_COOL),
        ],
        "necro_flame_effect" | "necro_flame_effect_hand" => {
            vec![vec3("g_vSelfIllumTint1", MAGIC_CORE)]
        }
        "necro_jar_glass" => vec![
            vec3("g_vSelfIllumTint1", JAR_GLOW),
            vec3("g_vSelfIllumFresnelMaskTint1", JAR_FRESNEL),
            vec3("g_vSolidOutlineTint", OUTLINE_COOL),
        ],
        "necro_jar_of_dread" => vec![vec3("g_vSolidOutlineTint", OUTLINE_FACE)],
        other => panic!("no edit recipe for {other}"),
    }
}

const MATERIALS: &[&str] = &[
    "necro_upper_dark",
    "necro_lower",
    "sleeves",
    "pantythose",
    "necro_eye",
    "necro_head",
    "necro_hair",
    "necro_hand",
    "necro_flame_effect",
    "necro_flame_effect_hand",
    "necro_jar_glass",
    "necro_jar_of_dread",
];

fn shirt_paint() -> Paint {
    Paint {
        stops: shirt_ramp(),
        lo: 0.090,
        hi: 0.230,
        fade: 0.340,
        accent: true,
    }
}

fn sleeve_paint() -> Paint {
    Paint {
        stops: shirt_ramp(),
        lo: 0.130,
        hi: 0.265,
        fade: 0.360,
        accent: true,
    }
}

fn skirt_paint() -> Paint {
    Paint {
        stops: skirt_ramp(),
        lo: 0.070,
        hi: 0.240,
        fade: 0.340,
        accent: true,
    }
}

fn boots_paint() -> Paint {
    Paint {
        stops: boots_ramp(),
        lo: 0.070,
        hi: 0.240,
        fade: 0.340,
        accent: true,
    }
}

fn tights_paint() -> Paint {
    Paint {
        stops: tights_ramp(),
        lo: 0.090,
        hi: 0.270,
        fade: 0.380,
        accent: false,
    }
}

fn hand_paint() -> Paint {
    Paint {
        stops: hand_ramp(),
        lo: 0.060,
        hi: 0.270,
        fade: 0.400,
        accent: false,
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let skin = args
        .first()
        .cloned()
        .expect("usage: reskin_necro_frostblue <graves-shirt_dir.vpk> <out_dir.vpk|--png prefix>");
    let arg2 = args
        .get(1)
        .cloned()
        .expect("second arg: <out_dir.vpk> or --png <prefix>");

    eprintln!("Graves \"Frostbite\" (necro) -- light blue reskin, satchel removed");

    let model_bytes = vpkmerge_core::read_vpk_entry(&skin, MODEL)?;

    //  The skirt, boots and tights share one 2048 albedo. Bake the tights'
    //  material mask so the legs can take a cooler ramp than the skirt.
    let tights_mask = bake_material_mask(&model_bytes, "pantythose", 2048)?;
    //  Boots: the bottom slice of the skirt material's own height range.
    let boots_mask = bake_height_mask(&model_bytes, "necro_lower", 0.0, BOOT_TOP_FRACTION, 2048)?;

    //  Every overlay re-ramps a sub-region of the same texture from the pristine
    //  source, so the order of the overlays does not compound their colours.
    let repaint = |entry: &str,
                   p: &Paint,
                   overlays: &[(&str, &Mask, &Paint)]|
     -> anyhow::Result<(String, Vec<u8>, Image)> {
        let bytes = vpkmerge_core::read_vpk_entry(&skin, entry)?;
        let mut img = morphic::decode(&bytes)?;
        let src = match &img.data {
            ImageData::Rgba8(v) => v.clone(),
            ImageData::Rgba16F(_) => anyhow::bail!("{entry}: unexpected HDR albedo"),
        };
        let n = paint(&mut img, &src, None, p)?;
        let mut notes = String::new();
        for (label, m, op) in overlays {
            let k = paint(&mut img, &src, Some(m), op)?;
            notes.push_str(&format!(", {k} re-ramped for the {label}"));
        }
        eprintln!(
            "  {} {}x{}: {n} texel(s) ramped{notes}",
            entry.rsplit('/').next().unwrap_or(entry),
            img.width,
            img.height,
        );
        let out = morphic::replace_mip_chain(&bytes, &img)?;
        Ok((entry.to_owned(), out, img))
    };

    let (_, shirt_tex, shirt_img) = repaint(SHIRT_COLOR, &shirt_paint(), &[])?;
    let (_, sleeve_tex, sleeve_img) = repaint(SLEEVE_COLOR, &sleeve_paint(), &[])?;
    let (_, lower_tex, lower_img) = repaint(
        LOWER_COLOR,
        &skirt_paint(),
        &[
            ("tights", &tights_mask, &tights_paint()),
            ("boots", &boots_mask, &boots_paint()),
        ],
    )?;
    let (_, hand_tex, hand_img) = repaint(HAND_COLOR, &hand_paint(), &[])?;

    //  --- preview mode: write the repainted art, no game needed.
    if arg2 == "--png" {
        let prefix = args.get(2).cloned().expect("--png needs an output prefix");
        for (img, name) in [
            (&shirt_img, "shirt"),
            (&sleeve_img, "sleeves"),
            (&lower_img, "lower"),
            (&hand_img, "hand"),
        ] {
            let png = morphic::encode_image(img, TextureFormat::PngRgba8888)?;
            let path = format!("{prefix}_{name}.png");
            std::fs::write(&path, &png)?;
            println!("wrote {path} ({}x{})", img.width, img.height);
        }
        return Ok(());
    }
    let out = arg2;

    //  --- shader-side tints -----------------------------------------------
    let mut vmats: Vec<(String, Vec<u8>)> = Vec::new();
    for m in MATERIALS {
        let entry = format!("{DIR}/{m}.vmat_c");
        let bytes = vpkmerge_core::read_vpk_entry(&skin, &entry)?;
        let (patched, stats) = patch_vmat_params(&bytes, &edits_for(m))?;
        if !stats.failed.is_empty() {
            anyhow::bail!("{m}: could not patch {:?}", stats.failed);
        }
        eprintln!(
            "  {m}.vmat_c: {} set, {} inserted",
            stats.set, stats.inserted
        );
        vmats.push((entry, patched));
    }

    //  --- drop the satchel --------------------------------------------------
    let (edited_model, removed) =
        morphic::model::remove_draw_calls_by_material(&model_bytes, BAG_MATERIAL)?;
    let dropped: usize = removed.len();
    eprintln!("  {MODEL}: removed {dropped} draw call(s) for '{BAG_MATERIAL}'");
    for r in &removed {
        eprintln!("    - {} ({} indices)", r.material, r.index_count);
    }

    let mut files: Vec<(&str, &[u8])> = vec![
        (MODEL, edited_model.as_slice()),
        (SHIRT_COLOR, shirt_tex.as_slice()),
        (SLEEVE_COLOR, sleeve_tex.as_slice()),
        (LOWER_COLOR, lower_tex.as_slice()),
        (HAND_COLOR, hand_tex.as_slice()),
    ];
    for (entry, bytes) in &vmats {
        files.push((entry.as_str(), bytes.as_slice()));
    }

    let readme = b"Graves \"Frostbite\" - light blue, no satchel\n\
Overlay for the Graves Shirt addon (TwoFacedoff). Load it ABOVE that mod.\n\
Outfit albedo gradient-mapped from charcoal to ice/powder blue (shirt), steel\n\
blue (skirt, boots) and cool blue-grey (tights, via the pantythose UV mask).\n\
Red plaid pinstripes and boot laces retinted pale cyan. Skin, white hair and\n\
all painted detail preserved. Green magic (hand flame, jar glow, face outline)\n\
retinted to cyan/ice. Back satchel draw calls removed from necro.vmdl_c.\n";
    files.push(("addoninfo.txt", readme));

    vpkmerge_core::pack(&files, &out)?;
    println!("wrote {out} ({} entries)", files.len());
    Ok(())
}
