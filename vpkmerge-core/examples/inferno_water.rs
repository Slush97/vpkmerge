// Retarget Infernus's ability + weapon VFX from FIRE to WATER, and pack the
// result as one addon VPK.
//
// Two mechanisms, composed per particle:
//
//   1. SHEET REPOINT. Every sprite-card renderer input
//      (`m_Renderers[i]/m_vecTexturesInput[j]/m_hTexture`) that names a fire sheet
//      is repointed at a water sheet Deadlock already ships. This is the part a
//      hue shift cannot do: it replaces the *imagery* (flame licks, lava crust,
//      fireball flipbooks) with water imagery (splashes, caustics, bursts), rather
//      than merely tinting fire blue.
//
//      Repointing is used instead of overwriting the fire textures in place because
//      every fire sheet is SHARED game-wide (`ramp_fire`, `stylized_fire_loop`,
//      `lava_blast`, `statuseffects/burning` ...): overwriting would turn *everyone's*
//      fire to water. Editing the reference inside Infernus's own particle touches
//      only Infernus. See `hero_recolor::inferno_recipe`, whose doc note called this
//      the missing "rename+repoint step".
//
//      The map is FLIPBOOK-CLASS PRESERVING: a fire sheet carrying VTEX SHEET
//      extra-data (a flipbook whose cells the renderer walks) is only ever swapped
//      for a water sheet that also carries it, and a plain continuous texture only
//      for another plain one. Mixing the two changes how the renderer walks UVs and
//      renders as a single frozen cell or a smear. Classes verified with
//      `examples/shts_check.rs`.
//
//   2. COLOR. The particle's own color params are recolored to a water hue via the
//      shipped `recolor_particle_bytes`, so lights, glows, and the untouched noise
//      sheets read blue instead of orange.
//
// `ramp_fire` has no shipped water counterpart (nothing in the pak is a blue
// equivalent of that gradient), so it is MINTED: the vanilla ramp is hue-shifted to
// the water hue and packed at a new vpkmerge-owned path, then repointed there. This
// is the "recolor to a new path + repoint" combination, which keeps the vanilla
// `ramp_fire` untouched for every other hero.
//
// Both edits are byte-faithful in-place KV3 patches (`patch_kv3_resource_strings_adding`
// then `patch_kv3_resource_scalars`), never a full re-encode: a re-encoded `.vpcf_c`
// red-errors in the engine. This rides the same patcher as the animated-prism pass,
// which is in-game confirmed.
//
// usage:
//   cargo run --release -p vpkmerge-core --example inferno_water -- \
//     <pak01_dir.vpk> <out_dir.vpk> [--hue 200] [--sat 0.8] [--val 1.0] [--dry-run]
use anyhow::{Context, Result};
use morphic::kv3::{Seg, Value};
use morphic::ImageData;
use std::collections::{BTreeMap, BTreeSet};

/// Where the minted water ramp is packed. Namespaced so it can never collide with
/// a Valve path or another mod's.
const WATER_RAMP: &str = "materials/particle/ramp/ramp_water_vpkmerge.vtex";
/// The vanilla ramp the minted one is derived from.
const RAMP_FIRE: &str = "materials/particle/ramp/ramp_fire.vtex";

/// The particle trees that make up Infernus's kit. Matches `inferno_recipe`.
const PREFIXES: &[&str] = &[
    "particles/abilities/inferno/",
    "particles/weapon_fx/inferno/",
];

/// FIRE sheet -> WATER sheet, flipbook-class preserving.
///
/// `flipbook` records the shared class of both sides purely so the build can assert
/// it and refuse a mismatched pair rather than shipping a smear.
struct Swap {
    fire: &'static str,
    water: &'static str,
    flipbook: bool,
    /// Which of the user-facing effects this swap is doing the work for.
    note: &'static str,
    /// When set, only apply this swap inside particles whose entry path contains
    /// one of these substrings. Needed for sheets that are *generic* rather than
    /// fire-specific (noise, generic beams): they are used all over the kit, so a
    /// global swap would change effects that already read fine. Scoping lets the
    /// ultimate's radius ring go bubbly without disturbing every other voronoi use.
    only: Option<&'static [&'static str]>,
}

/// Particles making up the ultimate (Concussive Combustion, internally "fire bomb").
const ULT: &[&str] = &["inferno_fire_bomb"];
/// Just the ultimate's charge-up radius indicator.
const ULT_RING: &[&str] = &["inferno_fire_bomb_charge_aoe"];
/// Effects that render on the floor, where suds belong.
const GROUND: &[&str] = &[
    "_ground",
    "_napalm_debuff",
    "inferno_napalm_spray_projectile_ground",
];

/// Authored sheet paths, painted by this build (see [`AUTHORED`]).
const AUTHORED_BUBBLES: &str = "materials/particle/vpkmerge_water/water_bubbles_field.vtex";
const AUTHORED_DROPLETS: &str = "materials/particle/vpkmerge_water/water_droplets.vtex";
const AUTHORED_SUDS: &str = "materials/particle/vpkmerge_water/water_suds.vtex";
/// Round 3 sheets. Each is painted over the very sheet it replaces, so format,
/// dimensions, mip count and alpha semantics match by construction.
const AUTHORED_CAUSTIC_TRANS: &str = "materials/particle/vpkmerge_water/caustic_web_trans.vtex";
const AUTHORED_CAUSTIC_COLOR: &str = "materials/particle/vpkmerge_water/caustic_web_color.vtex";
const AUTHORED_CAUSTIC_ALPHATEST: &str =
    "materials/particle/vpkmerge_water/caustic_web_alphatest.vtex";
const AUTHORED_CAUSTIC_DXT1: &str = "materials/particle/vpkmerge_water/caustic_web_dxt1.vtex";
const AUTHORED_CAUSTIC_LARGE: &str = "materials/particle/vpkmerge_water/caustic_web_large.vtex";
const AUTHORED_FLOW_SMALL: &str = "materials/particle/vpkmerge_water/flow_streaks_small.vtex";
const AUTHORED_FLOW_LARGE: &str = "materials/particle/vpkmerge_water/flow_streaks_large.vtex";
const AUTHORED_FLOW_TINY: &str = "materials/particle/vpkmerge_water/flow_streaks_tiny.vtex";
const AUTHORED_FLOW_WARP: &str = "materials/particle/vpkmerge_water/flow_streaks_warp.vtex";
const AUTHORED_SPLASH_LOBES: &str = "materials/particle/vpkmerge_water/splash_lobes.vtex";
const AUTHORED_GLINT: &str = "materials/particle/vpkmerge_water/droplet_glint.vtex";

/// Pick the water colour for one particle from its ROLE.
///
/// Driving the whole kit to a single absolute hue is what makes a recolour read as
/// "the same effect, tinted" rather than as a different element: real water is not
/// monochrome. Its colour is a depth cue. Thin, aerated volumes (spray, mist, foam)
/// are nearly white and desaturated; a body of water is a deeper, more saturated
/// teal; the lit core of a splash blows out to white.
///
/// So instead of one `Recolor`, each particle gets one of four grades chosen by what
/// it depicts. Same mechanism as before (`recolor_particle_bytes`), just no longer
/// flat. Hue stays in a narrow 193-207 band so the whole kit still reads as one
/// palette: the variation carried is saturation and value, which is what actually
/// encodes depth.
fn palette_for(entry: &str) -> (vpkmerge_core::Recolor, &'static str) {
    let e = entry;
    let has = |frags: &[&str]| frags.iter().any(|f| e.contains(f));

    // Aerated + backlit: spray, mist, dust plumes, foam. Nearly white.
    if has(&[
        "_spray", "_smoke", "_dust", "_mist", "_steam", "_bits", "_flash",
    ]) {
        return (
            vpkmerge_core::Recolor::new(195.0, 0.30, 1.18),
            "aerated (spray/mist/foam): near-white",
        );
    }
    // Deep volume: anything pooling on the floor, plus lingering debuffs.
    if has(&[
        "_ground",
        "_pool",
        "_debuff",
        "_scorch",
        "_char",
        "_napalm_debuff",
    ]) {
        return (
            vpkmerge_core::Recolor::new(206.0, 1.00, 0.80),
            "deep volume (floor/pool): saturated teal",
        );
    }
    // Blown-out cores and lights: the bright centre of a splash.
    if has(&["_light", "_glow", "_core", "_hot", "_flare", "_shine"]) {
        return (
            vpkmerge_core::Recolor::new(197.0, 0.45, 1.12),
            "lit core: pale cyan-white",
        );
    }
    // Droplets and flying water: bright and readable against the map.
    if has(&[
        "_ember", "_drip", "_droplet", "_fleks", "_impact", "_tracer",
    ]) {
        return (
            vpkmerge_core::Recolor::new(200.0, 0.62, 1.06),
            "flying droplets: bright cyan",
        );
    }
    // Everything else: the mid-tone body of the effect.
    (
        vpkmerge_core::Recolor::new(202.0, 0.78, 0.98),
        "body: mid water blue",
    )
}

