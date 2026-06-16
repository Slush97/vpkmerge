// Paradox "Fractal" -- a real escape-time fractal baked into the hero surface.
//
// The dream was "Mandelbrot sets on the skin, reacting to game events." The hard
// wall: dynamic expressions are a per-frame, per-material SCALAR VM with no loop,
// and we cannot author a custom fragment shader (Source 2 ships precompiled .vcs;
// no HLSL->.vcs compiler on our Linux pipeline). So a per-pixel runtime fractal
// is out. The loop instead runs ONCE, in Rust, at bake time -- a genuine
// escape-time iteration per texel -- and freezes into the body albedo. The proven
// reactivity levers (texture override + scroll + injected dynamic expressions)
// then animate and game-react on top. Same plumbing as reskin_chrono_living_opal.
//
//   base art   : escape-time Julia (default) or Mandelbrot albedo -- glowing
//                filaments near the set boundary over a dark substrate. Julia is
//                the default: it fills the plane organically, so the hero's
//                fragmented/mirrored UVs read as "fractal everywhere" rather than
//                a broken picture (the kintsugi lesson: organic hides UV seams).
//   gloss      : low uniform roughness so the filaments catch a sheen.
//   emissive   : the filament-brightness field becomes the self-illum mask, so the
//                fractal glows along its boundary.
//   scroll     : slow albedo + self-illum scroll so the surface drifts.
//   REACTIVITY : two dynamic expressions injected LZ4-native (vmat --set-expr):
//                  1. g_vColorTint1     <- neutral, washes to hot crimson as
//                     $ent_health drops (the fractal "heats up" when you're hurt).
//                  2. g_flSelfIllumScale1 <- base filament glow + a low-HP surge.
//                     Also the required non-empty self-illum expression (Yearlu's
//                     gotcha: no expression evaluates unless a self-illum one is).
//
// usage:
//   # preview the baked fractal, no game needed:
//   cargo run --release --example reskin_chrono_fractal -- <pak01_dir.vpk> --png <prefix>
//   cargo run --release --example reskin_chrono_fractal -- <pak01_dir.vpk> --png <prefix> --mandel
//   # de-risk: expr-only on the STOCK chrono body (cheapest $ent_health test):
//   cargo run --release --example reskin_chrono_fractal -- <pak01_dir.vpk> <out_dir.vpk> --probe
//   # full bake (static reactive: health flush + glow):
//   cargo run --release --example reskin_chrono_fractal -- <pak01_dir.vpk> <out_dir.vpk>
//   # maximally-live bake (two interfering layers w/ constant counter-scroll,
//   # age-cycling glow, camera-iridescent Fresnel rim, health flush):
//   cargo run --release --example reskin_chrono_fractal -- <pak01_dir.vpk> <out_dir.vpk> --live
use morphic::{Image, ImageData, TextureFormat};
use vpkmerge_core::{patch_vmat_params, VmatEdit};

const BODY_COLOR: &str =
    "models/heroes_staging/chrono/materials/chrono_v2_color_png_d1d22ba7.vtex_c";
const BODY_NORMAL: &str =
    "models/heroes_staging/chrono/materials/chrono_v2_vmat_g_tnormalroughness_ce38f34.vtex_c";
const BODY_EMISSIVE: &str =
    "models/heroes_staging/chrono/materials/chrono_v2_emissive_png_718bd18c.vtex_c";
const BODY_VMAT: &str = "models/heroes_staging/chrono/materials/chrono_v2.vmat_c";

// Slow diagonal drift (UV units/sec). No feature flag needed (the scroll lesson).
// CONSTANT (never velocity-driven; see reactive_edits). The two layers drift at
// different, counter-directed speeds so they slide past each other into a smooth,
// non-repeating interference.
const FLOW_ALBEDO: [f64; 2] = [0.010, 0.007];
const FLOW_SELFILLUM: [f64; 2] = [-0.012, 0.013];
// Uniform low roughness = sheen on the filaments (the Vindicta-dress lesson).
const ROUGHNESS_LOW: f64 = 0.24;

