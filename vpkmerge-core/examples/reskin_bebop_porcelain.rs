// Bebop "Porcelain" -- a clean powder-blue enamel reskin.
//
// WHY THIS ISN'T `vpkmerge texture --hue 207`. A plain absolute hue-set keeps
// each pixel's saturation and value, and Bebop's source palette sits at V
// 0.20-0.45 (measured over his five body albedos: 66% red-orange at S 0.64, 11%
// amber-tan, 5% deep red -- 82% warm overall). Setting that mass to hue 207
// lands DARK NAVY, not light blue. "Clean" and "light" are the value and
// saturation axes, not hue, so this builder splits the palette into bands and
// retargets each one independently:
//
//   paint  (hue 330-22, the red/deep-red painted steel) -> powder-blue enamel
//   canvas (hue 22-38, the rust-orange apron + leather) -> warm cream
//   brass  (hue 38-62, the amber fittings)              -> real gold, kept WARM
//   green  (hue 80-165, eyes + belt buckle)             -> untouched, it pops
//   everything else (neutral steel, cyan rivets, near-black) -> untouched
//
// The brass band is load-bearing: an earlier pass let brass and canvas both land
// cream, and the result read flat because the blue had no warm complement.
// Keeping the gold is what makes the blue sing.
//
// Each band also gets a VALUE REMAP (v -> lo + v*(hi-lo)) rather than a scale.
// That both lifts the midtone (so the enamel reads light) and compresses the
// range (so the baked-in rust mottling flattens into even paint instead of
// staying as grime under a new colour). Compression is the whole "clean" read.
//
// Bebop is unusually well suited to this: his body is FIVE separate materials
// (head / l_arm / r_arm / upper_body / lower_body), each with its own 2048 BC7
// albedo, and the UV islands land exactly on the painted content. So per-region
// theming needs no UV mask at all -- unlike chrono, where body and headbase
// share one texture and the mask tool is the only way to separate them.
//
// Only the albedos are touched. No `.vmat_c` edit (a KV3 re-encode renders the
// engine error shader on hero materials), no normal/roughness change: the
// chipped-paint edges and panel wear are already in the source maps and read
// correctly under the new colour.
//
// usage:
//   # preview the art, no game install needed (writes <prefix>_<part>.png):
//   cargo run --release --example reskin_bebop_porcelain -- <pak01_dir.vpk> --png <prefix>
//   # full addon bake:
//   cargo run --release --example reskin_bebop_porcelain -- <pak01_dir.vpk> <out_dir.vpk>
use anyhow::{Context, Result};
use morphic::{Image, ImageData, TextureFormat};

const DIR: &str = "models/heroes_staging/bebop/materials/";

/// The albedo set: (short name, entry filename). Every one is 2048 BC7 except
/// the sticky bomb; all are recoloured with the same band spec so the theme is
/// consistent across body, weapon and thrown props.
const ALBEDOS: &[(&str, &str)] = &[
    ("head", "bebop_head_color_png_2a75e8ba.vtex_c"),
    ("l_arm", "bebop_l_arm_color_png_7b6177ca.vtex_c"),
    ("r_arm", "bebop_r_arm_color_png_ec8bd2fb.vtex_c"),
    ("upper_body", "bebop_upper_body_color_png_6d743640.vtex_c"),
    ("lower_body", "bebop_lower_body_color_png_9d9092d1.vtex_c"),
    ("weapon", "bebop_weapon_color_png_2ecbbf7c.vtex_c"),
    (
        "hand_projectile",
        "bebop_hand_projectile_color_png_8d7bfc.vtex_c",
    ),
];

/// One hue band's retarget: an absolute target hue (`None` leaves hue alone), a
/// saturation scale, and a value REMAP window.
struct Band {
    /// Inclusive lower hue bound in degrees. Wraps through 0 when `lo > hi`.
    lo: f32,
    /// Exclusive upper hue bound in degrees.
    hi: f32,
    /// Absolute target hue in degrees, or `None` to leave the band's hue as-is.
    hue: Option<f32>,
    /// Saturation multiplier.
    sat: f32,
    /// Value remap window: `v -> val.0 + v * (val.1 - val.0)`.
    val: (f32, f32),
}

/// The "Porcelain" spec. Bands are tested in order and the first match wins, so
/// the windows must not need to overlap.
const PORCELAIN: &[Band] = &[
    // Painted steel -> powder-blue enamel. The widest value window of the three:
    // the panel forms need range or the big head flattens into a flat blue blob.
    Band {
        lo: 330.0,
        hi: 22.0,
        hue: Some(207.0),
        sat: 0.70,
        val: (0.26, 0.94),
    },
    // Canvas apron + leather straps -> warm cream. Kept warm on purpose: it is
    // what separates the soft goods from the enamel and stops him reading
    // monochrome.
    Band {
        lo: 22.0,
        hi: 38.0,
        hue: Some(36.0),
        sat: 0.52,
        val: (0.42, 0.90),
    },
    // Brass fittings -> real gold, saturation pushed UP. The warm complement.
    Band {
        lo: 38.0,
        hi: 62.0,
        hue: Some(41.0),
        sat: 1.30,
        val: (0.30, 0.92),
    },
];