const SWAPS: &[Swap] = &[
    // ---- flipbooks: the actual fire imagery -> actual water imagery -------------
    Swap {
        fire: "materials/particle/fire/fire_loop/stylized_fire_loop.vtex",
        water: "materials/particle/water/caustic/caustic.vtex",
        flipbook: true,
        note: "body/aura flame loop -> flowing water caustics",
        only: None,
    },
    Swap {
        fire: "materials/particle/fire/fire_loop/stylized_fire_loop_oflow.vtex",
        water: "materials/particle/water/caustic/caustic.vtex",
        flipbook: true,
        note: "flame loop (optical flow variant) -> water caustics",
        only: None,
    },
    Swap {
        // 2048x1024 -> 512x256: both 2:1, so the cell grid reads the same way.
        fire: "materials/particle/fire_particle_10/fire_particle_10_low.vtex",
        water: "materials/particle/water_splash/water_splash.vtex",
        flipbook: true,
        note: "generic flame puff -> water splash",
        only: None,
    },
    Swap {
        // 1024x512 -> 512x256: both 2:1.
        fire: "materials/particle/lava_blasts/lava_blast.vtex",
        water: "materials/particle/water_splash/water_splash_vertical.vtex",
        flipbook: true,
        note: "napalm/lava blast -> vertical water splash",
        only: None,
    },
    Swap {
        fire: "materials/particle/explosion/fireball/explosion_fireball.vtex",
        water: "materials/particle/water_splash/water_burst.vtex",
        flipbook: true,
        note: "Concussive Combustion fireball -> giant water burst",
        only: None,
    },
    Swap {
        fire: "materials/particle/explosion/fireball/explosion_fireball_mv.vtex",
        water: "materials/particle/water_splash/water_burst.vtex",
        flipbook: true,
        note: "fireball (motion vector variant) -> water burst",
        only: None,
    },
    Swap {
        fire: "materials/particle/lanaya/plasma_flame.vtex",
        water: "materials/particle/water_splash/water_splash_shear/water_splash_shear.vtex",
        flipbook: true,
        note: "plasma flame -> sheared water splash",
        only: None,
    },
    Swap {
        fire: "materials/particle/fire/scenes/ground_ember.vtex",
        water: "materials/particle/water_splash/water_splash_shear/water_splash_shear.vtex",
        flipbook: true,
        note: "floating embers -> flying droplets",
        only: None,
    },
    // ---- plain continuous textures ---------------------------------------------
    Swap {
        fire: RAMP_FIRE,
        water: WATER_RAMP,
        flipbook: false,
        note: "fire color ramp -> minted water ramp (packed by this build)",
        only: None,
    },
    Swap {
        fire: "materials/particle/noise/noise_flame_warp.vtex",
        water: "materials/particle/water/water_scroll.vtex",
        flipbook: false,
        note: "flame warp noise -> scrolling water (finger flames, auras)",
        only: None,
    },
    Swap {
        fire: "materials/particle/mask/stylized_fire.vtex",
        water: "materials/particle/water/water_scroll.vtex",
        flipbook: false,
        note: "stylized fire shape mask -> water scroll",
        only: None,
    },
    Swap {
        fire: "materials/particle/beams/beam_fire_mask.vtex",
        water: "materials/particle/beams/beam_water_shape.vtex",
        flipbook: false,
        note: "fire beam falloff -> water beam falloff",
        only: None,
    },
    Swap {
        fire: "materials/particle/beam_fire_02.vtex",
        water: "materials/particle/beams/beam_water_wobbly.vtex",
        flipbook: false,
        note: "fire beam -> wobbly water beam (water-bending trace)",
        only: None,
    },
    Swap {
        fire: "materials/particle/beam_jagged_01.vtex",
        water: "materials/particle/beams/beam_water_stringy.vtex",
        flipbook: false,
        note: "jagged fire beam -> stringy water strand",
        only: None,
    },
    Swap {
        fire: "materials/particle/lava_pool_glow.vtex",
        water: "materials/particle/ground/ground_water_radial_02_psd_bf3cd4dc.vtex",
        flipbook: false,
        note: "lava pool glow -> radial water pool (napalm on the floor)",
        only: None,
    },
    Swap {
        fire: "materials/particle/ground/ground_lava_crack.vtex",
        water: "materials/particle/ground/ground_water_radial_02_psd_bf3cd4dc.vtex",
        flipbook: false,
        note: "cracked lava ground -> water on the floor",
        only: None,
    },
    Swap {
        fire: "materials/models/statuseffects/burning.vtex",
        water: "materials/particle/water/water_scroll.vtex",
        flipbook: false,
        note: "burning debuff on enemies -> wet shimmer",
        only: None,
    },
    Swap {
        fire: "materials/particle/particle_ring_softouter.vtex",
        water: "materials/particle/ring/ring_bubble.vtex",
        flipbook: false,
        note: "AoE radius ring -> bubble ring",
        only: None,
    },
    // ---- round 2: found by watching it in game ---------------------------------
    Swap {
        // The flecks thrown off his body during Flame Dash. `water_drop` is the
        // obvious donor but it is PLAIN and fleks3 is a FLIPBOOK; `spray1` is the
        // flipbook droplet-spray sheet, so it keeps the class.
        fire: "materials/particle/impact/fleks3.vtex",
        water: "materials/particle/spray1/spray1.vtex",
        flipbook: true,
        note: "body embers/flecks -> spray droplets",
        only: None,
    },
    Swap {
        fire: "materials/particle/pyroclastic/pyroclastic.vtex",
        water: "materials/particle/spray1/spray1.vtex",
        flipbook: true,
        note: "pyroclastic smoke -> water spray",
        only: None,
    },
    // Generic sheets: scoped to the ult so the rest of the kit is untouched.
    Swap {
        fire: "materials/particle/noise/noise_voronoi_tiled/noise_voronoi_alpha.vtex",
        water: AUTHORED_BUBBLES,
        flipbook: false,
        note: "ult radius ring fill -> AUTHORED bubble field",
        only: Some(ULT_RING),
    },
    Swap {
        fire: "materials/particle/noise/noise_uv_distort_chunky.vtex",
        water: "materials/particle/noise/noise_uv_distort_bubbly.vtex",
        flipbook: false,
        note: "ult radius ring distortion -> bubbles",
        only: Some(ULT_RING),
    },
    // Authored sheets into the ult's remaining plain slots: this is what takes the
    // ultimate from "blue explosion" to something with actual water detail in it.
    Swap {
        fire: "materials/particle/noise/noise_voronoi_large_trans.vtex",
        water: AUTHORED_BUBBLES,
        flipbook: false,
        note: "ult AoE sprite fill -> AUTHORED bubble field",
        only: Some(ULT),
    },
    Swap {
        fire: "materials/particle/mask/mask_vignette.vtex",
        water: AUTHORED_DROPLETS,
        flipbook: false,
        note: "ult AoE vignette -> AUTHORED droplets",
        only: Some(ULT),
    },
    // NOTE: the ult's explode_streak renderer points at `materials/particle/base_rope.vtex`,
    // which is NOT in pak01 at all (a Valve leftover reference). No swap for it: there
    // is nothing there to replace, and the map validator refuses absent fire sheets.
    Swap {
        fire: "materials/particle/beam_hotwhite.vtex",
        water: AUTHORED_DROPLETS,
        flipbook: false,
        note: "ult hot-white beam -> AUTHORED droplets",
        only: Some(ULT),
    },
    Swap {
        fire: "materials/particle/geometric/hexagon.vtex",
        water: AUTHORED_BUBBLES,
        flipbook: false,
        note: "ult hex accents -> AUTHORED bubble field",
        only: Some(ULT),
    },
    // Suds on the ground: the dash trail and the ult's scorch ring. This is the
    // "soap suds / bubbling on the floor" item from the original request.
    Swap {
        fire: "materials/particle/noise/noise_cloud.vtex",
        water: AUTHORED_SUDS,
        flipbook: false,
        note: "ground fire cloud -> AUTHORED suds",
        only: Some(GROUND),
    },
    Swap {
        // Was scoped to GROUND for suds, which never fired: none of the ground
        // effects sample this sheet. Its three real uses are airborne, so it gets
        // the same laminar treatment as the other flow noises.
        fire: "materials/particle/noise/noise_flow/noise_flow_warp.vtex",
        water: AUTHORED_FLOW_WARP,
        flipbook: false,
        note: "flow warp turbulence -> AUTHORED laminar streaks",
        only: None,
    },
    Swap {
        fire: "materials/particle/beam_generic_7.vtex",
        water: "materials/particle/beams/beam_water_edge.vtex",
        flipbook: false,
        note: "ult ring edge -> water wave edge",
        only: Some(ULT),
    },
    Swap {
        fire: "materials/particle/smoke1/smoke1.vtex",
        water: "materials/particle/spray1/spray1.vtex",
        flipbook: true,
        note: "ult dust plume -> water spray",
        only: Some(ULT),
    },
    // ---- round 3: the sheets that were still stock kit-wide ---------------------
    //
    // Rounds 1-2 replaced the sheets that *look* like fire and scoped the generic
    // ones to the ultimate. What that left behind is the largest part of his
    // imagery: 27 uses of the voronoi noise family (the cell structure that gives
    // flame its body), 16 uses of the turbulent cloud noises, 10 uses of a spiky
    // flame-shaped sprite mask, and 6 of a star flare. None of them are fire
    // *pictures*, so a hue shift leaves them reading as blue fire.
    //
    // Each is repointed at a sheet painted over itself (see [`AUTHORED`]), so the
    // container matches exactly and only the imagery changes. Kit-wide, not scoped:
    // these are Infernus's own particles, so no other hero is touched.
    Swap {
        // Worley cell field: already structurally a caustic, just inverted in
        // character (thick soft walls, dark centres). Repainted as thin bright
        // filaments over deep water.
        fire: "materials/particle/noise/noise_voronoi_tiled/noise_voronoi_tiled_trans.vtex",
        water: AUTHORED_CAUSTIC_TRANS,
        flipbook: false,
        note: "flame cell noise -> AUTHORED caustic web",
        only: None,
    },
    Swap {
        fire: "materials/particle/noise/noise_voronoi_tiled/noise_voronoi_tiled_color.vtex",
        water: AUTHORED_CAUSTIC_COLOR,
        flipbook: false,
        note: "flame cell noise (color) -> AUTHORED caustic web",
        only: None,
    },
    Swap {
        fire: "materials/particle/noise/noise_voronoi_tiled/noise_voronoi_tiled_alphatest.vtex",
        water: AUTHORED_CAUSTIC_ALPHATEST,
        flipbook: false,
        note: "flame cell noise (alphatest) -> AUTHORED caustic web",
        only: None,
    },
    Swap {
        fire: "materials/particle/noise/noise_voronoi.vtex",
        water: AUTHORED_CAUSTIC_DXT1,
        flipbook: false,
        note: "flame cell noise (dxt1) -> AUTHORED caustic web",
        only: None,
    },
    Swap {
        fire: "materials/particle/noise/noise_voronoi_large_color.vtex",
        water: AUTHORED_CAUSTIC_LARGE,
        flipbook: false,
        note: "large flame cell noise -> AUTHORED caustic web",
        only: None,
    },
    Swap {
        // Fractal turbulence: what makes fire boil. Repainted as laminar streaks,
        // which is what makes water read as flowing rather than burning. Round 2
        // only reached this sheet on the ground effects (as suds); this covers the
        // other six uses.
        fire: "materials/particle/noise/noise_flow/noise_flow_lrg.vtex",
        water: AUTHORED_FLOW_LARGE,
        flipbook: false,
        note: "large flow turbulence -> AUTHORED laminar streaks",
        only: None,
    },
    Swap {
        // Unscoped fallback for the sheet the GROUND-scoped suds swap above already
        // claims: on the floor it becomes foam, everywhere else it becomes flow.
        // Resolution is first-match-wins with scoped entries taking priority, so
        // this entry must stay BELOW the scoped one.
        fire: "materials/particle/noise/noise_cloud.vtex",
        water: AUTHORED_FLOW_SMALL,
        flipbook: false,
        note: "airborne fire turbulence -> AUTHORED laminar streaks",
        only: None,
    },
    Swap {
        fire: "materials/particle/noisecloud01.vtex",
        water: AUTHORED_FLOW_TINY,
        flipbook: false,
        note: "cloud turbulence -> AUTHORED laminar streaks",
        only: None,
    },
    Swap {
        // The single biggest shape change in the whole build: this is the sprite
        // SILHOUETTE, a spiky star-shaped flame lick. Fire is spiky, water is
        // round; nothing else in the pipeline changes an outline.
        fire: "materials/particle/mask/mask_vignette_warp.vtex",
        water: AUTHORED_SPLASH_LOBES,
        flipbook: false,
        note: "spiky flame sprite mask -> AUTHORED rounded splash lobes",
        only: None,
    },
    Swap {
        fire: "materials/particle/particle_flares/particle_flare_001.vtex",
        water: AUTHORED_GLINT,
        flipbook: false,
        note: "star flare -> AUTHORED droplet glint",
        only: None,
    },
];

/// Hero-owned `.vtex_c` recolored to the water hue **in place** (no repoint).
///
/// This is the pass round 1 missed entirely, and it is what actually fixes the
/// flames on his head. These paths are inferno-namespaced, so overwriting them at
/// their own entry path affects only Infernus: no repoint needed, unlike the shared
/// fire sheets above.
///
/// `models/heroes_wip/inferno/inferno.vmdl_c` (the LIVE model, per
/// `vpkmerge model live-materials --hero inferno`) renders only four materials, of
/// which two carry his fire: `inferno_headglow` (meshes = flame_hair, inferno,
/// inferno_flames) and `inferno_armglow`. Their orange comes from these `g_tColor`
/// textures, NOT from the tint params, which is why recoloring the tint alone left
/// the head still burning. `inferno_flame01/02/03`, `inferno_flame_arm` and
/// `inferno_flame_strip` are NOT rendered by the live model: editing them does
/// nothing, so they are deliberately absent here.
///
/// Deliberately EXCLUDED: `inferno_body_color` and `inferno_clothes_color` (his skin
/// and outfit: the request was to change the fire, not the character), and every
/// normal / roughness / metalness / AO / outline-mask texture, because hue-shifting a
/// non-color map corrupts the lighting it drives rather than recoloring anything.
/// How a hero-owned texture is converted.
#[derive(Clone, Copy)]
enum HeroTexMode {
    /// Hue-rotate the existing pixels. Right when the source imagery is already
    /// water-agnostic (a soft glow, a blob) and only its colour is wrong.
    Hue,
    /// Repaint the pixels entirely. Right when the source imagery is *specifically
    /// fire*: hue-rotating a lava crust yields a blue lava crust, which reads as
    /// glowing rock, not water. Falls back to [`HeroTexMode::Hue`] under
    /// `--no-repaint` so the two can be compared in game.
    Paint(Painted),
}