// Escape-time budget. 256 is plenty for a skin-resolution render and keeps the
// bake fast even at 4096 (the body container resolution).
const MAX_ITER: u32 = 256;
const BAILOUT: f32 = 256.0; // large bailout -> smoother continuous coloring

// Default Julia constant: a classic filamentary dendrite (-0.7269 + 0.1889i).
const JULIA_C: [f32; 2] = [-0.7269, 0.1889];
// --live self-illum layer uses a SECOND, visibly different Julia constant so it
// is a distinct pattern from the albedo; scrolled independently, the two frozen
// layers interfere into live moire (the closest thing to an emergent computed
// pattern). A bulbous Julia (-0.391 + 0.587i) reads clearly against the dendrite.
const JULIA_C2: [f32; 2] = [-0.391, 0.587];
// --live: broaden the self-illum Fresnel rim so the camera iridescence catches
// across more of the silhouette (best-effort float patch).
const FRESNEL_EXP: f64 = 2.0;
// Julia view window in the complex plane (centered, square).
const JULIA_HALF: f32 = 1.55;
// Mandelbrot view: the seahorse-valley neighborhood, zoomed enough to show the
// self-similar detail without dissolving into noise.
const MANDEL_CENTER: [f32; 2] = [-0.745, 0.113];
const MANDEL_HALF: f32 = 0.075;

// ---------------------------------------------------------------------------
// The bake-time loop. Returns (escaped, smooth_iter). smooth_iter is the
// continuous (fractional) escape count: n + 1 - log2(log|z|), which removes the
// banding you get from the raw integer count. Interior points return escaped=false.
// ---------------------------------------------------------------------------
fn escape(mut zx: f32, mut zy: f32, cx: f32, cy: f32) -> (bool, f32) {
    for n in 0..MAX_ITER {
        let x2 = zx * zx;
        let y2 = zy * zy;
        if x2 + y2 > BAILOUT {
            let mag = (x2 + y2).sqrt();
            let mu = n as f32 + 1.0 - (mag.ln().ln() / std::f32::consts::LN_2);
            return (true, mu);
        }
        // z = z^2 + c
        let nzy = 2.0 * zx * zy + cy;
        zx = x2 - y2 + cx;
        zy = nzy;
    }
    (false, MAX_ITER as f32)
}

// Julia at UV (0..1) for an explicit constant: z0 = pixel, c = const.
fn julia(u: f32, v: f32, c: [f32; 2]) -> (bool, f32) {
    let zx = (u * 2.0 - 1.0) * JULIA_HALF;
    let zy = (v * 2.0 - 1.0) * JULIA_HALF;
    escape(zx, zy, c[0], c[1])
}

