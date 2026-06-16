// Paradox "Living Opal" -- the magnum-opus reactive skin.
//
// V2 PBR OVERHAUL: the v1 skin painted a nice opal texture but it read bland.
// v2 attacked that with the texture/roughness/reactivity levers that DON'T need
// a static-combo feature flip:
//   * sharper HARLEQUIN play-of-color (distinct spectral cells, dark matrix).
//   * wet, SPATIALLY-VARYING roughness (crests near-mirror, valleys satin) so
//     even the toon highlight reads as a polished gem, not satin plastic.
//   * a bolder whole-surface iridescence flash (v1's albedo wash was only 22%).
//
// IN-GAME FINDING (2026-06-14): the two big PBR levers I also tried -- flipping
// F_USE_NPR_LIGHTING off and enabling F_SHEEN -- are STATIC-COMBO feature flags,
// and flipping them post-compile renders the engine ERROR SHADER (red wireframe)
// on hero materials. So the sheen lobe is OUT, and NPR-off is opt-in only
// (--npr-off), isolated behind --probe-npr to test whether it survives ALONE
// (F_SHEEN is the likely primary culprit). Default bake stays on the toon path.
//
// It fuses every animation/reactivity lever we have proven, into ONE coherent
// identity on the chrono (Paradox) body:
//
//   base art   : a black-opal substrate albedo (dark gem matrix) + a flowing
//                ripple normal-roughness override (the liquid-metal mechanism:
//                animate the SURFACE, reflections crawl).
//   scroll     : slow normal + albedo + self-illum scroll so the substrate and
//                its micro-structure drift (g_v*ScrollSpeed1, no feature flag).
//   gloss      : wet, spatially-varying roughness (crests near-mirror).
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
//   # isolate the NPR-off flip ALONE on STOCK chrono (does real PBR specular
//   # survive post-compile, or red-wireframe like the NPR+SHEEN combo did?):
//   cargo run --release --example reskin_chrono_living_opal -- <pak01_dir.vpk> <out_dir.vpk> --probe-npr
//   # de-risk: expr-only on STOCK chrono materials (the multi-expression container):
//   cargo run --release --example reskin_chrono_living_opal -- <pak01_dir.vpk> <out_dir.vpk> --probe
//   # full bake (toon path, known-good). Add --npr-off ONLY if --probe-npr passed:
//   cargo run --release --example reskin_chrono_living_opal -- <pak01_dir.vpk> <out_dir.vpk> [--npr-off]
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
// Stock AO: the region-segmentation guide. It encodes her mechanical STRUCTURE
// (tubes, buckles, hourglass frame, strap hardware) as high-local-contrast detail,
// vs the smooth leather coat -> we split the single body texture into zones.
const BODY_AO: &str = "models/heroes_staging/chrono/materials/chrono_v2_ao_png_9ef0831f.vtex_c";
const BODY_VMAT: &str = "models/heroes_staging/chrono/materials/chrono_v2.vmat_c";
const GUN_COLOR: &str =
    "models/heroes_staging/chrono/chrono_gun/materials/chrono_gun_vmat_g_tcolor_7d4419c1.vtex_c";
const GUN_NORMAL: &str =
    "models/heroes_staging/chrono/chrono_gun/materials/chrono_gun_vmat_g_tnormalroughness_7cd9ceac.vtex_c";
const GUN_VMAT: &str = "models/heroes_staging/chrono/chrono_gun/materials/chrono_gun.vmat_c";
// Skin ACCENTS (the glowing head/shoulder pieces): tinted to match the opal body.
// The two self-illum accents (hourglass, shoulder) take the SAME camera-orbit
// iridescence as the body so the whole hero shimmers in sync; the glass dome has
// no self-illum so it gets a deep static opal-glass tint + matching outline.
const HEADGLASS_VMAT: &str = "models/heroes_staging/chrono/materials/chrono_v2_headglass.vmat_c";
const HOURGLASS_VMAT: &str =
    "models/heroes_staging/chrono/materials/chrono_v2_head_hourglass.vmat_c";
const SHOULDER_VMAT: &str = "models/heroes_staging/chrono/materials/chrono_shoulder.vmat_c";

// UV units/sec. Slow + diagonal = a gentle opal shimmer (not a racing scroll).
const FLOW_NORMAL: [f64; 2] = [0.03, 0.022]; // surface ripple crawl
const FLOW_ALBEDO: [f64; 2] = [0.011, 0.008]; // substrate micro-structure drift
const FLOW_SELFILLUM: [f64; 2] = [0.018, 0.013]; // emissive mask drift

// Wet gem gloss, SPATIALLY VARYING (set per-pixel in the roughness B channel):
// wave crests read near-mirror, the matrix valleys read satin. v1's flat 0.22
// satin is what made it look like painted plastic; a polished opal is wet.
const ROUGHNESS_WET: f32 = 0.07; // crests: wet polished gem (sharp env highlight)
const ROUGHNESS_SATIN: f32 = 0.32; // valleys / matrix: soft sheen
                                   // Constant TextureRoughness1 fallback (unbound sampler) -- keep it wet.
const ROUGHNESS_LOW: f64 = ROUGHNESS_WET as f64;
// Broaden the Fresnel rim so the iridescence catches across more of the silhouette.
const FRESNEL_EXP: f64 = 2.0;
// Self-illum mask range, applied to the FLECKS ONLY (LO = dark matrix glow ~0,
// HI = fleck glow). Kept gentle so the flecks get a soft inner fire without
// flooding the body to a fullbright blob (the v2 mistake + the Viscous lesson).
// Raised so the colour zones GLOW (self-illum w/ g_flSelfIllumAlbedoFactor1=1 ->
// the opal play-of-color emits, visible even in shadow; the toon-path in-game
// result was dark/muddy because diffuse albedo dies in shadow). Matrix LO stays
// ~0 so it reads as a *black* opal with internal fire, not a fullbright blob.
// With NPR-off, real environment reflections light the surface, so the glow is
// MODERATE (inner opal fire), not a flood -- too much self-illum hides the
// reflections that make it read as a wet gem. (On the toon fallback this is just
// a gentle fire.)
// Matrix base glow raised 0.02 -> 0.11 (anti-muddy): the toon path kills diffuse
// albedo in shadow, so the opal read brown/muddy on her back in-game. A faint
// self-illum floor keeps the opal alive even unlit, WITHOUT flooding to fullbright
// (the v2 mistake) -- the matrix is still clearly the dark part of a black opal.
const EMISSIVE_LO: f32 = 0.11;
const EMISSIVE_HI: f32 = 0.55;
// The mechanical hardware (AO-structure zones: tubes, buckles, hourglass frame)
// emits opal so it reads as energized tech, not dead metal. Moderate (not a flood).
const METAL_GLOW: f32 = 0.6;
// Strong glow for the STOCK structural regions (the back hourglass, the tubes
// running through her, the energy trim) that the stock self-illum mask lights up.
// We KEEP those regions (composite the opal flecks OVER them) so the designed
// energy structure reads as bright opal instead of being washed flat by the
// fleck field -- self-illum takes the albedo colour, so they glow opal.
const STRUCT_HI: f32 = 0.88;