const HERO_TEXTURES: &[(&str, HeroTexMode, &str)] = &[
    (
        "models/heroes_staging/inferno_v4/materials/inferno_glow_psd_3c639d14.vtex",
        // A hard-edged magma pattern: black cooled crust with molten veins between.
        // Repainted as caustics, which is the same "bright network over a dark
        // body" composition read as water, and normalized to the crust's own mean
        // luminance so his hair keeps the same presence.
        // Four cells, not the finer count the particle sheets use: the crust it
        // replaces is coarse, and matching its spatial scale is what keeps the
        // hair reading as one flowing mass instead of as busy noise.
        HeroTexMode::Paint(Painted::BodyCaustics { cells: 4 }),
        "headglow g_tColor: the flame HAIR + body flames",
    ),
    (
        "models/heroes_staging/inferno_v4/materials/inferno_glow_png_c2e03ac5.vtex",
        // A soft hand-drawn wisp. Its SHAPE is fine for water, so it is kept and
        // given a laminar surface rather than replaced.
        HeroTexMode::Paint(Painted::FlowAligned),
        "armglow g_tColor: the arm/hand flames",
    ),
    (
        "models/heroes_staging/inferno_v4/materials/inferno_arm_flame_translucency_psd_800636c0.vtex",
        HeroTexMode::Hue,
        "arm flame translucency",
    ),
    (
        "materials/particle/blobs/inferno_napalm_blob_vmat_g_tcolor_96d95806.vtex",
        HeroTexMode::Hue,
        "napalm blob colour (the goo)",
    ),
    (
        "materials/particle/abilities/inferno/inferno_fire_bomb_charge_ground_psd_73c3f892.vtex",
        HeroTexMode::Hue,
        "ult charge ground",
    ),
    (
        "materials/particle/abilities/inferno/inferno_fire_bomb_explode_ground_projected_vmat_g_tselfillum_670d93d.vtex",
        HeroTexMode::Hue,
        "ult explode ground glow",
    ),
    (
        "materials/particle/projected/inferno_fire_bomb_charge_ground_projected_vmat_g_tselfillum_670d93d.vtex",
        HeroTexMode::Hue,
        "ult charge ground glow",
    ),
    (
        "materials/particle/projected/inferno_incendiary_body_vmat_g_tselfillum_2a5a2fce.vtex",
        HeroTexMode::Hue,
        "incendiary body glow",
    ),
];

/// The four materials `vpkmerge model live-materials --hero inferno` reports the
/// live model actually renders.
///
/// Rounds 1-2 recoloured these materials' *textures* but never their *parameters*,
/// which left every one of them carrying an orange `g_vSelfIllumTint1`. That is
/// not a cosmetic omission: `g_flSelfIllumAlbedoFactor1` is 1 on both glow
/// materials, so the emitted colour is albedo times tint, and an orange tint over
/// a now-cyan albedo multiplies out to a muddy dark olive rather than to water.
/// The tints have to move with the textures.
///
/// Beyond colour, three of these carry the *timing* of fire: a per-frame
/// `g_flSelfIllumScale1` expression flickering at 2-3 rad/s, vertex jitter running
/// at flame frequencies, and an albedo scroll that licks upward. Water swells,
/// sways and runs down.
struct HeroMaterial {
    entry: &'static str,
    /// Saturation and value multipliers used when driving this material's tints to
    /// the water hue. The additive glow materials get their saturation pulled back:
    /// their tint multiplies an already-blue texture, so a fully saturated tint
    /// squares the chroma and the result goes inky.
    tint_sat: f64,
    tint_val: f64,
    /// Replacement for the self-illum pulse expression, when the material has one.
    /// Fire flickers; water swells. Same shape, slower and shallower.
    pulse: Option<&'static str>,
    /// Multiplier on `g_vJitterFrequencies*`: flame flicker -> water sway.
    jitter: f64,
    /// Multiplier on `g_vAlbedoScrollSpeed1`'s magnitude. The Y component is also
    /// negated: whichever way the sheet is oriented on the mesh, flipping the sign
    /// reverses the lick, and fire licks the opposite way to running water.
    scroll: f64,
    note: &'static str,
}

const HERO_MATERIALS: &[HeroMaterial] = &[
    HeroMaterial {
        entry: "models/heroes_staging/inferno_v4/materials/inferno_headglow.vmat",
        tint_sat: 0.55,
        tint_val: 1.0,
        pulse: None,
        jitter: 0.55,
        scroll: 0.6,
        note: "flame hair + body flames",
    },
    HeroMaterial {
        entry: "models/heroes_staging/inferno_v4/materials/inferno_armglow.vmat",
        tint_sat: 0.55,
        tint_val: 1.0,
        // was: 4 * sin(2 * time()) + 5   (a 1..9 flicker at 2 rad/s)
        pulse: Some("2 * sin(0.7 * time()) + 5"),
        jitter: 0.55,
        scroll: 0.6,
        note: "arm / hand flames",
    },
    HeroMaterial {
        entry: "models/heroes_staging/inferno_v4/materials/inferno_body.vmat",
        tint_sat: 0.85,
        tint_val: 1.0,
        // was: 0.5 * sin(3 * time()) + 0.5   (a 0..1 flicker at 3 rad/s)
        pulse: Some("0.32 * sin(0.8 * time()) + 0.55"),
        jitter: 1.0,
        scroll: 1.0,
        note: "the glowing veins in his skin, plus his warm outline",
    },
    HeroMaterial {
        entry: "models/heroes_staging/inferno_v4/materials/inferno_clothes.vmat",
        tint_sat: 0.85,
        tint_val: 1.0,
        pulse: None,
        jitter: 1.0,
        scroll: 1.0,
        note: "the emissive on his coat, plus his warm outline",
    },
];

/// `m_vectorParams` names that hold a COLOUR (and so should follow the water hue).
/// Name-matched rather than value-matched because plenty of non-colour vec4s
/// (`g_vAlbedoScrollSpeed1`, `g_vJitterAmplitudes*`, `g_vHighlightPositionWs1`)
/// would otherwise be silently hue-rotated into nonsense.
fn is_colour_param(name: &str) -> bool {
    (name.contains("Tint") || name.contains("Color"))
        && !name.contains("Scroll")
        && !name.contains("Scale")
        && !name.contains("Position")
        && !name.contains("TexCoord")
}

/// A tint only moves if it is actually carrying colour. A neutral white/grey is a
/// multiplier, not a colour: hue-rotating it would tint whatever it multiplies.
const TINT_SATURATION_FLOOR: f64 = 0.35;

/// Shared color textures referenced by a HERO-OWNED material. These cannot be
/// overwritten (they are used by other heroes' lava/fire effects), so a water copy
/// is minted at a new path and the hero's own `.vmat_c` is repointed at it: the same
/// mint-and-repoint trick as the ramp, one level up.
///
/// `lava_projected_glow_*` is the ultimate's ground glow, and it is exactly why the
/// ult had no water on the floor.
const MATERIAL_REPOINTS: &[(&str, &[&str])] = &[(
    "materials/particle/projected/inferno_fire_bomb_projected_scorch_light.vmat",
    &[
        "materials/particle/lava_blasts/lava_projected_glow_color_tga_dc448734.vtex",
        "materials/particle/lava_blasts/lava_projected_glow_trans_tga_7ccfd6b9.vtex",
    ],
)];

/// Minted-copy path for a shared texture: same basename, vpkmerge-namespaced dir.
fn minted_path(shared: &str) -> String {
    let base = shared.rsplit('/').next().unwrap_or(shared);
    format!("materials/particle/vpkmerge_water/{base}")
}

// ---------------------------------------------------------------------------
// Authored sheets
// ---------------------------------------------------------------------------

/// A procedurally painted water sheet, spliced into a shipped donor container.
///
/// Textures are NOT built from scratch: the pixels are painted over a real decoded
/// `.vtex_c` and re-encoded with [`morphic::replace_mip_chain`], so the result keeps
/// the donor's format, dimensions and full mip chain. This is the same path the
/// trippy reskins use, and it matters because an inline-PNG albedo is rejected by the
/// engine (renders purple): the bytes have to be a real BCn texture.
///
/// Every donor here must be PLAIN (no VTEX SHEET extra-data). A painted sheet is one
/// continuous image, so splicing it into a flipbook donor would leave stale cell
/// metadata describing frames that no longer exist. morphic cannot author SHEET
/// extra-data, so authored sheets can only ever replace plain slots.
struct Authored {
    /// Shipped texture whose container/format/dims are reused. Must be plain.
    donor: &'static str,
    /// New vpkmerge-owned entry path.
    path: &'static str,
    kind: Painted,
    note: &'static str,
}

#[derive(Clone, Copy, PartialEq)]
enum Painted {
    /// Sparse field of clear bubbles: bright rim, dark interior, specular dot.
    Bubbles,
    /// Scattered falling droplets with a highlight, elongated along V.
    Droplets,
    /// Dense clustered small bubbles: soap suds / foam.
    Suds,
    /// Radial colour ramp with a real water depth curve.
    WaterRamp,
    /// Thin bright filaments tracing the boundaries between warped Worley cells:
    /// the light network the sun casts through a rippled water surface. Replaces
    /// the voronoi noise family, which is the same construction read the other way
    /// round (thick soft walls, dark centres) and is what gives flame its cellular
    /// body. `cells` sets the base cell count so a 1024 sheet does not end up with
    /// 512-sized cells.
    CausticWeb { cells: u32 },
    /// Long directional filaments: laminar flow. Replaces the fractal turbulence
    /// noises, whose isotropic boil is exactly what makes fire read as burning.
    /// `stretch` is how many times longer than wide the streaks run.
    FlowStreaks { stretch: f32 },
    /// A centred, rounded, overlapping-lobe silhouette with a few thrown droplets:
    /// the shape of a splash. Replaces the spiky star-shaped flame sprite mask.
    /// Not tiling (it is a centred sprite, not a field).
    SplashLobes,
    /// A centred bead: bright specular core, soft bloom, a thin bubble rim and a
    /// short 4-point cross. Replaces the many-rayed star flare.
    DropletGlint,
    /// Caustic filaments over a lit body of water, normalized to the donor's own
    /// mean luminance and leaving the donor's alpha untouched. For hero-body glow
    /// textures, where the pattern has to keep the same on-screen presence as the
    /// lava crust it replaces, and where alpha is the material's own opacity (these
    /// sit on `F_TRANSLUCENT` materials) rather than a channel to repaint.
    BodyCaustics { cells: u32 },
    /// Fine striations running ALONG the donor's own luminance contours, so a
    /// hand-drawn flame wisp keeps its shape but gains a laminar water surface.
    /// Direction is read from the source image, which is what makes this safe on a
    /// texture whose UV orientation on the model is unknown.
    FlowAligned,
}