/// Saturation floor below which a pixel counts as neutral steel and is left
/// alone, and the value floor below which it counts as near-black.
const SAT_GATE: f32 = 0.13;
const VAL_GATE: f32 = 0.05;

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let h = if d < 1e-6 {
        0.0
    } else if mx == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if mx == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if mx < 1e-6 { 0.0 } else { d / mx };
    (h, s, mx)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let c = v * s;
    let hp = (h.rem_euclid(360.0)) / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (r + m, g + m, b + m)
}

fn in_band(h: f32, b: &Band) -> bool {
    if b.lo < b.hi {
        h >= b.lo && h < b.hi
    } else {
        h >= b.lo || h < b.hi
    }
}

fn rgba8_mut(img: &mut Image) -> Result<&mut Vec<u8>> {
    match &mut img.data {
        ImageData::Rgba8(v) => Ok(v),
        ImageData::Rgba16F(_) => {
            anyhow::bail!("HDR (f16) texture: the porcelain recolor is LDR-only")
        }
    }
}

/// Apply the band spec in place. Returns the per-band hit fraction, so the bake
/// log shows which regions each texture actually contains (a texture that
/// reports 0% paint would mean the band windows missed).
fn paint(img: &mut Image, spec: &[Band]) -> Result<[f32; 4]> {
    let px = rgba8_mut(img)?;
    let total = (px.len() / 4) as f32;
    let mut hits = [0f32; 4];
    for p in px.chunks_exact_mut(4) {
        let (r, g, b) = (
            f32::from(p[0]) / 255.0,
            f32::from(p[1]) / 255.0,
            f32::from(p[2]) / 255.0,
        );
        let (h, s, v) = rgb_to_hsv(r, g, b);
        if s <= SAT_GATE || v <= VAL_GATE {
            continue; // neutral steel / near-black: untouched
        }
        let Some((i, band)) = spec.iter().enumerate().find(|(_, b)| in_band(h, b)) else {
            hits[3] += 1.0; // an untouched chromatic pixel (the green eyes/belt)
            continue;
        };
        hits[i] += 1.0;
        let nh = band.hue.unwrap_or(h);
        let ns = (s * band.sat).clamp(0.0, 1.0);
        let nv = (band.val.0 + v * (band.val.1 - band.val.0)).clamp(0.0, 1.0);
        let (nr, ng, nb) = hsv_to_rgb(nh, ns, nv);
        p[0] = (nr * 255.0).round().clamp(0.0, 255.0) as u8;
        p[1] = (ng * 255.0).round().clamp(0.0, 255.0) as u8;
        p[2] = (nb * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    for h in &mut hits {
        *h /= total;
    }
    Ok(hits)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        anyhow::bail!(
            "usage: reskin_bebop_porcelain <pak01_dir.vpk> (<out_dir.vpk> | --png <prefix>)"
        );
    }
    let pak = args[0].clone();
    let png_mode = args[1] == "--png";
    let target = if png_mode {
        args.get(2).context("--png needs an output prefix")?.clone()
    } else {
        args[1].clone()
    };

    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, file) in ALBEDOS {
        let entry = format!("{DIR}{file}");
        let bytes = vpkmerge_core::read_vpk_entry(&pak, &entry)
            .with_context(|| format!("reading {entry}"))?;
        let info = morphic::inspect(&bytes)?;
        let mut img = morphic::decode(&bytes).with_context(|| format!("decoding {name}"))?;
        let hits = paint(&mut img, PORCELAIN)?;
        println!(
            "{name:16} {:?} {}x{}  paint {:>5.1}%  canvas {:>5.1}%  brass {:>4.1}%  kept-green {:>4.1}%",
            info.format,
            info.width,
            info.height,
            hits[0] * 100.0,
            hits[1] * 100.0,
            hits[2] * 100.0,
            hits[3] * 100.0
        );

        if png_mode {
            let png = morphic::encode_image(&img, TextureFormat::PngRgba8888)?;
            let path = format!("{target}_{name}.png");
            std::fs::write(&path, png)?;
            println!("  wrote {path}");
        } else {
            let out = morphic::replace_mip_chain(&bytes, &img)
                .with_context(|| format!("re-encoding {name} mip chain"))?;
            packed.push((entry, out));
        }
    }

    if !png_mode {
        let refs: Vec<(&str, &[u8])> = packed
            .iter()
            .map(|(e, b)| (e.as_str(), b.as_slice()))
            .collect();
        vpkmerge_core::pack(&refs, &target).context("packing addon VPK")?;
        println!("\npacked {} textures -> {target}", refs.len());
    }
    Ok(())
}