// ---------------------------------------------------------------------------
// WHITE (milky) opal variant. `--white` selects it; the default stays the
// in-game-confirmed BLACK opal toon build, byte-for-byte unchanged. A black opal
// blazes vivid play-of-color against a near-black matrix; a milk opal inverts
// that -- a bright translucent body with SOFT pastel colour floating in it, a
// gentle uniform inner glow (subsurface milkiness), and a touch more satin on the
// surface (milk-glass diffusion, not a black mirror). The three improvements the
// white build carries over the black one: richer two-scale play-of-color, a
// multi-scale AO region split, and a tuned milky glow.
// ---------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq, Eq)]
enum Variant {
    Black,
    White,
}

// Milky surface: crests still wet, but the satin floor lifts so the matrix reads
// as diffusing milk-glass instead of a black mirror.
const ROUGHNESS_WET_WHITE: f32 = 0.10;
const ROUGHNESS_SATIN_WHITE: f32 = 0.42;
// Milky inner light: a real emissive floor (the whole body glows softly,
// subsurface) -> fire patches a bit hotter. Still well below fullbright (the v2
// flood mistake) so it reads as translucent milk, not neon.
const EMISSIVE_LO_WHITE: f32 = 0.30;
const EMISSIVE_HI_WHITE: f32 = 0.62;
// Accents, milk-opal palette: a pale cool glass dome + soft pastel outline, and a
// pale milky base under the hourglass/shoulder iridescence (vs black's deep violet).
const DOME_TINT_WHITE: [f64; 3] = [0.60, 0.65, 0.78];
const DOME_OUTLINE_WHITE: [f64; 3] = [0.72, 0.82, 0.96];
const SHOULDER_ALBEDO_WHITE: [f64; 3] = [0.86, 0.87, 0.93];

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

// 1. Albedo tint (g_vColorTint1): a bolder whole-surface hue FLASH as you orbit
//    (v1's 22% wash was too timid -- the body looked static). Still keeps the
//    baked harlequin colours dominant; this shifts/flashes them, leaning crimson
//    at low HP. The play-of-color now reads as "living" across the whole face.
fn expr_color_tint(variant: Variant) -> String {
    match variant {
        Variant::Black => {
            let shimmer = gentle(
                &iridescent(&view_phase(0.95, 0.16)),
                "float3(0.92,0.90,1.0)",
                0.42,
            );
            // blend toward a soft crimson wash only as health drops
            format!("lerp(float3(0.95,0.5,0.5),{shimmer},smoothstep(0.20,0.55,$ent_health))")
        }
        Variant::White => {
            // milk opal: a soft pastel shimmer over a warm-white neutral, leaning
            // to a gentle fire-opal rose at low HP. Gentler blend (0.28) so the
            // bright milky albedo stays dominant -- no mood-ring flood.
            let shimmer = gentle(
                &iridescent(&view_phase(0.60, 0.14)),
                "float3(1.0,0.98,0.96)",
                0.28,
            );
            format!("lerp(float3(1.0,0.72,0.70),{shimmer},smoothstep(0.20,0.55,$ent_health))")
        }
    }
}

// 2. Fresnel rim (g_vSelfIllumFresnelMaskTint1): the tasteful camera hook -- the
//    grazing rim sheen shifts hue as you orbit. Moderate (rim only, small area).
fn expr_fresnel_tint(variant: Variant) -> String {
    match variant {
        Variant::Black => gentle(
            &iridescent(&view_phase(1.2, 0.22)),
            "float3(0.8,0.8,0.9)",
            0.6,
        ),
        Variant::White => gentle(
            &iridescent(&view_phase(0.75, 0.20)),
            "float3(0.92,0.94,1.0)",
            0.5,
        ),
    }
}

// 3. Self-illum scale: LOW base (flecks get a soft fire, not a flood) + a gentle
//    surge as health drops. Also the required non-empty self-illum expression.
//    White runs a lower base -- its emissive FLOOR is already higher (milky
//    subsurface glow), so a big scale on top would blow the body out.
fn expr_selfillum_scale(variant: Variant) -> &'static str {
    match variant {
        Variant::Black => "1.25+(1.0-$ent_health)*1.5",
        Variant::White => "0.9+(1.0-$ent_health)*1.1",
    }
}

// The KNOWN-GOOD reactive set: dynamic expressions on params that ALREADY exist
// on stock chrono_v2 (g_vColorTint1 / g_vSelfIllumFresnelMaskTint1 /
// g_flSelfIllumScale1). v1 confirmed this multi-expression container loads on
// chrono. NO sheen expression here -- F_SHEEN can't be enabled post-compile
// (red-wireframe in-game), so g_vSheenColorTint1 has no lobe to drive.
fn reactive_edits(variant: Variant) -> anyhow::Result<Vec<VmatEdit>> {
    Ok(vec![
        VmatEdit::expr("g_vSelfIllumFresnelMaskTint1", &expr_fresnel_tint(variant))?,
        VmatEdit::expr("g_vColorTint1", &expr_color_tint(variant))?,
        VmatEdit::expr("g_flSelfIllumScale1", expr_selfillum_scale(variant))?,
    ])
}

// ---------------------------------------------------------------------------
// ACCENT reactivity. The glowing hourglass + shoulder are self-illum accents
// (F_SELF_ILLUM, albedoFactor 0/1), so a self-illum tint expression recolors the
// glow directly -- drive them with the SAME camera iridescence as the body rim so
// the whole hero's play-of-color shimmers in sync. A self-illum expr is present on
// each (Yearlu's gotcha satisfied), so they evaluate.
// ---------------------------------------------------------------------------