const AUTHORED: &[Authored] = &[
    Authored {
        // 512x512 Bc7, plain, has alpha: the right container for cutout bubbles.
        donor: "materials/particle/ring/ring_bubble.vtex",
        path: "materials/particle/vpkmerge_water/water_bubbles_field.vtex",
        kind: Painted::Bubbles,
        note: "bubble field for the ult radius ring + AoE fill",
    },
    Authored {
        donor: "materials/particle/water/water_scroll.vtex",
        path: "materials/particle/vpkmerge_water/water_droplets.vtex",
        kind: Painted::Droplets,
        note: "falling droplets for the ult burst + weapon impacts",
    },
    Authored {
        donor: "materials/particle/water/water_scroll.vtex",
        path: "materials/particle/vpkmerge_water/water_suds.vtex",
        kind: Painted::Suds,
        note: "soap suds / foam for the dash trail + ult ground",
    },
    Authored {
        // Replaces the hue-rotated ramp. Rotating `ramp_fire`'s hue keeps fire's
        // saturation curve (saturated at the edge, desaturating to a yellow-white
        // core), which is not how water grades. A painted ramp puts deep teal at the
        // outside, a green-leaning cyan through the midtones (thin water really does
        // pull green) and blows out to white foam at the core.
        donor: RAMP_FIRE,
        path: WATER_RAMP,
        kind: Painted::WaterRamp,
        note: "water depth ramp: deep teal -> cyan -> white foam",
    },
    // ---- round 3: painted over the very sheet each one replaces ----------------
    Authored {
        donor: "materials/particle/noise/noise_voronoi_tiled/noise_voronoi_tiled_trans.vtex",
        path: AUTHORED_CAUSTIC_TRANS,
        kind: Painted::CausticWeb { cells: 5 },
        note: "caustic web (trans)",
    },
    Authored {
        donor: "materials/particle/noise/noise_voronoi_tiled/noise_voronoi_tiled_color.vtex",
        path: AUTHORED_CAUSTIC_COLOR,
        kind: Painted::CausticWeb { cells: 5 },
        note: "caustic web (color)",
    },
    Authored {
        donor: "materials/particle/noise/noise_voronoi_tiled/noise_voronoi_tiled_alphatest.vtex",
        path: AUTHORED_CAUSTIC_ALPHATEST,
        kind: Painted::CausticWeb { cells: 5 },
        note: "caustic web (alphatest)",
    },
    Authored {
        donor: "materials/particle/noise/noise_voronoi.vtex",
        path: AUTHORED_CAUSTIC_DXT1,
        kind: Painted::CausticWeb { cells: 6 },
        note: "caustic web (dxt1, no alpha)",
    },
    Authored {
        donor: "materials/particle/noise/noise_voronoi_large_color.vtex",
        path: AUTHORED_CAUSTIC_LARGE,
        kind: Painted::CausticWeb { cells: 8 },
        note: "caustic web (1024, larger cell count so the scale still reads)",
    },
    Authored {
        donor: "materials/particle/noise/noise_cloud.vtex",
        path: AUTHORED_FLOW_SMALL,
        kind: Painted::FlowStreaks { stretch: 7.0 },
        note: "laminar streaks (256)",
    },
    Authored {
        donor: "materials/particle/noise/noise_flow/noise_flow_lrg.vtex",
        path: AUTHORED_FLOW_LARGE,
        kind: Painted::FlowStreaks { stretch: 9.0 },
        note: "laminar streaks (512)",
    },
    Authored {
        donor: "materials/particle/noise/noise_flow/noise_flow_warp.vtex",
        path: AUTHORED_FLOW_WARP,
        kind: Painted::FlowStreaks { stretch: 8.0 },
        note: "laminar streaks (512, flow warp)",
    },
    Authored {
        donor: "materials/particle/noisecloud01.vtex",
        path: AUTHORED_FLOW_TINY,
        kind: Painted::FlowStreaks { stretch: 6.0 },
        note: "laminar streaks (128)",
    },
    Authored {
        donor: "materials/particle/mask/mask_vignette_warp.vtex",
        path: AUTHORED_SPLASH_LOBES,
        kind: Painted::SplashLobes,
        note: "rounded splash silhouette (replaces the spiky flame sprite mask)",
    },
    Authored {
        donor: "materials/particle/particle_flares/particle_flare_001.vtex",
        path: AUTHORED_GLINT,
        kind: Painted::DropletGlint,
        note: "droplet glint (replaces the many-rayed star flare)",
    },
];

/// Deterministic hash-based pseudo-random in `[0,1)`. A fixed integer hash keeps the
/// output reproducible across runs (the build must be byte-stable), which a seeded RNG
/// crate would also give but without adding a dependency.
fn rnd(mut x: u32) -> f32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 / 16_777_216.0
}

/// Shortest wrapped distance between two normalized coords, so every painted sheet
/// tiles seamlessly (these get scrolled, and a seam would read as a hard line).
fn wrapped(a: f32, b: f32) -> f32 {
    let d = (a - b).abs();
    if d > 0.5 {
        1.0 - d
    } else {
        d
    }
}

/// Value noise on a wrapping lattice. Periodic by construction (the lattice
/// coordinates are taken mod `period`), which is what lets the painted fields be
/// scrolled by a particle without showing a seam.
fn vnoise(x: f32, y: f32, period: u32, seed: u32) -> f32 {
    let p = period.max(1);
    let wrap = |i: i32| -> u32 { i.rem_euclid(p as i32) as u32 };
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    // Smoothstep the cell-local coords so the lattice does not show as a grid.
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let at = |ix: i32, iy: i32| -> f32 {
        rnd(wrap(ix)
            .wrapping_mul(0x9e37_79b1)
            .wrapping_add(wrap(iy).wrapping_mul(0x85eb_ca6b))
            .wrapping_add(seed))
    };
    let (ix, iy) = (x0 as i32, y0 as i32);
    let top = at(ix, iy) + (at(ix + 1, iy) - at(ix, iy)) * sx;
    let bot = at(ix, iy + 1) + (at(ix + 1, iy + 1) - at(ix, iy + 1)) * sx;
    top + (bot - top) * sy
}

/// Summed octaves of [`vnoise`], each at double the frequency (and so double the
/// period, keeping every octave tiling) and half the weight.
fn fbm(x: f32, y: f32, base_period: u32, octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut total, mut freq) = (0.0, 1.0, 0.0, 1.0);
    for o in 0..octaves {
        sum += amp * vnoise(x * freq, y * freq, base_period * (1 << o), seed + o * 7919);
        total += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / total
}

/// Distance to the two nearest points of a jittered toroidal grid, i.e. Worley
/// F1/F2. The caustic ridge lives exactly where `F2 - F1` goes to zero: the
/// boundary between two cells.
fn worley_f1f2(u: f32, v: f32, cells: u32, seed: u32) -> (f32, f32) {
    let n = cells.max(2);
    let scale = n as f32;
    let (cu, cv) = (u * scale, v * scale);
    let (bu, bv) = (cu.floor() as i32, cv.floor() as i32);
    let (mut f1, mut f2) = (f32::MAX, f32::MAX);
    for dv in -1..=1 {
        for du in -1..=1 {
            let (gu, gv) = (bu + du, bv + dv);
            // Wrap the cell index so the field tiles.
            let key = (gu.rem_euclid(n as i32) as u32)
                .wrapping_mul(0x27d4_eb2d)
                .wrapping_add((gv.rem_euclid(n as i32) as u32).wrapping_mul(0x1656_67b1))
                .wrapping_add(seed);
            let px = gu as f32 + rnd(key);
            let py = gv as f32 + rnd(key.wrapping_add(0x9e37_79b9));
            let d = ((cu - px).powi(2) + (cv - py).powi(2)).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2)
}

/// Caustic intensity at `(u, v)`: warped Worley cell boundaries, two octaves.
///
/// The warp is what turns a static Worley diagram into something that reads as
/// light refracted through a moving surface: without it the ridges are polygonal
/// and the field looks like cracked glass rather than water.
fn caustic_at(u: f32, v: f32, cells: u32) -> f32 {
    let warp = 0.055;
    let wu = u + warp * (std::f32::consts::TAU * (2.0 * v + 0.17)).sin();
    let wv = v + warp * (std::f32::consts::TAU * (2.0 * u + 0.41)).cos();

    let ridge = |cells: u32, seed: u32, width: f32| -> f32 {
        let (f1, f2) = worley_f1f2(wu.rem_euclid(1.0), wv.rem_euclid(1.0), cells, seed);
        (1.0 - ((f2 - f1) / width).clamp(0.0, 1.0)).powf(2.6)
    };
    // A coarse network carries the read; a finer one at a third of the weight
    // gives it the broken, glittering detail real caustics have.
    let coarse = ridge(cells, 1, 0.55);
    let fine = ridge(cells * 3, 977, 0.40);
    (coarse + 0.34 * fine).min(1.0)
}

/// Laminar flow intensity: fbm stretched hard along V so its features become long
/// filaments rather than isotropic blobs, then contrast-boosted into bands.
fn flow_at(u: f32, v: f32, stretch: f32) -> f32 {
    // Sampling V at 1/stretch the rate makes each feature `stretch` times longer
    // in V than it is wide in U.
    let base_period = 8u32;
    let n = fbm(
        u * base_period as f32,
        v * base_period as f32 / stretch,
        base_period,
        4,
        31,
    );
    // A gentle sideways wobble keeps the streaks from reading as a barcode.
    let wobble = 0.06 * (std::f32::consts::TAU * (3.0 * v)).sin();
    let n2 = fbm(
        (u + wobble) * base_period as f32 * 2.0,
        v * base_period as f32 * 2.0 / stretch,
        base_period * 2,
        3,
        613,
    );
    let mixed = 0.72 * n + 0.28 * n2;
    // Push the midtones apart so the field reads as distinct filaments.
    (((mixed - 0.42) * 2.6) + 0.5).clamp(0.0, 1.0)
}

/// One blob's contribution at a point: `(luminance, alpha)`.
fn bubble_at(dx: f32, dy: f32, radius: f32, rim: f32) -> (f32, f32) {
    let d = (dx * dx + dy * dy).sqrt();
    if d > radius {
        return (0.0, 0.0);
    }
    let t = d / radius;
    // Bright thin rim, faint interior: how a clear bubble actually reads.
    let rim_band = ((t - (1.0 - rim)) / rim.max(0.001)).clamp(0.0, 1.0);
    let lum = 0.15 + 0.85 * rim_band * rim_band;
    // A specular dot up-left of centre sells the sphere.
    let sx = dx + radius * 0.35;
    let sy = dy + radius * 0.35;
    let spec = (1.0 - ((sx * sx + sy * sy).sqrt() / (radius * 0.30)).clamp(0.0, 1.0)).powf(2.0);
    let lum = (lum + spec * 0.9).min(1.0);
    let alpha = (0.10 + 0.90 * rim_band).min(1.0);
    (lum, alpha)
}

/// Paint `image` in place. Output is near-white with shaped alpha: the particle's own
/// (already water-blue) color params tint it at runtime, which is how Valve's own
/// particle sheets are authored, so this stays consistent with the untouched ones.
fn paint(image: &mut morphic::Image, kind: Painted) -> Result<()> {
    let (w, h) = (image.width as usize, image.height as usize);
    let ImageData::Rgba8(px) = &mut image.data else {
        anyhow::bail!("authored sheets need an 8-bit donor (got f16)");
    };
    anyhow::ensure!(px.len() >= w * h * 4, "donor pixel buffer too small");

    // Field / sprite kinds that are evaluated per pixel rather than accumulated
    // from a blob list. Each returns early.
    match kind {
        Painted::CausticWeb { cells } => {
            paint_caustics(px, w, h, cells, None);
            return Ok(());
        }
        Painted::BodyCaustics { cells } => {
            // Match the donor's own mean luminance so the effect keeps the same
            // on-screen presence: these sit on an additive material, where a
            // darker texture simply means less of the character glows.
            let target = mean_luma(px);
            let alpha: Vec<u8> = px.iter().skip(3).step_by(4).copied().collect();
            paint_caustics(px, w, h, cells, Some(target));
            for (i, a) in alpha.into_iter().enumerate() {
                px[i * 4 + 3] = a;
            }
            return Ok(());
        }
        Painted::FlowStreaks { stretch } => {
            paint_flow(px, w, h, stretch);
            return Ok(());
        }
        Painted::SplashLobes => {
            paint_splash_lobes(px, w, h);
            return Ok(());
        }
        Painted::DropletGlint => {
            paint_glint(px, w, h);
            return Ok(());
        }
        Painted::FlowAligned => {
            paint_flow_aligned(px, w, h);
            return Ok(());
        }
        Painted::Bubbles | Painted::Droplets | Painted::Suds | Painted::WaterRamp => {}
    }

    // The ramp is a gradient, not a blob field: paint it and return.
    if matches!(kind, Painted::WaterRamp) {
        for y in 0..h {
            for x in 0..w {
                // ramp_fire is a radial ramp whose bright core sits bottom-centre.
                let u = (x as f32 + 0.5) / w as f32;
                let v = (y as f32 + 0.5) / h as f32;
                let d = (((u - 0.5) * 1.35).powi(2) + (v - 1.0).powi(2)).sqrt();
                let t = (1.0 - d.clamp(0.0, 1.0)).clamp(0.0, 1.0);
                // Three-stop water gradient, eased so the foam core stays tight.
                let (r, g, b) = if t < 0.55 {
                    let k = t / 0.55;
                    // deep teal -> cyan
                    (0.05 + 0.10 * k, 0.22 + 0.42 * k, 0.38 + 0.42 * k)
                } else {
                    let k = ((t - 0.55) / 0.45).powf(1.6);
                    // cyan -> white foam
                    (0.15 + 0.85 * k, 0.64 + 0.36 * k, 0.80 + 0.20 * k)
                };
                let o = (y * w + x) * 4;
                px[o] = (r * 255.0).clamp(0.0, 255.0) as u8;
                px[o + 1] = (g * 255.0).clamp(0.0, 255.0) as u8;
                px[o + 2] = (b * 255.0).clamp(0.0, 255.0) as u8;
                px[o + 3] = 255;
            }
        }
        return Ok(());
    }

    // (centre_x, centre_y, radius, rim_fraction) in normalized space.
    let mut blobs: Vec<(f32, f32, f32, f32)> = Vec::new();
    match kind {
        Painted::Bubbles => {
            for i in 0..46u32 {
                let r = 0.030 + 0.055 * rnd(i * 7 + 1);
                blobs.push((rnd(i * 13 + 2), rnd(i * 29 + 3), r, 0.30));
            }
        }
        Painted::Droplets => {
            for i in 0..90u32 {
                let r = 0.010 + 0.022 * rnd(i * 11 + 5);
                blobs.push((rnd(i * 17 + 7), rnd(i * 31 + 11), r, 0.85));
            }
        }
        Painted::Suds => {
            // Clusters of many tiny bubbles: foam reads as density, not as shapes, so
            // this is deliberately dense and overlapping. Sparse clusters read as
            // scattered specks rather than a layer of suds.
            for c in 0..64u32 {
                let (cx, cy) = (rnd(c * 19 + 13), rnd(c * 23 + 17));
                for k in 0..26u32 {
                    let i = c * 37 + k;
                    let ang = rnd(i * 5 + 19) * std::f32::consts::TAU;
                    let rad = 0.060 * rnd(i * 9 + 23).sqrt();
                    let r = 0.008 + 0.024 * rnd(i * 3 + 29);
                    blobs.push((
                        (cx + ang.cos() * rad).rem_euclid(1.0),
                        (cy + ang.sin() * rad).rem_euclid(1.0),
                        r,
                        0.45,
                    ));
                }
            }
        }
        // Handled by the early returns above.
        Painted::WaterRamp
        | Painted::CausticWeb { .. }
        | Painted::BodyCaustics { .. }
        | Painted::FlowStreaks { .. }
        | Painted::SplashLobes
        | Painted::DropletGlint
        | Painted::FlowAligned => {
            unreachable!("per-pixel kinds are painted before the blob field")
        }
    }

    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32;
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let (mut lum, mut alpha) = (0.0f32, 0.0f32);
            for &(bx, by, r, rim) in &blobs {
                let dx = wrapped(u, bx) * if u > bx { 1.0 } else { -1.0 };
                let mut dy = wrapped(v, by) * if v > by { 1.0 } else { -1.0 };
                if matches!(kind, Painted::Droplets) {
                    // Squash V so droplets read as falling teardrops.
                    dy *= 0.55;
                }
                let (l, a) = bubble_at(dx, dy, r, rim);
                // Bubbles overlap by taking the brightest, not by summing: summing
                // blows out to a white blanket where clusters touch.
                lum = lum.max(l);
                alpha = alpha.max(a);
            }
            let o = (y * w + x) * 4;
            // A faint cool bias so the sheet still reads watery where a particle
            // happens to tint it white.
            px[o] = (lum * 235.0).min(255.0) as u8;
            px[o + 1] = (lum * 247.0).min(255.0) as u8;
            px[o + 2] = 255.0_f32.min(lum * 255.0 + 12.0) as u8;
            px[o + 3] = (alpha * 255.0).min(255.0) as u8;
        }
    }
    Ok(())
}

