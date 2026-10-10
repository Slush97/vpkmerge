// Calico (`nano`) recolour: white skin, blonde hair, light blue outfit.
//
// WHY THIS ISN'T `vpkmerge texture --hue N`. A plain absolute hue-set keeps each
// pixel's saturation and value, and every one of the three asks here is a
// VALUE/SATURATION move, not a hue move:
//
//   * her skin is dark brown (V 0.28-0.39 @ S 0.42-0.49). Hue-setting that lands
//     a differently-tinted dark brown, never white. "White" is +V and -S.
//   * her hair is near-BLACK (V 0.067 median). Any hue at V 0.067 is still black.
//     "Blonde" is a big +V with a warm hue and moderate S.
//   * her outfit is dark periwinkle (V 0.15 trousers .. 0.62 gloves). Hue-setting
//     to a blue hue lands navy. "Light blue" is +V.
//
// So each region gets an absolute target hue, a saturation SCALE, and a value
// REMAP (v -> out_lo + t*(out_hi-out_lo) over an input window), which lifts the
// midtone and rescales the range instead of shifting it.
//
// FINDING THE REGIONS IS THE HARD PART. Measured against the live pak, not
// guessed (see docs/handoff-calico-porcelain-blonde.md for the probe method):
//
//   * The model has only TWO mesh parts (body, cat_v2) and three materials, so
//     `model mask --by part/--by material` cannot separate hair from skin.
//   * Her face AND her whole scalp are `nanov2_head`; the BUN is `nanov2_body`.
//     Verified with a per-material flat-colour render.
//   * Valve painted the shaved scalp SKIN-TONED, so within the head texture hair
//     and skin are the same hue family. Value splits them cleanly: the head
//     albedo is bimodal, hair at V 0.08-0.12 (31% of texels) and skin at
//     V 0.20-0.42 (53%), with a near-empty valley at V 0.12-0.20 to cut on.
//     Gold sits alone at a flat V 0.68 / S 0.68.
//   * The bun lives in one tight, effectively exclusive UV rectangle of the body
//     texture (u 0.878-0.975, v 0.416-0.551): 958 of the 1190 faces whose UV
//     centre lands there are the bun, and the rest are head-material (different
//     texture) plus 12 stray waist slivers. So the bun is gated by UV box AND
//     colour, which needs no baked mask and keeps this builder standalone.
//   * The head's `g_tNprTransmissiveColor` is 4x4 (a flat constant), so there is
//     no authored skin/hair mask to borrow.
//
// The blue eyeshadow (hue 272 @ S 0.10) survives because the head spec leaves
// desaturated and cool pixels alone. The white dress shirt and the grey gun
// survive for the same reason on the body side. The gold (glasses, earrings, hair
// spike, shirt trim) is deliberately KEPT: it is the warm complement that stops
// the pale-blue-and-ivory read going flat, the same lesson as bebop's brass.
//
// Only the two albedos are touched: no `.vmat_c` edit (a KV3 re-encode renders
// the engine error shader on hero materials) and no normal/roughness change, so
// the skin pores, fabric sheen and the bun's strand relief still read.
//
// usage:
//   # preview the art, no game install needed (writes <prefix>_{head,body}.png):
//   cargo run --release --example reskin_nano_porcelain_blonde -- <pak01_dir.vpk> --png <prefix>
//   # full addon bake:
//   cargo run --release --example reskin_nano_porcelain_blonde -- <pak01_dir.vpk> <out_dir.vpk>
//   # accent flavour for the sash + coat tails (default `harmony`):
//   ... [--accents harmony|mono|warm]
use anyhow::{Context, Result};
use morphic::{Image, ImageData, TextureFormat};

const DIR: &str = "models/heroes_staging/nano/nano_v2/materials/";
const HEAD: &str = "nanov2_head_color_png_4a754211.vtex_c";
const BODY: &str = "nanov2_body_color_png_95dda15b.vtex_c";