// Sample the fractal at UV (0..1). Julia iterates z0 = pixel, c = const; Mandelbrot
// iterates z0 = 0, c = pixel.
fn sample(u: f32, v: f32, mandel: bool) -> (bool, f32) {
    if mandel {
        let cx = MANDEL_CENTER[0] + (u * 2.0 - 1.0) * MANDEL_HALF;
        let cy = MANDEL_CENTER[1] + (v * 2.0 - 1.0) * MANDEL_HALF;
        escape(0.0, 0.0, cx, cy)
    } else {
        julia(u, v, JULIA_C)
    }
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

fn smoothstep_f(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// Filament brightness in 0..1: points NEAR the set escape slowly (high smooth
// iter), so brightness rises toward the boundary; the deep exterior is dark.
fn brightness(escaped: bool, mu: f32) -> f32 {
    if !escaped {
        return 0.0; // interior = unlit substrate
    }
    smoothstep_f(0.0, MAX_ITER as f32 * 0.45, mu)
}

// Fractal albedo pixel: teal->violet hue banded by iteration count, value driven
// by filament brightness, over a deep near-black substrate (so g_vColorTint1 and
// the glow have a dark canvas to read against, the opal lesson).
fn fractal_pixel(u: f32, v: f32, mandel: bool) -> [f32; 3] {
    let (escaped, mu) = sample(u, v, mandel);
    let b = brightness(escaped, mu);
    if !escaped {
        // dark hued substrate, very faint so the interior is not a flat void
        return hsv2rgb(0.62, 0.55, 0.06);
    }
    // Cohesive Paradox palette: map filament brightness across a constrained
    // teal->violet->magenta band (no full-spectrum wrap, which reads as confetti
    // on the hero's fragmented UVs). Dim filaments stay teal; the bright boundary
    // peaks push toward violet/magenta. A slow iteration term adds gentle banding
    // without cycling the whole wheel.
    let hue = 0.50 + 0.32 * b + (mu * 0.004).sin() * 0.03;
    let sat = 0.78 - 0.18 * b; // hotter filaments desaturate slightly toward white
    let val = 0.10 + 0.90 * b;
    let mut c = hsv2rgb(hue, sat, val);
    // hot near-white sparkle on the very brightest filaments
    let spark = smoothstep_f(0.86, 1.0, b);
    for ci in c.iter_mut() {
        *ci += spark * (0.97 - *ci) * 0.6;
    }
    c
}

fn byte(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

fn rgba8_mut(img: &mut Image) -> anyhow::Result<&mut Vec<u8>> {
    match &mut img.data {
        ImageData::Rgba8(v) => Ok(v),
        ImageData::Rgba16F(_) => anyhow::bail!("unexpected HDR texture"),
    }
}

fn paint_albedo(img: &mut Image, mandel: bool) -> anyhow::Result<()> {
    let (w, h) = (img.width, img.height);
    let px = rgba8_mut(img)?;
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let rgb = fractal_pixel(u, v, mandel);
            let i = ((y * w + x) * 4) as usize;
            px[i] = byte(rgb[0]);
            px[i + 1] = byte(rgb[1]);
            px[i + 2] = byte(rgb[2]);
            px[i + 3] = 255;
        }
    }
    Ok(())
}

// Self-illum mask = filament brightness. Single-channel ATI1N/BC4: set all the
// same and let replace_mip_chain re-encode BC4. In --live mode this is a SECOND
// Julia (JULIA_C2), a distinct pattern from the albedo, so when the two layers
// scroll at different (velocity-reactive) speeds the glow drifts across the base
// and the surface reads as a live, never-repeating interference.
fn paint_emissive_mask(img: &mut Image, mandel: bool, live: bool) -> anyhow::Result<()> {
    let (w, h) = (img.width, img.height);
    let px = rgba8_mut(img)?;
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let (escaped, mu) = if live {
                julia(u, v, JULIA_C2)
            } else {
                sample(u, v, mandel)
            };
            let b = byte(brightness(escaped, mu) * 0.7); // gentle, not a flood
            let i = ((y * w + x) * 4) as usize;
            px[i] = b;
            px[i + 1] = b;
            px[i + 2] = b;
            px[i + 3] = b;
        }
    }
    Ok(())
}

// Flat tangent normal (0,0,1) + uniform low roughness in B. Gives a clean glossy
// surface so the baked filaments catch a sheen, without inventing surface relief.
fn paint_normal_roughness(img: &mut Image) -> anyhow::Result<()> {
    let (w, h) = (img.width, img.height);
    let px = rgba8_mut(img)?;
    let rough = byte(ROUGHNESS_LOW as f32);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            px[i] = 128; // normal.x = 0
            px[i + 1] = 128; // normal.y = 0
            px[i + 2] = rough; // roughness (linear slot)
            px[i + 3] = 255;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Reactive expressions (compiled by morphic::vfx_expr to engine bytecode).
// ---------------------------------------------------------------------------
// Neutral white normally; washes to hot crimson as health drops -> the fractal
// "heats up" when you take damage. A faint time() shimmer keeps it alive.
fn expr_color_tint() -> String {
    "lerp(float3(1.0,0.30,0.18),\
     float3(0.95+0.05*sin(time()),0.97,1.0),\
     smoothstep(0.20,0.60,$ent_health))"
        .to_string()
}
// Base filament glow + a surge as health drops. Required non-empty self-illum expr.
fn expr_selfillum_scale() -> &'static str {
    "0.9+(1.0-$ent_health)*2.0"
}

// --- --live expressions ----------------------------------------------------
// 3 cosines 120deg apart = a hue wheel; `phase` walks it.
fn iridescent(phase: &str) -> String {
    format!(
        "float3(0.5+0.5*cos({phase}),0.5+0.5*cos(({phase})+2.0944),0.5+0.5*cos(({phase})+4.1888))"
    )
}