// Hourglass glow: a VIVID iridescent sweep over a deep opal-violet base (it is the
// centerpiece, so bolder than the body rim), flushing crimson at low HP like the
// body. g_flSelfIllumAlbedoFactor1 is 0 here, so this tint IS the glow colour.
fn expr_hourglass_tint(variant: Variant) -> String {
    match variant {
        Variant::Black => {
            let shimmer = gentle(
                &iridescent(&view_phase(1.1, 0.20)),
                "float3(0.30,0.20,0.65)",
                0.78,
            );
            format!("lerp(float3(0.95,0.22,0.30),{shimmer},smoothstep(0.20,0.55,$ent_health))")
        }
        Variant::White => {
            // pale milky base, still a vivid centerpiece sweep but pastel; rose
            // (fire-opal) flush at low HP to match the white body tint.
            let shimmer = gentle(
                &iridescent(&view_phase(0.72, 0.18)),
                "float3(0.72,0.76,0.92)",
                0.70,
            );
            format!("lerp(float3(1.0,0.66,0.62),{shimmer},smoothstep(0.20,0.55,$ent_health))")
        }
    }
}

// Hourglass Fresnel rim: a phase-offset iridescence so the grazing edge shimmers a
// different hue than the face (the thin-film depth cue).
fn expr_hourglass_rim(variant: Variant) -> String {
    match variant {
        Variant::Black => gentle(
            &iridescent(&view_phase(1.35, 0.26)),
            "float3(0.6,0.6,0.9)",
            0.7,
        ),
        Variant::White => gentle(
            &iridescent(&view_phase(0.90, 0.24)),
            "float3(0.85,0.88,1.0)",
            0.6,
        ),
    }
}

// Hourglass glow scale: keep the strong native glow (~4.2) + a low-HP surge.
fn expr_hourglass_scale() -> &'static str {
    "4.0+(1.0-$ent_health)*2.5"
}

// Shoulder shares the identity (albedoFactor 1 -> glow = albedo*tint, so we null
// the tan albedo to neutral first, below, and let this iridescence read clean).
fn expr_shoulder_scale() -> &'static str {
    "8.0+(1.0-$ent_health)*4.0"
}

fn hourglass_edits(variant: Variant) -> anyhow::Result<Vec<VmatEdit>> {
    Ok(vec![
        VmatEdit::expr("g_vSelfIllumTint1", &expr_hourglass_tint(variant))?,
        VmatEdit::expr("g_vSelfIllumFresnelMaskTint1", &expr_hourglass_rim(variant))?,
        VmatEdit::expr("g_flSelfIllumScale1", expr_hourglass_scale())?,
    ])
}

fn shoulder_edits(variant: Variant) -> anyhow::Result<Vec<VmatEdit>> {
    Ok(vec![
        VmatEdit::expr("g_vSelfIllumTint1", &expr_hourglass_tint(variant))?,
        VmatEdit::expr("g_flSelfIllumScale1", expr_shoulder_scale())?,
    ])
}

// Deep opal-glass tint for the dome (a *black* opal: dark, faintly violet, so it
// reads as polished gem glass) + recolor the stock RED solid outline to opal
// violet so the silhouette matches.
const DOME_TINT: [f64; 3] = [0.07, 0.06, 0.15];
const DOME_OUTLINE: [f64; 3] = [0.45, 0.12, 0.85];
// Neutral-ish grey for the shoulder albedo (was tan) so its iridescent glow is clean.
const SHOULDER_ALBEDO: [f64; 3] = [0.80, 0.80, 0.86];