/// The bun's UV rectangle in the body texture, widened a hair off the measured
/// face bounds (u 0.878-0.975, v 0.416-0.551) so edge texels are covered. `v` is
/// GL-style with 0 at the bottom, which is how the mesh's UVs read.
const BUN_U: (f32, f32) = (0.872, 0.980);
const BUN_V: (f32, f32) = (0.410, 0.557);

/// A hue band's retarget: absolute target hue, saturation scale, value remap.
struct Band {
    /// Inclusive lower hue bound in degrees. Wraps through 0 when `lo > hi`.
    lo: f32,
    /// Exclusive upper hue bound in degrees.
    hi: f32,
    /// Absolute target hue in degrees, or `None` to keep the source hue.
    hue: Option<f32>,
    /// Saturation multiplier, applied before the clamp to [0,1].
    sat: f32,
    /// Input value window the remap reads from.
    v_in: (f32, f32),
    /// Output value window the remap writes to.
    v_out: (f32, f32),
}

/// Absolute colour target for a region selected by value rather than by hue.
struct Target {
    hue: f32,
    /// Absolute saturation when `sat_scale` is `None`, else unused.
    sat: f32,
    /// Scale the source saturation instead of setting it, when `Some`.
    sat_scale: Option<f32>,
    v_in: (f32, f32),
    v_out: (f32, f32),
}

/// Blonde. Shared by the head's hairline/brows and the body's bun so the two
/// meet seamlessly at the crown. Paler and less saturated than the gold
/// accessories (S 0.68 / V 0.68) on purpose, so blonde and brass stay distinct.
/// The output window is deliberately NARROW (0.20 wide for a 0.13-wide input).
/// Her hair is painted near-black and flat, so a wide window multiplies the
/// BC7 noise in that near-black up into visible blotching: a first pass at
/// (0.46, 0.76) rendered as mottled khaki. The hair's form comes from the normal
/// map and AO, exactly as it does for the stock black, so the albedo only has to
/// supply a clean colour.
/// Blonde-on-pale separates by SATURATION, not by value. Golden blonde is about
/// rgb(230,195,120) = hue 41 / S 0.48 / V 0.90, and light skin is about
/// rgb(230,200,180) = hue 26 / S 0.22 / V 0.90: nearly the same brightness, more
/// than twice the saturation. An earlier pass made the blonde slightly DARKER
/// than the skin (V 0.56-0.76 vs 0.62-0.82) at a middling S 0.46, and it read as
/// a dirty smudge on the skull instead of as hair. So the blonde is now both
/// brighter and much more saturated than the skin.
///
/// The output window stays narrow for its input: her hair is painted near-black
/// and flat, and a wide window multiplies the BC7 noise in that near-black up
/// into visible blotching (a 0.46-0.76 pass rendered as mottled khaki). The
/// hair's form comes from the normal map and AO, exactly as it does for the stock
/// black, so the albedo only has to supply a clean colour.
const BLONDE: Target = Target {
    hue: 41.0,
    sat: 0.50,
    sat_scale: None,
    v_in: (0.02, 0.15),
    v_out: (0.72, 0.90),
};

/// Porcelain skin. Hue stays warm (26) rather than neutral: a fully desaturated
/// face reads as a grey statue, not as pale skin. The source's own shading
/// gradient is preserved through the remap, so the face keeps its form.
///
/// The output window tops out well below 1.0. A first pass used (0.72, 0.97) and
/// the face rendered as a featureless paper-white blob: the brows, nostrils and
/// lip line all clipped away together. Light skin albedo really does live around
/// V 0.70-0.85 (here cheek V 0.34 -> 0.78, about rgb(199,172,155)), and leaving
/// headroom is what keeps the face's form.
const SKIN: Target = Target {
    hue: 26.0,
    sat: 0.0,
    sat_scale: Some(0.45),
    v_in: (0.20, 0.44),
    v_out: (0.66, 0.86),
};