/// Mean Rec.709 luminance of an RGBA8 buffer, in `[0,1]`.
fn mean_luma(px: &[u8]) -> f32 {
    if px.len() < 4 {
        return 0.0;
    }
    let mut sum = 0.0f64;
    let n = px.len() / 4;
    for c in px.chunks_exact(4) {
        sum += f64::from(c[0]).mul_add(
            0.2126,
            f64::from(c[1]).mul_add(0.7152, f64::from(c[2]) * 0.0722),
        );
    }
    (sum / (n as f64) / 255.0) as f32
}

/// Rescale RGB so the buffer's mean luminance lands on `target`, preserving hue
/// and relative contrast. Used where the painted texture has to carry the same
/// visual weight as the one it replaces.
fn normalize_luma(px: &mut [u8], target: f32) {
    let current = mean_luma(px);
    if current <= 0.001 {
        return;
    }
    let gain = (target / current).clamp(0.25, 6.0);
    for c in px.chunks_exact_mut(4) {
        for k in 0..3 {
            c[k] = (f32::from(c[k]) * gain).min(255.0) as u8;
        }
    }
}

/// Water colour for a caustic sample: deep teal in the cell interiors rising to
/// a cyan-white filament. Interiors are kept non-black so the sheet still reads
/// as a body of water rather than as light on nothing.
fn caustic_color(t: f32) -> (f32, f32, f32) {
    let deep = (0.03, 0.20, 0.32);
    let mid = (0.10, 0.62, 0.82);
    let foam = (0.88, 0.99, 1.0);
    if t < 0.5 {
        let k = t / 0.5;
        (
            deep.0 + (mid.0 - deep.0) * k,
            deep.1 + (mid.1 - deep.1) * k,
            deep.2 + (mid.2 - deep.2) * k,
        )
    } else {
        let k = ((t - 0.5) / 0.5).powf(1.4);
        (
            mid.0 + (foam.0 - mid.0) * k,
            mid.1 + (foam.1 - mid.1) * k,
            mid.2 + (foam.2 - mid.2) * k,
        )
    }
}

fn paint_caustics(px: &mut [u8], w: usize, h: usize, cells: u32, luma_target: Option<f32>) {
    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32;
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let t = caustic_at(u, v, cells);
            let (r, g, b) = caustic_color(t);
            let o = (y * w + x) * 4;
            px[o] = (r * 255.0).min(255.0) as u8;
            px[o + 1] = (g * 255.0).min(255.0) as u8;
            px[o + 2] = (b * 255.0).min(255.0) as u8;
            // Keep a floor of coverage so a sheet sampled for its alpha does not
            // disappear between filaments.
            px[o + 3] = ((0.22 + 0.78 * t) * 255.0).min(255.0) as u8;
        }
    }
    if let Some(target) = luma_target {
        normalize_luma(px, target);
    }
}

fn paint_flow(px: &mut [u8], w: usize, h: usize, stretch: f32) {
    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32;
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32;
            let t = flow_at(u, v, stretch);
            // Flowing water is a value ramp, not a hue ramp: the deep parts are
            // darker teal, the aerated crests near white.
            let (r, g, b) = (
                0.06 + 0.86 * t.powf(1.8),
                0.30 + 0.68 * t,
                0.46 + 0.54 * t.powf(0.8),
            );
            let o = (y * w + x) * 4;
            px[o] = (r * 255.0).min(255.0) as u8;
            px[o + 1] = (g * 255.0).min(255.0) as u8;
            px[o + 2] = (b * 255.0).min(255.0) as u8;
            px[o + 3] = ((0.15 + 0.85 * t) * 255.0).min(255.0) as u8;
        }
    }
}

/// Rounded splash silhouette: a lobed radial shape plus a few thrown droplets.
///
/// The donor is a spiky star-shaped flame mask, so this is the one place in the
/// build where an *outline* changes. Fire is jagged because it is combustion
/// fronts; a splash is jagged the other way, in smooth surface-tension lobes. The
/// shape is authored as a radius that varies with angle rather than as a union of
/// circles: circles large enough to match the donor's coverage fuse into a
/// featureless disc, whereas a few summed cosines give arms that stay distinct at
/// any size. Centred, not tiling.
fn paint_splash_lobes(px: &mut [u8], w: usize, h: usize) {
    // Arms: (harmonic, amplitude, phase). Coprime-ish harmonics keep the outline
    // from settling into an obvious symmetry.
    let arms = [
        (3.0f32, 0.17f32, 0.7f32),
        (5.0, 0.12, 2.1),
        (7.0, 0.06, 4.3),
    ];
    let base_radius = 0.45f32;
    // Detached droplets thrown clear of the main mass.
    let drops: Vec<(f32, f32, f32)> = (0..6u32)
        .map(|i| {
            let ang = rnd(i * 17 + 11) * std::f32::consts::TAU;
            let dist = 0.40 + 0.07 * rnd(i * 31 + 13);
            (
                0.5 + ang.cos() * dist,
                0.5 + ang.sin() * dist,
                0.016 + 0.024 * rnd(i * 7 + 19),
            )
        })
        .collect();

    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32 - 0.5;
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32 - 0.5;
            let d = (u * u + v * v).sqrt();
            let theta = v.atan2(u);
            let radius = base_radius
                * (1.0
                    + arms
                        .iter()
                        .map(|&(k, a, p)| a * (k * theta + p).cos())
                        .sum::<f32>());
            // Soft shoulder over ~4% of the frame: the surface-tension edge.
            let mut shape = ((radius - d) / 0.045).clamp(0.0, 1.0);
            for &(cx, cy, r) in &drops {
                let dd = ((u + 0.5 - cx).powi(2) + (v + 0.5 - cy).powi(2)).sqrt();
                shape = shape.max((1.0 - (dd / r).clamp(0.0, 1.0)).min(1.0));
            }
            let shaped = shape * shape * (3.0 - 2.0 * shape);
            // Bright throughout with a brighter band just inside the edge: a water
            // lobe catches the light at its rim, and a flat interior is what made
            // the first attempt read as a grey disc.
            let edge = (1.0 - ((radius - d) / (base_radius * 0.34)).clamp(0.0, 1.0)).powf(1.5);
            let lum = (0.80 + 0.25 * edge).min(1.0) * shaped;
            let o = (y * w + x) * 4;
            px[o] = (lum * 235.0).min(255.0) as u8;
            px[o + 1] = (lum * 249.0).min(255.0) as u8;
            px[o + 2] = (lum * 255.0).min(255.0) as u8;
            px[o + 3] = (shaped * 255.0).min(255.0) as u8;
        }
    }
}

/// A single bead of water catching the light: specular core, soft bloom, a thin
/// bubble rim, and a short 4-point cross. Centred, not tiling.
fn paint_glint(px: &mut [u8], w: usize, h: usize) {
    for y in 0..h {
        let v = (y as f32 + 0.5) / h as f32 - 0.5;
        for x in 0..w {
            let u = (x as f32 + 0.5) / w as f32 - 0.5;
            let d = (u * u + v * v).sqrt();
            // Core + bloom.
            let core = (1.0 - (d / 0.13).clamp(0.0, 1.0)).powf(1.3);
            let bloom = (1.0 - (d / 0.48).clamp(0.0, 1.0)).powf(1.7) * 0.70;
            // Bubble rim: a bright ring, the read that says "droplet" rather than
            // "spark". Wide enough to survive the mip chain.
            let rim = (1.0 - ((d - 0.25).abs() / 0.055).clamp(0.0, 1.0)).powf(1.5) * 0.08;
            // Cross flare, far tighter than the many-rayed star it replaces.
            let cross = {
                let ax = (1.0 - (u.abs() / 0.44).clamp(0.0, 1.0)).powf(1.8)
                    * (1.0 - (v.abs() / 0.030).clamp(0.0, 1.0));
                let ay = (1.0 - (v.abs() / 0.44).clamp(0.0, 1.0)).powf(1.8)
                    * (1.0 - (u.abs() / 0.030).clamp(0.0, 1.0));
                (ax + ay) * 0.55
            };
            let lum = (core + bloom + rim + cross).min(1.0);
            let o = (y * w + x) * 4;
            px[o] = (lum * 226.0).min(255.0) as u8;
            px[o + 1] = (lum * 246.0).min(255.0) as u8;
            px[o + 2] = (lum * 255.0).min(255.0) as u8;
            px[o + 3] = (lum * 255.0).min(255.0) as u8;
        }
    }
}