// IN-GAME FINDING (2026-06-14): flipping the static-combo feature flags
// `F_USE_NPR_LIGHTING` / `F_SHEEN` post-compile renders the engine ERROR SHADER
// (red wireframe) on hero materials -- the combined probe proved it. F_SHEEN is
// the likely primary culprit (needs a sheen combo+sampler that isn't compiled
// into chrono's variant). So sheen is dead and NPR-off is OPT-IN only, isolated
// behind `--probe-npr` / `--npr-off` to test whether NPR-off ALONE survives.
fn npr_off_edit() -> Vec<VmatEdit> {
    vec![VmatEdit::Int {
        name: "F_USE_NPR_LIGHTING".to_string(),
        value: 0,
    }]
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

// ---------------------------------------------------------------------------
// GEM DEPTH (--depth): the toon path bands specular, so flat albedo + ripple
// read as a painted surface. To fake a cut/layered gem with real internal
// structure we add a faceted Voronoi field (adapted from reskin_vindicta_geode):
//   * a COARSE cell layer = big colour domains that read as colour suspended at
//     depth inside the stone (the opal "layers"), each a gently-tilted facet.
//   * a FINE cell layer = micro crystal sparkle + seam glints (reflectivity).
//   * seam/trough darkening (AO) so domains read RECESSED, bright pinfire on top
//     reads as the polished surface = a tonal stack that fakes depth.
// Opal is a smooth cabochon, so the tilt is gentle (vs the geode's cut quartz).
// Periodic over a grid torus so the scroll still tiles seamlessly.
// ---------------------------------------------------------------------------
const FACET_GRID_COARSE: i32 = 8; // big internal colour domains
const FACET_GRID_FINE: i32 = 30; // micro crystal sparkle
const FACET_TILT: f32 = 0.34; // gentle facet tilt (~19deg; opal is smooth)

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

struct Facet {
    nx: f32, // tangent-space normal X/Y in ~[-1,1]
    ny: f32,
    seam: f32, // 1 near a cell border (glint / pinfire)
    ao: f32,   // 1 = lit, <1 in crevices/troughs
    lit: f32,  // 1 toward a cell seed (facet centre catches light)
    cwx: i32,  // coarse cell coords (drive the colour domain)
    cwy: i32,
}

fn facet_field(u: f32, v: f32) -> Facet {
    let (f1, f2, cwx, cwy) = voronoi(u, v, FACET_GRID_COARSE);
    // per-cell facet orientation (constant across the cell, hard seams = facets)
    let phi = cell_hash(cwx, cwy, 3) * TAU;
    let theta = (0.4 + 0.6 * cell_hash(cwx, cwy, 4)) * FACET_TILT;
    let (mut nx, mut ny) = (theta.sin() * phi.cos(), theta.sin() * phi.sin());
    // round the very centre slightly flatter (cabochon, not a sharp point)
    let center = 1.0 - smoothstep_f(0.0, 0.06, f1);
    nx *= 1.0 - 0.35 * center;
    ny *= 1.0 - 0.35 * center;
    // fine micro-relief perturbation
    let (g1, g2, gx, gy) = voronoi(u + 0.21, v + 0.13, FACET_GRID_FINE);
    let mphi = cell_hash(gx, gy, 5) * TAU;
    nx += 0.12 * mphi.cos();
    ny += 0.12 * mphi.sin();
    let seam_c = 1.0 - smoothstep_f(0.0, 0.035, f2 - f1);
    let seam_f = 1.0 - smoothstep_f(0.0, 0.022, g2 - g1);
    let seam = seam_c.max(seam_f * 0.85).min(1.0);
    let trough = smoothstep_f(0.0, 0.12, f1);
    let ao = (1.0 - 0.6 * seam) * (0.82 + 0.18 * (1.0 - trough));
    let lit = (1.0 - (f1 / 0.09).min(1.0)).powf(1.5);
    Facet {
        nx,
        ny,
        seam,
        ao,
        lit,
        cwx,
        cwy,
    }
}

// Depth opal colour: a per-DOMAIN hue (colour lives in the coarse facet cell, so
// it reads as a 3D region), recessed by AO, with bright pinfire on the seams =
// a layered stack (colour at depth + sparkle on the surface).
fn opal_pixel_depth(u: f32, v: f32) -> [f32; 3] {
    let f = facet_field(u, v);
    // hue: coarse-cell domain + a little intra-domain drift (full wheel)
    let hue =
        (cell_hash(f.cwx, f.cwy, 8) * 2.4 + fbm(u * 1.4 + 4.0, v * 1.4 + 7.0, 3, 3) * 0.28).fract();
    let fire = fleck_mask(u, v);
    // value: deep-black matrix, fire on the crests, lifted toward each facet's lit
    // centre. AO only GENTLY recesses (0.7..1.0) so domains keep saturated colour
    // to EMIT (the self-illum samples this albedo) instead of crushing to mud.
    let val = (0.10 + 0.9 * fire.powf(1.3)) * (0.7 + 0.3 * f.ao) * (0.85 + 0.4 * f.lit);
    let sat = 0.82 + 0.16 * fire;
    let mut c = hsv2rgb(hue, sat.min(1.0), val.min(1.0));
    // pinfire: near-white sparks mostly on grain PEAKS (the surface sparkle layer),
    // only a faint seam contribution so cells read as smooth colour domains, not
    // leaded-glass grout -- the depth comes from the faceted NORMALS, not painted
    // borders.
    let grain = fbm(u + 2.0, v + 5.0, 5, 3);
    let spark =
        (f.seam * 0.28 + smoothstep_f(0.80, 0.97, height(u, v) * 0.5 + grain * 0.5)).min(1.0);
    for ci in c.iter_mut() {
        *ci += spark * (0.97 - *ci) * 0.5;
    }
    c
}

// Black-opal play-of-color. The KEY to the opal look: the colour is SPATIAL
// (many hues across the surface at once), not a single camera-driven tint. A
// low-frequency hue field paints coloured zones; the fire field sets how bright
// each is, so adjacent regions read different colours over a dark hued matrix =
// real opal. The camera expressions then only add a subtle living shimmer.
fn opal_pixel(variant: Variant, u: f32, v: f32) -> [f32; 3] {
    match variant {
        Variant::Black => opal_pixel_black(u, v),
        Variant::White => opal_pixel_white(u, v),
    }
}

fn opal_pixel_black(u: f32, v: f32) -> [f32; 3] {
    // spatial hue field, HIGHER-FREQUENCY and QUANTIZED into many distinct
    // spectral cells = the harlequin mosaic of a real black opal (v1's smooth
    // low-freq gradient read as one purple wash). The hue spans the FULL wheel
    // (greens, blues, oranges, the prized reds), not a narrow band. A little
    // intra-cell drift keeps the cells from looking posterized-flat.
    let raw = fbm(u * 2.0 + 4.0, v * 2.0 + 7.0, 3, 4);
    let cells = 9.0;
    let q = (raw * cells).floor() / cells;
    let hue = q * 2.7 + 0.08 + fbm(u * 3.1 + 1.0, v * 3.1 + 9.0, 4, 2) * 0.05;
    let fire = fleck_mask(u, v);
    // value: DEEP near-black matrix in the valleys (it is a *black* opal) ->
    // blazing fire on the crests. fire^1.4 = a sharp dark/bright split that still
    // lets colour bleed across most of the face (opal is fiery, not two specks).
    let val = 0.07 + 0.92 * fire.powf(1.4);
    let sat = 0.80 + 0.18 * fire;
    let mut c = hsv2rgb(hue, sat, val);
    // hot near-white pinfire sparks on the very brightest grains
    let grain = fbm(u + 2.0, v + 5.0, 5, 3);
    let spark = smoothstep_f(0.82, 0.97, height(u, v) * 0.5 + grain * 0.5);
    for ci in c.iter_mut() {
        *ci += spark * (0.97 - *ci) * 0.55;
    }
    c
}

// Milk-opal play-of-color (IMPROVED, two-scale). A bright translucent body with
// SOFT pastel colour floating in it -- the inverse of the black opal's dark
// matrix + blazing fire. Three richness levers over the black build's single
// quantized field:
//   * a COARSE quantized harlequin field (the big colour domains)
//   * a FINER intra-domain hue drift (so domains aren't posterized flat)
//   * a slow diagonal SCHILLER sweep (the directional colour sheen real opal
//     shows when you tilt it) -- a small hue push, not a wash.
// Brightness is near-constant (milky); it is the SATURATION that the fire field
// modulates (low-sat creamy matrix -> pastel colour patches), capped so it never
// goes black-opal-gemmy.
fn opal_pixel_white(u: f32, v: f32) -> [f32; 3] {
    let coarse = fbm(u * 2.0 + 4.0, v * 2.0 + 7.0, 3, 4);
    let cells = 11.0;
    let q = (coarse * cells).floor() / cells;
    let fine = fbm(u * 5.0 + 1.0, v * 5.0 + 9.0, 5, 3);
    let schiller = 0.5 + 0.5 * (TAU * (0.8 * u + 1.3 * v)).sin();
    let hue = q * 2.7 + 0.10 + fine * 0.12 + schiller * 0.18;
    let fire = fleck_mask(u, v);
    // bright milky value (fire only lifts it a touch); pastel chroma on the fire
    // patches over a near-white low-sat matrix.
    let val = 0.76 + 0.15 * fire.powf(1.1);
    let sat = (0.06 + 0.46 * fire.powf(1.25)).min(0.52);
    let mut c = hsv2rgb(hue, sat, val);
    // pin-point colour sparks (milk-opal pinfire): tiny brighter flecks rather
    // than the black build's near-white sparks (which would vanish on a pale body).
    let grain = fbm(u + 2.0, v + 5.0, 5, 3);
    let spark = smoothstep_f(0.86, 0.98, height(u, v) * 0.5 + grain * 0.5);
    for ci in c.iter_mut() {
        *ci = (*ci + spark * 0.35 * (1.0 - *ci)).min(1.0);
    }
    c
}

// ---------------------------------------------------------------------------
// AO-DRIVEN REGION SEGMENTATION. chrono's body is ONE material with overlapping
// UVs and NO vertex-color/tint mask, so the only region signal is the textures'
// own content. The stock AO map encodes the mechanical hardware (tubes, buckles,
// the hourglass frame, strap fittings) as high-local-contrast detail, vs the
// smooth leather coat. `structure(u,v)` ~1 over hardware, ~0 over leather; we use
// it to make the hardware read as crisp jeweled metal (wetter, brighter, glowing)
// while the coat stays the deep opal matrix.
// ---------------------------------------------------------------------------
struct AoMask {
    w: usize,
    h: usize,
    g: Vec<f32>,
}
impl AoMask {
    fn from_image(img: &Image) -> anyhow::Result<Self> {
        let (w, h) = (img.width as usize, img.height as usize);
        let px = match &img.data {
            ImageData::Rgba8(v) => v,
            ImageData::Rgba16F(_) => anyhow::bail!("unexpected HDR AO map"),
        };
        let g = (0..w * h).map(|i| px[i * 4] as f32 / 255.0).collect();
        Ok(Self { w, h, g })
    }
    fn at(&self, u: f32, v: f32) -> f32 {
        let x = ((u.fract() + 1.0).fract() * self.w as f32) as usize % self.w;
        let y = ((v.fract() + 1.0).fract() * self.h as f32) as usize % self.h;
        self.g[y * self.w + x]
    }
    // Local AO contrast in a cross+diagonal window of radius `d`: hard hardware
    // edges have a big light-to-dark swing; smooth leather barely changes.
    fn contrast(&self, u: f32, v: f32, d: f32) -> f32 {
        let s = [
            self.at(u, v),
            self.at(u + d, v),
            self.at(u - d, v),
            self.at(u, v + d),
            self.at(u, v - d),
            self.at(u + d, v + d),
            self.at(u - d, v - d),
        ];
        let mut lo = 1.0f32;
        let mut hi = 0.0f32;
        for &x in &s {
            lo = lo.min(x);
            hi = hi.max(x);
        }
        hi - lo
    }

    // Mechanical-structure weight (black build): single tight-kernel contrast.
    fn structure(&self, u: f32, v: f32) -> f32 {
        smoothstep_f(0.10, 0.38, self.contrast(u, v, 2.0 / self.w as f32))
    }

    // IMPROVED multi-scale structure (white build): combine a FINE kernel (tube
    // edges, buckle teeth) with a BROADER one (the hourglass frame, strap plates),
    // so both crisp detail and big hardware regions get the jeweled-metal
    // treatment -- the single tight cross missed the broad plates and left them
    // reading as coat.
    fn structure_ms(&self, u: f32, v: f32) -> f32 {
        let fine = smoothstep_f(0.10, 0.38, self.contrast(u, v, 2.0 / self.w as f32));
        let broad = smoothstep_f(0.14, 0.46, self.contrast(u, v, 5.0 / self.w as f32));
        fine.max(broad * 0.85)
    }

    // Region weight by variant: white uses the improved multi-scale split.
    fn region(&self, variant: Variant, u: f32, v: f32) -> f32 {
        match variant {
            Variant::Black => self.structure(u, v),
            Variant::White => self.structure_ms(u, v),
        }
    }
}

fn load_ao(pak: &str) -> anyhow::Result<AoMask> {
    let bytes = vpkmerge_core::read_vpk_entry(pak, BODY_AO)?;
    AoMask::from_image(&morphic::decode(&bytes)?)
}

// Emissive mask = ONLY the fire flecks (not the whole body), and gentle. So the
// flecks carry a soft inner glow while the dark matrix stays dark. Single-channel
// ATI1N/BC4; set all RGBA the same and let replace_mip_chain re-encode BC4.
fn paint_emissive_mask(
    img: &mut Image,
    depth: bool,
    variant: Variant,
    ao: Option<&AoMask>,
) -> anyhow::Result<()> {
    let (elo, ehi) = match variant {
        Variant::Black => (EMISSIVE_LO, EMISSIVE_HI),
        Variant::White => (EMISSIVE_LO_WHITE, EMISSIVE_HI_WHITE),
    };
    let (w, h) = (img.width, img.height);
    let px = rgba8_mut(img)?;
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let i = ((y * w + x) * 4) as usize;
            // STOCK structural glow: read the stock self-illum value at this texel
            // (the back hourglass, tubes, and trim are the lit regions) and keep it
            // as a strong opal glow, so the energy structure pops off the dark coat.
            let stock = px[i] as f32 / 255.0;
            let structural = smoothstep_f(0.10, 0.45, stock) * STRUCT_HI;
            // AO-structure (the mechanical hardware) also emits, so the tubes/frame
            // read as energized even where the stock mask left them dark.
            let metal_glow = ao.map_or(0.0, |a| a.region(variant, u, v)) * METAL_GLOW;
            // The opal fire flecks over the coat (the existing field).
            let fleck = if depth {
                let f = facet_field(u, v);
                (elo + (ehi - elo) * fleck_mask(u + 0.03, v + 0.02) + 0.22 * f.seam).min(0.85)
            } else {
                elo + (ehi - elo) * fleck_mask(u, v)
            };
            // Union: structure / hardware win where they exist, flecks elsewhere.
            let m = structural.max(fleck).max(metal_glow);
            let b = byte(m);
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
fn paint_thumbnail(img: &mut Image, depth: bool, variant: Variant) -> anyhow::Result<()> {
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
            let su = (x as f32 / w as f32) * 1.3;
            let sv = (y as f32 / h as f32) * 1.3;
            let mut c = if depth {
                opal_pixel_depth(su, sv)
            } else {
                opal_pixel(variant, su, sv)
            };
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

fn paint_albedo(
    img: &mut Image,
    smooth: bool,
    depth: bool,
    variant: Variant,
    ao: Option<&AoMask>,
) -> anyhow::Result<()> {
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
            let mut rgb = if depth {
                opal_pixel_depth(su, sv)
            } else {
                opal_pixel(variant, su, sv)
            };
            // AO-structure zones (mechanical hardware) read as JEWELED METAL: lift
            // value (it catches light) and pull toward its brightest channel (a
            // metallic specular tint) so the hardware separates from the matte coat.
            if let Some(ao) = ao {
                let m = ao.region(variant, u, v);
                if m > 0.001 {
                    match variant {
                        Variant::Black => {
                            let lift = 1.0 + 0.7 * m;
                            for c in rgb.iter_mut() {
                                *c = (*c * lift).min(1.0);
                            }
                            let mx = rgb[0].max(rgb[1]).max(rgb[2]);
                            for c in rgb.iter_mut() {
                                *c += m * 0.3 * (mx - *c);
                            }
                        }
                        Variant::White => {
                            // polished pearl/steel hardware on the milky body: keep
                            // it bright but pull it cooler and slightly desaturated
                            // so it reads as metal, not just more milk.
                            let lift = 1.0 + 0.45 * m;
                            for c in rgb.iter_mut() {
                                *c = (*c * lift).min(1.0);
                            }
                            let steel = [0.86, 0.90, 0.98];
                            for (c, &s) in rgb.iter_mut().zip(steel.iter()) {
                                *c += m * 0.30 * (s - *c);
                            }
                        }
                    }
                }
            }
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
fn paint_normal_roughness(
    img: &mut Image,
    gamma_precomp: bool,
    depth: bool,
    variant: Variant,
    ao: Option<&AoMask>,
) -> anyhow::Result<()> {
    let (wet, satin) = match variant {
        Variant::Black => (ROUGHNESS_WET, ROUGHNESS_SATIN),
        Variant::White => (ROUGHNESS_WET_WHITE, ROUGHNESS_SATIN_WHITE),
    };
    let (w, h) = (img.width, img.height);
    let eps = 2.0 / w as f32;
    let bump = 0.05;
    let px = rgba8_mut(img)?;
    for y in 0..h {
        let v = y as f32 / h as f32;
        for x in 0..w {
            let u = x as f32 / w as f32;
            let n = surface_normal(u, v, eps, bump);
            // In depth mode the faceted crystal normal is the PRIMARY relief (the
            // ripple is demoted to a fine perturbation), and roughness is keyed to
            // the facets: centres wet/mirror, edges rougher, seams glint-sharp.
            let (nx, ny, rough) = if depth {
                let f = facet_field(u, v);
                let nx = (f.nx + n[0] * 0.3).clamp(-1.0, 1.0);
                let ny = (f.ny + n[1] * 0.3).clamp(-1.0, 1.0);
                let r = (ROUGHNESS_WET + 0.18 * (1.0 - f.lit) - 0.05 * f.seam).clamp(0.035, 0.5);
                (nx, ny, r)
            } else {
                // Wet on the wave crests, satin in the valleys = facet-flash gloss
                // (the polished-vs-matrix break a flat roughness can't give).
                let slope = (1.0 - n[2]).clamp(0.0, 1.0);
                let crest = smoothstep_f(0.45, 0.85, height(u, v));
                let r = (wet + (satin - wet) * (1.0 - crest) + 0.05 * slope).clamp(0.04, 0.5);
                (n[0], n[1], r)
            };
            // Hardware (AO-structure) reads as polished jeweled metal: pull its
            // roughness toward the wet/near-mirror end so it glints sharply.
            let rough = if let Some(ao) = ao {
                let m = ao.region(variant, u, v);
                (rough - m * (rough - wet)).clamp(0.035, 0.5)
            } else {
                rough
            };
            let nrx = nx * 0.5 + 0.5;
            let nry = ny * 0.5 + 0.5;
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

fn rough_low_edits(v: &Value, variant: Variant) -> Vec<(Vec<Seg>, f64)> {
    let r = match variant {
        Variant::Black => ROUGHNESS_LOW,
        Variant::White => ROUGHNESS_WET_WHITE as f64,
    };
    vcomp_edits(v, "TextureRoughness1", &[(0, r), (1, r), (2, r)])
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
fn body_double_patches(bytes: &[u8], variant: Variant) -> anyhow::Result<(Vec<u8>, usize)> {
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
    all.extend(rough_low_edits(&v, variant));
    all.extend(fscalar_edit(
        &v,
        "g_flSelfIllumFresnelMaskExponent",
        FRESNEL_EXP,
    ));
    // NOTE: chrono's self-illum already takes the ALBEDO colour
    // (g_flSelfIllumAlbedoFactor1 is a tagless 1.0) and g_vSelfIllumTint1 is
    // already white, so the opal play-of-color glows from within for free. We do
    // NOT patch those: 1.0 has no double-table slot (tagless), so a double-patch
    // attempt fails and (batch being all-or-nothing) would silently drop EVERY
    // edit. The glow is driven entirely by the brighter emissive mask
    // (EMISSIVE_HI) + the self-illum scale expression instead.
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
    let mut probe_npr = false;
    let mut npr_off = false;
    let mut depth = false;
    let mut white = false;
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
            "--probe-npr" => {
                probe_npr = true;
                i += 1;
            }
            "--npr-off" => {
                npr_off = true;
                i += 1;
            }
            "--depth" => {
                depth = true;
                i += 1;
            }
            "--white" => {
                white = true;
                i += 1;
            }
            _ => {
                pos.push(raw[i].clone());
                i += 1;
            }
        }
    }
    let variant = if white {
        Variant::White
    } else {
        Variant::Black
    };
    // The faceted gem-depth mode was rejected by the user and tuned for the dark
    // matrix; the milk opal is a smooth cabochon, so --white forces depth off.
    if depth && variant == Variant::White {
        eprintln!("note: --depth is a black-opal-only mode; ignoring it for --white");
    }
    let depth = depth && variant == Variant::Black;
    let pak = pos.first().cloned().expect(
        "usage: reskin_chrono_living_opal <pak01_dir.vpk> <out_dir.vpk|--png prefix> \
         [--probe] [--gamma-precomp]",
    );
    let arg2 = pos
        .get(1)
        .cloned()
        .expect("second arg: <out_dir.vpk> or --png <prefix>");

    let variant_name = match variant {
        Variant::Black => "BLACK opal (dark matrix, vivid fire)",
        Variant::White => "WHITE / milk opal (pale body, pastel fire)",
    };
    eprintln!(
        "Living Opal v2 (Paradox/chrono)  variant={variant_name}  probe={probe}  \
         probe_npr={probe_npr}  npr_off={npr_off}  depth={depth}  gamma_precomp={gamma_precomp}"
    );
    eprintln!("reactive expressions:");
    eprintln!(
        "  fresnel rim  g_vSelfIllumFresnelMaskTint1 = {}",
        expr_fresnel_tint(variant)
    );
    eprintln!(
        "  body tint    g_vColorTint1                = {}",
        expr_color_tint(variant)
    );
    eprintln!(
        "  selfillum    g_flSelfIllumScale1          = {}",
        expr_selfillum_scale(variant)
    );

    // --- preview mode: render the opal albedo + ripple normal, no game needed.
    if arg2 == "--png" {
        let prefix = pos.get(2).cloned().expect("--png needs an output prefix");
        let ao = load_ao(&pak)?;
        let blank = || Image {
            width: 768,
            height: 768,
            data: ImageData::Rgba8(vec![255u8; 768 * 768 * 4]),
        };
        let mut pa = blank();
        paint_albedo(&mut pa, false, depth, variant, Some(&ao))?;
        let mut pn = blank();
        paint_normal_roughness(&mut pn, gamma_precomp, depth, variant, Some(&ao))?;
        let mut pt = Image {
            width: 512,
            height: 512,
            data: ImageData::Rgba8(vec![255u8; 512 * 512 * 4]),
        };
        paint_thumbnail(&mut pt, depth, variant)?;
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

    // --- probe-npr mode: F_USE_NPR_LIGHTING=0 ALONE on the STOCK chrono body.
    //     The combined NPR+SHEEN probe red-wireframed in-game; this isolates
    //     whether NPR-off by ITSELF survives (F_SHEEN is the likely culprit --
    //     it needs a sheen combo+sampler chrono's variant may not carry). If the
    //     body renders normally with continuous (non-toon) specular, NPR-off is
    //     salvageable and can be folded into the full bake via --npr-off.
    if probe_npr {
        let body_vmat_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_VMAT)?;
        let (patched, stats) = patch_vmat_params(&body_vmat_bytes, &npr_off_edit())?;
        report_stats("npr-off", &stats);
        anyhow::ensure!(
            stats.failed.is_empty(),
            "F_USE_NPR_LIGHTING could not be set -- the lighting flip is the whole point"
        );
        let readme = b"Living Opal NPR-OFF PROBE -- chrono body, vmat ONLY (stock textures).\n\
            Flips ONLY F_USE_NPR_LIGHTING -> 0 (real PBR specular, no toon banding),\n\
            no sheen, no other edits. In-game test: the body should render NORMALLY\n\
            but glossier, with a continuous specular highlight instead of toon\n\
            cel-bands. If it renders red wireframe / error shader, NPR-off is also\n\
            unhonored post-compile -- the skin must stay fully on the toon path\n\
            (textures + existing-param expressions only).\n";
        vpkmerge_core::pack(
            &[
                (BODY_VMAT, patched.as_slice()),
                ("README.txt", readme.as_slice()),
            ],
            &out,
        )?;
        println!("wrote npr-off probe VPK: {out}");
        return Ok(());
    }

    // --- probe mode: ONLY the three expressions, on the STOCK chrono body
    //     material. Smallest possible in-game test of whether a multi-expression
    //     LZ4-native container loads on this hero (the one unproven risk).
    if probe {
        let body_vmat_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_VMAT)?;
        let (patched, stats) = patch_vmat_params(&body_vmat_bytes, &reactive_edits(variant)?)?;
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
    let ao = load_ao(&pak)?;
    eprintln!(
        "AO region mask loaded ({}x{}) -> hardware/leather segmentation",
        ao.w, ao.h
    );

    let mut body_albedo = morphic::decode(&body_color_bytes)?;
    paint_albedo(&mut body_albedo, false, depth, variant, Some(&ao))?;
    let new_body_color = morphic::replace_mip_chain(&body_color_bytes, &body_albedo)?;

    let mut body_nr = morphic::decode(&body_color_bytes)?;
    paint_normal_roughness(&mut body_nr, gamma_precomp, depth, variant, Some(&ao))?;
    let new_body_nr = morphic::replace_mip_chain(&body_color_bytes, &body_nr)?;

    // Emissive mask override: use the emissive texture's OWN container (4096 ATI1N)
    // so dims/format match (replace_mip_chain cannot resize/reformat).
    let emissive_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_EMISSIVE)?;
    let mut body_emissive = morphic::decode(&emissive_bytes)?;
    paint_emissive_mask(&mut body_emissive, depth, variant, Some(&ao))?;
    let new_body_emissive = morphic::replace_mip_chain(&emissive_bytes, &body_emissive)?;
    eprintln!("emissive mask overridden (whole body now self-illum-eligible)");

    // Gun: no AO segmentation (different mesh/UVs; the body AO would misalign), so
    // keep its smooth opal flow as-is.
    let mut gun_albedo = morphic::decode(&body_color_bytes)?;
    paint_albedo(&mut gun_albedo, true, depth, variant, None)?;
    let new_gun_color = morphic::replace_mip_chain(&body_color_bytes, &gun_albedo)?;
    let mut gun_nr = morphic::decode(&body_color_bytes)?;
    paint_normal_roughness(&mut gun_nr, gamma_precomp, depth, variant, None)?;
    let new_gun_nr = morphic::replace_mip_chain(&body_color_bytes, &gun_nr)?;
    eprintln!("textures: body + gun opal albedo + ripple normal-roughness re-encoded");

    // body vmat: double-patches first (preserve framing), then OPTIONALLY the
    // NPR-off flip (opt-in, --npr-off; default stays on the proven toon path),
    // then the dynamic expressions last (blob-aware insert).
    let body_vmat_bytes = vpkmerge_core::read_vpk_entry(&pak, BODY_VMAT)?;
    let (body_doubled, n_doubles) = body_double_patches(&body_vmat_bytes, variant)?;
    eprintln!("body vmat: {n_doubles} in-place double edits (scroll + gloss + Fresnel)");
    let body_pbr = if npr_off {
        let (b, pbr_stats) = patch_vmat_params(&body_doubled, &npr_off_edit())?;
        report_stats("npr-off (opt-in)", &pbr_stats);
        anyhow::ensure!(
            pbr_stats.failed.is_empty(),
            "F_USE_NPR_LIGHTING could not be set"
        );
        b
    } else {
        eprintln!("npr-off: skipped (toon path; pass --npr-off to flip, only if the probe passed)");
        body_doubled
    };
    let (new_body_vmat, body_stats) = patch_vmat_params(&body_pbr, &reactive_edits(variant)?)?;
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
    gun_edits.extend(rough_low_edits(&gv, variant));
    let (new_gun_vmat, n_gun) = patch_optional(gun_vmat_bytes.clone(), &gun_edits);
    eprintln!("gun vmat: {n_gun} in-place double edits (flow + gloss)");

    // --- ACCENTS: tint the glowing head/shoulder pieces to match the opal body ---
    // Hourglass: static framing untouched, iridescent self-illum expressions added.
    // Accent palettes by variant: black = deep opal-violet; white = pale milky.
    let (dome_tint, dome_outline, shoulder_albedo) = match variant {
        Variant::Black => (DOME_TINT, DOME_OUTLINE, SHOULDER_ALBEDO),
        Variant::White => (DOME_TINT_WHITE, DOME_OUTLINE_WHITE, SHOULDER_ALBEDO_WHITE),
    };

    let hourglass_bytes = vpkmerge_core::read_vpk_entry(&pak, HOURGLASS_VMAT)?;
    let (new_hourglass_vmat, hg_stats) =
        patch_vmat_params(&hourglass_bytes, &hourglass_edits(variant)?)?;
    report_stats("hourglass expressions", &hg_stats);
    anyhow::ensure!(
        hg_stats.failed.is_empty(),
        "an hourglass expression failed to inject -- aborting"
    );

    // Head glass dome: deep opal-glass tint + opal outline (static; no self-illum).
    let headglass_bytes = vpkmerge_core::read_vpk_entry(&pak, HEADGLASS_VMAT)?;
    let hgl = morphic::decode_kv3_resource(&headglass_bytes)?;
    let mut dome_edits = vcomp_edits(
        &hgl,
        "TextureColor1",
        &[(0, dome_tint[0]), (1, dome_tint[1]), (2, dome_tint[2])],
    );
    dome_edits.extend(vcomp_edits(
        &hgl,
        "g_vSolidOutlineTint",
        &[
            (0, dome_outline[0]),
            (1, dome_outline[1]),
            (2, dome_outline[2]),
        ],
    ));
    let (new_headglass_vmat, n_dome) = patch_optional(headglass_bytes.clone(), &dome_edits);
    eprintln!("head glass dome: {n_dome} opal-glass tint + outline edits");

    // Shoulder: null the tan albedo to neutral (so the iridescent glow reads clean),
    // then add the same iridescent self-illum expressions. Best-effort: this is the
    // un-versioned material, so if the model does not bind it the override is inert.
    let shoulder_bytes = vpkmerge_core::read_vpk_entry(&pak, SHOULDER_VMAT)?;
    let sh = morphic::decode_kv3_resource(&shoulder_bytes)?;
    let shoulder_neutral = vcomp_edits(
        &sh,
        "TextureColor1",
        &[
            (0, shoulder_albedo[0]),
            (1, shoulder_albedo[1]),
            (2, shoulder_albedo[2]),
        ],
    );
    let (shoulder_neutralized, n_sh) = patch_optional(shoulder_bytes.clone(), &shoulder_neutral);
    let (new_shoulder_vmat, sh_stats) =
        patch_vmat_params(&shoulder_neutralized, &shoulder_edits(variant)?)?;
    report_stats("shoulder expressions", &sh_stats);
    eprintln!("shoulder: {n_sh} albedo-neutralize edit(s) + iridescent self-illum");

    let substrate = match variant {
        Variant::Black => {
            "BLACK-opal substrate albedo (sharp harlequin cells, deep-black matrix);\n\
             vivid play-of-color blazes against the dark body."
        }
        Variant::White => {
            "WHITE / MILK-opal substrate albedo (bright translucent body, soft pastel\n\
             play-of-color: two-scale harlequin + schiller sheen) with a gentle\n\
             uniform inner glow (milky subsurface) and a touch more satin surface."
        }
    };
    let readme = format!(
        "Paradox \"Living Opal\" v2 ({variant_name}) -- reactive magnum-opus skin\n\
        ====================================================\n\
        vpkmerge test build. Hero: Paradox (chrono).\n\n\
        {substrate}\n\n\
        Flowing ripple normal with WET, variable roughness + slow scroll, with\n\
        THREE dynamic expressions:\n\
          1. Fresnel rim tint  -> grazing rim iridescence sweeps hue as you ORBIT\n\
             (thin-film play-of-color via $camera_origin/$ent_origin).\n\
          2. Body color tint   -> whole-surface hue flash, flushes warm at low\n\
             $ent_health.\n\
          3. Self-illum scale  -> base glow + low-HP surge.\n\n\
        Stays on the toon lighting path (the NPR-off + F_SHEEN PBR levers render\n\
        the error shader post-compile on hero materials). npr_off={npr_off}.\n\
        In-game checks: surface reads as a polished gem; orbit -> rim hue sweep;\n\
        take damage -> warm flush + glow surge. gamma_precomp={gamma_precomp}.\n"
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
            (HOURGLASS_VMAT, new_hourglass_vmat.as_slice()),
            (HEADGLASS_VMAT, new_headglass_vmat.as_slice()),
            (SHOULDER_VMAT, new_shoulder_vmat.as_slice()),
            ("README.txt", readme.as_bytes()),
        ],
        &out,
    )?;
    println!("wrote addon VPK: {out}");
    Ok(())
}