/// Head value cuts: at or below `HAIR_V` is pure hair, at or above `SKIN_V` is
/// pure skin, between them the two targets cross-fade. The window is the empty
/// valley in the head albedo's value histogram, so the hairline lands soft while
/// the flat hair and the flat face each stay pure.
const HAIR_V: f32 = 0.13;
const SKIN_V: f32 = 0.21;

/// The main garment: blazer, sleeves, shoulder puffs, gloves, cuffs and
/// trousers all sit in one hue band (measured 247-259), so a single band
/// recolours the whole outfit coherently. The value window is wide and the
/// output window keeps its ordering, so the existing hierarchy survives:
/// trousers stay the deepest (V 0.15 -> 0.40), blazer mid (0.28-0.43 -> 0.56-0.71),
/// gloves palest (0.62 -> 0.92). The output floor is deliberately low: a first
/// pass floored at 0.34 and the trousers came out nearly the same blue as the
/// blazer, which flattened her into one monolithic jumpsuit.
const GARMENT: Band = Band {
    lo: 225.0,
    hi: 278.0,
    hue: Some(205.0),
    sat: 1.30,
    v_in: (0.03, 0.65),
    v_out: (0.26, 0.95),
};

fn accent_bands(flavor: &str) -> Result<Vec<Band>> {
    // (sash @ hue ~287, coat tails + red lining @ hue ~344 and 0-14)
    Ok(match flavor {
        // Sash to a deeper coordinated blue, tails to champagne: keeps one warm
        // mass to answer the gold and the blonde.
        "harmony" => vec![
            Band {
                lo: 278.0,
                hi: 322.0,
                hue: Some(218.0),
                sat: 1.05,
                v_in: (0.05, 0.60),
                v_out: (0.30, 0.74),
            },
            Band {
                lo: 322.0,
                hi: 14.0,
                hue: Some(40.0),
                sat: 0.55,
                v_in: (0.05, 0.60),
                v_out: (0.42, 0.90),
            },
        ],
        // Everything into the blue family.
        "mono" => vec![
            Band {
                lo: 278.0,
                hi: 322.0,
                hue: Some(215.0),
                sat: 1.15,
                v_in: (0.05, 0.60),
                v_out: (0.26, 0.70),
            },
            Band {
                lo: 322.0,
                hi: 14.0,
                hue: Some(200.0),
                sat: 0.85,
                v_in: (0.05, 0.60),
                v_out: (0.46, 0.94),
            },
        ],
        // Sash to gold, tails left coral: maximum warm pop against the blue.
        "warm" => vec![Band {
            lo: 278.0,
            hi: 322.0,
            hue: Some(41.0),
            sat: 1.20,
            v_in: (0.05, 0.60),
            v_out: (0.34, 0.84),
        }],
        other => anyhow::bail!("unknown --accents flavour `{other}` (harmony | mono | warm)"),
    })
}

/// Saturation at or below which a pixel counts as neutral (white shirt, grey
/// steel, charcoal) and the value at or below which it counts as near-black.
const SAT_GATE: f32 = 0.13;
const VAL_GATE: f32 = 0.02;

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
    let hp = h.rem_euclid(360.0) / 60.0;
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

/// `v -> out_lo + clamp01((v - in_lo)/(in_hi - in_lo)) * (out_hi - out_lo)`.
fn remap(v: f32, i: (f32, f32), o: (f32, f32)) -> f32 {
    let t = ((v - i.0) / (i.1 - i.0)).clamp(0.0, 1.0);
    (o.0 + t * (o.1 - o.0)).clamp(0.0, 1.0)
}

fn apply_target(t: &Target, s: f32, v: f32) -> (f32, f32, f32) {
    let ns = t.sat_scale.map_or(t.sat, |k| s * k).clamp(0.0, 1.0);
    (t.hue, ns, remap(v, t.v_in, t.v_out))
}