/// Overlay fine striations that run ALONG the donor's own luminance contours.
///
/// The donor here is a hand-drawn flame wisp whose orientation on the model is
/// unknown (its UV layout is not something we can read off the texture), so any
/// striation direction picked in texture space is a guess. Reading the direction
/// out of the image itself sidesteps that: the perpendicular of the luminance
/// gradient is the direction the artist's own shape flows in, so the striations
/// follow the wisp no matter how the mesh maps it.
fn paint_flow_aligned(px: &mut [u8], w: usize, h: usize) {
    let luma_at = |src: &[u8], x: usize, y: usize| -> f32 {
        let o = (y.min(h - 1) * w + x.min(w - 1)) * 4;
        f32::from(src[o]).mul_add(
            0.2126,
            f32::from(src[o + 1]).mul_add(0.7152, f32::from(src[o + 2]) * 0.0722),
        ) / 255.0
    };
    let src = px.to_vec();
    let target = mean_luma(&src);

    for y in 0..h {
        for x in 0..w {
            let base = luma_at(&src, x, y);
            // Banding on the luminance LEVEL puts every band on an iso-luminance
            // contour, which is the flow line of the artist's own shape. No
            // direction has to be chosen, so nothing depends on the UV layout.
            let stripe = 0.5 + 0.5 * (base * 42.0).sin();
            // Central differences give the gradient magnitude, used only to fade
            // the striation out where the image is flat and the contours carry no
            // meaningful direction.
            let gx = luma_at(&src, x + 1, y) - luma_at(&src, x.saturating_sub(1), y);
            let gy = luma_at(&src, x, y + 1) - luma_at(&src, x, y.saturating_sub(1));
            let mag = (gx * gx + gy * gy).sqrt();
            let strength = (mag * 6.0).clamp(0.0, 1.0) * 0.45;
            let t = (base * (1.0 - strength) + stripe * base * strength * 2.0).clamp(0.0, 1.0);
            // Water surface: value carries the shape, hue stays in the water band.
            let o = (y * w + x) * 4;
            px[o] = (t.powf(2.0) * 235.0).min(255.0) as u8;
            px[o + 1] = (t.powf(1.15) * 232.0).min(255.0) as u8;
            px[o + 2] = (t.powf(0.85) * 255.0).min(255.0) as u8;
        }
    }
    normalize_luma(px, target);
}

// ---------------------------------------------------------------------------
// Physics: fire is buoyant and flickers, water is heavy and flows
// ---------------------------------------------------------------------------

/// Buoyant gravity (a positive Z, which is what makes flame and smoke rise) is
/// negated so the same particles fall. Values already negative are debris that
/// falls correctly and are left alone.
const FALL_SCALE: f64 = 1.0;
/// Water drags far harder than hot gas. Applied to `C_OP_BasicMovement/m_fDrag`.
const DRAG_SCALE: f64 = 1.8;
/// `m_fDrag` above this is a hard stop rather than a fluid, so the scale is capped.
const DRAG_CEILING: f64 = 0.45;
/// Curl-noise amplitude scale. Fire's flicker is high-amplitude small-scale curl;
/// water's motion is broader and calmer.
const TURBULENCE_SCALE: f64 = 0.55;
/// Curl-noise frequency scale: lower frequency means broader, slower swirls.
const TURBULENCE_FREQ_SCALE: f64 = 0.7;

#[derive(Default)]
struct PhysicsStats {
    gravity_flipped: usize,
    drag_raised: usize,
    turbulence_calmed: usize,
}

impl PhysicsStats {
    fn total(&self) -> usize {
        self.gravity_flipped + self.drag_raised + self.turbulence_calmed
    }
}

/// A double that is exactly 0.0 or 1.0 may be stored as a tagless KV3 singleton
/// with no data lane, which an in-place patch cannot address. Those two values are
/// therefore never targeted; skipping them is lossless here because a zero gravity
/// component stays zero and a drag of 1.0 is already past the ceiling.
fn is_patchable_double(v: f64) -> bool {
    v != 0.0 && v != 1.0
}

/// Read `m_vLiteralValue` from a PVEC input, but only when the input is actually
/// literal: `PVEC_TYPE_FLOAT_COMPONENTS` and friends ignore that array and take
/// their value from `m_FloatComponentX/Y/Z`, so patching it would be a silent
/// no-op that the build would nonetheless report as a change.
fn literal_vec_path(input: &Value, base: &[Seg]) -> Option<(Vec<Seg>, Vec<f64>)> {
    if input.get("m_nType").and_then(Value::as_str)? != "PVEC_TYPE_LITERAL" {
        return None;
    }
    let values: Vec<f64> = input
        .get("m_vLiteralValue")?
        .as_array()?
        .iter()
        .filter_map(Value::as_f64)
        .collect();
    let mut path = base.to_vec();
    path.push(Seg::Key("m_vLiteralValue".to_string()));
    Some((path, values))
}

/// Walk a particle and collect the in-place double edits that turn its motion from
/// fire into water.
fn collect_physics_edits(
    v: &Value,
    path: &mut Vec<Seg>,
    edits: &mut Vec<(Vec<Seg>, f64)>,
    stats: &mut PhysicsStats,
) {
    if let Value::Object(pairs) = v {
        match v.get("_class").and_then(Value::as_str) {
            Some("C_OP_BasicMovement") => {
                if let Some(g) = v.get("m_Gravity").and_then(Value::as_array) {
                    // Z is up in Source 2, so a positive Z gravity is buoyancy.
                    if let Some(z) = g.get(2).and_then(Value::as_f64) {
                        if z > 0.0 && is_patchable_double(z) {
                            let mut p = path.clone();
                            p.push(Seg::Key("m_Gravity".to_string()));
                            p.push(Seg::Index(2));
                            edits.push((p, -z * FALL_SCALE));
                            stats.gravity_flipped += 1;
                        }
                    }
                }
                if let Some(d) = v.get("m_fDrag").and_then(Value::as_f64) {
                    let next = (d * DRAG_SCALE).min(DRAG_CEILING);
                    if is_patchable_double(d)
                        && is_patchable_double(next)
                        && (next - d).abs() > 1e-9
                    {
                        let mut p = path.clone();
                        p.push(Seg::Key("m_fDrag".to_string()));
                        edits.push((p, next));
                        stats.drag_raised += 1;
                    }
                }
            }
            Some("C_OP_CurlNoiseForce") => {
                for (key, scale) in [
                    ("m_vecNoiseScale", TURBULENCE_SCALE),
                    ("m_vecNoiseFreq", TURBULENCE_FREQ_SCALE),
                ] {
                    let Some(input) = v.get(key) else { continue };
                    let mut base = path.clone();
                    base.push(Seg::Key(key.to_string()));
                    let Some((vec_path, values)) = literal_vec_path(input, &base) else {
                        continue;
                    };
                    let mut touched = false;
                    for (i, &c) in values.iter().enumerate() {
                        let next = c * scale;
                        if !is_patchable_double(c) || !is_patchable_double(next) {
                            continue;
                        }
                        let mut p = vec_path.clone();
                        p.push(Seg::Index(i));
                        edits.push((p, next));
                        touched = true;
                    }
                    if touched {
                        stats.turbulence_calmed += 1;
                    }
                }
            }
            _ => {}
        }
        for (k, child) in pairs {
            path.push(Seg::Key(k.clone()));
            collect_physics_edits(child, path, edits, stats);
            path.pop();
        }
    } else if let Value::Array(items) = v {
        for (i, item) in items.iter().enumerate() {
            path.push(Seg::Index(i));
            collect_physics_edits(item, path, edits, stats);
            path.pop();
        }
    }
}

// ---------------------------------------------------------------------------
// Colour helpers for the material pass
// ---------------------------------------------------------------------------