// Glow color slowly cycles the spectrum with time alive ($ent_age), so the
// self-illum interference layer is never a fixed color.
fn expr_selfillum_tint() -> String {
    iridescent("$ent_age*0.5")
}

// Fresnel rim: a genuine per-pixel (rim) channel. normalize($camera_origin -
// $ent_origin) projected on a fixed axis sweeps the hue as you ORBIT the camera;
// a slow time() term keeps it alive when nobody moves.
fn expr_fresnel_tint() -> String {
    iridescent(
        "(dot3(normalize($camera_origin-$ent_origin),float3(2.6,1.5,0.4))*1.2+time()*0.22)*3.14159265",
    )
}

// NOTE: scroll is NOT velocity-reactive. The engine computes scroll offset as
// scrollSpeed * time, so making scrollSpeed track the (spiky, unsmoothable)
// $ent_abs_velocity multiplies that noise by an ever-growing time term -> the
// texture teleports/jitters whenever you move. Scroll stays CONSTANT (two
// different, counter-directed speeds for smooth interference); the live channels
// below use only SMOOTH inputs (health, age, camera orbit).

fn reactive_edits(live: bool) -> anyhow::Result<Vec<VmatEdit>> {
    let mut edits = vec![
        VmatEdit::expr("g_vColorTint1", &expr_color_tint())?,
        VmatEdit::expr("g_flSelfIllumScale1", expr_selfillum_scale())?,
    ];
    if live {
        edits.push(VmatEdit::expr("g_vSelfIllumTint1", &expr_selfillum_tint())?);
        edits.push(VmatEdit::expr(
            "g_vSelfIllumFresnelMaskTint1",
            &expr_fresnel_tint(),
        )?);
    }
    Ok(edits)
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
    let mut mandel = false;
    let mut probe = false;
    let mut live = false;
    let mut pos: Vec<String> = Vec::new();
    for a in &raw {
        match a.as_str() {
            "--mandel" => mandel = true,
            "--probe" => probe = true,
            "--live" => live = true,
            _ => pos.push(a.clone()),
        }
    }
    let pak = pos.first().cloned().expect(
        "usage: reskin_chrono_fractal <pak01_dir.vpk> <out_dir.vpk|--png prefix> \
         [--mandel] [--live] [--probe]",
    );
    let arg2 = pos
        .get(1)
        .cloned()
        .expect("second arg: <out_dir.vpk> or --png <prefix>");

    let kind = if mandel { "Mandelbrot" } else { "Julia" };
    eprintln!("Fractal skin (Paradox/chrono)  set={kind}  live={live}  probe={probe}");
    eprintln!("reactive expressions:");
    eprintln!("  body tint  g_vColorTint1        = {}", expr_color_tint());
    eprintln!(
        "  selfillum  g_flSelfIllumScale1  = {}",
        expr_selfillum_scale()
    );
    if live {
        eprintln!(
            "  glow tint  g_vSelfIllumTint1     = {}",
            expr_selfillum_tint()
        );
        eprintln!(
            "  fresnel    g_vSelfIllumFresnelMaskTint1 = {}",
            expr_fresnel_tint()
        );
        eprintln!("  scroll: CONSTANT (two counter-directed layers; not velocity-driven)");
    }

    // --- preview: render the baked albedo + emissive mask, no game needed.
    if arg2 == "--png" {
        let prefix = pos.get(2).cloned().expect("--png needs an output prefix");
        let blank = |n: u32| Image {
            width: n,
            height: n,
            data: ImageData::Rgba8(vec![255u8; (n * n * 4) as usize]),
        };
        let mut pa = blank(1024);
        paint_albedo(&mut pa, mandel)?;
        let mut pe = blank(512);
        paint_emissive_mask(&mut pe, mandel, live)?;
        for (img, suffix) in [(&pa, "albedo"), (&pe, "emissive")] {
            let png = morphic::encode_image(img, TextureFormat::PngRgba8888)?;
            let path = format!("{prefix}_{suffix}.png");
            std::fs::write(&path, &png)?;
            println!("wrote {path} ({}x{})", img.width, img.height);
        }
        return Ok(());
    }
    let out = arg2;

    // --- probe: ONLY the two expressions, on the STOCK chrono body material.
    //     Cheapest in-game test of whether hero entities publish $ent_health.
    if probe {
        let body_vmat_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_VMAT)?;
        let (patched, stats) = patch_vmat_params(&body_vmat_bytes, &reactive_edits(live)?)?;
        report_stats("body expressions", &stats);
        anyhow::ensure!(
            stats.failed.is_empty(),
            "an expression failed to inject -- aborting probe"
        );
        let readme = b"Fractal PROBE -- chrono body, expressions ONLY (stock textures).\n\
            In-game test: at full HP the body should look normal; take damage and\n\
            it should flush crimson and glow harder. If it reads red/hot at FULL HP,\n\
            hero entities do not publish $ent_health (reads 0) and the reactive\n\
            axis needs a different attribute. If it renders red wireframe, the\n\
            expression container did not load on chrono.\n";
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
    // The body's 4096 BC7 container is the template for albedo + normal overrides
    // (replace_mip_chain keeps the source dims/format, so reuse it for both).
    let body_color_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_COLOR)?;

    let mut body_albedo = morphic::decode(&body_color_bytes)?;
    paint_albedo(&mut body_albedo, mandel)?;
    let new_body_color = morphic::replace_mip_chain(&body_color_bytes, &body_albedo)?;

    let mut body_nr = morphic::decode(&body_color_bytes)?;
    paint_normal_roughness(&mut body_nr)?;
    let new_body_nr = morphic::replace_mip_chain(&body_color_bytes, &body_nr)?;

    // Emissive uses its OWN container (dims/format must match the slot).
    let emissive_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_EMISSIVE)?;
    let mut body_emissive = morphic::decode(&emissive_bytes)?;
    paint_emissive_mask(&mut body_emissive, mandel, live)?;
    let new_body_emissive = morphic::replace_mip_chain(&emissive_bytes, &body_emissive)?;
    let layer2 = if live {
        " (layer-2 Julia for interference)"
    } else {
        ""
    };
    eprintln!(
        "textures: {kind} albedo + gloss normal-roughness + filament emissive{layer2} re-encoded"
    );

    // body vmat: gloss/scroll double-patches first (preserve framing), then inject
    // the reactive expressions (blob-aware LZ4-native insert). In --live mode the
    // scroll vectors are expression-owned, so apply_scroll_gloss skips them.
    let body_vmat_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_VMAT)?;
    let body_doubled = apply_scroll_gloss(&body_vmat_bytes, live)?;
    let (new_body_vmat, body_stats) = patch_vmat_params(&body_doubled, &reactive_edits(live)?)?;
    report_stats("body expressions", &body_stats);
    anyhow::ensure!(
        body_stats.failed.is_empty(),
        "a body expression failed to inject -- aborting"
    );

    let readme = if live {
        format!(
            "Paradox \"Fractal LIVE\" -- escape-time {kind} skin\n\
            ===============================================\n\
            vpkmerge test build. Hero: Paradox (chrono).\n\n\
            Two escape-time Julia layers (a dendrite albedo + a bulbous self-illum\n\
            mask), iterated per-texel in Rust at bake time and frozen, then made to\n\
            LIVE via dynamic expressions over game state:\n\
              1. Body color tint   -> washes to hot crimson as $ent_health drops.\n\
              2. Self-illum scale  -> base glow + low-HP surge.\n\
              3. Glow tint         -> cycles the spectrum with $ent_age.\n\
              4. Fresnel rim       -> iridescence that sweeps as you ORBIT (camera).\n\
            The two layers scroll at CONSTANT, counter-directed speeds so they slide\n\
            past each other into a smooth, never-repeating interference. (Scroll is\n\
            NOT velocity-driven: offset = speed*time, so velocity-driven scroll\n\
            jitters; movement reactivity is not viable without temporal smoothing.)\n\n\
            In-game checks: slow drifting interference + rim iridescence that sweeps\n\
            as you ORBIT; take damage -> crimson flush + glow surge.\n\
            No per-pixel runtime fractal exists (no shader loop); the spatial\n\
            structure is baked, the LIFE is expression-driven transform + interference.\n"
        )
    } else {
        format!(
            "Paradox \"Fractal\" -- escape-time {kind} skin\n\
            ==========================================\n\
            vpkmerge test build. Hero: Paradox (chrono).\n\n\
            A real escape-time {kind} set, iterated per-texel in Rust at bake time and\n\
            frozen into the body albedo (glowing filaments over a dark substrate),\n\
            with low-roughness gloss, a filament self-illum mask, slow scroll, and TWO\n\
            dynamic expressions:\n\
              1. Body color tint  -> washes to hot crimson as $ent_health drops.\n\
              2. Self-illum scale -> base glow + a low-HP surge.\n\n\
            In-game checks: take damage -> the fractal heats up (crimson + glow surge).\n\
            Per-pixel runtime fractal is impossible (no shader loop); the loop is baked.\n"
        )
    };

    vpkmerge_core::pack(
        &[
            (BODY_COLOR, new_body_color.as_slice()),
            (BODY_NORMAL, new_body_nr.as_slice()),
            (BODY_EMISSIVE, new_body_emissive.as_slice()),
            (BODY_VMAT, new_body_vmat.as_slice()),
            ("README.txt", readme.as_bytes()),
        ],
        &out,
    )?;
    println!("wrote addon VPK: {out}");
    Ok(())
}