fn rgba8_mut(img: &mut Image) -> Result<&mut Vec<u8>> {
    match &mut img.data {
        ImageData::Rgba8(v) => Ok(v),
        ImageData::Rgba16F(_) => anyhow::bail!("HDR (f16) texture: this recolour is LDR-only"),
    }
}

fn write_px(p: &mut [u8], r: f32, g: f32, b: f32) {
    p[0] = (r * 255.0).round().clamp(0.0, 255.0) as u8;
    p[1] = (g * 255.0).round().clamp(0.0, 255.0) as u8;
    p[2] = (b * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// Head: hair/skin split by value, gold and cool makeup left alone.
/// Returns (hair%, blend%, skin%, kept%).
fn paint_head(img: &mut Image) -> Result<[f32; 4]> {
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

        // gold accessories (glasses, earrings, the bun's spike)
        if v > 0.55 && s > 0.50 && (15.0..60.0).contains(&h) {
            hits[3] += 1.0;
            continue;
        }

        // HAIR IS TESTED BEFORE THE SATURATION / COOL-HUE GATE, on purpose.
        // Her hair is painted near-black, where hue and saturation are numerically
        // unstable: plenty of hair texels measure S < 0.13 or a nonsense cool hue.
        // Gating on those first (the first version's mistake) skipped exactly
        // those texels and left them black, which rendered as dark speckle
        // through the new blonde. Anything this dark is hair, brow or lash
        // regardless of what its hue says, so value decides alone.
        let (nh, ns, nv) = if v <= HAIR_V {
            hits[0] += 1.0;
            apply_target(&BLONDE, s, v)
        } else if s <= SAT_GATE || (120.0..300.0).contains(&h) {
            // mid/bright and desaturated or cool: the blue eyeshadow (hue 272 @
            // S 0.10 @ V 0.29) and the sclera. Left alone.
            hits[3] += 1.0;
            continue;
        } else if v < SKIN_V {
            // cross-fade the hairline, brows and lash lines
            hits[1] += 1.0;
            let t = (v - HAIR_V) / (SKIN_V - HAIR_V);
            let (ah, asat, av) = apply_target(&BLONDE, s, v);
            let (bh, bsat, bv) = apply_target(&SKIN, s, v);
            (
                ah + (bh - ah) * t,
                asat + (bsat - asat) * t,
                av + (bv - av) * t,
            )
        } else {
            hits[2] += 1.0;
            apply_target(&SKIN, s, v)
        };
        let (nr, ng, nb) = hsv_to_rgb(nh, ns, nv);
        write_px(p, nr, ng, nb);
    }
    for x in &mut hits {
        *x /= total;
    }
    Ok(hits)
}

/// Body: the bun (UV box + dark warm) goes blonde, the garment band goes light
/// blue, the accent bands follow the chosen flavour, neutrals are left alone.
/// Returns (bun%, garment%, accent%, kept%).
fn paint_body(img: &mut Image, accents: &[Band]) -> Result<[f32; 4]> {
    let (w, h_px) = (img.width as usize, img.height as usize);
    let px = rgba8_mut(img)?;
    let total = (w * h_px) as f32;
    let mut hits = [0f32; 4];
    for (i, p) in px.chunks_exact_mut(4).enumerate() {
        let (x, y) = (i % w, i / w);
        // texture rows run top-down; mesh UV v runs bottom-up
        let u = (x as f32 + 0.5) / w as f32;
        let v_uv = 1.0 - (y as f32 + 0.5) / h_px as f32;
        let in_bun_box = (BUN_U.0..=BUN_U.1).contains(&u) && (BUN_V.0..=BUN_V.1).contains(&v_uv);

        let (r, g, b) = (
            f32::from(p[0]) / 255.0,
            f32::from(p[1]) / 255.0,
            f32::from(p[2]) / 255.0,
        );
        let (hue, s, v) = rgb_to_hsv(r, g, b);

        // The bun: dark, inside its own UV rectangle. Checked first so the
        // near-black hair never falls through to the neutral or warm-leather keep
        // rules and stays black. As on the head, hue is unreliable this dark, so
        // the test excludes only what is unmistakably the surrounding periwinkle
        // coat rather than requiring a warm hue.
        let coat_in_box = (200.0..300.0).contains(&hue) && s > 0.25;
        if in_bun_box && v <= 0.20 && !coat_in_box {
            hits[0] += 1.0;
            let (nh, ns, nv) = apply_target(&BLONDE, s, v);
            let (nr, ng, nb) = hsv_to_rgb(nh, ns, nv);
            write_px(p, nr, ng, nb);
            continue;
        }

        if s <= SAT_GATE || v <= VAL_GATE {
            hits[3] += 1.0; // white shirt, grey gun, charcoal
            continue;
        }

        let band = if in_band(hue, &GARMENT) {
            hits[1] += 1.0;
            Some(&GARMENT)
        } else if let Some(bd) = accents.iter().find(|bd| in_band(hue, bd)) {
            hits[2] += 1.0;
            Some(bd)
        } else {
            hits[3] += 1.0; // gold trim + warm leather: the warm complement
            None
        };
        if let Some(bd) = band {
            let nh = bd.hue.unwrap_or(hue);
            let ns = (s * bd.sat).clamp(0.0, 1.0);
            let nv = remap(v, bd.v_in, bd.v_out);
            let (nr, ng, nb) = hsv_to_rgb(nh, ns, nv);
            write_px(p, nr, ng, nb);
        }
    }
    for x in &mut hits {
        *x /= total;
    }
    Ok(hits)
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut accents = "harmony".to_string();
    if let Some(i) = args.iter().position(|a| a == "--accents") {
        accents = args
            .get(i + 1)
            .context("--accents needs a flavour (harmony | mono | warm)")?
            .clone();
        args.drain(i..=i + 1);
    }
    if args.len() < 2 {
        anyhow::bail!(
            "usage: reskin_nano_porcelain_blonde <pak01_dir.vpk> (<out_dir.vpk> | --png <prefix>) [--accents harmony|mono|warm]"
        );
    }
    let pak = args[0].clone();
    let png_mode = args[1] == "--png";
    let target = if png_mode {
        args.get(2).context("--png needs an output prefix")?.clone()
    } else {
        args[1].clone()
    };
    let accent_spec = accent_bands(&accents)?;
    println!("accents: {accents}");

    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();
    for (name, file) in [("head", HEAD), ("body", BODY)] {
        let entry = format!("{DIR}{file}");
        let bytes = vpkmerge_core::read_vpk_entry(&pak, &entry)
            .with_context(|| format!("reading {entry}"))?;
        let info = morphic::inspect(&bytes)?;
        let mut img = morphic::decode(&bytes).with_context(|| format!("decoding {name}"))?;
        let hits = if name == "head" {
            let h = paint_head(&mut img)?;
            println!(
                "head  {:?} {}x{}  hair {:>5.1}%  hairline {:>4.1}%  skin {:>5.1}%  kept {:>5.1}%",
                info.format,
                info.width,
                info.height,
                h[0] * 100.0,
                h[1] * 100.0,
                h[2] * 100.0,
                h[3] * 100.0
            );
            h
        } else {
            let h = paint_body(&mut img, &accent_spec)?;
            println!(
                "body  {:?} {}x{}  bun  {:>5.2}%  garment {:>5.1}%  accents {:>4.1}%  kept {:>5.1}%",
                info.format,
                info.width,
                info.height,
                h[0] * 100.0,
                h[1] * 100.0,
                h[2] * 100.0,
                h[3] * 100.0
            );
            h
        };
        // a zero-hit primary region means the selectors missed: fail loud rather
        // than quietly bake the stock texture
        anyhow::ensure!(
            hits[0] > 0.0,
            "{name}: no pixels matched the primary region"
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