fn rgb_to_hsv(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f64::EPSILON {
        0.0
    } else if (max - r).abs() < f64::EPSILON {
        60.0 * (((g - b) / d) % 6.0)
    } else if (max - g).abs() < f64::EPSILON {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max <= f64::EPSILON { 0.0 } else { d / max };
    (h.rem_euclid(360.0), s, max)
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> [f64; 3] {
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
    [r + m, g + m, b + m]
}

/// Drive one RGB triple to the water hue, keeping its own saturation and value
/// structure (scaled). This is the same "set hue, keep S and V" rule the texture,
/// particle and vertex-colour recolors use, so one hue lands them all together.
fn to_water(rgb: [f64; 3], recolor: vpkmerge_core::Recolor, sat: f64, val: f64) -> [f64; 3] {
    let (_, s, v) = rgb_to_hsv(rgb[0], rgb[1], rgb[2]);
    hsv_to_rgb(
        recolor.hue,
        (s * recolor.saturation * sat).clamp(0.0, 1.0),
        (v * recolor.value * val).clamp(0.0, 1.0),
    )
}

// ---------------------------------------------------------------------------

/// Every `m_hTexture` under the renderers, with its KV3 path.
fn texture_inputs(tree: &Value) -> Vec<(Vec<Seg>, String)> {
    let mut out = Vec::new();
    let Some(renderers) = tree.get("m_Renderers").and_then(Value::as_array) else {
        return out;
    };
    for (ri, renderer) in renderers.iter().enumerate() {
        let Some(inputs) = renderer.get("m_vecTexturesInput").and_then(Value::as_array) else {
            continue;
        };
        for (ii, input) in inputs.iter().enumerate() {
            if let Some(tex) = input.get("m_hTexture").and_then(Value::as_str) {
                out.push((
                    vec![
                        Seg::Key("m_Renderers".to_string()),
                        Seg::Index(ri),
                        Seg::Key("m_vecTexturesInput".to_string()),
                        Seg::Index(ii),
                        Seg::Key("m_hTexture".to_string()),
                    ],
                    tex.to_string(),
                ));
            }
        }
    }
    out
}

/// Does this `.vtex_c` carry VTEX SHEET extra-data (type 2)? Mirrors
/// `examples/shts_check.rs`; used to assert the map's class claims against the
/// live pak instead of trusting the table.
fn is_flipbook(vpk: &str, vtex_entry: &str) -> Result<bool> {
    let bytes = vpkmerge_core::read_vpk_entry(vpk, vtex_entry)?;
    let u32_at = |b: &[u8], o: usize| -> Option<u32> {
        b.get(o..o + 4)
            .map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]]))
    };
    // Locate DATA via the block table at offset 8.
    let block_offset = u32_at(&bytes, 8).context("short resource")? as usize;
    let block_count = u32_at(&bytes, 12).context("short resource")? as usize;
    let table = 8 + block_offset;
    for i in 0..block_count {
        let e = table + i * 12;
        let Some(tag) = bytes.get(e..e + 4) else {
            break;
        };
        if tag != b"DATA" {
            continue;
        }
        let rel = u32_at(&bytes, e + 4).context("short block")? as usize;
        let size = u32_at(&bytes, e + 8).context("short block")? as usize;
        let data = bytes
            .get(e + 4 + rel..e + 4 + rel + size)
            .context("DATA out of range")?;
        let off = u32_at(data, 32).context("short vtex header")? as usize;
        let count = u32_at(data, 36).context("short vtex header")? as usize;
        for k in 0..count {
            if u32_at(data, 32 + off + k * 12) == Some(2) {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    anyhow::bail!("no DATA block in {vtex_entry}")
}

/// Every string leaf in a KV3 tree, paired with its KV3 path. Used to locate a
/// texture reference anywhere inside a `.vmat_c` without hardcoding the slot name
/// (the same ref can sit under `m_textureParams` or a template's data block).
fn string_leaves(v: &Value) -> Vec<(Vec<Seg>, String)> {
    fn walk(v: &Value, path: &mut Vec<Seg>, out: &mut Vec<(Vec<Seg>, String)>) {
        match v {
            Value::String(s) => out.push((path.clone(), s.clone())),
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    path.push(Seg::Index(i));
                    walk(x, path, out);
                    path.pop();
                }
            }
            Value::Object(o) => {
                for (k, x) in o {
                    path.push(Seg::Key(k.clone()));
                    walk(x, path, out);
                    path.pop();
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, &mut Vec::new(), &mut out);
    out
}

/// `.vtex` (source ref) -> `.vtex_c` (compiled entry).
fn compiled(p: &str) -> String {
    if p.ends_with("_c") {
        p.to_string()
    } else {
        format!("{p}_c")
    }
}

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    if argv.len() < 3 {
        eprintln!(
            "usage: inferno_water <pak01_dir.vpk> <out_dir.vpk> \
             [--hue 200] [--sat 0.8] [--val 1.0] [--dry-run] \
             [--no-physics] [--no-materials] [--no-repaint] [--preview-dir DIR]"
        );
        std::process::exit(2);
    }
    let pak = argv[1].clone();
    let out = argv[2].clone();
    let flag = |name: &str, default: f64| -> f64 {
        argv.iter()
            .position(|a| a == name)
            .and_then(|i| argv.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let hue = flag("--hue", 200.0);
    let sat = flag("--sat", 0.8);
    let val = flag("--val", 1.0);
    let dry_run = argv.iter().any(|a| a == "--dry-run");
    // Each round-3 pass is independently switchable. Nothing here is in-game
    // confirmed yet, so being able to build the same addon minus one pass is how
    // a bad-looking result gets attributed to the pass that caused it.
    let no_physics = argv.iter().any(|a| a == "--no-physics");
    let no_materials = argv.iter().any(|a| a == "--no-materials");
    let no_repaint = argv.iter().any(|a| a == "--no-repaint");
    // Every painted sheet also written as a PNG. Painting is the one pass whose
    // output cannot be judged from a log line.
    let preview_dir: Option<String> = argv
        .iter()
        .position(|a| a == "--preview-dir")
        .and_then(|i| argv.get(i + 1))
        .cloned();
    if let Some(dir) = &preview_dir {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {dir}"))?;
    }
    let save_preview = |name: &str, image: &morphic::Image| -> Result<()> {
        let Some(dir) = &preview_dir else {
            return Ok(());
        };
        let ImageData::Rgba8(px) = &image.data else {
            return Ok(());
        };
        image::save_buffer(
            std::path::Path::new(dir).join(format!("{name}.png")),
            px,
            image.width,
            image.height,
            image::ColorType::Rgba8,
        )
        .with_context(|| format!("writing preview for {name}"))?;
        Ok(())
    };
    let recolor = vpkmerge_core::Recolor::new(hue, sat, val);

    println!("Infernus fire -> water");
    println!("  pak : {pak}");
    println!("  out : {out}");
    println!("  water color: hue {hue} deg, saturation x{sat}, value x{val}");
    println!(
        "  passes: physics={} materials={} hero-repaint={}\n",
        !no_physics, !no_materials, !no_repaint
    );

    // ---- 1. validate the swap map against the live pak ---------------------
    // A stale map (Valve renamed or removed a sheet) must fail loudly here rather
    // than silently skipping the swap and shipping orange fire.
    println!("== validating swap map ==");
    let vpk = valve_pak::open(&pak)?;
    let present: BTreeSet<String> = vpk.file_paths().cloned().collect();
    let mut problems = Vec::new();
    // A fire sheet may carry several swaps (a scoped one plus an unscoped
    // fallback), but at most one unscoped: two would make the winner depend on
    // table order, which is exactly the silent-collapse bug this guards.
    for s in SWAPS {
        if s.only.is_none()
            && SWAPS
                .iter()
                .filter(|o| o.fire == s.fire && o.only.is_none())
                .count()
                > 1
        {
            problems.push(format!("more than one unscoped swap for {}", s.fire));
        }
    }
    problems.sort();
    problems.dedup();
    for s in SWAPS {
        let fire_c = compiled(s.fire);
        if !present.contains(&fire_c) {
            problems.push(format!("fire sheet missing from pak: {}", s.fire));
            continue;
        }
        // Minted / authored targets are produced by this build, so they are not in the
        // pak yet. Their class is their donor's, checked where they are created.
        if s.water == WATER_RAMP || s.water.starts_with("materials/particle/vpkmerge_water/") {
            if let Some(scope) = s.only {
                let n = vpk
                    .file_paths()
                    .filter(|p| p.ends_with(".vpcf_c") && scope.iter().any(|f| p.contains(f)))
                    .count();
                if n == 0 {
                    problems.push(format!(
                        "scope matches no particle: {scope:?} for {}",
                        s.fire
                    ));
                }
            }
            continue;
        }
        if let Some(scope) = s.only {
            // A scoped swap must actually name real particles, else it is dead config.
            let n = vpk
                .file_paths()
                .filter(|p| p.ends_with(".vpcf_c") && scope.iter().any(|frag| p.contains(frag)))
                .count();
            if n == 0 {
                problems.push(format!(
                    "scope matches no particle: {scope:?} for {}",
                    s.fire
                ));
            }
        }
        let water_c = compiled(s.water);
        if !present.contains(&water_c) {
            problems.push(format!("water sheet missing from pak: {}", s.water));
            continue;
        }
        let (fire_fb, water_fb) = (is_flipbook(&pak, &fire_c)?, is_flipbook(&pak, &water_c)?);
        if fire_fb != water_fb {
            problems.push(format!(
                "CLASS MISMATCH {} (flipbook={fire_fb}) -> {} (flipbook={water_fb})",
                s.fire, s.water
            ));
        } else if fire_fb != s.flipbook {
            problems.push(format!(
                "map claims flipbook={} but pak says {fire_fb}: {}",
                s.flipbook, s.fire
            ));
        }
    }
    if problems.is_empty() {
        println!("  {} swaps, all present and class-matched\n", SWAPS.len());
    } else {
        for p in &problems {
            println!("  PROBLEM {p}");
        }
        anyhow::bail!("{} swap-map problem(s); refusing to build", problems.len());
    }

    // The water ramp used to be minted here by hue-rotating `ramp_fire`; it is now
    // painted with a real water depth curve instead, in the AUTHORED pass below.
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();

    // ---- 2b. recolor the hero-owned textures in place ----------------------
    // The pass round 1 missed. No repoint: these paths are his alone.
    println!(
        "== recoloring {} hero-owned textures ==",
        HERO_TEXTURES.len()
    );
    for (path, mode, what) in HERO_TEXTURES {
        let entry = compiled(path);
        if !present.contains(&entry) {
            anyhow::bail!("hero texture missing from pak: {path} ({what})");
        }
        let src = vpkmerge_core::read_vpk_entry(&pak, &entry)?;
        let (watered, how) = match mode {
            HeroTexMode::Paint(kind) if !no_repaint => {
                let mut image =
                    morphic::decode(&src).map_err(|e| anyhow::anyhow!("decoding {path}: {e}"))?;
                paint(&mut image, *kind)?;
                save_preview(path.rsplit('/').next().unwrap_or(path), &image)?;
                let bytes = morphic::replace_mip_chain(&src, &image)
                    .map_err(|e| anyhow::anyhow!("re-encoding repainted {path}: {e}"))?;
                (bytes, "repainted")
            }
            HeroTexMode::Paint(_) | HeroTexMode::Hue => (
                vpkmerge_core::recolor_texture_hue(&src, recolor)
                    .with_context(|| format!("recoloring {path}"))?,
                "hue-shifted",
            ),
        };
        println!("  {how:<12} {what}\n               {path}");
        files.push((entry, watered));
    }
    println!();

    // ---- 2b2. move the hero materials' own parameters to water --------------
    // Textures alone are not enough: these materials multiply their (now cyan)
    // albedo by a tint that rounds 1-2 left orange, and they carry fire's timing
    // in their pulse expression, jitter frequency and scroll direction.
    if no_materials {
        println!("== hero materials: SKIPPED (--no-materials) ==\n");
    } else {
        println!("== hero materials ({}) ==", HERO_MATERIALS.len());
        for m in HERO_MATERIALS {
            let entry = compiled(m.entry);
            if !present.contains(&entry) {
                anyhow::bail!("hero material missing from pak: {}", m.entry);
            }
            let src = vpkmerge_core::read_vpk_entry(&pak, &entry)?;
            let tree = morphic::decode_kv3_resource(&src)
                .map_err(|e| anyhow::anyhow!("decoding {}: {e}", m.entry))?;

            // Component-level double edits rather than whole-vector sets: a vec4's
            // unused 0.0 lanes are tagless KV3 singletons with no bytes to patch,
            // so writing all four components would fail on every one of these.
            let mut doubles: Vec<(Vec<Seg>, f64)> = Vec::new();
            // Colour params that cannot be written in place because at least one
            // channel they need is a tagless zero.
            let mut promote: Vec<(String, [f64; 3], usize)> = Vec::new();
            let mut changed: Vec<String> = Vec::new();
            let params = tree
                .get("m_vectorParams")
                .and_then(Value::as_array)
                .map(<[Value]>::to_vec)
                .unwrap_or_default();
            for (i, param) in params.iter().enumerate() {
                let Some(name) = param.get("m_name").and_then(Value::as_str) else {
                    continue;
                };
                let Some(rgba) = param.get("m_value").and_then(Value::as_array) else {
                    continue;
                };
                let component = |k: usize| rgba.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                let at = |k: usize| {
                    vec![
                        Seg::Key("m_vectorParams".to_string()),
                        Seg::Index(i),
                        Seg::Key("m_value".to_string()),
                        Seg::Index(k),
                    ]
                };

                if is_colour_param(name) {
                    let rgb = [component(0), component(1), component(2)];
                    let (_, s, _) = rgb_to_hsv(rgb[0], rgb[1], rgb[2]);
                    if s < TINT_SATURATION_FLOOR {
                        continue; // a neutral multiplier, not a colour
                    }
                    let next = to_water(rgb, recolor, m.tint_sat, m.tint_val);
                    // A channel stored as a tagless KV3 zero has no bytes to
                    // overwrite. That is not a corner case here: an orange tint is
                    // [r, g, 0], and water needs blue in exactly that lane, so the
                    // in-place patch would silently leave every glow GREEN.
                    let blocked: Vec<usize> = (0..3)
                        .filter(|&k| !is_patchable_double(component(k)))
                        .filter(|&k| (next[k] - component(k)).abs() > 1e-9)
                        .collect();
                    if blocked.is_empty() {
                        for (k, &c) in next.iter().enumerate() {
                            if is_patchable_double(component(k)) && is_patchable_double(c) {
                                doubles.push((at(k), c));
                            }
                        }
                    } else {
                        promote.push((name.to_string(), next, blocked.len()));
                    }
                    changed.push(format!(
                        "{name} [{:.2} {:.2} {:.2}] -> [{:.2} {:.2} {:.2}]{}",
                        rgb[0],
                        rgb[1],
                        rgb[2],
                        next[0],
                        next[1],
                        next[2],
                        if blocked.is_empty() {
                            ""
                        } else {
                            "  (needs a promoted lane)"
                        }
                    ));
                } else if name.contains("JitterFrequencies") && m.jitter != 1.0 {
                    for k in 0..3 {
                        let c = component(k);
                        let next = c * m.jitter;
                        if is_patchable_double(c) && is_patchable_double(next) {
                            doubles.push((at(k), next));
                        }
                    }
                    changed.push(format!("{name} x{}", m.jitter));
                } else if name == "g_vAlbedoScrollSpeed1" && m.scroll != 1.0 {
                    // X keeps its direction and slows; Y reverses. Fire licks one
                    // way along the sheet, running water goes the other.
                    for (k, sign) in [(0usize, 1.0f64), (1, -1.0)] {
                        let c = component(k);
                        let next = c * m.scroll * sign;
                        if is_patchable_double(c) && is_patchable_double(next) {
                            doubles.push((at(k), next));
                        }
                    }
                    changed.push(format!(
                        "{name} [{:.2} {:.2}] -> [{:.2} {:.2}] (Y reversed)",
                        component(0),
                        component(1),
                        component(0) * m.scroll,
                        -component(1) * m.scroll
                    ));
                }
            }

            let mut bytes = src.clone();
            if !doubles.is_empty() {
                bytes = morphic::patch_kv3_resource_doubles(&bytes, &doubles)
                    .map_err(|e| anyhow::anyhow!("patching {} params: {e}", m.entry))?;
            }

            // Two ways out of a missing lane, chosen by whether the material
            // carries binary blobs:
            //
            //  - No blobs: a full re-encode promotes the tagless zero to a real
            //    double, which is the fallback `recolor_material_color_bytes`
            //    already relies on. `patch_vmat_params` takes that route by itself.
            //  - Blobs: a re-encode emits them uncompressed and the engine
            //    misreads that (a red-wireframe material), so the static param
            //    cannot be touched at all. Instead the value goes in as a *dynamic*
            //    expression, which is a structural insert into `m_dynamicParams`
            //    and leaves the blob region compressed. A dynamic param overrides
            //    the static one of the same name, which is how Valve's own
            //    materials drive a tint per frame.
            let blobbed = morphic::kv3_resource_has_blobs(&bytes).unwrap_or(true);
            for (name, rgb, n_blocked) in &promote {
                let edit = if blobbed {
                    vpkmerge_core::VmatEdit::expr(
                        name.clone(),
                        &format!("float3({:.4},{:.4},{:.4})", rgb[0], rgb[1], rgb[2]),
                    )?
                } else {
                    vpkmerge_core::VmatEdit::Vector {
                        name: name.clone(),
                        value: [rgb[0], rgb[1], rgb[2], 0.0],
                    }
                };
                let (patched, stats) = vpkmerge_core::patch_vmat_params(&bytes, &[edit])
                    .with_context(|| format!("promoting {name} on {}", m.entry))?;
                if !stats.failed.is_empty() {
                    anyhow::bail!(
                        "{}: {name} has {n_blocked} channel(s) with no data lane and \
                         neither promotion route worked ({:?}); shipping it would \
                         leave the tint green",
                        m.entry,
                        stats.failed
                    );
                }
                bytes = patched;
                changed.push(format!(
                    "{name} promoted via {}",
                    if blobbed {
                        "a dynamic expression (blobbed material)"
                    } else {
                        "re-encode"
                    }
                ));
            }

            if let Some(pulse) = m.pulse {
                let edit = vpkmerge_core::VmatEdit::expr("g_flSelfIllumScale1", pulse)?;
                let (patched, stats) = vpkmerge_core::patch_vmat_params(&bytes, &[edit])
                    .with_context(|| format!("retiming the pulse on {}", m.entry))?;
                if !stats.failed.is_empty() {
                    anyhow::bail!(
                        "{}: could not retime {:?}; a blobbed material that refuses \
                         the expression swap must not be shipped half-edited",
                        m.entry,
                        stats.failed
                    );
                }
                bytes = patched;
                changed.push(format!("pulse -> {pulse}"));
            }
            morphic::decode_kv3_resource(&bytes)
                .map_err(|e| anyhow::anyhow!("{} no longer decodes after edit: {e}", m.entry))?;

            println!("  {} ({} edits)", m.note, changed.len());
            for c in &changed {
                println!("      {c}");
            }
            files.push((entry, bytes));
        }
        println!();
    }

    // ---- 2c. mint water copies of shared textures a hero material uses ------
    println!("== material repoints ==");
    for (mat, shared) in MATERIAL_REPOINTS {
        let mat_entry = compiled(mat);
        if !present.contains(&mat_entry) {
            anyhow::bail!("hero material missing from pak: {mat}");
        }
        let mut bytes = vpkmerge_core::read_vpk_entry(&pak, &mat_entry)?;
        let tree = morphic::decode_kv3_resource(&bytes)
            .map_err(|e| anyhow::anyhow!("decoding {mat}: {e}"))?;

        // Find where in the material tree each shared texture is referenced.
        let mut edits: Vec<(Vec<Seg>, String)> = Vec::new();
        for tex in *shared {
            let target = minted_path(tex);
            let mut found = false;
            for (path, value) in string_leaves(&tree) {
                if value == *tex {
                    edits.push((path, target.clone()));
                    found = true;
                }
            }
            if !found {
                anyhow::bail!("{mat} does not reference {tex}");
            }
            // Mint the water copy at the new path.
            let src = vpkmerge_core::read_vpk_entry(&pak, &compiled(tex))?;
            let watered = vpkmerge_core::recolor_texture_hue(&src, recolor)
                .with_context(|| format!("minting water copy of {tex}"))?;
            println!("  minted {target}");
            files.push((compiled(&target), watered));
        }
        bytes = morphic::patch_kv3_resource_strings_adding(&bytes, &edits)
            .map_err(|e| anyhow::anyhow!("repointing {mat}: {e}"))?;
        morphic::decode_kv3_resource(&bytes)
            .map_err(|e| anyhow::anyhow!("{mat} no longer decodes after repoint: {e}"))?;
        println!("  repointed {mat} ({} ref(s))", edits.len());
        files.push((mat_entry, bytes));
    }
    println!();

    // ---- 2d. paint the authored sheets -------------------------------------
    println!("== authored sheets ==");
    for a in AUTHORED {
        let donor_entry = compiled(a.donor);
        if !present.contains(&donor_entry) {
            anyhow::bail!("authored-sheet donor missing from pak: {}", a.donor);
        }
        // A flipbook donor would leave stale cell metadata over a continuous image.
        if is_flipbook(&pak, &donor_entry)? {
            anyhow::bail!(
                "donor {} is a FLIPBOOK; authored sheets need a plain donor",
                a.donor
            );
        }
        let donor = vpkmerge_core::read_vpk_entry(&pak, &donor_entry)?;
        let mut image = morphic::decode(&donor)
            .map_err(|e| anyhow::anyhow!("decoding donor {}: {e}", a.donor))?;
        paint(&mut image, a.kind)?;
        save_preview(a.path.rsplit('/').next().unwrap_or(a.path), &image)?;
        let bytes = morphic::replace_mip_chain(&donor, &image)
            .map_err(|e| anyhow::anyhow!("encoding authored sheet {}: {e}", a.path))?;
        println!(
            "  {}x{}  {}\n      {}  (donor {})",
            image.width,
            image.height,
            a.note,
            a.path,
            a.donor.rsplit('/').next().unwrap_or(a.donor)
        );
        files.push((compiled(a.path), bytes));
    }
    println!();

    // ---- 3. retarget every particle ---------------------------------------
    // A fire sheet can carry several swaps: a scoped one for the effect it needs
    // special treatment in, plus an unscoped fallback for everywhere else. Keeping
    // a Vec per key (rather than the single-entry map this used to be) is what
    // stops the second entry from being silently dropped.
    let mut by_fire: BTreeMap<&str, Vec<(usize, &Swap)>> = BTreeMap::new();
    for (i, s) in SWAPS.iter().enumerate() {
        by_fire.entry(s.fire).or_default().push((i, s));
    }
    // Scoped entries first, so first-match-wins resolves to the specific rule.
    for v in by_fire.values_mut() {
        v.sort_by_key(|(_, s)| s.only.is_none());
    }
    let particles: Vec<String> = {
        let mut v: Vec<String> = vpk
            .file_paths()
            .filter(|p| PREFIXES.iter().any(|pre| p.starts_with(pre)) && p.ends_with(".vpcf_c"))
            .cloned()
            .collect();
        v.sort();
        v
    };

    println!("== retargeting {} particles ==", particles.len());
    // Keyed by index into SWAPS, not by sheet name: several rules can share a
    // source (scoped + fallback) and several can share a target, so only the rule
    // itself identifies which one fired.
    let mut swap_hits: BTreeMap<usize, usize> = BTreeMap::new();
    let mut grade_hits: BTreeMap<&str, usize> = BTreeMap::new();
    let mut repointed = 0usize;
    let mut recolored = 0usize;
    let mut physics_particles = 0usize;
    let mut physics = PhysicsStats::default();
    let mut untouched = Vec::new();

    for entry in &particles {
        let original = vpkmerge_core::read_vpk_entry(&pak, entry)?;
        let tree = morphic::decode_kv3_resource(&original)
            .map_err(|e| anyhow::anyhow!("decoding {entry}: {e}"))?;

        // 3a. sheet repoint
        let mut edits: Vec<(Vec<Seg>, String)> = Vec::new();
        for (path, tex) in texture_inputs(&tree) {
            let Some(candidates) = by_fire.get(tex.as_str()) else {
                continue;
            };
            // First match wins, scoped before unscoped: a swap that names this
            // particle beats the kit-wide fallback for the same sheet.
            let hit = candidates.iter().find(|(_, s)| {
                s.only
                    .is_none_or(|scope| scope.iter().any(|frag| entry.contains(frag)))
            });
            if let Some((i, s)) = hit {
                edits.push((path, s.water.to_string()));
                *swap_hits.entry(*i).or_default() += 1;
            }
        }
        let mut bytes = original.clone();
        let did_repoint = !edits.is_empty();
        if did_repoint {
            bytes = morphic::patch_kv3_resource_strings_adding(&bytes, &edits)
                .map_err(|e| anyhow::anyhow!("repointing sheets in {entry}: {e}"))?;
            repointed += 1;
        }

        // 3b. color params, graded by the particle's role rather than one flat hue
        let (grade, grade_name) = palette_for(entry);
        *grade_hits.entry(grade_name).or_default() += 1;
        let did_recolor = match vpkmerge_core::recolor_particle_bytes(&bytes, grade) {
            Ok(Some(next)) => {
                bytes = next;
                recolored += 1;
                true
            }
            Ok(None) => false,
            Err(e) => anyhow::bail!("recoloring {entry}: {e}"),
        };

        // 3c. physics: colour and imagery say water, motion still says fire
        let mut did_physics = false;
        if !no_physics {
            let mut phys_edits = Vec::new();
            let mut here = PhysicsStats::default();
            collect_physics_edits(&tree, &mut Vec::new(), &mut phys_edits, &mut here);
            if !phys_edits.is_empty() {
                bytes = morphic::patch_kv3_resource_doubles(&bytes, &phys_edits)
                    .map_err(|e| anyhow::anyhow!("retuning physics in {entry}: {e}"))?;
                physics.gravity_flipped += here.gravity_flipped;
                physics.drag_raised += here.drag_raised;
                physics.turbulence_calmed += here.turbulence_calmed;
                physics_particles += 1;
                did_physics = true;
            }
        }

        if did_repoint || did_recolor || did_physics {
            // Re-decode as a load sanity check: a patch that corrupted the framing
            // must fail here, not in the engine.
            morphic::decode_kv3_resource(&bytes)
                .map_err(|e| anyhow::anyhow!("{entry} no longer decodes after patch: {e}"))?;
            files.push((entry.clone(), bytes));
        } else {
            untouched.push(entry.clone());
        }
    }

    println!("  {repointed} had sheets repointed, {recolored} had color params recolored");
    if no_physics {
        println!("  physics: SKIPPED (--no-physics)");
    } else {
        println!(
            "  physics: {physics_particles} particles, {} edits \
             ({} buoyant gravities flipped to fall, {} drags raised, \
             {} curl-noise inputs calmed)",
            physics.total(),
            physics.gravity_flipped,
            physics.drag_raised,
            physics.turbulence_calmed
        );
    }
    println!(
        "  {} entries to pack, {} untouched",
        files.len(),
        untouched.len()
    );

    println!("\n== colour grades (water is not monochrome) ==");
    for (name, n) in &grade_hits {
        println!("  {n:>3} particles  {name}");
    }

    println!("\n== swap usage ==");
    for (i, s) in SWAPS.iter().enumerate() {
        let n = swap_hits.get(&i).copied().unwrap_or(0);
        let mark = if n == 0 { "  (unused)" } else { "" };
        println!(
            "  {n:>3}x {}\n        -> {}   [{}]{mark}",
            s.fire, s.water, s.note
        );
    }

    if dry_run {
        println!("\n--dry-run: nothing written");
        return Ok(());
    }

    // ---- 4. pack ----------------------------------------------------------
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &out)?;
    println!("\nwrote {out} ({} entries)", refs.len());
    Ok(())
}