// Best-effort in-place scroll + gloss double-patches (no re-encode). Renamed or
// absent params are skipped so they can never sink the bake. Scroll is always a
// CONSTANT here (both static and live), because velocity-driven scroll jitters
// (offset = scrollSpeed * time). --live additionally broadens the Fresnel rim.
fn apply_scroll_gloss(bytes: &[u8], live: bool) -> anyhow::Result<Vec<u8>> {
    use morphic::kv3::{Seg, Value};
    let v = morphic::decode_kv3_resource(bytes)?;
    let vidx = |name: &str| -> Option<usize> {
        v.get("m_vectorParams")?
            .as_array()?
            .iter()
            .position(|p| p.get("m_name").and_then(Value::as_str) == Some(name))
    };
    let mut edits: Vec<(Vec<Seg>, f64)> = Vec::new();
    let mut push_xy = |name: &str, xy: [f64; 2]| {
        if let Some(i) = vidx(name) {
            for (k, val) in [(0usize, xy[0]), (1, xy[1])] {
                edits.push((
                    vec![
                        Seg::Key("m_vectorParams".to_string()),
                        Seg::Index(i),
                        Seg::Key("m_value".to_string()),
                        Seg::Index(k),
                    ],
                    val,
                ));
            }
        }
    };
    push_xy("g_vAlbedoScrollSpeed1", FLOW_ALBEDO);
    push_xy("g_vSelfIllumScrollSpeed1", FLOW_SELFILLUM);
    if let Some(i) = vidx("TextureRoughness1") {
        for k in 0..3 {
            edits.push((
                vec![
                    Seg::Key("m_vectorParams".to_string()),
                    Seg::Index(i),
                    Seg::Key("m_value".to_string()),
                    Seg::Index(k),
                ],
                ROUGHNESS_LOW,
            ));
        }
    }
    // Fresnel-rim exponent (float param) so the camera-iridescence rim is wide.
    if live {
        if let Some(i) = v
            .get("m_floatParams")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter().position(|p| {
                    p.get("m_name").and_then(Value::as_str)
                        == Some("g_flSelfIllumFresnelMaskExponent")
                })
            })
        {
            edits.push((
                vec![
                    Seg::Key("m_floatParams".to_string()),
                    Seg::Index(i),
                    Seg::Key("m_flValue".to_string()),
                ],
                FRESNEL_EXP,
            ));
        }
    }
    if edits.is_empty() {
        return Ok(bytes.to_vec());
    }
    match morphic::patch_kv3_resource_doubles(bytes, &edits) {
        Ok(b) => Ok(b),
        Err(_) => Ok(bytes.to_vec()),
    }
}
