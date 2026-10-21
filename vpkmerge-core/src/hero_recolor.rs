//! Compose a hero's full ability-VFX recolor into one addon VPK.
//!
//! A Deadlock hero's ability effects carry color through three independent
//! mechanisms, each needing a different edit (see
//! `../grimoire/docs/ability-vfx-recolor.md`):
//!
//! 1. **Particle params** (`.vpcf_c`): `m_ConstantColor` / gradient stops, edited
//!    by an in-place KV3 scalar patch ([`morphic::patch_kv3_resource_scalars`]).
//! 2. **Color-bearing textures** (`.vtex_c`): self-illum / albedo color maps that
//!    a particle param can only multiply over, so they get their own hue shift
//!    ([`crate::recolor::recolor_texture_hue`]).
//! 3. **Mesh vertex colors** (`.vmdl_c`): baked per-vertex `COLOR` (Paige's ult
//!    horse/knight), reachable only by editing the mesh
//!    ([`crate::recolor::recolor_model_vertex_colors`]).
//!
//! All three use the **same absolute `set_color`** (set hue, scale each source
//! color's saturation + value), so one [`crate::Recolor`] target lands particles,
//! textures, and models on a single color. This module reads the recipe's entries out of a VPK (a skin
//! VPK first, then the base pak), recolors each, and packs the whole set into one
//! standalone addon that overrides the base in place: the single-call bridge a
//! mod manager drives.
//!
//! The recipe is currently a built-in per-hero table pinned from in-game-tested
//! recolor mods and local asset audits. Generalizing it to automatic discovery
//! is a later step; the composition here does not change when it does.

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use morphic::kv3::{Seg, Value};

use crate::recolor::{set_color, Recolor};
use crate::recolor_isolate::IsolationStats;

const YAMATO_SHADOW_SHAPE_COLOR_TEXTURE: &str =
    "models/heroes_staging/yamato_v2/materials/yamoto_shadow_shape_color_psd_fe3c64a6.vtex_c";
// The `nokill` status map is not pinned: a neutral barrel creature's
// self-explode status samples it too.
const YAMATO_SHADOW_STATUS_TEXTURES: &[&str] =
    &["materials/particle/abilities/yamato/yamato_shadow_redemption_complete_status.vtex_c"];

/// The fixed set of base-game entries that carry one hero's ability-VFX color,
/// grouped by the recolor mechanism each needs.
#[derive(Debug, Clone)]
pub struct HeroRecolorRecipe {
    /// Model/particle codename (Paige = `bookworm`), the namespace `models/` and
    /// `particles/abilities/` use (NOT the sound codename).
    pub codename: String,
    /// `particles/{abilities,weapon_fx}/<codename>/` roots; every `.vpcf_c` under
    /// these gets its color params retinted.
    pub particle_prefixes: Vec<String>,
    /// Color-bearing `.vtex_c` entries (self-illum / albedo color maps).
    pub texture_entries: Vec<String>,
    /// `.vmat_c` entries whose ability color is a material `g_vColorTint` constant
    /// (not a texture): the `g_t*color` slot is a flat 4x4 placeholder, so the color
    /// is retinted by an in-place double patch ([`recolor_material_color_bytes`]).
    pub material_entries: Vec<String>,
    /// `.vmdl_c` entries whose color is baked into mesh vertex colors.
    pub model_entries: Vec<String>,
    /// Entries (or path prefixes) that another hero, an item or a neutral also
    /// renders. These are never edited in place: where the hero reaches one
    /// through its own particles or pinned materials, the bake recolors a
    /// hero-local copy and repoints that reference to it, so the original stays
    /// vanilla for everyone else. Each gets the transform it would have had
    /// (particle recolor, material stamp when also in `material_entries`,
    /// texture recolor when also in `texture_entries`). `recolor-qa` computes
    /// the required set and fails a recipe that bleeds.
    pub shared_entries: Vec<String>,
    /// A representative color-bearing `.vtex_c` (one of `texture_entries`) to
    /// render as a fast, no-re-encode UI preview swatch (see
    /// [`recolor_hero_preview_png`]). Pick one that reads as the hero's main
    /// ability glow, not a tiny swatch or a near-black map. `None` for a
    /// particle-only hero (no color texture to swatch); the preview path then
    /// errors rather than guessing one.
    pub preview_texture: Option<String>,
}

/// The built-in recipe for a hero codename, or `None` if no recipe is pinned yet.
#[must_use]
pub fn recipe_for(codename: &str) -> Option<HeroRecolorRecipe> {
    let codename = codename.to_lowercase();
    let recipe = match codename.as_str() {
        "bookworm" => Some(paige_recipe()),
        // Particle-only heroes: ability/weapon VFX carry color purely through
        // `.vpcf_c` color params (no color textures or baked vertex colors), so one
        // `--hue` lands the whole ability/weapon set and nothing on the skin. Each
        // was pinned from an in-game recolor mod and confirmed with
        // `examples/particle_scan.rs` (base vs mod). Source hue is noted for
        // reference; the actual target hue is supplied at recolor time.
        //   codename    hero       src pak  .vpcf_c edited  source hue
        //   unicorn     Celeste    pak07    228             ~329 (pink)
        //   gigawatt    Seven      pak01    170             ~300 (magenta)
        //   vampirebat  Mina       pak05    117             ~295 (pink)
        //   necro       Graves     pak09    246             ~330 (pink)
        //   wraith      Wraith     pak11    127             ~330 (pink)
        //   inferno     Infernus   pak08    143             ~220 (blue)
        // Seven/Wraith/Infernus carry KV3 v4 particles; the in-place patcher handles
        // v4 + v5, so all recolor at full coverage.
        //
        // Graves (`necro`) is NOT particle-only (see `necro_recipe`): her large ability
        // maps are grayscale (particle-tinted), but the gravestone's transmissive glow
        // is a small chromatic texture, and the pickup-sphere / ult-jar color is a
        // material `g_vColorTint` CONSTANT (its `g_tColor` slot is a flat 4x4
        // placeholder), retinted by the in-place double patch. So her recipe adds one
        // texture + two materials on top of the particles.
        "necro" => Some(necro_recipe()),
        // Infernus (`inferno`): particles plus his two additive body glow layers. See
        // [`inferno_recipe`].
        "inferno" => Some(inferno_recipe()),
        "yamato" => Some(yamato_recipe()),
        // Paradox (`chrono`): ability VFX color is overwhelmingly particle-driven (the
        // teal time-warp; the `recolor_assets` walk found 0 chromatic ability textures,
        // her ability maps are grayscale ramps tinted by particle params). The two real
        // chromatic textures are the time-stop bubble energy bands on her bubble /
        // projectile prop models, recolored so the dome matches. See [`chrono_recipe`].
        "chrono" => Some(chrono_recipe()),
        "abrams" => Some(abrams_recipe()),
        "archer" => Some(archer_recipe()),
        "chessmaster" => Some(chessmaster_recipe()),
        "digger" => Some(digger_recipe()),
        "doorman" => Some(doorman_recipe()),
        "drifter" => Some(drifter_recipe()),
        "dynamo" => Some(dynamo_recipe()),
        "familiar" => Some(familiar_recipe()),
        "fencer" => Some(fencer_recipe()),
        "frank" => Some(frank_recipe()),
        "ghost" => Some(ghost_recipe()),
        "haze" => Some(haze_recipe()),
        "kelvin" => Some(kelvin_recipe()),
        "nano" => Some(nano_recipe()),
        "lash" => Some(lash_recipe()),
        "mcginnis" => Some(mcginnis_recipe()),
        "magician" => Some(magician_recipe()),
        "pocket" => Some(pocket_recipe()),
        "priest" => Some(priest_recipe()),
        "tengu" => Some(tengu_recipe()),
        "viper" => Some(viper_recipe()),
        "viscous" => Some(viscous_recipe()),
        "werewolf" => Some(werewolf_recipe()),
        "unicorn" => Some(unicorn_recipe()),
        "astro" => Some(astro_recipe()),
        "gigawatt" => Some(gigawatt_recipe()),
        "punkgoat" => Some(punkgoat_recipe()),
        "vampirebat" => Some(vampirebat_recipe()),
        "hornet" | "mirage" | "shiv" | "warden" | "wraith" => Some(particle_only_recipe(&codename)),
        "nurse" => Some(nurse_recipe()),
        "artist" => Some(artist_recipe()),
        "deadpack" => Some(deadpack_recipe()),
        "baba" => Some(baba_recipe()),
        "ratking" => Some(ratking_recipe()),
        "bebop" => Some(bebop_recipe()),
        _ => None,
    };
    recipe.map(|mut r| {
        r.shared_entries = SHARED_ENTRIES
            .iter()
            .find(|(c, _)| *c == r.codename)
            .map(|(_, entries)| entries.iter().map(|s| (*s).to_string()).collect())
            .unwrap_or_default();
        r
    })
}

/// Entries a hero reaches that another hero, an item or a neutral also renders,
/// so the bake copies and repoints them instead of editing in place (see
/// [`HeroRecolorRecipe::shared_entries`]). Generated by `vpkmerge recolor-qa
/// --suggest-shared`; regenerate after a game update.
const SHARED_ENTRIES: &[(&str, &[&str])] = &[
    (
        "bookworm",
        &[
            "particles/abilities/bookworm/bookworm_aoe_cylinder.vpcf_c",
            "particles/abilities/bookworm/bookworm_knockback.vpcf_c",
        ],
    ),
    (
        "inferno",
        &[
            "particles/abilities/inferno/inferno_afterburn_debuff",
            "particles/abilities/inferno/inferno_flame_dash_tag_enemy.vpcf_c",
            "particles/abilities/inferno/inferno_flame_dash_tag_enemy_embers.vpcf_c",
            "particles/abilities/inferno/inferno_flame_dash_tag_enemy_flash.vpcf_c",
            "particles/abilities/inferno/inferno_flame_dash_tag_enemy_slow_arrow.vpcf_c",
            "particles/abilities/inferno/inferno_flame_dash_tag_enemy_splash.vpcf_c",
            "particles/abilities/inferno/inferno_flame_dash_tag_enemy_trail.vpcf_c",
            "particles/abilities/inferno/inferno_flamedash_",
        ],
    ),
    (
        "yamato",
        &[
            "particles/abilities/yamato/yamato_nightmare_cast_blast.vpcf_c",
            "particles/abilities/yamato/yamato_nightmare_cast_blast_a.vpcf_c",
            "particles/abilities/yamato/yamato_nightmare_cast_blast_c.vpcf_c",
            "particles/abilities/yamato/yamato_nightmare_cast_blast_d.vpcf_c",
        ],
    ),
    (
        "chrono",
        &[
            "particles/abilities/chrono/chrono_time_bomb_projectile.vpcf_c",
            "particles/abilities/chrono/chrono_time_bomb_projectile_trail_",
        ],
    ),
    (
        "abrams",
        &[
            "particles/abilities/abrams/abrams_charge_wall_impact.vpcf_c",
            "particles/abilities/abrams/abrams_charge_wall_impact_directional_flash.vpcf_c",
            "particles/abilities/abrams/abrams_charge_wall_impact_dust_",
            "particles/abilities/abrams/abrams_charge_wall_impact_flash.vpcf_c",
            "particles/abilities/abrams/abrams_charge_wall_impact_sparks.vpcf_c",
            "particles/abilities/abrams/abrams_leap_immunity_",
            "particles/abilities/bull/bull_charge_edge.vpcf_c",
        ],
    ),
    (
        "archer",
        &[
            "particles/abilities/archer/archer_guided_arrow_owl_trail_sparkle.vpcf_c",
            "particles/abilities/archer/archer_guided_arrow_proj_trail.vpcf_c",
            "particles/abilities/archer/archer_guided_arrow_proj_trail_left.vpcf_c",
            "particles/abilities/archer/archer_guided_arrow_proj_trail_right.vpcf_c",
            "particles/abilities/archer/archer_immobilize_trap_preview.vpcf_c",
            "particles/abilities/archer/archer_immobilize_trap_preview_energy.vpcf_c",
            "particles/abilities/archer/archer_immobilize_trap_preview_model",
            "particles/abilities/archer/archer_power_jump_buff.vpcf_c",
            "particles/abilities/archer/archer_power_jump_buff_energy.vpcf_c",
            "particles/abilities/archer/archer_power_jump_buff_rings",
            "particles/abilities/archer/archer_power_jump_buff_trail.vpcf_c",
        ],
    ),
    (
        "astro",
        &[
            "particles/abilities/astro/astro_bounce_pad_destroy_dust.vpcf_c",
            "particles/abilities/astro/astro_lasso_chain",
        ],
    ),
    (
        "bebop",
        &["particles/abilities/bebop/bebop_uppercut_impact_sparks.vpcf_c"],
    ),
    (
        "drifter",
        &[
            "particles/abilities/drifter/drifter_claw_channel_lv",
            "particles/abilities/drifter/drifter_claw_impact_lv",
        ],
    ),
    (
        "dynamo",
        &[
            "particles/abilities/dynamo/dynamo_void_sphere_preview.vpcf_c",
            "particles/abilities/dynamo/dynamo_void_sphere_preview_beam.vpcf_c",
            "particles/abilities/dynamo/dynamo_void_sphere_preview_light.vpcf_c",
            "particles/abilities/dynamo/dynamo_void_sphere_preview_proj.vpcf_c",
            "particles/abilities/dynamo/dynamo_void_sphere_preview_sun.vpcf_c",
        ],
    ),
    (
        "familiar",
        &[
            "particles/abilities/familiar/familiar_naptime_explode",
            "particles/abilities/familiar/familiar_naptime_radius_indicator.vpcf_c",
            "particles/abilities/familiar/familiar_naptime_radius_indicator_beam.vpcf_c",
            "particles/abilities/familiar/familiar_naptime_radius_indicator_embers.vpcf_c",
        ],
    ),
    (
        "frank",
        &[
            "particles/abilities/frank/frank_jumpstart_buff.vpcf_c",
            "particles/abilities/frank/frank_jumpstart_buff_elec",
            "particles/abilities/frank/frank_jumpstart_buff_light.vpcf_c",
            "particles/abilities/frank/frank_jumpstart_buff_pnt.vpcf_c",
            "particles/abilities/frank/frank_jumpstart_buff_smoke.vpcf_c",
            "particles/abilities/frank/frank_jumpstart_buff_sparks.vpcf_c",
            "particles/abilities/frank/frank_jumpstart_buff_trail.vpcf_c",
            "particles/abilities/frank/frank_jumpstart_buff_wave.vpcf_c",
            "particles/abilities/frank/frank_jumpstart_buff_zap_",
        ],
    ),
    (
        "ghost",
        &[
            "particles/abilities/ghost/ghost_blood_bomb_explode",
            "particles/abilities/ghost/ghost_blood_bomb_projectile.vpcf_c",
            "particles/abilities/ghost/ghost_blood_bomb_projectile_glow.vpcf_c",
            "particles/abilities/ghost/ghost_blood_bomb_projectile_trail.vpcf_c",
            "particles/abilities/ghost/ghost_death_",
            "particles/abilities/ghost/ghost_life_drain_cast_preview_fire.vpcf_c",
            "particles/weapon_fx/ghost/ghost_",
        ],
    ),
    (
        "nano",
        &[
            "particles/abilities/nano/nano_cat_cast.vpcf_c",
            "particles/abilities/nano/nano_ultimate_hit_burst.vpcf_c",
            "particles/abilities/nano/nano_ultimate_hit_tgt.vpcf_c",
            "particles/abilities/nano/nano_ultimate_target_debuff.vpcf_c",
        ],
    ),
    (
        "haze",
        &[
            "particles/abilities/haze/haze_invis_smoke_bomb.vpcf_c",
            "particles/abilities/haze/haze_invis_smoke_bomb_flash.vpcf_c",
            "particles/abilities/haze/haze_invis_smoke_bomb_ring.vpcf_c",
            "particles/abilities/haze/haze_sleep_dagger_debuff.vpcf_c",
            "particles/abilities/haze/haze_sleep_dagger_debuff_spiral",
            "particles/abilities/haze/haze_sleep_dagger_debuff_z_motion.vpcf_c",
            "particles/weapon_fx/haze/haze_muzzle_",
            "particles/weapon_fx/haze/haze_tracer_impact.vpcf_c",
            "particles/weapon_fx/haze/haze_tracer_impact_debris.vpcf_c",
            "particles/weapon_fx/haze/haze_tracer_impact_detail.vpcf_c",
            "particles/weapon_fx/haze/haze_tracer_impact_dust.vpcf_c",
        ],
    ),
    (
        "lash",
        &["particles/abilities/lash/lash_flog_lifeleach_tgt_impact.vpcf_c"],
    ),
    (
        "mirage",
        &["particles/abilities/mirage/mirage_sand_phantom_deploy"],
    ),
    ("pocket", &["particles/abilities/pocket/pocket_damage_"]),
    (
        "priest",
        &[
            "particles/abilities/priest/priest_cone_aoe.vpcf_c",
            "particles/abilities/priest/priest_cone_aoe_embers.vpcf_c",
            "particles/abilities/priest/priest_cone_aoe_light",
            "particles/abilities/priest/priest_cone_aoe_model.vpcf_c",
            "particles/abilities/priest/priest_cone_aoe_muzzle",
            "particles/abilities/priest/priest_cone_aoe_projection.vpcf_c",
            "particles/abilities/priest/priest_cone_aoe_smoke.vpcf_c",
            "particles/abilities/priest/priest_cone_aoe_streaks.vpcf_c",
            "particles/abilities/priest/priest_cone_aoe_trail_embers.vpcf_c",
            "particles/abilities/priest/priest_snaptrap_root.vpcf_c",
            "particles/abilities/priest/priest_snaptrap_root_base_beams.vpcf_c",
            "particles/abilities/priest/priest_snaptrap_root_endcap.vpcf_c",
            "particles/abilities/priest/priest_snaptrap_root_ground_projected.vpcf_c",
        ],
    ),
    (
        "ratking",
        &[
            "particles/abilities/ratking/ratking_minion_ambient.vpcf_c",
            "particles/abilities/ratking/ratking_minion_ambient_emote.vpcf_c",
            "particles/abilities/ratking/ratking_minion_ambient_eyes",
            "particles/abilities/ratking/ratking_minion_ambient_trail.vpcf_c",
            "particles/abilities/ratking/ratking_nibble_debuff.vpcf_c",
            "particles/abilities/ratking/ratking_nibble_debuff_mark.vpcf_c",
            "particles/abilities/ratking/ratking_nibble_debuff_model.vpcf_c",
            "particles/abilities/ratking/ratking_nibble_debuff_model_ambient.vpcf_c",
            "particles/abilities/ratking/ratking_nibble_debuff_model_endcap_flash.vpcf_c",
            "particles/abilities/ratking/ratking_nibble_debuff_model_glow.vpcf_c",
        ],
    ),
    (
        "shiv",
        &[
            "particles/abilities/shiv/shiv_dash_dust.vpcf_c",
            "particles/abilities/shiv/shiv_dash_echo.vpcf_c",
            "particles/abilities/shiv/shiv_dash_pnt.vpcf_c",
            "particles/abilities/shiv/shiv_dash_screen_lines.vpcf_c",
            "particles/abilities/shiv/shiv_dash_start.vpcf_c",
            "particles/abilities/shiv/shiv_dash_tgt.vpcf_c",
            "particles/abilities/shiv/shiv_dash_tgt_icon.vpcf_c",
            "particles/abilities/shiv/shiv_dash_tgt_ring",
            "particles/abilities/shiv/shiv_dash_track_trail.vpcf_c",
            "particles/abilities/shiv/shiv_dash_trail",
            "particles/abilities/shiv/shiv_flash_",
            "particles/abilities/shiv/shiv_thrown_shiv.vpcf_c",
            "particles/abilities/shiv/shiv_thrown_shiv_endcap",
            "particles/abilities/shiv/shiv_thrown_shiv_impact",
            "particles/abilities/shiv/shiv_thrown_shiv_launch.vpcf_c",
            "particles/abilities/shiv/shiv_thrown_shiv_model.vpcf_c",
            "particles/abilities/shiv/shiv_thrown_shiv_trail",
        ],
    ),
    (
        "tengu",
        &[
            "particles/abilities/tengu/tengu_stone_form_ring.vpcf_c",
            "particles/abilities/tengu/tengu_stone_form_stomp.vpcf_c",
            "particles/abilities/tengu/tengu_stone_form_stomp_aoe.vpcf_c",
            "particles/abilities/tengu/tengu_stone_form_stomp_debris.vpcf_c",
            "particles/abilities/tengu/tengu_stone_form_stomp_dust_spear.vpcf_c",
            "particles/abilities/tengu/tengu_stone_form_stomp_hot_proj.vpcf_c",
            "particles/abilities/tengu/tengu_stone_form_stomp_light.vpcf_c",
            "particles/abilities/tengu/tengu_stone_form_stomp_model.vpcf_c",
            "particles/abilities/tengu/tengu_stone_form_stomp_proj.vpcf_c",
            "particles/abilities/tengu/tengu_stone_form_stomp_shake.vpcf_c",
            "particles/abilities/tengu/tengu_stone_form_stomp_smoke.vpcf_c",
            "particles/abilities/tengu/tengu_takeoff_",
        ],
    ),
    (
        "unicorn",
        &[
            "particles/abilities/unicorn/unicorn_prismatic_shield_explode_flares.vpcf_c",
            "particles/abilities/unicorn/unicorn_prismatic_shield_explode_light.vpcf_c",
            "particles/abilities/unicorn/unicorn_prismatic_shield_explode_shard",
            "particles/abilities/unicorn/unicorn_prismatic_shield_silence_",
        ],
    ),
    (
        "gigawatt",
        &[
            "particles/abilities/gigawatt/gigawatt_static_charge_explosion",
            "particles/abilities/gigawatt/gigawatt_voltage_drain_buff",
        ],
    ),
    (
        "vampirebat",
        &[
            "particles/abilities/vampirebat/vampirebat_lovebites_damage.vpcf_c",
            "particles/abilities/vampirebat/vampirebat_lovebites_damage_body",
            "particles/abilities/vampirebat/vampirebat_lovebites_damage_icon_top",
        ],
    ),
    (
        "warden",
        &[
            "particles/abilities/warden/warden_crowd_control_cast.vpcf_c",
            "particles/abilities/warden/warden_crowd_control_cast_darken_proj.vpcf_c",
            "particles/abilities/warden/warden_crowd_control_cast_embers.vpcf_c",
            "particles/abilities/warden/warden_crowd_control_cast_flash.vpcf_c",
            "particles/abilities/warden/warden_crowd_control_cast_light.vpcf_c",
            "particles/abilities/warden/warden_crowd_control_cast_shock",
            "particles/abilities/warden/warden_high_alert_buff_shield.vpcf_c",
        ],
    ),
    (
        "wraith",
        &["particles/abilities/wraith/wraith_rapid_fire_buff_spiral.vpcf_c"],
    ),
];

/// Hero codenames with built-in ability-VFX recolor recipes.
#[must_use]
pub const fn pinned_hero_codenames() -> &'static [&'static str] {
    &[
        "bookworm",
        "necro",
        "inferno",
        "yamato",
        "chrono",
        "abrams",
        "archer",
        "artist",
        "astro",
        "baba",
        "bebop",
        "chessmaster",
        "deadpack",
        "digger",
        "doorman",
        "drifter",
        "dynamo",
        "familiar",
        "fencer",
        "frank",
        "ghost",
        "nano",
        "haze",
        "hornet",
        "kelvin",
        "lash",
        "mcginnis",
        "magician",
        "mirage",
        "nurse",
        "pocket",
        "priest",
        "punkgoat",
        "ratking",
        "shiv",
        "tengu",
        "unicorn",
        "gigawatt",
        "vampirebat",
        "viper",
        "viscous",
        "warden",
        "werewolf",
        "wraith",
    ]
}

/// Paige (`bookworm`). Pinned from the in-game-verified purple recolor:
/// `pak02` (particles), `pak04` (the 9 color textures), and the ult vertex-color
/// addon (`models/particle/bookworm_horse_knight` + `bookworm_mace`). Source of
/// truth: `../grimoire/docs/ability-vfx-recolor.md` + `docs/handoff-vertex-color-recolor.md`.
fn paige_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "bookworm".to_string(),
        particle_prefixes: vec![
            "particles/abilities/bookworm/".to_string(),
            "particles/weapon_fx/bookworm/".to_string(),
        ],
        texture_entries: [
            // bullets (projectile self-illum)
            "materials/particle/abilities/bookworm/bookworm_projectile_self_illum_vmat_g_tcolor_7b26a19f.vtex_c",
            // AOE ground (hero-named in a shared dir)
            "materials/particle/projected/bookworm_aoe_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            // ground streak
            "materials/particle/ground/ground_streak_bookworm_psd_5a44028c.vtex_c",
            // model self-illum / albedo color maps
            "models/heroes_wip/bookworm/materials/bookworm_ui_effects_color_psd_a29be817.vtex_c",
            "models/heroes_wip/bookworm/materials/bookworm_shield_illustrated_color_psd_81f5497b.vtex_c",
            "models/heroes_wip/bookworm/materials/bookworm_sword_illustrated_color_psd_4eb22603.vtex_c",
            "models/heroes_wip/bookworm/materials/bookworm_stone_illustrated_color_psd_8ed29960.vtex_c",
            "models/heroes_wip/bookworm/materials/bookworm_dragon_color_tga_ed3d3b5.vtex_c",
            "models/heroes_wip/bookworm/materials/bookworm_dragon_vmat_g_tnprtransmissivecolor_70d941b.vtex_c",
            // dash butterflies + the two cast books
            "models/heroes_wip/bookworm/butterfly/bookworm_butterfly_color_psd_b4619bc6.vtex_c",
            "models/heroes_wip/bookworm/materials/bookworm_book_color_psd_bd83eb89.vtex_c",
            "models/heroes_wip/bookworm/materials/bookworm_book_color_dragon_psd_c3b5e08a.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        // The illustrated props and the dragon glow through green self-illum tints
        // over their (pinned) albedos; without the stamp they render muddy.
        material_entries: [
            "materials/particle/abilities/bookworm/bookworm_projectile_self_illum.vmat_c",
            "models/heroes_wip/bookworm/materials/bookworm_dragon.vmat_c",
            "models/heroes_wip/bookworm/materials/bookworm_shield.vmat_c",
            "models/heroes_wip/bookworm/materials/bookworm_stone.vmat_c",
            "models/heroes_wip/bookworm/materials/bookworm_sword.vmat_c",
            "models/heroes_wip/bookworm/materials/bookworm_sword_additive.vmat_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        model_entries: [
            // the ult body that actually renders (found via the ult model particle)
            "models/particle/bookworm_horse_knight.vmdl_c",
            // melee swing
            "models/particle/bookworm_mace.vmdl_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        shared_entries: Vec::new(),
        // The general ability effects color map: a large, clearly-chromatic
        // texture that reads as Paige's glow, so the UI swatch is representative.
        preview_texture: Some(
            "models/heroes_wip/bookworm/materials/bookworm_ui_effects_color_psd_a29be817.vtex_c"
                .to_string(),
        ),
    }
}

/// Particle-only recipe: just the two `particles/{abilities,weapon_fx}/<codename>/`
/// prefixes, no color textures or vertex-color models. The shape shared by every
/// hero whose ability VFX carry color purely through `.vpcf_c` color params, so one
/// `--hue` lands the whole ability/weapon set and touches nothing on the skin. The
/// per-hero provenance (source mod + scanned hue) is tabulated at [`recipe_for`].
fn particle_only_recipe(codename: &str) -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: codename.to_string(),
        particle_prefixes: vec![
            format!("particles/abilities/{codename}/"),
            format!("particles/weapon_fx/{codename}/"),
        ],
        texture_entries: Vec::new(),
        material_entries: Vec::new(),
        model_entries: Vec::new(),
        // Particle-only: no color texture to render as a swatch.
        shared_entries: Vec::new(),
        preview_texture: None,
    }
}

/// The standard two particle roots plus pinned color textures and tint
/// materials.
fn standard_recipe(
    codename: &str,
    texture_entries: &[&str],
    material_entries: &[&str],
    preview_texture: Option<&str>,
) -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: codename.to_string(),
        particle_prefixes: vec![
            format!("particles/abilities/{codename}/"),
            format!("particles/weapon_fx/{codename}/"),
        ],
        texture_entries: texture_entries.iter().map(|s| (*s).to_string()).collect(),
        material_entries: material_entries.iter().map(|s| (*s).to_string()).collect(),
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: preview_texture.map(str::to_string),
    }
}

/// Infernus (`inferno`): particle color params plus the two additive glow layers
/// of his live body model, `inferno_headglow` (the flame hair) and
/// `inferno_armglow`. Their orange crust albedos and red NPR transmissive maps are
/// hero-only and recolored in place, and the self-illum tints are stamped, so the
/// tint multiplies a recolored albedo of the same hue instead of the vanilla
/// orange. `inferno_body` stays vanilla (its tint does not color his fire). The
/// fire ramp / burning textures the particles sample are shared game-wide, so the
/// hue comes from the particle params alone.
fn inferno_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "inferno".to_string(),
        particle_prefixes: vec![
            "particles/abilities/inferno/".to_string(),
            "particles/weapon_fx/inferno/".to_string(),
            // The napalm debuff overlay. Its flame-dash sibling stays out: neutrals
            // and the boss spawn that one too.
            "particles/status_fx/status_fx_inferno_napalm_debuff.vpcf_c".to_string(),
        ],
        texture_entries: [
            "models/heroes_staging/inferno_v4/materials/inferno_glow_png_c2e03ac5.vtex_c",
            "models/heroes_staging/inferno_v4/materials/inferno_glow_psd_3c639d14.vtex_c",
            "models/heroes_staging/inferno_v4/materials/inferno_armglow_vmat_g_tnprtransmissivecolor_df833b87.vtex_c",
            "models/heroes_staging/inferno_v4/materials/inferno_headglow_vmat_g_tnprtransmissivecolor_783cf05a.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: [
            "models/heroes_staging/inferno_v4/materials/inferno_armglow.vmat_c",
            "models/heroes_staging/inferno_v4/materials/inferno_headglow.vmat_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "models/heroes_staging/inferno_v4/materials/inferno_glow_psd_3c639d14.vtex_c"
                .to_string(),
        ),
    }
}

/// Abrams (`abrams`): particles plus two hero-specific projected self-illum
/// textures found by `examples/recolor_assets.rs`. The rest of the referenced
/// ability textures are masks, normals, AO, or shared defaults.
///
/// Valve is migrating his asset basename to `bull` (his `hero_atlas` record
/// points card art at `bull_card` and his kit's charge/leap/passive particles
/// plus all weapon FX now live under the `bull` dirs; `weapon_fx/abrams/` is
/// empty as of the 2026-06-11 update). Cover both namespaces until the
/// `abilities/abrams/` tree is gone too.
fn abrams_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "abrams".to_string(),
        particle_prefixes: vec![
            "particles/abilities/abrams/".to_string(),
            "particles/abilities/bull/".to_string(),
            "particles/weapon_fx/bull/".to_string(),
        ],
        texture_entries: [
            "materials/particle/abilities/abrams/abrams_leap_ground_impact_hot_symbol_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/abrams_siphon_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: Vec::new(),
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "materials/particle/abilities/abrams/abrams_leap_ground_impact_hot_symbol_projected_vmat_g_tselfillum_670d93d.vtex_c"
                .to_string(),
        ),
    }
}

/// Apollo (`fencer`): particles plus hero-specific projected sigils, the ult
/// gradient color map, and the sword dissolve color map. These were isolated by
/// the texture audit; shared defaults and data maps are deliberately excluded.
fn fencer_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "fencer".to_string(),
        particle_prefixes: vec![
            "particles/abilities/fencer/".to_string(),
            "particles/weapon_fx/fencer/".to_string(),
        ],
        texture_entries: [
            "materials/particle/projected/fencer_preview_line_projected_decal_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/fencer_sigil_pentagram_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/abilities/fencer/fencer_ult_gradient_color_psd_51322651.vtex_c",
            "models/heroes_wip/fencer/materials/fencer_sword_color_tga_52ec8bfe.vtex_c",
            // red gradient albedos of the ult mask FX and the reload sword materialize
            "models/abilities/materials/fencer_mask_color_psd_466e30e0.vtex_c",
            "models/abilities/materials/fencer_mask_color_psd_ebfbc521.vtex_c",
            "models/heroes_wip/fencer/materials/fencer_sword_color_tga_70290e99.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: vec![
            "materials/particle/abilities/fencer/fencer_mask_dissolve.vmat_c".to_string(),
            "models/heroes_wip/fencer/materials/fencer_sword_dissolve_emissive.vmat_c".to_string(),
        ],
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "materials/particle/abilities/fencer/fencer_ult_gradient_color_psd_51322651.vtex_c"
                .to_string(),
        ),
    }
}

/// Lady Geist (`ghost`): particles plus the two hero-specific chromatic clothes
/// FX maps used by ability particles. The audit also found a shared Shiv ability
/// detail texture; it is intentionally left alone to avoid cross-hero bleed.
fn ghost_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "ghost".to_string(),
        particle_prefixes: vec![
            "particles/abilities/ghost/".to_string(),
            "particles/weapon_fx/ghost/".to_string(),
        ],
        texture_entries: [
            "models/heroes_staging/ghost/materials/ghost2_clothes_fx_prop_color_psd_b398de35.vtex_c",
            "models/heroes_staging/ghost/materials/ghost2_clothes_color_png_fc80b39a.vtex_c",
            "materials/particle/abilities/ghost/ghost_health_swap_precast_proj_vmat_g_tselfillum_670d93d.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: Vec::new(),
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "models/heroes_staging/ghost/materials/ghost2_clothes_fx_prop_color_psd_b398de35.vtex_c"
                .to_string(),
        ),
    }
}

/// Calico (`nano`): particles plus the hero-specific ult ground projection and
/// cat statue color map. Shared Operative/noise textures are excluded even though
/// they are chromatic.
fn nano_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "nano".to_string(),
        particle_prefixes: vec![
            "particles/abilities/nano/".to_string(),
            "particles/weapon_fx/nano/".to_string(),
        ],
        texture_entries: [
            "materials/particle/abilities/nano/nano_ult_ground_dark_proj_vmat_g_tselfillum_670d93d.vtex_c",
            "models/heroes_staging/nano/cat_statue/materials/cat_statue_color_png_8892a790.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: Vec::new(),
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "materials/particle/abilities/nano/nano_ult_ground_dark_proj_vmat_g_tselfillum_670d93d.vtex_c"
                .to_string(),
        ),
    }
}

/// Lash (`lash`): particles plus the hero-specific cable material color texture.
/// The shatter/crack ground texture referenced by some particles is shared and is
/// intentionally not recolored in place.
fn lash_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "lash".to_string(),
        particle_prefixes: vec![
            "particles/abilities/lash/".to_string(),
            "particles/weapon_fx/lash/".to_string(),
        ],
        texture_entries: [
            "materials/particle/cables/lash_cable_material_vmat_g_tcolor_8ca8af3e.vtex_c",
            // Ground Strike's blue cone streak (a black-margin sprite sheet)
            "materials/particle/abilities/lash/lash_ground_strike_scroll.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: Vec::new(),
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "materials/particle/cables/lash_cable_material_vmat_g_tcolor_8ca8af3e.vtex_c"
                .to_string(),
        ),
    }
}

/// `McGinnis` (`mcginnis`): particles plus the hero-specific turret goo color
/// textures found in the local `.disabled` scan. Shared/default textures are
/// deliberately excluded.
fn mcginnis_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "mcginnis".to_string(),
        particle_prefixes: vec![
            "particles/abilities/mcginnis/".to_string(),
            "particles/weapon_fx/mcginnis/".to_string(),
            // reload FX spawned by her reload clips, under the `forge` codename
            "particles/weapon_fx/forge/forge_reload_handle_charge.vpcf_c".to_string(),
            "particles/weapon_fx/forge/forge_reload_sparks.vpcf_c".to_string(),
        ],
        texture_entries: [
            "materials/particle/abilities/mcginnis/mcginnis_turret_ambient_goo_vmat_g_tcolor_974c5f09.vtex_c",
            "materials/particle/abilities/mcginnis/mcginnis_turret_ambient_goo_vmat_g_tsheen_7edd324d.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: Vec::new(),
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "materials/particle/abilities/mcginnis/mcginnis_turret_ambient_goo_vmat_g_tcolor_974c5f09.vtex_c"
                .to_string(),
        ),
    }
}

/// Sinclair (`magician`): particles plus two hero-specific chromatic ability
/// textures from the local `.disabled` scan and texture audit.
fn magician_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "magician".to_string(),
        particle_prefixes: vec![
            "particles/abilities/magician/".to_string(),
            "particles/weapon_fx/magician/".to_string(),
        ],
        texture_entries: [
            "materials/particle/projected/magician_hex_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/abilities/magician/magician_bolt_vmat_g_tcolor_978bc798.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        // the bolt model's lavender tints and the Spectral Assistant turret's purple
        material_entries: vec![
            "materials/particle/abilities/magician/magician_bolt.vmat_c".to_string(),
            "models/heroes_staging/magician/materials/magician_spectral_assistant.vmat_c".to_string(),
        ],
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "materials/particle/projected/magician_hex_ground_projected_vmat_g_tselfillum_670d93d.vtex_c"
                .to_string(),
        ),
    }
}

/// Pocket (`pocket`): particles plus the barrage tracer glow (albedo + tint
/// material), the Affliction `AoE` frog prop and the slime ground decal. The frog's
/// model particle is color-free, so its albedo and transmissive maps are the only
/// axis that reaches it. Left out: the satchel ring decal (neutral telegraphs draw
/// the same material) and the suitcase / ethereal-body / deployable albedos
/// (achromatic or never rendered in the 2026-10 build).
fn pocket_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "pocket".to_string(),
        particle_prefixes: vec![
            "particles/abilities/pocket/".to_string(),
            "particles/weapon_fx/pocket/".to_string(),
            // The satchel's hit children still live in the legacy synth dir.
            "particles/abilities/synth/synth_pulse_hit_beam.vpcf_c".to_string(),
            "particles/abilities/synth/synth_pulse_hit_tgt.vpcf_c".to_string(),
        ],
        texture_entries: [
            "materials/particle/abilities/pocket/pocket_magic_missile_illum_vmat_g_tcolor_754e94bd.vtex_c",
            // The Affliction AoE frog prop: albedo + flat yellow transmissive.
            "models/abilities/materials/pocket_frog_small_vmat_g_tcolor_be6f0aff.vtex_c",
            "models/abilities/materials/pocket_frog_small_vmat_g_tnprtransmissivecolor_4d5e61a7.vtex_c",
            // (the satchel ring decal is also drawn by neutral telegraphs)
            "materials/particle/projected/pocket_slime_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: vec![
            "materials/particle/abilities/pocket/pocket_magic_missile_illum.vmat_c".to_string(),
            "materials/models/particle/pocket_affliction_erosion.vmat_c".to_string(),
        ],
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "materials/particle/abilities/pocket/pocket_magic_missile_illum_vmat_g_tcolor_754e94bd.vtex_c"
                .to_string(),
        ),
    }
}

/// Celeste (`unicorn`): particles plus the color-bearing projected ground masks.
/// Her ability-3 ground decal uses `unicorn_radiant_flare_*_projected` materials;
/// particle tint alone makes those read as flat filled circles, so animated prism
/// uses a projected-texture rainbow over the authored falloff.
fn unicorn_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "unicorn",
        &[
            "materials/particle/abilities/unicorn/unicorn_prismatic_shield_ground_warning_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/unicorn_flux_rainbow_ground_projected_light_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/unicorn_radiant_flare_ground_advance_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/unicorn_radiant_flare_ground_preview_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/unicorn_radiant_flare_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "models/abilities/materials/unicorn_star_dagger_color_png_e159bb53.vtex_c",
            "models/abilities/materials/unicorn_star_dagger_vmat_g_tnprtransmissivecolor_6f45577b.vtex_c",
        ],
        &[],
        Some(
            "materials/particle/projected/unicorn_radiant_flare_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ),
    )
}

/// Grey Talon (`archer`): particles plus the charged-shot gradient texture.
fn archer_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "archer",
        &[
            "materials/particle/abilities/archer/archer_charged_shot_gradient_color_v2_psd_51e62704.vtex_c",
            "materials/particle/abilities/archer/archer_charged_shot_gradient_color_psd_17c02a47.vtex_c",
            "models/heroes_staging/archer/materials/archer_guided_arrow_color_psd_edd3d0f5.vtex_c",
            "materials/particle/abilities/archer/archer_dreamcatcher_pattern_psd_68e6811a.vtex_c",
        ],
        &[
            "materials/models/particle/archer_immobilize_trap_aura.vmat_c",
        ],
        Some(
            "materials/particle/abilities/archer/archer_charged_shot_gradient_color_v2_psd_51e62704.vtex_c",
        ),
    )
}

/// Bebop: mostly particle-driven. Hyper Beam's projected ground/surface decals
/// use bright round masks tinted by the particle color, so prism leaves those
/// decal particles vanilla while recoloring the beam, sparks, and perimeter.
fn bebop_recipe() -> HeroRecolorRecipe {
    // The hook fist and sticky bomb shells glow through static self-illum tints;
    // the bomb shell's 1x1 red albedo has to move with its tint.
    standard_recipe(
        "bebop",
        &["models/heroes_staging/bebop/materials/bebop_sticky_bomb_glow_vmat_g_tcolor_c2575562.vtex_c"],
        &[
            "models/heroes_staging/bebop/materials/bebop_hand_projectile.vmat_c",
            "models/heroes_staging/bebop/materials/bebop_sticky_bomb_glow.vmat_c",
        ],
        None,
    )
}

/// Solomon (`chessmaster`): teal and gold, mostly particle color. The dark-piece
/// cooldown status sits under `status_fx` with a flat tint texture of its own. The
/// projected ground sigils keep a flat green `g_tSelfIllum` placeholder, the teal
/// shell and queen stomp sphere and the gold piece trim are static self-illum
/// tints, and the spectral hand and knight buff shield glow through baked vertex
/// color. The idle ambient (floating pieces and ring, spawned by the body model)
/// rides the dir prefix and recolors too. The pieces' stone albedos and NPR
/// transmissive constants, the body and the borrowed assets (Ivy's stomp, the
/// shared `deployment_preview`, Silver's boot kick bubble) stay vanilla.
fn chessmaster_recipe() -> HeroRecolorRecipe {
    let mut r = standard_recipe(
        "chessmaster",
        &[
            "materials/particle/abilities/chessmaster/chessmaster_move_piece_cooldown_color_tint.vtex_c",
            "materials/particle/abilities/chessmaster/chessmaster_queen_cast_color_tint.vtex_c",
            "materials/particle/abilities/chessmaster/chessmaster_queen_warning_cast_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/chessmaster_bishop_aura_ground_square_dark_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/chessmaster_queen_stomp_ground_bright_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/chessmaster_queen_stomp_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/chessmaster_queen_stomp_post_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/chessmaster_square_board_edge_psd_853ade7.vtex_c",
            "models/heroes_wip/chessmaster/materials/chessmaster_rookglow_vmat_g_tnprtransmissivecolor_c88597d0.vtex_c",
        ],
        &[
            "models/heroes_wip/chessmaster/materials/chessmaster_bishop.vmat_c",
            "models/heroes_wip/chessmaster/materials/chessmaster_chesspieces_fx.vmat_c",
            "models/heroes_wip/chessmaster/materials/chessmaster_knight.vmat_c",
            "models/heroes_wip/chessmaster/materials/chessmaster_queen.vmat_c",
            "models/heroes_wip/chessmaster/materials/chessmaster_rook.vmat_c",
            "models/heroes_wip/chessmaster/materials/chessmaster_rookglow.vmat_c",
            "models/particle/materials/chessmaster_queen_stomp_impact_sphere.vmat_c",
        ],
        Some(
            "materials/particle/projected/chessmaster_queen_stomp_ground_bright_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ),
    );
    r.particle_prefixes
        .push("particles/status_fx/status_fx_chessmaster".to_string());
    r.model_entries = [
        "models/heroes_wip/chessmaster/chessmaster_hand.vmdl_c",
        "models/heroes_wip/chessmaster/chessmaster_knightshield.vmdl_c",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    r
}

/// Holliday (`astro`): particles plus the Powder Keg barrel's rune glow, a static
/// self-illum tint (the barrel's wood albedo is shared with her body).
fn astro_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "astro",
        &[],
        &["models/abilities/materials/astro_barrelv2.vmat_c"],
        None,
    )
}

/// Rat King (`ratking`): his ability and weapon colors are particle params (warm
/// red and orange constants), so the two particle roots carry most of the set.
/// Outside them: the green self-illum ring of the ult-aura and grenade-blast ground
/// decals, the pale sludge albedo of the planted flag's enemy-aura decal, the crown
/// over buffed allies, and the glowing sewage of the planted diorama. His body, rat
/// minions and props keep their surface colors; the ult anticipation circle is Lady
/// Geist's blood bomb decal, so it stays vanilla.
fn ratking_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "ratking",
        &[
            "materials/particle/abilities/ratking/ratking_standard_bearer_aura_caster_ground_projected_bright_psd_e4a346cc.vtex_c",
            "materials/particle/abilities/ratking/ratking_standard_bearer_exploded_ground_color_psd_f71dffe9.vtex_c",
            "materials/particle/abilities/ratking/ratking_scrap_grenade_warning_ground_projected_light_vmat_g_tselfillum_670d93d.vtex_c",
            "models/heroes_wip/ratking/materials/ratking_crown_fx_vmat_g_tcolor_518041ec.vtex_c",
            "models/heroes_wip/ratking/materials/ratking_crown_fx_vmat_g_tnprtransmissivecolor_ee835aa2.vtex_c",
            "models/heroes_wip/ratking/materials/ratking_ult_diorama_sewage_vmat_g_tnprtransmissivecolor_1fff71fb.vtex_c",
        ],
        &[
            "models/heroes_wip/ratking/materials/ratking_crown_fx.vmat_c",
            "models/heroes_wip/ratking/materials/ratking_ult_diorama_sewage.vmat_c",
        ],
        Some(
            "materials/particle/abilities/ratking/ratking_standard_bearer_aura_caster_ground_projected_bright_psd_e4a346cc.vtex_c",
        ),
    )
}

/// Baba (`baba`): her brew effects are particle-driven (the materials follow
/// `$COLORTINT` / `$SELFILLUMTINT`), but two projected ground decals keep a green
/// self-illum map under the particle tint (Hexing Brew's explosion ring, Bench
/// Run's slam) and the Pigeon Assault pigeon is a purple additive model whose
/// only color axis is its albedo. Left out: the lens glow and the body's
/// self-illum gems (skin), the bench's bead glow (no live setter for
/// `$baba_beads_self_illum`) and the arm effect (its particle is dead).
fn baba_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "baba",
        &[
            "materials/particle/abilities/baba/baba_hexing_brew_explosion_ground_bright_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/abilities/baba/baba_bench_slam_impact_ground_bright_projected_psd_ead108de.vtex_c",
            "models/heroes_wip/baba/materials/pigeon_effect_color_tga_f5ac1d89.vtex_c",
        ],
        &[],
        Some("models/heroes_wip/baba/materials/pigeon_effect_color_tga_f5ac1d89.vtex_c"),
    )
}

/// Violet (`artist`): her paint is particle-driven, plus the brush stroke and
/// paint globule props (colored albedos, transmissive maps and self-illum tints)
/// and the green self-illum maps of her splat and cone-fan ground decals. The
/// rolled painting prop keeps its artwork.
fn artist_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "artist",
        &[
            "materials/particle/abilities/artist/artist_splat_ground_trans_02_psd_c97b253f.vtex_c",
            "materials/particle/projected/artist_cone_fan_ground_projected_helper_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/artist_cone_fan_ground_projected_motion_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/artist_cone_fan_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "models/heroes_wip/artist/materials/artist_ability1_brush_color_png_19e5c1f2.vtex_c",
            "models/heroes_wip/artist/materials/artist_ability1_brush_color_png_6ca5701c.vtex_c",
            "models/heroes_wip/artist/materials/artist_ability1_brush_trans_png_f2abaf3f.vtex_c",
            "models/heroes_wip/artist/materials/artist_ability_brush_color_png_2ac14041.vtex_c",
            "models/heroes_wip/artist/materials/artist_ability_brush_color_png_b7e770ed.vtex_c",
            "models/heroes_wip/artist/materials/artist_ability_brush_trans_png_e2b43a5f.vtex_c",
            "models/heroes_wip/artist/materials/artist_globule_color_png_299b0d1.vtex_c",
            "models/heroes_wip/artist/materials/artist_globule_color_png_ae3f23c4.vtex_c",
            "models/heroes_wip/artist/materials/artist_globule_vmat_g_tnprtransmissivecolor_fdf22e73.vtex_c",
        ],
        &[
            "models/heroes_wip/artist/materials/artist_ability1_brush.vmat_c",
            "models/heroes_wip/artist/materials/artist_ability1_brush_outline.vmat_c",
            "models/heroes_wip/artist/materials/artist_ability_brush.vmat_c",
            "models/heroes_wip/artist/materials/artist_ability_brush_outline.vmat_c",
        ],
        Some("models/heroes_wip/artist/materials/artist_globule_color_png_299b0d1.vtex_c"),
    )
}

/// Nurse Harrow (`nurse`): particles plus the ampoule glass, the Adrenaline Rush
/// vapors trails and the vapors / crash status icons over a target (colored
/// albedos and transmissive maps, and baked vertex color on the vapors icon).
/// One hue merges the teal vapors and orange crash icons, as it does their
/// particles. The gun liquid, leech jar and eye glows on her body stay vanilla.
fn nurse_recipe() -> HeroRecolorRecipe {
    let mut r = standard_recipe(
        "nurse",
        &[
            "models/heroes_wip/nurse/materials/nurse_ampoule_glass_vmat_g_tnprtransmissivecolor_a78c804d.vtex_c",
            "models/heroes_wip/nurse/materials/nurse_pick_screen_effect_trail_trans_psd_67ac9064.vtex_c",
            "models/heroes_wip/nurse/materials/nurse_pick_screen_effect_trail_trans_psd_693b69bb.vtex_c",
            "models/heroes_wip/nurse/materials/nurse_vapors_crash_icon_color_psd_173a92c0.vtex_c",
            "models/heroes_wip/nurse/materials/nurse_vapors_crash_icon_color_psd_cb3867c9.vtex_c",
            "models/heroes_wip/nurse/materials/nurse_vapors_icon_color_tga_2d8423b9.vtex_c",
            "models/heroes_wip/nurse/materials/nurse_vapors_icon_color_tga_a2d206eb.vtex_c",
        ],
        &[
            "models/heroes_wip/nurse/materials/nurse_ampoule_glass.vmat_c",
            "models/heroes_wip/nurse/materials/nurse_vapors_crash_trails.vmat_c",
            "models/heroes_wip/nurse/materials/nurse_vapors_trails.vmat_c",
        ],
        Some("models/heroes_wip/nurse/materials/nurse_vapors_icon_color_tga_a2d206eb.vtex_c"),
    );
    r.model_entries = vec!["models/heroes_wip/nurse/nurse_vapors_icon.vmdl_c".to_string()];
    r
}

/// Deadman Danny (`deadpack`): his kit kept its development name, so nearly
/// every particle lives under `airheart/`; his own dir only holds the VTOL
/// ground preview. Outside the particles: the lava erode prop, the green
/// self-illum maps of the mark, crater and magma pool decals, and the missile
/// and fuel globule glow tints. The missile and weapon paint stay vanilla, as do
/// the fuel, lens and head accents on his body.
fn deadpack_recipe() -> HeroRecolorRecipe {
    let mut r = standard_recipe(
        "deadpack",
        &[
            "materials/particle/abilities/airheart/airheart_chimera_mark_ground_psd_3abc7a7.vtex_c",
            "materials/particle/abilities/airheart/airheart_crater_projected_v1_trans_psd_e32a6d4f.vtex_c",
            "materials/particle/abilities/airheart/airheart_magma_pool_self_illum_psd_c5cab65a.vtex_c",
            "models/abilities/materials/airheart_lava_erode_vmat_g_tcolor_693a7ca5.vtex_c",
            "models/abilities/materials/airheart_lava_erode_vmat_g_tnprtransmissivecolor_3bdbb943.vtex_c",
        ],
        &[
            "models/abilities/materials/airheart_lava_erode.vmat_c",
            "models/abilities/materials/deadpack_globules.vmat_c",
            "models/heroes_wip/deadpack/materials/deadpack_missile.vmat_c",
        ],
        Some("materials/particle/abilities/airheart/airheart_magma_pool_self_illum_psd_c5cab65a.vtex_c"),
    );
    r.particle_prefixes = vec![
        "particles/abilities/deadpack/".to_string(),
        "particles/abilities/airheart/".to_string(),
        "particles/weapon_fx/airheart/".to_string(),
        "particles/heroes/airheart/".to_string(),
    ];
    r
}

/// Seven (`gigawatt`): Power Surge's chain jump and the Storm Cloud status sit
/// outside his particle dirs.
fn gigawatt_recipe() -> HeroRecolorRecipe {
    let mut r = particle_only_recipe("gigawatt");
    r.particle_prefixes.extend([
        "particles/abilities/gigawatt_chain_lightning".to_string(),
        "particles/status_fx/gigawatt_status_storm_cloud".to_string(),
    ]);
    r
}

/// Billy (`punkgoat`): the tether chain, sigil nails and the profanity text over a
/// tethered enemy take their color from material tints, not particles.
fn punkgoat_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "punkgoat",
        &[
            "models/heroes_wip/punkgoat/materials/punkgoat_chain_vmat_g_tcolor_a9fad00e.vtex_c",
            "models/heroes_wip/punkgoat/materials/anarchylogo_emissive_psd_793f527d.vtex_c",
        ],
        &[
            "models/heroes_wip/punkgoat/materials/punkgoat_chain.vmat_c",
            "models/heroes_wip/punkgoat/materials/punkgoat_profanity.vmat_c",
        ],
        None,
    )
}

/// Mina (`vampirebat`): the blink / bat swarm bat prop is red through its
/// material tints and a flat red albedo.
fn vampirebat_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "vampirebat",
        &[
            "models/particle/materials/vampirebat_bat_vmat_g_tcolor_a29884ae.vtex_c",
            "materials/particle/projected/vampirebat_blink_preview_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/vampirebat_steallife_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ],
        &[
            "models/particle/materials/vampirebat_bat.vmat_c",
        ],
        None,
    )
}

/// Mo & Krill (`digger`): the burrow explosion ground sigil. The channel sigil's
/// self-illum scale is a static 0, so its placeholder map never renders.
fn digger_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "digger",
        &[
            "materials/particle/abilities/digger/digger_burrow_explode_ground_dark_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ],
        &[],
        Some(
            "materials/particle/abilities/digger/digger_burrow_explode_ground_dark_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ),
    )
}

/// The Doorman: most of his orange is NOT in particles or textures. The doorway
/// portals, luggage cart, bell, ult key and the key glow on his body are additive
/// self-illum materials whose color is a `g_vColorTint1` / `g_vSelfIllumTint1`
/// constant over grayscale gradients, so they are stamped as material entries.
/// The bell's flat orange `g_tColor` is the one prop texture that carries chroma.
///
/// The exit door (`door_end` material group: `doorman_door_portal_blue`) and the
/// allied cart (`doorman_luggagetrolley_friendly`) are blue in vanilla; they are
/// stamped too because the particle pass already flattens their `_enddoor` /
/// `_allied` particles onto the same hue. The placement previews
/// (`doorman_door_preview{,_bad}`: valid blue / invalid red) stay vanilla so a red
/// recolor can't read as "can't place here". The grenade debuff ground decal is
/// left out too: Victor's revive and an item draw the same material. The orange (enemy-side) cart
/// material `doorman_luggagetrolley` is left out because Solomon's
/// `chessmaster_rook_buff_shield_model` renders it too.
/// `doorman_hotel_victim_debuff_keyhole` looks magenta but is sampled as
/// `MIX_BALPHA` (blue channel = alpha): data, not color.
fn doorman_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "doorman".to_string(),
        particle_prefixes: vec![
            "particles/abilities/doorman/".to_string(),
            "particles/weapon_fx/doorman/".to_string(),
            "particles/status_fx/status_fx_doorman".to_string(),
        ],
        texture_entries: [
            "models/heroes_wip/doorman/materials/doorman_bell_vmat_g_tcolor_f7d92786.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: [
            "models/heroes_wip/doorman/materials/doorman_door_portal.vmat_c",
            "models/heroes_wip/doorman/materials/doorman_door_portal_blue.vmat_c",
            "models/heroes_wip/doorman/materials/doorman_gundoor_portal.vmat_c",
            "models/heroes_wip/doorman/materials/doorman_luggagetrolley_friendly.vmat_c",
            "models/heroes_wip/doorman/materials/doorman_bell.vmat_c",
            "models/heroes_wip/doorman/materials/doorman_ult_key.vmat_c",
            "models/heroes_wip/doorman_v2/materials/doorman_keyglow.vmat_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "models/heroes_wip/doorman/materials/doorman_bell_vmat_g_tcolor_f7d92786.vtex_c"
                .to_string(),
        ),
    }
}

fn drifter_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "drifter",
        &[
            "materials/particle/projected/drifter_claw_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/abilities/drifter/drifter_stalker_victim_screen_eyes.vtex_c",
        ],
        &[],
        Some(
            "materials/particle/projected/drifter_claw_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ),
    )
}

fn dynamo_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "dynamo".to_string(),
        particle_prefixes: vec![
            "particles/abilities/dynamo/".to_string(),
            "particles/weapon_fx/dynamo/".to_string(),
            "particles/status_fx/status_fx_dynamo".to_string(),
            // His Quantum Entanglement / Singularity targeting ring, still under the
            // legacy `sumo` name. (`particles/dynamo/` is Bebop's sticky-bomb buff.)
            "particles/abilities/sumo/sumo_singularity_preview_tgt".to_string(),
            // His stomp, beside the ability dirs under the old `sumo` name.
            "particles/abilities/sumo_stomp".to_string(),
        ],
        // The void sphere symbols are grayscale (particle-tinted) and the sphere /
        // heal-buff model tints follow particle expressions. The Singularity ripple
        // and the tracer rocket still use `sumo` assets.
        texture_entries: [
            "materials/particle/abilities/sumo_singularity_ripple_vmat_g_tcolor_9f33a436.vtex_c",
            "models/particle/materials/sumo_bullet_gradient_color_psd_c52e3bfb.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: [
            "materials/particle/abilities/sumo_singularity_ripple.vmat_c",
            "materials/particle/abilities/sumo_singularity_ripple_boundary.vmat_c",
            "models/particle/materials/sumo_rocket.vmat_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "materials/particle/abilities/sumo_singularity_ripple_vmat_g_tcolor_9f33a436.vtex_c"
                .to_string(),
        ),
    }
}

fn familiar_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "familiar",
        &[
            "materials/particle/abilities/familiar/familiar_naptime_coneradius_intersection_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/abilities/familiar/familiar_pillow_explode_ground_bright_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/abilities/familiar/familiar_spotlight_ground_edge_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/abilities/familiar/familiar_spotlight_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ],
        &[],
        Some(
            "materials/particle/abilities/familiar/familiar_spotlight_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ),
    )
}

/// Victor (`frank`): high-saturation ground projections from the ability audit,
/// plus the Aura of Suffering membrane color map (`painaura_model_fade_color`,
/// the chromatic layer of the green aura). The aura's membrane glow is the
/// particle COLOR fed into the model material's `g_vSelfIllumTint1 = $ILLUMCOLOR`
/// (renderer `m_MaterialVars` maps it to particle field 17), and the sphere /
/// ground rings are likewise particle-driven (`g_vColorTint1 = $COLORTINT`), so
/// the particle prefix pass recolors the whole aura. The 2026-06-25 game update
/// reworked these `frank_painaura_aura_*` files; the recipe still matches them by
/// prefix, but any addon baked before that update must be re-baked to pick up the
/// new/changed files. No material entry is pinned: `frank_painaura_sphere.vmat_c`
/// carries no static `g_vColorTint` constant (its tint is the `$COLORTINT`
/// expression the particles drive), so a material patch is a no-op there.
fn frank_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "frank".to_string(),
        particle_prefixes: vec![
            "particles/abilities/frank/".to_string(),
            "particles/weapon_fx/frank/".to_string(),
        ],
        // (frank_painaura_aoe_ground_projected is also Celeste's dagger target decal.)
        texture_entries: [
            "materials/particle/abilities/frank/frank_painaura_model_fade_color_psd_807c6243.vtex_c",
            "materials/particle/abilities/frank/frank_revive_marker_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/frank_shock_miss_projected_bright_vmat_g_tselfillum_670d93d.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: Vec::new(),
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "materials/particle/abilities/frank/frank_painaura_model_fade_color_psd_807c6243.vtex_c"
                .to_string(),
        ),
    }
}

fn haze_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "haze",
        &[
            "materials/particle/abilities/haze/haze_tracer_self_illum_vmat_g_tcolor_52a5b2da.vtex_c",
        ],
        &[
            "materials/particle/abilities/haze/haze_tracer_self_illum.vmat_c",
        ],
        Some(
            "materials/particle/abilities/haze/haze_tracer_self_illum_vmat_g_tcolor_52a5b2da.vtex_c",
        ),
    )
}

/// Kelvin: dome projections plus the ice-dome model color map. The generic
/// `ice_surface` texture stays excluded because it is shared broadly.
fn kelvin_recipe() -> HeroRecolorRecipe {
    // `kelvin_ice_dome_projected_psd_d86c1818` is the projected material's
    // g_tNormalRoughness; a hue set on it bends the normals, so it stays out.
    standard_recipe(
        "kelvin",
        &[
            "materials/particle/projected/kelvin_ice_dome_projected_psd_b5785889.vtex_c",
            "models/abilities/materials/ice_dome_color_psd_3a38e562.vtex_c",
        ],
        &[],
        Some("materials/particle/projected/kelvin_ice_dome_projected_psd_b5785889.vtex_c"),
    )
}

fn priest_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "priest",
        &[
            "materials/particle/abilities/priest/priest_flashbang_debuff_aoe_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/priest_snaptrap_projectile_aoe_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ],
        &[],
        Some(
            "materials/particle/projected/priest_snaptrap_projectile_aoe_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ),
    )
}

/// Ivy (`tengu`): particle namespace is `tengu`, while two color-bearing ability
/// assets still use Ivy-era paths.
fn tengu_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "tengu",
        &[
            "materials/particle/cables/ivy_vine_cable_vmat_g_tcolor_9509ed42.vtex_c",
            "models/abilities/materials/ivy_entangling_thorns_vmat_g_tcolor_59ac0039.vtex_c",
            "models/abilities/materials/ivy_entangling_thorns_vmat_g_tnprtransmissivecolor_4d5e61a7.vtex_c",
            "models/abilities/materials/ivy_thornbomb_color_png_6ca9c4e1.vtex_c",
        ],
        &[
            "models/heroes_staging/tengu/tengu_v2/materials/tengu_dissolve_mat.vmat_c",
        ],
        Some(
            "models/abilities/materials/ivy_entangling_thorns_vmat_g_tcolor_59ac0039.vtex_c",
        ),
    )
}

fn viper_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "viper",
        &["materials/particle/abilities/viper/viper_petrify_symbol_ground_psd_a643967f.vtex_c"],
        &["materials/models/particle/viper_venom_blobs.vmat_c"],
        Some("materials/particle/abilities/viper/viper_petrify_symbol_ground_psd_a643967f.vtex_c"),
    )
}

fn viscous_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "viscous".to_string(),
        particle_prefixes: vec![
            "particles/abilities/viscous/".to_string(),
            "particles/weapon_fx/viscous/".to_string(),
            "particles/abilities/melee/viscous/".to_string(),
        ],
        texture_entries: [
            "materials/models/particle/viscous_puddle_telegraph_vmat_g_tcolor_ac749641.vtex_c",
            // (viscous_detail_psd_a2817163 is shared with a neutral sludge creature.)
            "materials/particle/abilities/viscous/viscous_detail_psd_3c03ec04.vtex_c",
            "materials/particle/abilities/viscous/viscous_detail_psd_4414414e.vtex_c",
            "models/heroes_staging/viscous/materials/viscous_punch_preview_vmat_g_tcolor_32414205.vtex_c",
            "models/heroes_staging/viscous/materials/viscous_punch_vmat_g_tcolor_afc99362.vtex_c",
            "models/heroes_staging/viscous/materials/viscous_ball_vmat_g_tcolor_2c347bde.vtex_c",
            "models/heroes_staging/viscous/materials/viscous_melee_anvil_vmat_g_tcolor_aff502e0.vtex_c",
            "models/abilities/materials/viscous_cube_color_png_81d0eb6a.vtex_c",
            "models/abilities/materials/viscous_cube_color_png_85c8b349.vtex_c",
            "models/abilities/materials/viscous_cube_color_png_daff99b9.vtex_c",
            "models/abilities/materials/viscous_fist_color_psd_ab531623.vtex_c",
            "models/abilities/materials/viscous_fist_color_psd_d8e8086a.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: vec![
            "models/abilities/materials/viscous_slime.vmat_c".to_string(),
            "models/abilities/materials/viscous_slime_blobs.vmat_c".to_string(),
            "models/heroes_staging/viscous/materials/viscous_ball.vmat_c".to_string(),
            "materials/particle/abilities/viscous/viscous_goo_charge_projectile.vmat_c".to_string(),
            "materials/particle/abilities/viscous/viscous_goo_projectile.vmat_c".to_string(),
            "models/heroes_staging/viscous/materials/viscous_melee_anvil.vmat_c".to_string(),
            // translucent goo / blob materials with a baked green g_vColorTint
            "materials/models/particle/viscous_projectile_erode.vmat_c".to_string(),
            "materials/models/particle/viscous_splash_erode.vmat_c".to_string(),
            "materials/particle/abilities/viscous/viscous_base_projectile.vmat_c".to_string(),
            "materials/particle/blobs/viscous_blob_transluscent.vmat_c".to_string(),
            "materials/particle/blobs/viscous_debuff_blob.vmat_c".to_string(),
            "materials/particle/blobs/viscous_sprinkler_blob.vmat_c".to_string(),
            "materials/particle/projected/viscous_blob_projected.vmat_c".to_string(),
        ],
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "materials/models/particle/viscous_puddle_telegraph_vmat_g_tcolor_ac749641.vtex_c"
                .to_string(),
        ),
    }
}

fn werewolf_recipe() -> HeroRecolorRecipe {
    standard_recipe(
        "werewolf",
        &[
            "materials/particle/abilities/werewolf/werewolf_cripplingslash_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/werewolf_transform_bite_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/projected/werewolf_transform_crushing_leap_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ],
        &[],
        Some(
            "materials/particle/abilities/werewolf/werewolf_cripplingslash_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
        ),
    )
}

/// Yamato: most ability/weapon VFX color lives in particle color params. Unlike
/// the generic particle-only heroes, three status particles live under
/// `particles/status_fx/`, and a few hero-specific textures are chromatic:
/// a green projected blade-dash self-illum swatch, the two shadow-redemption
/// status maps, plus the red shadow-form body albedo used by the model's
/// `shadow` material group. The other Yamato ability textures audited from
/// `pak01` are white alpha masks or grayscale ramps, so they are left
/// particle-tinted. The `pak01` audit patched 234 `.vpcf_c` files cleanly, with
/// 66 color-free helpers skipped.
fn yamato_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "yamato".to_string(),
        particle_prefixes: vec![
            "particles/abilities/yamato/".to_string(),
            "particles/weapon_fx/yamato/".to_string(),
            "particles/status_fx/status_fx_yamato".to_string(),
        ],
        texture_entries: [
            "materials/particle/projected/yamato_blade_dash_ground_projected_vmat_g_tselfillum_670d93d.vtex_c",
            "materials/particle/abilities/yamato/yamato_shadow_redemption_complete_status.vtex_c",
            YAMATO_SHADOW_SHAPE_COLOR_TEXTURE,
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: Vec::new(),
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "materials/particle/abilities/yamato/yamato_shadow_redemption_complete_status.vtex_c"
                .to_string(),
        ),
    }
}

/// Paradox (`chrono`). Ability VFX color is overwhelmingly in particle color params
/// (the teal time-warp), so the prism/recolor lands the whole ability set off the two
/// particle prefixes. The only real chromatic ability textures are the time-stop bubble
/// energy bands (`chrono_fx_bubble02/04`, baked crimson scanlines on her bubble and
/// projectile fx prop models); they take the spectrum/hue too so the time-stop dome
/// matches. Her hourglass-head / shoulder glow are skin accents (a separate reskin), not
/// ability props, so they are intentionally excluded here.
fn chrono_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "chrono".to_string(),
        particle_prefixes: vec![
            "particles/abilities/chrono/".to_string(),
            "particles/weapon_fx/chrono/".to_string(),
        ],
        texture_entries: [
            "models/heroes_staging/chrono/materials/chrono_fx_bubble02_color_psd_f57b1ef0.vtex_c",
            "models/heroes_staging/chrono/materials/chrono_fx_bubble04_color_psd_ee26af5c.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        material_entries: Vec::new(),
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: Some(
            "models/heroes_staging/chrono/materials/chrono_fx_bubble04_color_psd_ee26af5c.vtex_c"
                .to_string(),
        ),
    }
}

/// Graves (`necro`). Particles carry most of her VFX color (her large ability maps
/// are grayscale, tinted by the particle color), but two ability MODELS hold their
/// color in a material `g_vColorTint` constant rather than a texture, and the
/// gravestone's transmissive glow is a small chromatic texture. See the audit notes
/// at [`recipe_for`]. Her large color maps stay particle-driven (no `texture_entries`
/// beyond the glow); no baked vertex colors.
fn necro_recipe() -> HeroRecolorRecipe {
    HeroRecolorRecipe {
        codename: "necro".to_string(),
        particle_prefixes: vec![
            "particles/abilities/necro/".to_string(),
            "particles/weapon_fx/necro/".to_string(),
            // The held-weapon ambient flame (the green fire on her gun/hand) lives
            // under particles/heroes/, not abilities/weapon_fx; the reference pink
            // recolor edits these too. Without this prefix the held flame stays green.
            "particles/heroes/necro/".to_string(),
        ],
        // Her ability PROPS (the zombie/shambler, the ult jar, the gravestone) carry
        // their green in real albedo + transmissive textures, in-game confirmed as the
        // missing piece (particles + tint constants alone left the 3D props green).
        // These live in the hero material dir but are ability props, not her body
        // skin (head/hand/upper/lower/hair/skirt/bag/eye stay vanilla).
        texture_entries: [
            // shambler = the summoned zombie: albedo (2048, chromatic) + transmissive
            "models/heroes_wip/necro/materials/necro_shambler_color_tga_7b1de566.vtex_c",
            "models/heroes_wip/necro/materials/necro_shambler_vmat_g_tnprtransmissivecolor_337e62d.vtex_c",
            // ult jar (jar of dread) + its glass
            "models/heroes_wip/necro/materials/necro_jar_of_dread_color_tga_7f34b26.vtex_c",
            "models/heroes_wip/necro/materials/necro_jar_glass_color_tga_c6d5a0ec.vtex_c",
            // gravestone: the green transmissive glow (the standing model and the
            // destruction fx reference it at two paths; the albedo is grey).
            "models/heroes_wip/necro/materials/necro_gravestone_vmat_g_tnprtransmissivecolor_e8edad5e.vtex_c",
            "models/abilities/materials/necro_gravestone_destruction_vmat_g_tnprtransmissivecolor_e8edad5e.vtex_c",
            // the green transmissive glow around the soul-picker hand (its bony
            // albedo is grey and doubles as the weapon hand's skin).
            "models/heroes_wip/necro/materials/necro_hand_vmat_g_tnprtransmissivecolor_c987b5a.vtex_c",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        // Effect-material tints (g_vColorTint / g_vSelfIllumTint), STAMPED with the
        // brand color (see [`recolor_material_color_bytes`]). The reference pink mod
        // stamps one color on all of these, including the held flaming-hand prop and
        // its aura, which a saturation-preserving recolor leaves vanilla.
        material_entries: vec![
            "models/abilities/materials/necro_pickup_sphere.vmat_c".to_string(),
            "materials/particle/abilities/necro/necro_jar_glass.vmat_c".to_string(),
            "models/abilities/materials/necro_hands.vmat_c".to_string(),
            // the green flame aura is self-illum (the selfillum texture is just a
            // grayscale mask).
            "models/heroes_wip/necro/materials/necro_flame_effect.vmat_c".to_string(),
            // The soul-picker hand effect: its g_vColorTint is white but driven by a
            // dynamic expression; the yellow g_vSelfIllumTint is static and stamps.
            // (necro_flame_effect_hand, necro_picker_effect and picker_hand_glow went
            // unreferenced in the 2026-10 build.)
            "models/heroes_wip/necro/materials/necro_picker_hand_effect.vmat_c".to_string(),
            // The gravestone's glowing skull / R.I.P. text / cracks: a g_vSelfIllumTint
            // (the necro yellow-green that reads as gold on the bright emissive),
            // masked by a grayscale selfillum texture. The standing stone and its
            // destruction fx carry the tint on the material, not a texture.
            "models/heroes_wip/necro/materials/necro_gravestone.vmat_c".to_string(),
            "models/abilities/materials/necro_gravestone_destruction.vmat_c".to_string(),
        ],
        model_entries: Vec::new(),
        shared_entries: Vec::new(),
        preview_texture: None,
    }
}

/// What a [`recolor_hero_to_addon`] run produced.
#[derive(Debug, Clone, Default)]
pub struct HeroRecolorReport {
    pub codename: String,
    pub hue: f64,
    pub saturation: f64,
    pub value: f64,
    /// `.vpcf_c` files that had at least one color channel changed (packed).
    pub particles_recolored: usize,
    /// `.vpcf_c` files under the prefixes that carry no color param (left alone).
    pub particles_no_color: usize,
    /// `.vpcf_c` files that carry color but could not be patched in place (e.g. a
    /// non-v5 KV3 block the in-place scalar patcher does not handle). Skipped and
    /// left vanilla rather than aborting the whole bake; a nonzero count means the
    /// recolor is partial for this hero.
    pub particles_unpatchable: usize,
    pub textures_recolored: usize,
    /// `.vmat_c` entries whose `g_vColorTint` constant was retinted in place.
    pub materials_recolored: usize,
    /// `.vmat_c` entries that carry a tint but couldn't be patched in place (e.g. a
    /// non-v5 KV3 block, or a ZSTD-compressed binary-blob section: there is no ZSTD
    /// encoder, so it cannot be re-emitted). Blobbed **LZ4** v5 materials ARE handled
    /// now (re-emitted still compressed); they no longer land here.
    pub materials_unpatchable: usize,
    pub models_recolored: usize,
    /// Vertices whose baked color changed across the recolored models.
    pub model_vertices: usize,
    /// Shared entries written as hero-local copies instead of in place.
    pub isolation: IsolationStats,
    /// Total entries packed into the addon.
    pub total_entries: usize,
}

/// What a [`prism_recolor_hero_to_addon`] run produced.
#[derive(Debug, Clone, Default)]
pub struct HeroPrismRecolorReport {
    pub codename: String,
    /// `.vpcf_c` files under the recipe prefixes.
    pub particles_total: usize,
    /// `.vpcf_c` files whose color/tint channels were changed and packed.
    pub particles_recolored: usize,
    /// `.vpcf_c` files under the prefixes that carry no color param.
    pub particles_no_color: usize,
    /// Color-bearing particles that the scalar patcher rejected.
    pub particles_unpatchable: usize,
    /// Existing gradient color stop arrays recolored as spectral ramps.
    pub gradient_fields: usize,
    /// Non-gradient color/tint arrays recolored.
    pub color_fields: usize,
    /// Color fields lifted brighter so the spectrum reads in game.
    pub boosted_fields: usize,
    /// Black gradient endpoints lifted to dark spectral color instead of pure gaps.
    pub lifted_black_gradient_fields: usize,
    /// Random min/max/fade fields spread across wider hue offsets.
    pub random_range_fields: usize,
    /// Explicit recipe color textures recolored to deterministic spectrum hues
    /// or, for animated Yamato Shadow Form, scrollable rainbow band maps.
    pub textures_recolored: usize,
    /// Explicit recipe material tint constants recolored to deterministic spectrum hues.
    pub materials_recolored: usize,
    pub materials_unpatchable: usize,
    /// Explicit recipe vertex-color models recolored to deterministic spectrum hues.
    pub models_recolored: usize,
    pub model_vertices: usize,
    /// High-visibility particles that got at least one animation timing edit
    /// (`--animated` only; 0 for a static prism). A particle can be counted here
    /// and in `particles_recolored` both: animation is layered on the colored bytes.
    pub particles_animated: usize,
    /// Texture-scroll inputs repointed at particle age (the spectrum sweeps over
    /// each particle's lifetime).
    pub texture_age_inputs: usize,
    /// Texture-offset scroll multipliers boosted (wider spectral travel).
    pub texture_offset_multipliers: usize,
    /// Gradient stop positions retimed so spectral changes read earlier/evener.
    pub gradient_timing_edits: usize,
    /// Existing age-driven color gradients flipped to loop instead of playing once.
    pub color_gradient_loops: usize,
    /// Runtime color-cycle operators inserted for constant-color particles.
    pub color_cycle_operators: usize,
    /// Shared entries written as hero-local copies instead of in place.
    pub isolation: IsolationStats,
    /// Total entries packed into the addon.
    pub total_entries: usize,
}

#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_field_names)]
struct ParticlePrismStats {
    gradient_fields: usize,
    color_fields: usize,
    boosted_fields: usize,
    lifted_black_gradient_fields: usize,
    random_range_fields: usize,
}

/// Particle-shape scan for deciding whether a hero is a good candidate for
/// rainbow / animated rainbow VFX.
#[derive(Debug, Clone, Default)]
pub struct HeroRainbowSupportReport {
    pub codename: String,
    pub particles_total: usize,
    pub particles_decoded: usize,
    pub particles_decode_failed: usize,
    /// Particles whose existing color scalars can be patched in place.
    pub particles_patchable: usize,
    /// Particles with no color edit to apply (usually color-free helper systems
    /// or black literal defaults).
    pub particles_color_free: usize,
    /// Color-bearing particles that the scalar patcher rejected.
    pub particles_unpatchable: usize,
    /// Color/tint-keyed Color32 arrays found in particle KV3.
    pub color_fields: usize,
    /// Non-black color/tint arrays, a better proxy for visible color controls.
    pub visible_color_fields: usize,
    /// Objects with a non-empty `m_Gradient.m_Stops`.
    pub gradient_fields: usize,
    pub gradient_stops: usize,
    pub multi_stop_gradient_fields: usize,
    /// Gradient color inputs driven by collection age.
    pub collection_age_gradient_fields: usize,
    /// Gradient color inputs driven by particle lifetime.
    pub particle_age_gradient_fields: usize,
    /// Gradient color inputs with `PF_INPUT_MODE_LOOPED`.
    pub looped_gradient_fields: usize,
    pub random_color_initializers: usize,
    pub color_interpolate_ops: usize,
    pub collection_age_inputs: usize,
    pub particle_age_inputs: usize,
    pub looped_inputs: usize,
    pub texture_entries: usize,
    pub material_entries: usize,
    pub model_entries: usize,
}

/// Scan one pinned hero recipe and report how much of its particle VFX can
/// support rainbow treatment with the current in-place scalar patcher.
pub fn scan_hero_rainbow_support(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    codename: &str,
) -> Result<HeroRainbowSupportReport> {
    let recipe = recipe_for(codename).with_context(|| {
        format!(
            "no built-in ability-VFX recolor recipe for hero codename {codename:?} \
             (pinned: {})",
            pinned_hero_codenames().join(", ")
        )
    })?;
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let particle_entries = recipe_particle_entries(&vpks, &recipe);

    let mut report = HeroRainbowSupportReport {
        codename: recipe.codename.clone(),
        particles_total: particle_entries.len(),
        texture_entries: recipe.texture_entries.len(),
        material_entries: recipe.material_entries.len(),
        model_entries: recipe.model_entries.len(),
        ..Default::default()
    };

    for entry in &particle_entries {
        let bytes = read_entry(&vpks, entry)
            .with_context(|| format!("reading particle {entry} (listed but unreadable)"))?;
        let Ok(value) = morphic::decode_kv3_resource(&bytes) else {
            report.particles_decode_failed += 1;
            continue;
        };
        report.particles_decoded += 1;
        collect_rainbow_support_stats(&value, false, &mut report);

        let mut edits = Vec::new();
        collect_color_edits(
            &value,
            &mut Vec::new(),
            false,
            Recolor::hue(300.0),
            &mut edits,
        );
        if edits.is_empty() {
            report.particles_color_free += 1;
            continue;
        }
        match morphic::patch_kv3_resource_scalars(&bytes, &edits) {
            Ok(_) => report.particles_patchable += 1,
            Err(_) => report.particles_unpatchable += 1,
        }
    }

    Ok(report)
}

/// Recolor a hero's full ability-VFX set (particles + color textures + vertex
/// colors) to `hue_deg` and pack it into one addon VPK at `out`, each entry at
/// its base path so the addon overrides the base game in place.
///
/// Entries are read from `vpk` first, then `base` (so an active skin's overriding
/// files win); pass `base = None` to recolor straight from the base pak. Errors
/// if no recipe is pinned for `codename`, or if a recipe texture/model entry is
/// missing from the VPK(s) (a likely path-drift bug) so a silently incomplete
/// addon is never written. Particles that carry no color param are skipped, not
/// an error (most of a hero's `.vpcf_c` are color-free helpers); a color-bearing
/// particle that can't be patched in place (a non-v5 KV3 block) is also skipped
/// (counted in `particles_unpatchable`) rather than aborting the whole bake.
pub fn recolor_hero_to_addon(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    codename: &str,
    recolor: Recolor,
    out: impl AsRef<Path>,
) -> Result<HeroRecolorReport> {
    let recipe = recipe_for(codename).with_context(|| {
        format!(
            "no built-in ability-VFX recolor recipe for hero codename {codename:?} \
             (pinned: {})",
            pinned_hero_codenames().join(", ")
        )
    })?;

    let vpks = open_vpks(vpk.as_ref(), base)?;
    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();
    let mut report = HeroRecolorReport {
        codename: recipe.codename.clone(),
        hue: recolor.hue,
        saturation: recolor.saturation,
        value: recolor.value,
        ..Default::default()
    };

    // 1. Particles: every `.vpcf_c` under the recipe prefixes. Color params get
    //    retinted in place; the (many) color-free files are skipped.
    let particle_entries = recipe_particle_entries(&vpks, &recipe);
    for entry in &particle_entries {
        let bytes = read_entry(&vpks, entry)
            .with_context(|| format!("reading particle {entry} (listed but unreadable)"))?;
        match recolor_particle_bytes(&bytes, recolor) {
            Ok(Some(new_bytes)) => {
                packed.push((entry.clone(), new_bytes));
                report.particles_recolored += 1;
            }
            Ok(None) => report.particles_no_color += 1,
            // A particle that carries color but can't be patched in place (a non-v5
            // KV3 block the scalar patcher rejects) is skipped, not fatal: leave it
            // vanilla and keep going so the rest of the hero still recolors.
            Err(e) => {
                report.particles_unpatchable += 1;
                eprintln!("  note: skipping {entry} (left vanilla): {e:#}");
            }
        }
    }

    // 2. Color textures + 3. vertex-color models: explicit recipe entries. A
    //    missing entry is a recipe/path bug, so collect and fail loudly.
    let mut missing: Vec<&str> = Vec::new();

    for entry in &recipe.texture_entries {
        match read_entry(&vpks, entry) {
            Some(bytes) => {
                let new_bytes = crate::recolor::recolor_texture_hue(&bytes, recolor)
                    .with_context(|| format!("recoloring texture {entry}"))?;
                packed.push((entry.clone(), new_bytes));
                report.textures_recolored += 1;
            }
            None => missing.push(entry),
        }
    }

    // Material color-tint constants: retint each `.vmat_c`'s `g_vColorTint` in
    // place. Blobbed LZ4 v5 materials are retinted too (kept compressed, only the
    // changed buffer recompressed; see docs/spike-blobbed-vmat-recolor.md). A
    // material with no patchable tint, or one the in-place patcher still can't reach
    // (a non-v5 block or a ZSTD-blob section), is skipped with a note rather than
    // aborting the whole hero.
    for entry in &recipe.material_entries {
        match read_entry(&vpks, entry) {
            Some(bytes) => match recolor_material_color_bytes(&bytes, recolor) {
                Ok(Some(new_bytes)) => {
                    packed.push((entry.clone(), new_bytes));
                    report.materials_recolored += 1;
                }
                Ok(None) => eprintln!("  note: {entry} has no g_vColorTint constant; skipping"),
                Err(e) => {
                    report.materials_unpatchable += 1;
                    eprintln!("  note: skipping material {entry} (left vanilla): {e:#}");
                }
            },
            None => missing.push(entry),
        }
    }

    for entry in &recipe.model_entries {
        match read_entry(&vpks, entry) {
            Some(bytes) => {
                let (new_bytes, stats) =
                    crate::recolor::recolor_model_vertex_colors(&bytes, recolor)
                        .with_context(|| format!("recoloring model {entry}"))?;
                if stats.buffers_recolored == 0 {
                    // Found but colorless: a recipe over-include, not a path bug.
                    // Skip rather than override the base with an identical model.
                    eprintln!("  note: {entry} has no color-bearing vertex buffer; skipping");
                    continue;
                }
                packed.push((entry.clone(), new_bytes));
                report.models_recolored += 1;
                report.model_vertices += stats.vertices;
            }
            None => missing.push(entry),
        }
    }

    if !missing.is_empty() {
        anyhow::bail!(
            "{} recipe entr{} not found in the given VPK(s) (recipe drift?): {}",
            missing.len(),
            if missing.len() == 1 { "y" } else { "ies" },
            missing.join(", ")
        );
    }

    let (packed, isolation) = isolate_bake(
        &vpks,
        &recipe,
        &particle_entries,
        packed.into_iter().collect(),
    )?;
    report.isolation = isolation;
    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    crate::pack(&refs, out.as_ref())
        .with_context(|| format!("packing hero recolor into {}", out.as_ref().display()))?;
    report.total_entries = packed.len();
    Ok(report)
}

/// Recolor a hero's ability-VFX set as a prism/rainbow addon.
///
/// This is the app-facing version of the in-game-proven prism particle probes:
/// it only patches existing particle color/tint scalars in place, so compiled
/// particle resource framing is preserved. Explicit recipe textures/materials/
/// vertex-color models are included too, but they receive deterministic
/// per-entry spectrum hues rather than true texture animation.
///
/// With `animated`, high-visibility effects (glow/beam/trail/arc/slash/... per
/// [`is_prism_animation_target`]) get an extra byte-faithful timing pass on top
/// of the color spread: texture-scroll inputs are repointed at particle age, the
/// scroll multiplier is boosted, and gradient stop positions are retimed, so the
/// spectrum sweeps over each particle's lifetime instead of sitting static. The
/// timing edits are best-effort per file (skipped where the fields aren't present)
/// and never touch color, so `animated = false` reproduces the static prism byte
/// for byte. Promoted from `examples/yamato_anim_prism_micro.rs`, generalized off
/// Yamato to run on any pinned hero's particle prefixes.
/// User adjustments layered on top of the deterministic prism spectrum: rotate
/// the whole rainbow's start hue and scale its saturation / brightness. The
/// default reproduces the original prism byte-for-byte (offset 0, scales 1.0), so
/// callers that want the canonical rainbow can ignore this entirely.
#[derive(Debug, Clone, Copy)]
pub struct PrismTuning {
    /// Degrees added to every spectrum hue (rotate where the rainbow starts).
    pub hue_offset: f64,
    /// Saturation scale on the spectrum (1.0 = engine default, <1 pastels it).
    pub saturation: f64,
    /// Brightness (HSV value) scale on the spectrum (1.0 = engine default).
    pub brightness: f64,
    /// Strength of the optional animation pass. `1.0` keeps the original
    /// animated-prism timing; higher values push texture scroll and color timing
    /// harder, while `0.0` disables animation edits even if `animated` is true.
    pub animation_intensity: f64,
    /// How far the optional animation pass should go.
    pub animation_style: PrismAnimationStyle,
    /// When set, the prism samples this custom gradient instead of the full
    /// rainbow wheel (the effect's spectral position maps onto these stops). The
    /// rotation / saturation / brightness above still apply on top. `None` = the
    /// canonical rainbow.
    pub gradient: Option<PrismGradient>,
}

impl Default for PrismTuning {
    fn default() -> Self {
        Self {
            hue_offset: 0.0,
            saturation: 1.0,
            brightness: 1.0,
            animation_intensity: 1.0,
            animation_style: PrismAnimationStyle::Sweep,
            gradient: None,
        }
    }
}

/// Optional animation depth for prism VFX.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrismAnimationStyle {
    /// Repoint safe texture scrolls, boost their travel, and retime early gradient
    /// stops. This is the original `--animated` behavior.
    #[default]
    Sweep,
    /// Sweep plus loop existing age-driven color gradients.
    Loop,
    /// Loop plus insert a runtime color-cycle operator for particles whose visible
    /// color is a static constant.
    Cycle,
}

/// One stop of a [`PrismGradient`]: a hue (degrees) and saturation (0..1) at a
/// normalized position (0..1) along the effect's spectral spread.
#[derive(Debug, Clone, Copy)]
pub struct GradientStop {
    pub position: f64,
    pub hue: f64,
    pub saturation: f64,
}

/// Max stops a [`PrismGradient`] holds. Fixed-capacity so the gradient stays
/// `Copy` and can ride inside [`PrismTuning`] without cloning at every spectrum site.
pub const MAX_GRADIENT_STOPS: usize = 8;

/// Built-in gradient preset names (see [`PrismGradient::preset`]).
pub const PRISM_PRESET_NAMES: &[&str] = &[
    "fire", "ice", "toxic", "sunset", "ocean", "neon", "gold", "void",
];

/// A custom color ramp the prism samples instead of the full rainbow: an effect's
/// spectral position `t` (0..1) maps onto these stops (hue + saturation
/// interpolated), so e.g. `fire` reads red -> orange -> yellow rather than a full
/// spectrum. Brightness still comes from the source effect.
#[derive(Debug, Clone, Copy)]
pub struct PrismGradient {
    stops: [GradientStop; MAX_GRADIENT_STOPS],
    len: usize,
}

impl PrismGradient {
    /// Build from 2..=[`MAX_GRADIENT_STOPS`] stops (sorted by position here).
    /// Returns `None` if the count is out of range.
    #[must_use]
    pub fn from_stops(input: &[GradientStop]) -> Option<Self> {
        if input.len() < 2 || input.len() > MAX_GRADIENT_STOPS {
            return None;
        }
        let mut stops = [GradientStop {
            position: 0.0,
            hue: 0.0,
            saturation: 1.0,
        }; MAX_GRADIENT_STOPS];
        stops[..input.len()].copy_from_slice(input);
        stops[..input.len()].sort_by(|a, b| a.position.total_cmp(&b.position));
        Some(Self {
            stops,
            len: input.len(),
        })
    }

    fn from_triples(triples: &[(f64, f64, f64)]) -> Option<Self> {
        let stops: Vec<GradientStop> = triples
            .iter()
            .map(|&(position, hue, saturation)| GradientStop {
                position,
                hue,
                saturation,
            })
            .collect();
        Self::from_stops(&stops)
    }

    /// A built-in preset by name (case-insensitive), or `None` if unknown.
    #[must_use]
    pub fn preset(name: &str) -> Option<Self> {
        let stops: &[(f64, f64, f64)] = match name.to_ascii_lowercase().as_str() {
            "fire" => &[(0.0, 0.0, 1.0), (0.5, 25.0, 1.0), (1.0, 50.0, 1.0)],
            "ice" => &[(0.0, 190.0, 1.0), (0.5, 215.0, 0.9), (1.0, 205.0, 0.25)],
            "toxic" => &[(0.0, 110.0, 1.0), (0.5, 90.0, 1.0), (1.0, 72.0, 1.0)],
            "sunset" => &[(0.0, 280.0, 1.0), (0.5, 325.0, 0.95), (1.0, 30.0, 1.0)],
            "ocean" => &[(0.0, 175.0, 1.0), (0.5, 205.0, 1.0), (1.0, 235.0, 1.0)],
            "neon" => &[(0.0, 300.0, 1.0), (0.5, 240.0, 1.0), (1.0, 180.0, 1.0)],
            "gold" => &[(0.0, 25.0, 1.0), (0.5, 45.0, 1.0), (1.0, 55.0, 0.55)],
            "void" => &[(0.0, 270.0, 1.0), (0.5, 305.0, 1.0), (1.0, 240.0, 1.0)],
            _ => return None,
        };
        Self::from_triples(stops)
    }

    /// Parse a `--gradient` spec: a preset name, or a stop list
    /// `pos:hue[:sat],pos:hue[:sat],...` (pos 0..1, hue degrees, sat 0..1 default 1).
    pub fn from_spec(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        if let Some(g) = Self::preset(spec) {
            return Ok(g);
        }
        let mut stops = Vec::new();
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let mut it = part.split(':');
            let pos: f64 = it
                .next()
                .and_then(|s| s.trim().parse().ok())
                .ok_or_else(|| format!("bad gradient stop position in {part:?}"))?;
            let hue: f64 = it
                .next()
                .and_then(|s| s.trim().parse().ok())
                .ok_or_else(|| format!("bad gradient stop hue in {part:?}"))?;
            let saturation: f64 = match it.next() {
                Some(s) => s
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad gradient stop saturation in {part:?}"))?,
                None => 1.0,
            };
            stops.push(GradientStop {
                position: pos.clamp(0.0, 1.0),
                hue,
                saturation: saturation.clamp(0.0, 1.0),
            });
        }
        Self::from_stops(&stops).ok_or_else(|| {
            format!(
                "gradient needs 2..={MAX_GRADIENT_STOPS} stops or a preset name ({})",
                PRISM_PRESET_NAMES.join(", ")
            )
        })
    }

    /// (hue degrees, saturation 0..1) at spectral position `t`, linearly
    /// interpolating between the bracketing stops.
    fn sample(&self, t: f64) -> (f64, f64) {
        let t = t.clamp(0.0, 1.0);
        let stops = &self.stops[..self.len];
        let last = stops[self.len - 1];
        if t <= stops[0].position {
            return (stops[0].hue, stops[0].saturation);
        }
        if t >= last.position {
            return (last.hue, last.saturation);
        }
        for pair in stops.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t >= a.position && t <= b.position {
                let span = (b.position - a.position).max(1e-9);
                let f = (t - a.position) / span;
                // Interpolate hue along the SHORTEST arc, so a gradient crossing the
                // 360/0 boundary (e.g. pink 325 -> orange 30) travels through red, not
                // the long way through cyan. hsv_to_rgb wraps the result via rem_euclid.
                let mut dh = b.hue - a.hue;
                if dh > 180.0 {
                    dh -= 360.0;
                } else if dh < -180.0 {
                    dh += 360.0;
                }
                return (
                    a.hue + dh * f,
                    a.saturation + (b.saturation - a.saturation) * f,
                );
            }
        }
        (last.hue, last.saturation)
    }
}

/// [`prism_recolor_hero_to_addon_tuned`] with no spectrum tuning: the canonical
/// rainbow. Kept as the stable entry point for callers (e.g. the GUI Prism tab)
/// that don't expose the rotation / saturation knobs.
pub fn prism_recolor_hero_to_addon(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    codename: &str,
    animated: bool,
    out: impl AsRef<Path>,
) -> Result<HeroPrismRecolorReport> {
    prism_recolor_hero_to_addon_tuned(vpk, base, codename, animated, PrismTuning::default(), out)
}

/// Spectrum-tuned sibling of [`prism_recolor_hero_to_addon`]: the same rainbow
/// bake, but `tuning` rotates the whole spectrum's start hue and scales its
/// saturation / brightness uniformly across particles, textures, materials, and
/// models, so a UI can offer "rotate / desaturate the rainbow" without losing the
/// per-effect spread.
#[allow(clippy::too_many_lines)]
pub fn prism_recolor_hero_to_addon_tuned(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    codename: &str,
    animated: bool,
    tuning: PrismTuning,
    out: impl AsRef<Path>,
) -> Result<HeroPrismRecolorReport> {
    let recipe = recipe_for(codename).with_context(|| {
        format!(
            "no built-in ability-VFX recolor recipe for hero codename {codename:?} \
             (pinned: {})",
            pinned_hero_codenames().join(", ")
        )
    })?;

    let vpks = open_vpks(vpk.as_ref(), base)?;
    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    let mut report = HeroPrismRecolorReport {
        codename: recipe.codename.clone(),
        ..Default::default()
    };

    let particle_entries = recipe_particle_entries(&vpks, &recipe);
    report.particles_total = particle_entries.len();
    for entry in &particle_entries {
        let bytes = read_entry(&vpks, entry)
            .with_context(|| format!("reading particle {entry} (listed but unreadable)"))?;

        // 1. Color spread: retint every color-bearing particle to its spectrum.
        let (mut working, had_color) =
            match prism_recolor_particle_bytes(&bytes, &recipe.codename, entry, tuning) {
                Ok(Some((new_bytes, stats))) => {
                    report.gradient_fields += stats.gradient_fields;
                    report.color_fields += stats.color_fields;
                    report.boosted_fields += stats.boosted_fields;
                    report.lifted_black_gradient_fields += stats.lifted_black_gradient_fields;
                    report.random_range_fields += stats.random_range_fields;
                    (new_bytes, true)
                }
                Ok(None) => (bytes, false),
                // A color-bearing particle the scalar patcher rejects is left vanilla,
                // not fatal; it also gets no animation pass (the bytes are unchanged).
                Err(e) => {
                    report.particles_unpatchable += 1;
                    eprintln!("  note: skipping {entry} (left vanilla): {e:#}");
                    continue;
                }
            };

        // 2. Optional animation pass on high-visibility effects, layered on the
        //    colored bytes. Best-effort: a file missing the timing fields is left as
        //    the color-only result.
        let mut animated_this = false;
        if animated && tuning.animation_intensity > 0.0 && is_prism_animation_target(entry) {
            match apply_prism_animation(&working, entry, tuning) {
                Ok((new_bytes, stats)) if stats.total() > 0 => {
                    working = new_bytes;
                    report.texture_age_inputs += stats.age_inputs;
                    report.texture_offset_multipliers += stats.multipliers;
                    report.gradient_timing_edits += stats.gradient_timing;
                    animated_this = true;
                }
                Ok(_) => {}
                Err(e) => eprintln!("  note: animation skipped for {entry}: {e:#}"),
            }

            if matches!(
                tuning.animation_style,
                PrismAnimationStyle::Loop | PrismAnimationStyle::Cycle
            ) {
                match loop_animate_particle_bytes(&working) {
                    Ok(Some(new_bytes)) => {
                        working = new_bytes;
                        report.color_gradient_loops += 1;
                        animated_this = true;
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("  note: color-gradient loop skipped for {entry}: {e:#}"),
                }
            }

            if tuning.animation_style == PrismAnimationStyle::Cycle {
                match insert_color_cycle_operator_tuned(&working, tuning) {
                    Ok(Some(new_bytes)) => {
                        working = new_bytes;
                        report.color_cycle_operators += 1;
                        animated_this = true;
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("  note: color-cycle insert skipped for {entry}: {e:#}"),
                }
            }

            if animated_this {
                report.particles_animated += 1;
            }
        }

        if had_color {
            report.particles_recolored += 1;
            packed.push((entry.clone(), working));
        } else if animated_this {
            // Color-free but animated: still a real override worth packing.
            packed.push((entry.clone(), working));
        } else {
            report.particles_no_color += 1;
        }
    }

    for entry in &recipe.texture_entries {
        match read_entry(&vpks, entry) {
            Some(bytes) => {
                let new_bytes =
                    prism_recolor_texture_bytes(&recipe.codename, entry, &bytes, animated, tuning)
                        .with_context(|| format!("prism-recoloring texture {entry}"))?;
                packed.push((entry.clone(), new_bytes));
                report.textures_recolored += 1;
            }
            None => missing.push(entry),
        }
    }

    for entry in &recipe.material_entries {
        match read_entry(&vpks, entry) {
            Some(bytes) => {
                let recolor = spectrum_recolor_for(&recipe.codename, entry, 0.33, tuning);
                match recolor_material_color_bytes(&bytes, recolor) {
                    Ok(Some(new_bytes)) => {
                        packed.push((entry.clone(), new_bytes));
                        report.materials_recolored += 1;
                    }
                    Ok(None) => eprintln!("  note: {entry} has no prism material tint; skipping"),
                    Err(e) => {
                        report.materials_unpatchable += 1;
                        eprintln!("  note: skipping material {entry} (left vanilla): {e:#}");
                    }
                }
            }
            None => missing.push(entry),
        }
    }

    for entry in &recipe.model_entries {
        match read_entry(&vpks, entry) {
            Some(bytes) => {
                let recolor = spectrum_recolor_for(&recipe.codename, entry, 0.66, tuning);
                let (new_bytes, stats) =
                    crate::recolor::recolor_model_vertex_colors(&bytes, recolor)
                        .with_context(|| format!("prism-recoloring model {entry}"))?;
                if stats.buffers_recolored == 0 {
                    eprintln!("  note: {entry} has no color-bearing vertex buffer; skipping");
                    continue;
                }
                packed.push((entry.clone(), new_bytes));
                report.models_recolored += 1;
                report.model_vertices += stats.vertices;
            }
            None => missing.push(entry),
        }
    }

    if !missing.is_empty() {
        anyhow::bail!(
            "{} recipe entr{} not found in the given VPK(s) (recipe drift?): {}",
            missing.len(),
            if missing.len() == 1 { "y" } else { "ies" },
            missing.join(", ")
        );
    }

    let (packed, isolation) = isolate_bake(
        &vpks,
        &recipe,
        &particle_entries,
        packed.into_iter().collect(),
    )?;
    report.isolation = isolation;
    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    crate::pack(&refs, out.as_ref())
        .with_context(|| format!("packing hero prism recolor into {}", out.as_ref().display()))?;
    report.total_entries = packed.len();
    Ok(report)
}

/// Render a hero's recolor as a PNG swatch for a live UI preview, without baking
/// the whole addon. Reads the recipe's representative `preview_texture` from the
/// VPK(s) and recolors just its top mip (no lossy re-encode, no pack), so a color
/// picker can repaint as the user drags. The PNG is the design-intent color the
/// full bake will land on. Errors if no recipe is pinned, or the preview texture
/// is missing from the VPK(s).
pub fn recolor_hero_preview_png(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    codename: &str,
    recolor: Recolor,
) -> Result<Vec<u8>> {
    let recipe = recipe_for(codename).with_context(|| {
        format!(
            "no built-in ability-VFX recolor recipe for hero codename {codename:?} \
             (pinned: {})",
            pinned_hero_codenames().join(", ")
        )
    })?;
    let preview_texture = recipe.preview_texture.as_deref().with_context(|| {
        format!(
            "hero {codename:?} is particle-only (no color texture), so there is no \
             preview swatch to render; bake the addon with `recolor_hero_to_addon` instead"
        )
    })?;
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let bytes = read_entry(&vpks, preview_texture).with_context(|| {
        format!("preview texture {preview_texture} not found in the given VPK(s)")
    })?;
    crate::recolor::recolor_texture_preview_png(&bytes, recolor)
        .context("rendering hero recolor preview")
}

/// Recolor one `.vpcf_c` as a static prism by changing existing color/tint
/// channels only. Returns `None` when the particle has no color channels.
fn prism_recolor_particle_bytes(
    vpcf_bytes: &[u8],
    codename: &str,
    entry: &str,
    tuning: PrismTuning,
) -> Result<Option<(Vec<u8>, ParticlePrismStats)>> {
    if prism_particle_passthrough(codename, entry) {
        return Ok(None);
    }
    let value = morphic::decode_kv3_resource(vpcf_bytes)
        .map_err(|e| anyhow::anyhow!("decoding particle KV3: {e}"))?;
    let mut edits = Vec::new();
    let mut stats = ParticlePrismStats::default();
    collect_prism_edits(
        codename,
        entry,
        &value,
        &mut Vec::new(),
        false,
        None,
        tuning,
        &mut edits,
        &mut stats,
    );
    if edits.is_empty() {
        return Ok(None);
    }
    let new_bytes = morphic::patch_kv3_resource_scalars(vpcf_bytes, &edits)
        .map_err(|e| anyhow::anyhow!("patching particle prism scalars: {e}"))?;
    Ok(Some((new_bytes, stats)))
}

fn prism_particle_passthrough(codename: &str, entry: &str) -> bool {
    if codename != "bebop" {
        return false;
    }
    let entry = entry.to_ascii_lowercase();
    [
        "particles/abilities/bebop/bebop_laser_beam_proj_ground.vpcf_c",
        "particles/abilities/bebop/bebop_laser_beam_end_proj_ground.vpcf_c",
        "particles/abilities/bebop/bebop_laser_beam_end_proj_surface.vpcf_c",
    ]
    .contains(&entry.as_str())
}

/// The particle-flow enum that drives an input from a particle's normalized age.
const PRISM_ANIM_AGE_TYPE: &str = "PF_TYPE_PARTICLE_AGE_NORMALIZED";

/// Effect-name keywords that mark a high-visibility particle worth animating (its
/// spectrum should sweep over the particle's life, not sit static).
const PRISM_ANIM_TARGET_KEYWORDS: &[&str] = &[
    "arc", "beam", "charge", "core", "dash", "endcap", "energy", "flash", "glow", "light", "magic",
    "pulse", "ring", "rope", "slash", "streak", "sweep", "tracer", "trail",
];

/// Effect-name keywords whose particles should stay static even if they also match
/// a target keyword (smoke/dust/etc. read worse cycling; `power_slash` was excluded
/// after an in-game pass on Yamato and is harmless elsewhere, no other hero has it).
const PRISM_ANIM_SKIP_KEYWORDS: &[&str] = &[
    "blood",
    "darkness",
    "debris",
    "dust",
    "fog",
    "gas",
    "pnt",
    "power_slash",
    "shake",
    "sleep",
    "smoke",
];

/// Whether a `.vpcf_c` entry is a high-visibility effect the animation pass should
/// retime. The caller has already restricted to the hero's particle prefixes, so
/// this is the effect-keyword filter only: a target keyword present, no skip keyword.
#[must_use]
pub fn is_prism_animation_target(entry: &str) -> bool {
    let name = entry.to_ascii_lowercase();
    PRISM_ANIM_TARGET_KEYWORDS
        .iter()
        .any(|kw| name.contains(kw))
        && !PRISM_ANIM_SKIP_KEYWORDS.iter().any(|kw| name.contains(kw))
}

#[derive(Debug, Clone, Copy, Default)]
struct PrismAnimStats {
    age_inputs: usize,
    multipliers: usize,
    gradient_timing: usize,
}

impl PrismAnimStats {
    fn total(self) -> usize {
        self.age_inputs + self.multipliers + self.gradient_timing
    }
}

/// Public summary of the byte-faithful particle timing animation pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParticleTimingAnimationStats {
    pub age_inputs: usize,
    pub texture_offset_multipliers: usize,
    pub gradient_timing_edits: usize,
}

impl ParticleTimingAnimationStats {
    #[must_use]
    pub fn total(self) -> usize {
        self.age_inputs + self.texture_offset_multipliers + self.gradient_timing_edits
    }
}

impl From<PrismAnimStats> for ParticleTimingAnimationStats {
    fn from(value: PrismAnimStats) -> Self {
        Self {
            age_inputs: value.age_inputs,
            texture_offset_multipliers: value.multipliers,
            gradient_timing_edits: value.gradient_timing,
        }
    }
}

#[derive(Default)]
#[allow(clippy::struct_field_names)]
struct PrismAnimPlan {
    string_paths: Vec<Vec<Seg>>,
    /// Texture-offset multiplier leaves, each with its authored value so the
    /// boost can scale it (preserving sign and relative speed) instead of
    /// stamping an absolute that would flip an authored `-0.1` to `+2.5`.
    mult_paths: Vec<(Vec<Seg>, f64)>,
    gradient_paths: Vec<(Vec<Seg>, f64)>,
}

/// True if `needle` appears as a string literal anywhere in the tree (so it is in
/// the file's string table and can be referenced by an in-place string patch).
fn has_value_string(v: &Value, needle: &str) -> bool {
    match v {
        Value::String(s) => s == needle,
        Value::Array(items) => items.iter().any(|v| has_value_string(v, needle)),
        Value::Object(pairs) => pairs.iter().any(|(_, v)| has_value_string(v, needle)),
        _ => false,
    }
}

/// Retimed position for an early gradient stop (tighten the spectral ramp toward
/// the particle's start). Only the first few stops are nudged; later stops keep
/// their authored timing.
fn prism_anim_stop_target(label: &str) -> Option<f64> {
    if label.contains("/m_Stops[1]/m_flPosition") {
        Some(0.18)
    } else if label.contains("/m_Stops[2]/m_flPosition") {
        Some(0.45)
    } else if label.contains("/m_Stops[3]/m_flPosition") {
        Some(0.72)
    } else {
        None
    }
}

fn path_starts_with(path: &[Seg], prefix: &[Seg]) -> bool {
    path.len() >= prefix.len() && path.iter().zip(prefix).all(|(a, b)| a == b)
}

/// Walk the KV3 tree collecting the three animation edit sites: texture-offset
/// input-type strings, texture-offset multipliers, and early gradient stop times.
fn collect_prism_anim_plan(
    v: &Value,
    path: &mut Vec<Seg>,
    skip_offset_prefixes: &[Vec<Seg>],
    plan: &mut PrismAnimPlan,
) {
    let label = path_label(path);
    let lower = label.to_ascii_lowercase();
    let skip_offset = skip_offset_prefixes
        .iter()
        .any(|prefix| path_starts_with(path, prefix));

    if matches!(v, Value::String(s) if s != PRISM_ANIM_AGE_TYPE)
        && lower.contains("/m_texturecontrols/")
        && lower.contains("/m_flfinaltextureoffset")
        && lower.ends_with("/m_ntype")
        && !skip_offset
    {
        plan.string_paths.push(path.clone());
    }

    if lower.contains("/m_texturecontrols/")
        && lower.contains("/m_flfinaltextureoffset")
        && lower.ends_with("/m_flmultfactor")
        && !skip_offset
    {
        if let Some(original) = v.as_f64() {
            plan.mult_paths.push((path.clone(), original));
        }
    }

    if lower.contains("/m_gradient/m_stops") && lower.ends_with("/m_flposition") {
        if let Some(target) = prism_anim_stop_target(&label) {
            plan.gradient_paths.push((path.clone(), target));
        }
    }

    match v {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                path.push(Seg::Index(i));
                collect_prism_anim_plan(item, path, skip_offset_prefixes, plan);
                path.pop();
            }
        }
        Value::Object(pairs) => {
            for (key, child) in pairs {
                path.push(Seg::Key(key.clone()));
                collect_prism_anim_plan(child, path, skip_offset_prefixes, plan);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Patch one numeric leaf, trying the stored-double encoding first, then float.
/// Returns the (possibly unchanged) bytes and whether the patch landed.
#[allow(clippy::cast_possible_truncation)]
fn patch_one_number(bytes: Vec<u8>, path: Vec<Seg>, value: f64) -> (Vec<u8>, bool) {
    if let Ok(patched) = morphic::patch_kv3_resource_doubles(&bytes, &[(path.clone(), value)]) {
        return (patched, true);
    }
    if let Ok(patched) = morphic::patch_kv3_resource_floats(&bytes, &[(path, value as f32)]) {
        return (patched, true);
    }
    (bytes, false)
}

/// Layer the animation timing pass onto one (already prism-colored) particle's
/// bytes. All three edits are byte-faithful in-place patches, so the compiled
/// particle framing is preserved; each is best-effort and skipped where its fields
/// aren't present. Color is never touched. Promoted from the Yamato micro-pass.
fn apply_prism_animation(
    vpcf_bytes: &[u8],
    _entry: &str,
    tuning: PrismTuning,
) -> Result<(Vec<u8>, PrismAnimStats)> {
    let tree = morphic::decode_kv3_resource(vpcf_bytes)
        .map_err(|e| anyhow::anyhow!("decoding particle KV3 for animation: {e}"))?;
    let mut plan = PrismAnimPlan::default();
    let skip_offset = non_tiling_texture_inputs(&tree);
    collect_prism_anim_plan(&tree, &mut Vec::new(), &skip_offset, &mut plan);

    let mut out = vpcf_bytes.to_vec();
    let mut stats = PrismAnimStats::default();
    let intensity = tuning.animation_intensity.clamp(0.0, 4.0);

    // Texture-scroll input -> particle age, only when that enum already exists in
    // the string table (so the in-place string patch can reference it).
    if has_value_string(&tree, PRISM_ANIM_AGE_TYPE) {
        for path in plan.string_paths {
            if let Ok(patched) = morphic::patch_kv3_resource_strings(
                &out,
                &[(path, PRISM_ANIM_AGE_TYPE.to_string())],
            ) {
                out = patched;
                stats.age_inputs += 1;
            }
        }
    }

    // Scale the authored multiplier rather than stamping an absolute: an
    // authored `-0.1` (slow reverse crawl) becomes `-0.4` at full intensity,
    // never a sign-flipped `+2.5`. A `0.0` (static by design) stays static.
    let scroll_boost = 1.0 + 1.5 * intensity;
    for (path, original) in plan.mult_paths {
        let target = original * scroll_boost;
        if (target - original).abs() < f64::EPSILON {
            continue;
        }
        let (patched, ok) = patch_one_number(out, path, target);
        out = patched;
        if ok {
            stats.multipliers += 1;
        }
    }

    let timing_scale = intensity.sqrt().max(0.01);
    for (path, value) in plan.gradient_paths {
        let value = (value / timing_scale).clamp(0.04, 0.95);
        let (patched, ok) = patch_one_number(out, path, value);
        out = patched;
        if ok {
            stats.gradient_timing += 1;
        }
    }

    Ok((out, stats))
}

/// Apply the same timing animation pass used by animated prism VFX to one
/// particle: age-driven texture offset, boosted scroll multipliers, and tighter
/// early gradient stops. Returns `None` when the particle has no safe timing
/// fields to edit.
pub fn animate_particle_timing_bytes(
    vpcf_bytes: &[u8],
    intensity: f64,
) -> Result<Option<(Vec<u8>, ParticleTimingAnimationStats)>> {
    let tuning = PrismTuning {
        animation_intensity: intensity,
        ..PrismTuning::default()
    };
    let (new_bytes, stats) = apply_prism_animation(vpcf_bytes, "", tuning)?;
    if stats.total() == 0 {
        Ok(None)
    } else {
        Ok(Some((new_bytes, stats.into())))
    }
}

#[derive(Debug, Clone, Copy)]
struct PrismTheme {
    base: f64,
    span: f64,
    jitter: f64,
}

fn prism_theme_for(codename: &str, entry: &str) -> PrismTheme {
    let base = hash01(codename) * 360.0;
    let e = entry.to_ascii_lowercase();
    if e.contains("weapon_fx") || e.contains("tracer") || e.contains("bullet") {
        PrismTheme {
            base: base + 25.0,
            span: 310.0,
            jitter: 24.0,
        }
    } else if e.contains("beam") || e.contains("laser") || e.contains("arc") {
        PrismTheme {
            base: base + 185.0,
            span: 175.0,
            jitter: 18.0,
        }
    } else if e.contains("projectile")
        || e.contains("grenade")
        || e.contains("dart")
        || e.contains("bomb")
        || e.contains("explode")
        || e.contains("impact")
    {
        PrismTheme {
            base: base + 45.0,
            span: 230.0,
            jitter: 20.0,
        }
    } else if e.contains("shield")
        || e.contains("heal")
        || e.contains("buff")
        || e.contains("status")
        || e.contains("aura")
    {
        PrismTheme {
            base: base + 125.0,
            span: 185.0,
            jitter: 18.0,
        }
    } else {
        PrismTheme {
            base,
            span: 320.0,
            jitter: 24.0,
        }
    }
}

fn spectrum_recolor_for(codename: &str, entry: &str, offset: f64, tuning: PrismTuning) -> Recolor {
    let t = (hash01(&format!("{codename}:{entry}")) + offset).fract();
    if let Some(g) = tuning.gradient {
        let (hue, sat) = g.sample(t);
        Recolor::new(
            (hue + tuning.hue_offset).rem_euclid(360.0),
            sat * tuning.saturation,
            tuning.brightness,
        )
    } else {
        let hue = (t + tuning.hue_offset / 360.0).fract() * 360.0;
        Recolor::new(hue, tuning.saturation, tuning.brightness)
    }
}

fn prism_texture_recolor_for(
    codename: &str,
    entry: &str,
    offset: f64,
    tuning: PrismTuning,
) -> Recolor {
    if tuning.gradient.is_none()
        && codename == "yamato"
        && entry == YAMATO_SHADOW_SHAPE_COLOR_TEXTURE
    {
        // This full-body shadow-form albedo sits under status-effect color warp and
        // cloak lighting. A saturated spectrum hue turns the ult body into a loud
        // static sheet, so keep it cool and subdued while still removing the red.
        // The tuning still rotates / scales it so it tracks the rest of the rainbow.
        // A custom gradient overrides this and samples like everything else.
        Recolor::new(
            (190.0 + tuning.hue_offset).rem_euclid(360.0),
            0.45 * tuning.saturation,
            0.72 * tuning.brightness,
        )
    } else {
        spectrum_recolor_for(codename, entry, offset, tuning)
    }
}

pub(crate) fn prism_recolor_texture_bytes(
    codename: &str,
    entry: &str,
    bytes: &[u8],
    animated: bool,
    tuning: PrismTuning,
) -> Result<Vec<u8>> {
    if animated && codename == "yamato" && is_yamato_shadow_status_texture(entry) {
        return rainbowize_yamato_shadow_status_texture(bytes, tuning);
    }
    if animated {
        if let Some(profile) = projected_texture_prism_profile(codename, entry) {
            return rainbowize_projected_texture(bytes, tuning, profile);
        }
    }

    let recolor = prism_texture_recolor_for(codename, entry, 0.0, tuning);
    crate::recolor::recolor_texture_hue(bytes, recolor)
}

fn is_yamato_shadow_status_texture(entry: &str) -> bool {
    YAMATO_SHADOW_STATUS_TEXTURES.contains(&entry)
}

#[derive(Debug, Clone, Copy)]
struct ProjectedTexturePrism {
    value_scale: f64,
    radial_repeats: f64,
    angular_repeats: f64,
}

fn projected_texture_prism_profile(codename: &str, entry: &str) -> Option<ProjectedTexturePrism> {
    let e = entry.to_ascii_lowercase();
    if codename == "digger" && e.contains("digger_burrow") && e.contains("ground_dark_projected") {
        return Some(ProjectedTexturePrism {
            value_scale: 0.68,
            radial_repeats: 1.5,
            angular_repeats: 1.0,
        });
    }
    if codename == "frank"
        && (e.contains("frank_revive_marker_ground_projected")
            || e.contains("frank_shock_miss_projected_bright"))
    {
        return Some(ProjectedTexturePrism {
            value_scale: 0.88,
            radial_repeats: 2.0,
            angular_repeats: 1.0,
        });
    }
    if codename == "unicorn" && e.contains("unicorn_radiant_flare_ground") {
        return Some(ProjectedTexturePrism {
            value_scale: 0.72,
            radial_repeats: 1.75,
            angular_repeats: 1.0,
        });
    }
    if codename == "unicorn"
        && (e.contains("unicorn_prismatic_shield_ground_warning_projected")
            || e.contains("unicorn_flux_rainbow_ground_projected_light"))
    {
        return Some(ProjectedTexturePrism {
            value_scale: 0.78,
            radial_repeats: 1.75,
            angular_repeats: 1.0,
        });
    }
    None
}

/// Projected ground textures are often grayscale/self-illum masks where a single
/// hue recolor reads as a flat disk. For animated prism builds, paint hue bands
/// over the existing luminance/alpha instead, so the authored edge and falloff
/// stay intact while the decal still reads rainbow.
#[allow(clippy::cast_precision_loss)]
fn rainbowize_projected_texture(
    vtex_bytes: &[u8],
    tuning: PrismTuning,
    profile: ProjectedTexturePrism,
) -> Result<Vec<u8>> {
    let mut image = morphic::decode(vtex_bytes).context("decoding projected prism texture")?;
    let width = image.width as usize;
    let height = image.height as usize;
    let morphic::ImageData::Rgba8(pixels) = &mut image.data else {
        anyhow::bail!("projected prism texture supports LDR (8-bit) textures only");
    };

    for row in 0..height {
        let tex_v = row as f64 / height.max(1) as f64;
        for col in 0..width {
            let tex_u = col as f64 / width.max(1) as f64;
            let pixel_index = (row * width + col) * 4;
            let source_v = f64::from(
                pixels[pixel_index]
                    .max(pixels[pixel_index + 1])
                    .max(pixels[pixel_index + 2]),
            ) / 255.0;
            if source_v < 0.015 {
                continue;
            }
            let dx = tex_u - 0.5;
            let dy = tex_v - 0.5;
            let radius = (dx.mul_add(dx, dy * dy).sqrt() * 2.0).clamp(0.0, 1.5);
            let angle = (dy.atan2(dx) / std::f64::consts::TAU + 1.0).fract();
            let band = (angle * profile.angular_repeats + radius * profile.radial_repeats).fract();
            let out = hsv_to_rgb_i64(
                band * 360.0 + tuning.hue_offset,
                tuning.saturation.clamp(0.0, 1.0),
                (source_v.powf(0.9) * profile.value_scale * tuning.brightness).clamp(0.0, 1.0),
            );
            pixels[pixel_index] = clamp_channel(out[0]);
            pixels[pixel_index + 1] = clamp_channel(out[1]);
            pixels[pixel_index + 2] = clamp_channel(out[2]);
            // Alpha remains the authored projection mask.
        }
    }

    morphic::replace_mip_chain(vtex_bytes, &image).context("re-encoding projected prism texture")
}

/// Yamato Shadow Form's body overlay ignores the particle color-cycle operator in
/// practice, but its authored status-effect detail maps visibly scroll over the
/// model. Replacing those maps with repeated hue bands lets the existing scroll
/// carry an animated rainbow while preserving the original texture's luminance and
/// alpha mask.
#[allow(clippy::cast_precision_loss)]
fn rainbowize_yamato_shadow_status_texture(
    vtex_bytes: &[u8],
    tuning: PrismTuning,
) -> Result<Vec<u8>> {
    let mut image =
        morphic::decode(vtex_bytes).context("decoding Yamato Shadow Form status texture")?;
    let w = image.width as usize;
    let h = image.height as usize;
    let morphic::ImageData::Rgba8(px) = &mut image.data else {
        anyhow::bail!("Yamato Shadow Form status rainbow supports LDR (8-bit) textures only");
    };

    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let rgb = rainbow_status_band_rgb(
                [px[i], px[i + 1], px[i + 2]],
                x as f64 / w as f64,
                y as f64 / h as f64,
                tuning,
            );
            px[i] = rgb[0];
            px[i + 1] = rgb[1];
            px[i + 2] = rgb[2];
            // Alpha is the status-effect mask and must stay authored.
        }
    }

    morphic::replace_mip_chain(vtex_bytes, &image)
        .context("re-encoding Yamato Shadow Form rainbow status texture")
}

fn rainbow_status_band_rgb(rgb: [u8; 3], u: f64, v: f64, tuning: PrismTuning) -> [u8; 3] {
    let value = (f64::from(rgb[0].max(rgb[1]).max(rgb[2])) / 255.0)
        .powf(0.85)
        .clamp(0.0, 1.0);
    // Eight diagonal hue repeats across the top mip. The status effect's authored
    // UV scroll moves these bands over the model surface in-game.
    let band = (u * 8.0 + v * 3.0).fract();
    let out = hsv_to_rgb_i64(
        band * 360.0 + tuning.hue_offset,
        tuning.saturation.clamp(0.0, 1.0),
        (value * tuning.brightness).clamp(0.0, 1.0),
    );
    [
        clamp_channel(out[0]),
        clamp_channel(out[1]),
        clamp_channel(out[2]),
    ]
}

// Exact float comparisons are intentional: `max` is built from `r`/`g`/`b` by
// `.max()`, so `max == r` etc. are exact-by-construction channel selects.
#[allow(clippy::float_cmp, clippy::many_single_char_names)]
fn rgb_to_hsv(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]
fn hsv_to_rgb_i64(h: f64, s: f64, v: f64) -> [i64; 3] {
    let c = v * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as i64,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as i64,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as i64,
    ]
}

// FNV-1a over the bytes, mapped into 0..1. The named FNV offset/prime constants
// read better unseparated; the final scale to 0..1 is a deliberate hash->float.
#[allow(clippy::unreadable_literal, clippy::cast_precision_loss)]
fn hash01(s: &str) -> f64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    h as f64 / u64::MAX as f64
}

fn path_label(path: &[Seg]) -> String {
    let mut out = String::new();
    for seg in path {
        match seg {
            Seg::Key(k) => {
                out.push('/');
                out.push_str(k);
            }
            Seg::Index(i) => {
                out.push('[');
                out.push_str(&i.to_string());
                out.push(']');
            }
        }
    }
    out
}

fn effect_label(entry: &str, path: &[Seg]) -> String {
    format!(
        "{}{}",
        entry.to_ascii_lowercase(),
        path_label(path).to_ascii_lowercase()
    )
}

fn spectral_path(label: &str) -> bool {
    [
        "glow",
        "light",
        "beam",
        "core",
        "flash",
        "ring",
        "symbol",
        "energy",
        "magic",
        "trail",
        "arc",
        "rope",
        "slash",
        "sweep",
        "tracer",
        "streak",
        "pulse",
        "endcap",
        "projectile",
        "explode",
        "impact",
    ]
    .iter()
    .any(|needle| label.contains(needle))
}

fn subdued_path(label: &str) -> bool {
    ["smoke", "dust", "debris", "darkness", "fog", "gas", "blood"]
        .iter()
        .any(|needle| label.contains(needle))
}

fn hue_at(theme: PrismTheme, entry: &str, label: &str, t: f64) -> f64 {
    let jitter = (hash01(&format!("{entry}{label}")) - 0.5) * 2.0 * theme.jitter;
    theme.base + theme.span * t + jitter
}

/// (hue degrees, saturation 0..1) for a color at spectral position `t`, honoring a
/// custom gradient when set, else the per-effect themed rainbow. Rotation and the
/// saturation scale are applied here; the caller applies brightness.
fn prism_hue_sat(
    theme: PrismTheme,
    entry: &str,
    label: &str,
    effect_label: &str,
    t: f64,
    tuning: PrismTuning,
) -> (f64, f64) {
    let (hue, sat) = match tuning.gradient {
        Some(g) => g.sample(t),
        None => (hue_at(theme, entry, label, t), saturation_for(effect_label)),
    };
    (
        hue + tuning.hue_offset,
        (sat * tuning.saturation).clamp(0.0, 1.0),
    )
}

fn value_floor(source_v: f64, label: &str, gradient: bool) -> (f64, bool, bool) {
    if label.contains("particles/abilities/bebop/bebop_laser_beam_end_proj_ground") {
        return (source_v, false, false);
    }
    if label.contains("particles/abilities/bebop/bebop_laser_beam_proj_ground") {
        let floor = 0.62;
        return (source_v.max(floor), source_v < floor, false);
    }

    if source_v < 0.02 {
        return if gradient && spectral_path(label) {
            (0.30, true, true)
        } else {
            (source_v, false, false)
        };
    }

    if subdued_path(label) {
        (source_v.max(0.48), false, false)
    } else if spectral_path(label) {
        (source_v.max(0.96), source_v < 0.96, false)
    } else if gradient {
        (source_v.max(0.86), source_v < 0.86, false)
    } else {
        (source_v.max(0.78), source_v < 0.78, false)
    }
}

fn saturation_for(label: &str) -> f64 {
    if subdued_path(label) {
        0.82
    } else {
        1.0
    }
}

#[allow(clippy::too_many_arguments, clippy::cast_precision_loss)]
fn prism_gradient_stop(
    rgb: [i64; 3],
    codename: &str,
    entry: &str,
    path: &[Seg],
    index: usize,
    count: usize,
    position: Option<f64>,
    tuning: PrismTuning,
    stats: &mut ParticlePrismStats,
) -> [i64; 3] {
    stats.gradient_fields += 1;
    let (_, _, v) = rgb_to_hsv(
        rgb[0] as f64 / 255.0,
        rgb[1] as f64 / 255.0,
        rgb[2] as f64 / 255.0,
    );
    let path_label = path_label(path).to_ascii_lowercase();
    let effect_label = effect_label(entry, path);
    let theme = prism_theme_for(codename, entry);
    let t = if count <= 1 {
        hash01(&format!("{entry}{path_label}"))
    } else if count == 2 {
        [0.10, 0.82][index.min(1)]
    } else if let Some(position) = position {
        position.clamp(0.0, 1.0)
    } else {
        index as f64 / (count - 1) as f64
    };
    let (hue, sat) = prism_hue_sat(theme, entry, &path_label, &effect_label, t, tuning);
    let (val, boosted, lifted_black) = value_floor(v, &effect_label, true);
    if boosted {
        stats.boosted_fields += 1;
    }
    if lifted_black {
        stats.lifted_black_gradient_fields += 1;
    }
    hsv_to_rgb_i64(hue, sat, (val * tuning.brightness).clamp(0.0, 1.0))
}

#[allow(clippy::cast_precision_loss)]
fn prism_color_field(
    rgb: [i64; 3],
    codename: &str,
    entry: &str,
    path: &[Seg],
    tuning: PrismTuning,
    stats: &mut ParticlePrismStats,
) -> [i64; 3] {
    stats.color_fields += 1;
    let (_, _, v) = rgb_to_hsv(
        rgb[0] as f64 / 255.0,
        rgb[1] as f64 / 255.0,
        rgb[2] as f64 / 255.0,
    );
    if v < 0.02 {
        return rgb;
    }
    if entry.contains("particles/abilities/haze/haze_flurry_ground_proj_ring") {
        return hsv_to_rgb_i64(
            52.0 + tuning.hue_offset,
            tuning.saturation.clamp(0.0, 1.0),
            tuning.brightness.clamp(0.0, 1.0),
        );
    }

    let path_label = path_label(path).to_ascii_lowercase();
    let effect_label = effect_label(entry, path);
    let theme = prism_theme_for(codename, entry);
    let base_t = hash01(&format!("{entry}{path_label}"));
    let t = if path_label.ends_with("/m_colormin") {
        stats.random_range_fields += 1;
        0.02 + base_t * 0.18
    } else if path_label.ends_with("/m_colormax") {
        stats.random_range_fields += 1;
        0.70 + base_t * 0.28
    } else if path_label.ends_with("/m_colorfade") {
        stats.random_range_fields += 1;
        0.40 + base_t * 0.35
    } else {
        base_t
    };
    let (hue, sat) = prism_hue_sat(theme, entry, &path_label, &effect_label, t, tuning);
    let (val, boosted, _) = value_floor(v, &effect_label, false);
    if boosted {
        stats.boosted_fields += 1;
    }
    hsv_to_rgb_i64(hue, sat, (val * tuning.brightness).clamp(0.0, 1.0))
}

fn prism_path_is_stops(path: &[Seg]) -> bool {
    matches!(path.last(), Some(Seg::Key(k)) if k == "m_Stops")
}

#[allow(clippy::too_many_arguments)]
fn collect_prism_edits(
    codename: &str,
    entry: &str,
    v: &Value,
    path: &mut Vec<Seg>,
    colorish: bool,
    gradient_stop: Option<(usize, usize, Option<f64>)>,
    tuning: PrismTuning,
    edits: &mut Vec<(Vec<Seg>, i64)>,
    stats: &mut ParticlePrismStats,
) {
    if colorish {
        if let Some(rgb) = as_color(v) {
            let new = if let Some((i, n, position)) = gradient_stop {
                prism_gradient_stop(rgb, codename, entry, path, i, n, position, tuning, stats)
            } else {
                prism_color_field(rgb, codename, entry, path, tuning, stats)
            };
            for (i, &nv) in new.iter().enumerate() {
                if nv != rgb[i] {
                    let mut p = path.clone();
                    p.push(Seg::Index(i));
                    edits.push((p, nv));
                }
            }
            return;
        }
    }

    match v {
        Value::Object(pairs) => {
            for (k, child) in pairs {
                let kl = k.to_lowercase();
                let c = kl.contains("color") || kl.contains("tint");
                path.push(Seg::Key(k.clone()));
                collect_prism_edits(
                    codename,
                    entry,
                    child,
                    path,
                    c,
                    gradient_stop,
                    tuning,
                    edits,
                    stats,
                );
                path.pop();
            }
        }
        Value::Array(items) => {
            let stops = prism_path_is_stops(path);
            let len = items.len();
            for (i, item) in items.iter().enumerate() {
                path.push(Seg::Index(i));
                let child_gradient = if stops {
                    let position = item.get("m_flPosition").and_then(Value::as_f64);
                    Some((i, len, position))
                } else {
                    gradient_stop
                };
                collect_prism_edits(
                    codename,
                    entry,
                    item,
                    path,
                    false,
                    child_gradient,
                    tuning,
                    edits,
                    stats,
                );
                path.pop();
            }
        }
        _ => {}
    }
}

/// Make a particle's prism-recolored color gradients *cycle* over time, returning
/// the new bytes or `None` if the particle has no loopable color gradient.
///
/// A Source 2 color gradient (`m_Gradient/m_Stops`, which the prism pass rewrote into
/// a spectrum) is sampled by a float driver (`m_FloatInterp`). When that driver reads
/// `PF_TYPE_COLLECTION_AGE` (the whole effect's age) in `PF_INPUT_MODE_LOOPED`, the
/// gradient lookup wraps over the driver's input range every cycle, so the spectrum
/// scrolls continuously: a true animated rainbow, not the one-shot sweep that a
/// clamped driver gives. This finds every gradient driven by an age input and flips
/// its driver to looped collection-age (see [`collect_loop_edits`]).
///
/// Done with [`morphic::patch_kv3_resource_strings_adding`], which *adds* the
/// `PF_INPUT_MODE_LOOPED` / `PF_TYPE_COLLECTION_AGE` enum strings to the KV3 string
/// table when a particle lacks them (only ~5 of 300 Yamato particles already carry
/// `LOOPED`), so coverage is broad rather than limited to particles that happened to
/// intern the string. Every other byte is preserved, so the compiled particle stays
/// engine-loadable (unlike a full re-encode, which red-errors).
pub fn loop_animate_particle_bytes(vpcf_bytes: &[u8]) -> Result<Option<Vec<u8>>> {
    let value = morphic::decode_kv3_resource(vpcf_bytes)
        .map_err(|e| anyhow::anyhow!("decoding particle KV3: {e}"))?;
    let mut edits = Vec::new();
    collect_loop_edits(&value, &mut Vec::new(), &mut edits);
    if edits.is_empty() {
        return Ok(None);
    }
    let new_bytes = morphic::patch_kv3_resource_strings_adding(vpcf_bytes, &edits)
        .map_err(|e| anyhow::anyhow!("looping particle color gradients: {e}"))?;
    Ok(Some(new_bytes))
}

/// Insert a runtime color-cycle operator into a particle whose visible color is a
/// static top-level `m_ConstantColor`.
///
/// This is phase #2 for animated prism particles: phase #1 can only loop existing
/// age-driven color gradients. Many particles instead carry one constant RGB color
/// and no color-gradient driver to flip. For those, append a sparse
/// `C_OP_SetVec` operator to `m_Operators` that writes the tint RGB attribute from
/// a rainbow `PVEC_TYPE_FLOAT_INTERP_GRADIENT`, driven by looped collection age
/// over `0..1` seconds. The structural edit goes through morphic's byte-faithful
/// KV3 array insertion, not a full particle re-encode.
pub fn insert_color_cycle_operator(vpcf_bytes: &[u8]) -> Result<Option<Vec<u8>>> {
    insert_color_cycle_operator_tuned(vpcf_bytes, PrismTuning::default())
}

/// Tuned sibling of [`insert_color_cycle_operator`], used by custom VFX themes
/// that want the inserted runtime color-cycle gradient to follow a specific
/// palette rather than the default rainbow.
pub fn insert_color_cycle_operator_with_tuning(
    vpcf_bytes: &[u8],
    tuning: PrismTuning,
) -> Result<Option<Vec<u8>>> {
    insert_color_cycle_operator_tuned(vpcf_bytes, tuning)
}

fn insert_color_cycle_operator_tuned(
    vpcf_bytes: &[u8],
    tuning: PrismTuning,
) -> Result<Option<Vec<u8>>> {
    let value = morphic::decode_kv3_resource(vpcf_bytes)
        .map_err(|e| anyhow::anyhow!("decoding particle KV3: {e}"))?;
    let Some(operators) = value.get("m_Operators").and_then(Value::as_array) else {
        return Ok(None);
    };
    let Some(rgb) = visible_constant_color(&value) else {
        return Ok(None);
    };
    if has_age_driven_color_gradient(&value) {
        return Ok(None);
    }

    let op = color_cycle_setvec_operator(rgb, tuning);
    let path = vec![Seg::Key("m_Operators".to_string())];
    let new_bytes =
        morphic::patch_kv3_resource_array_insert(vpcf_bytes, &path, operators.len(), &op)
            .map_err(|e| anyhow::anyhow!("inserting particle color-cycle operator: {e}"))?;
    Ok(Some(new_bytes))
}

fn visible_constant_color(value: &Value) -> Option<[i64; 3]> {
    let color = value.get("m_ConstantColor")?;
    let rgb = as_color(color)?;
    if rgb.iter().copied().max().unwrap_or(0) <= 1 {
        return None;
    }
    if let Value::Array(ch) = color {
        if ch.get(3).and_then(Value::as_int).is_some_and(|a| a <= 0) {
            return None;
        }
    }
    Some(rgb)
}

fn has_age_driven_color_gradient(v: &Value) -> bool {
    match v {
        Value::Object(pairs) => {
            let has_gradient = v
                .get("m_Gradient")
                .and_then(|g| g.get("m_Stops"))
                .and_then(Value::as_array)
                .is_some_and(|stops| !stops.is_empty());
            if has_gradient {
                let kind = v
                    .get("m_FloatInterp")
                    .and_then(|interp| interp.get("m_nType"))
                    .and_then(Value::as_str);
                if matches!(
                    kind,
                    Some("PF_TYPE_COLLECTION_AGE" | "PF_TYPE_PARTICLE_AGE_NORMALIZED")
                ) {
                    return true;
                }
            }
            pairs
                .iter()
                .any(|(_, child)| has_age_driven_color_gradient(child))
        }
        Value::Array(items) => items.iter().any(has_age_driven_color_gradient),
        _ => false,
    }
}

#[allow(clippy::cast_precision_loss)]
fn color_cycle_setvec_operator(rgb: [i64; 3], tuning: PrismTuning) -> Value {
    let (base_hue, _, source_v) = rgb_to_hsv(
        rgb[0] as f64 / 255.0,
        rgb[1] as f64 / 255.0,
        rgb[2] as f64 / 255.0,
    );
    let value = (source_v.max(0.95) * tuning.brightness).clamp(0.0, 1.0);
    let cycle_seconds = (1.0 / tuning.animation_intensity.max(0.25)).clamp(0.2, 4.0);
    let stops = [
        (0.0_f32, 0.0_f64),
        (0.166_666_67_f32, 60.0),
        (0.333_333_34_f32, 120.0),
        (0.5_f32, 180.0),
        (0.666_666_7_f32, 240.0),
        (0.833_333_3_f32, 300.0),
        (1.0_f32, 360.0),
    ]
    .into_iter()
    .map(|(t, hue_offset)| {
        gradient_stop(
            t,
            base_hue + hue_offset + tuning.hue_offset,
            (tuning.saturation.clamp(0.0, 1.0), value),
        )
    })
    .collect();

    Value::Object(vec![
        (
            "_class".to_string(),
            Value::String("C_OP_SetVec".to_string()),
        ),
        (
            "m_InputValue".to_string(),
            Value::Object(vec![
                (
                    "m_nType".to_string(),
                    Value::String("PVEC_TYPE_FLOAT_INTERP_GRADIENT".to_string()),
                ),
                ("m_nVectorAttribute".to_string(), Value::Int(6)),
                (
                    "m_FloatInterp".to_string(),
                    Value::Object(vec![
                        (
                            "m_nType".to_string(),
                            Value::String("PF_TYPE_COLLECTION_AGE".to_string()),
                        ),
                        (
                            "m_nMapType".to_string(),
                            Value::String("PF_MAP_TYPE_DIRECT".to_string()),
                        ),
                        (
                            "m_nInputMode".to_string(),
                            Value::String("PF_INPUT_MODE_LOOPED".to_string()),
                        ),
                        ("m_flInput0".to_string(), Value::Double(0.0)),
                        ("m_flInput1".to_string(), Value::Double(cycle_seconds)),
                        ("m_flOutput0".to_string(), Value::Double(0.0)),
                        ("m_flOutput1".to_string(), Value::Double(1.0)),
                    ]),
                ),
                (
                    "m_Gradient".to_string(),
                    Value::Object(vec![("m_Stops".to_string(), Value::Array(stops))]),
                ),
            ]),
        ),
    ])
}

fn gradient_stop(position: f32, hue: f64, sv: (f64, f64)) -> Value {
    let rgb = hsv_to_rgb_i64(hue, sv.0, sv.1);
    Value::Object(vec![
        (
            "m_flPosition".to_string(),
            Value::Double(f64::from(position)),
        ),
        (
            "m_Color".to_string(),
            Value::Array(rgb.into_iter().map(Value::Int).collect()),
        ),
    ])
}

/// Collect the string edits that loop a particle's age-driven color gradients. For
/// every object carrying both a non-empty `m_Gradient/m_Stops` and an `m_FloatInterp`
/// driver whose `m_nType` is an age input, set the driver's `m_nInputMode` to
/// `PF_INPUT_MODE_LOOPED` (unless already looped) and, when it reads per-particle age,
/// retarget it to `PF_TYPE_COLLECTION_AGE` so the whole effect cycles together rather
/// than each particle flickering over its short life.
fn collect_loop_edits(v: &Value, path: &mut Vec<Seg>, edits: &mut Vec<(Vec<Seg>, String)>) {
    match v {
        Value::Object(pairs) => {
            let has_gradient = v
                .get("m_Gradient")
                .and_then(|g| g.get("m_Stops"))
                .and_then(Value::as_array)
                .is_some_and(|s| !s.is_empty());
            if has_gradient {
                if let Some(interp) = v.get("m_FloatInterp") {
                    let kind = interp.get("m_nType").and_then(Value::as_str);
                    let is_age = matches!(
                        kind,
                        Some("PF_TYPE_COLLECTION_AGE" | "PF_TYPE_PARTICLE_AGE_NORMALIZED")
                    );
                    if is_age {
                        let already_looped = interp.get("m_nInputMode").and_then(Value::as_str)
                            == Some("PF_INPUT_MODE_LOOPED");
                        if !already_looped {
                            let mut p = path.clone();
                            p.push(Seg::Key("m_FloatInterp".to_string()));
                            p.push(Seg::Key("m_nInputMode".to_string()));
                            edits.push((p, "PF_INPUT_MODE_LOOPED".to_string()));
                        }
                        if kind == Some("PF_TYPE_PARTICLE_AGE_NORMALIZED") {
                            let mut p = path.clone();
                            p.push(Seg::Key("m_FloatInterp".to_string()));
                            p.push(Seg::Key("m_nType".to_string()));
                            edits.push((p, "PF_TYPE_COLLECTION_AGE".to_string()));
                        }
                    }
                }
            }
            for (k, child) in pairs {
                path.push(Seg::Key(k.clone()));
                collect_loop_edits(child, path, edits);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                path.push(Seg::Index(i));
                collect_loop_edits(item, path, edits);
                path.pop();
            }
        }
        _ => {}
    }
}

/// A particle texture input's UV offset can be safely driven by particle age only
/// when the sampled texture is a *tiling / continuous* type (a beam, a noise field,
/// a caustic, a gradient ramp): scrolling its UV wraps seamlessly. A *sprite-sheet /
/// flipbook / flare* texture is laid out as discrete cells, so scrolling its UV
/// crosses into the neighbouring cell and reveals a hard square edge: the artifact
/// that broke Yamato's Power Slash when the animated-prism pass drove every offset.
///
/// Given a decoded `.vpcf_c` tree, this returns the
/// `m_Renderers[i]/m_vecTexturesInput[j]` path prefixes whose offset controls an
/// animation pass must leave alone. It is deliberately *conservative* (default-deny):
/// an input whose `m_hTexture` is missing, non-string, or not on the tiling allowlist
/// is treated as non-tiling, so the pass never produces a square. Gradient-stop
/// retiming is independent of this (it changes color timing, not UV) and stays
/// unconstrained.
///
/// The allowlist ([`is_tiling_particle_texture`]) is name-based against the canonical
/// `materials/particle/` tiling families seen in the Deadlock pak: every
/// offset-animated Yamato input resolved to a `beam_*` or `noise_*` texture, and the
/// lone `particle_flare_*` sprite is exactly what this skips. The authoritative
/// successor is a `morphic` sprite-sheet (SHTS) reader that resolves
/// `m_hTexture -> .vtex_c` and checks the real sequence count; this is the tree-only
/// approximation until that lands.
#[must_use]
pub fn non_tiling_texture_inputs(tree: &Value) -> Vec<Vec<Seg>> {
    let mut out = Vec::new();
    let Some(renderers) = tree.get("m_Renderers").and_then(Value::as_array) else {
        return out;
    };
    for (ri, renderer) in renderers.iter().enumerate() {
        let Some(inputs) = renderer.get("m_vecTexturesInput").and_then(Value::as_array) else {
            continue;
        };
        for (ii, input) in inputs.iter().enumerate() {
            let tiling = input
                .get("m_hTexture")
                .and_then(Value::as_str)
                .is_some_and(is_tiling_particle_texture);
            if !tiling {
                out.push(vec![
                    Seg::Key("m_Renderers".to_string()),
                    Seg::Index(ri),
                    Seg::Key("m_vecTexturesInput".to_string()),
                    Seg::Index(ii),
                ]);
            }
        }
    }
    out
}

/// Whether a particle `m_hTexture` path names a tiling / continuous texture whose UV
/// offset can be animated without revealing a sprite-sheet cell edge. Matched on the
/// basename against the canonical Source 2 tiling families (beams, noise, caustics,
/// scrolls, gradients). See [`non_tiling_texture_inputs`] for why this is an allowlist.
#[must_use]
pub fn is_tiling_particle_texture(h_texture: &str) -> bool {
    // These roots name particle textures authored to tile / scroll seamlessly; a
    // sprite sheet, flipbook, or flare (which reveal a cell edge when scrolled) match
    // none of them and so are treated as non-tiling.
    const TILING: &[&str] = &[
        "beam", "noise", "caustic", "voronoi", "scroll", "streak", "flow", "tiled", "warp",
        "perlin", "ramp", "gradient",
    ];
    // Falloff art masquerading as tiling: beam_edge / beam_smoke / *_mask /
    // *_shape / *_soft strips tile along the beam axis only. Offsetting across
    // the falloff wraps the gradient into a hard seam (the squared-off Rem
    // Helping Hand heal ring), so these stay non-tiling even when a TILING
    // root also matches.
    const FALLOFF: &[&str] = &["edge", "mask", "shape", "smoke", "soft"];
    let name = h_texture
        .rsplit('/')
        .next()
        .unwrap_or(h_texture)
        .to_ascii_lowercase();
    if FALLOFF.iter().any(|root| name.contains(root)) {
        return false;
    }
    TILING.iter().any(|root| name.contains(root))
}

/// Recolor one `.vpcf_c`'s color params to `hue_deg` in place, returning the new
/// bytes, or `None` when the file carries no color param to change.
///
/// Decodes the KV3 value tree, collects an in-place scalar edit for every
/// color/tint-keyed integer array (length 3-4, values 0-255), and applies them
/// with [`morphic::patch_kv3_resource_scalars`] (which preserves the KV3 v5
/// framing, value flags, and typed-array tags a full re-encode would strip,
/// breaking the particle's resource references). Promoted from the in-game-proven
/// `examples/recolor_particles.rs`.
pub fn recolor_particle_bytes(vpcf_bytes: &[u8], recolor: Recolor) -> Result<Option<Vec<u8>>> {
    let value = morphic::decode_kv3_resource(vpcf_bytes)
        .map_err(|e| anyhow::anyhow!("decoding particle KV3: {e}"))?;
    let mut edits: Vec<(Vec<Seg>, i64)> = Vec::new();
    collect_color_edits(&value, &mut Vec::new(), false, recolor, &mut edits);
    if edits.is_empty() {
        return Ok(None);
    }
    let new_bytes = morphic::patch_kv3_resource_scalars(vpcf_bytes, &edits)
        .map_err(|e| anyhow::anyhow!("patching particle color scalars: {e}"))?;
    Ok(Some(new_bytes))
}

/// Recolor a material's `g_vColorTint*` / `g_vSelfIllumTint*` constants (and, on a
/// material that draws one, a chromatic `g_vSolidOutlineTint`) by **stamping** them
/// with the brand hue (`recolor`'s hue at its saturation, via
/// [`crate::recolor::stamp_rgb`]), returning the new bytes, or `None` if the
/// material has no such tint param.
///
/// The third color carrier (after particle params and color textures): an ability
/// effect's color is a flat tint constant (an RGBA `f64` vector). The reference
/// recolor mods stamp ONE brand hue on the effect tints, including neutral white
/// ones (e.g. an additive glow's white `g_vColorTint`). A hue-only,
/// saturation-preserving recolor can't colorize a white tint, so this stamps the
/// absolute hue instead. Each param keeps its own brightness: the stamped HSV value
/// is the source tint's max RGB component (times `recolor.value`), so a white tint
/// becomes the full brand color but a deliberately dim one (a `(0.28, 0.16, 0.09)`
/// glow under a large `g_flSelfIllumScale`) stays dim instead of blowing out. Which
/// tints are stamped is decided by [`should_stamp_tint`]: the emissive
/// `g_vSelfIllumTint` always, but a neutral base `g_vColorTint` on a solid
/// (non-additive) material is left alone so the prop body stays its own color and
/// only its glow recolors.
///
/// Two write paths, chosen so the result is engine-loadable:
/// - When every stamped channel is a stored `DOUBLE`, the change is a byte-faithful
///   in-place double patch ([`morphic::patch_kv3_resource_doubles`]), which also
///   handles a blobbed material (re-emitted still compressed). Every tint vector in
///   the live pak is a typed `DOUBLE` array, so this is the path real materials take.
/// - When a channel is a tagless `DOUBLE_ZERO`/`DOUBLE_ONE` (a neutral 0.0/1.0 with
///   no stored bytes), it cannot be patched in place; the material is re-encoded
///   with [`morphic::encode_kv3_resource_like`], which keeps Valve's wire shape,
///   LZ4 and per-blob framing and promotes only the changed channel to a stored
///   double.
pub fn recolor_material_color_bytes(
    vmat_bytes: &[u8],
    recolor: Recolor,
) -> Result<Option<Vec<u8>>> {
    let value = morphic::decode_kv3_resource(vmat_bytes)
        .map_err(|e| anyhow::anyhow!("decoding material KV3: {e}"))?;
    if value
        .get("m_vectorParams")
        .and_then(Value::as_array)
        .is_none()
    {
        return Ok(None);
    }

    let edits = stamp_tint_edits(&value, recolor);
    if edits.is_empty() {
        return Ok(None);
    }

    // Byte-faithful in-place stamp (every channel is a stored double). Handles a
    // blobbed material via the compressed re-emit.
    if let Ok(new_bytes) = morphic::patch_kv3_resource_doubles(vmat_bytes, &edits) {
        return Ok(Some(new_bytes));
    }

    // A neutral channel (tagless 0.0/1.0) has no stored bytes to patch, so the
    // in-place patch reports it as "not found". Promote it by re-encoding.
    let mut tree = value;
    stamp_tint_tree(&mut tree, recolor);
    let new_bytes = morphic::encode_kv3_resource_like(vmat_bytes, &tree)
        .map_err(|e| anyhow::anyhow!("re-encoding material to stamp tint: {e}"))?;
    Ok(Some(new_bytes))
}

/// Every stampable tint param of a material (see [`should_stamp_tint`]) as
/// `(index into m_vectorParams, linear RGB to stamp)`. Each target is the brand hue
/// at that param's own brightness, its source max RGB component.
fn stamp_targets(value: &Value, recolor: Recolor) -> Vec<(usize, [f64; 3])> {
    let additive = material_is_additive_or_unlit(value);
    let solid_outline = material_flag_set(value, &["F_SOLID_COLOR_OUTLINE"]);
    let Some(params) = value.get("m_vectorParams").and_then(Value::as_array) else {
        return Vec::new();
    };
    params
        .iter()
        .enumerate()
        .filter(|(_, param)| should_stamp_tint(param, additive, solid_outline))
        .filter_map(|(i, param)| {
            let rgba = param.get("m_value").and_then(Value::as_array)?;
            let brightness = rgba
                .iter()
                .take(3)
                .filter_map(Value::as_f64)
                .fold(0.0, f64::max);
            Some((i, crate::recolor::stamp_rgb(recolor, brightness)))
        })
        .collect()
}

/// The set of in-place double edits to stamp every stampable tint RGB channel that
/// differs from its target (see [`stamp_targets`]). A channel already equal to its
/// target is skipped (no-op); alpha (index 3) is never touched.
fn stamp_tint_edits(value: &Value, recolor: Recolor) -> Vec<(Vec<Seg>, f64)> {
    let mut edits = Vec::new();
    let Some(params) = value.get("m_vectorParams").and_then(Value::as_array) else {
        return edits;
    };
    for (i, target) in stamp_targets(value, recolor) {
        let Some(rgba) = params[i].get("m_value").and_then(Value::as_array) else {
            continue;
        };
        for (k, &t) in target.iter().enumerate() {
            let Some(orig) = rgba.get(k).and_then(Value::as_f64) else {
                continue;
            };
            if (orig - t).abs() <= f64::EPSILON {
                continue; // already the target (e.g. a tint whose R is already 1.0)
            }
            edits.push((
                vec![
                    Seg::Key("m_vectorParams".to_string()),
                    Seg::Index(i),
                    Seg::Key("m_value".to_string()),
                    Seg::Index(k),
                ],
                t,
            ));
        }
    }
    edits
}

/// Set the RGB of every stampable tint param's `m_value` to its target (see
/// [`stamp_targets`]) directly in the decoded tree (the re-encode path, which can
/// promote a tagless 0.0/1.0 to a real double).
fn stamp_tint_tree(value: &mut Value, recolor: Recolor) {
    let targets = stamp_targets(value, recolor);
    let Some(Value::Array(params)) = value.get_mut("m_vectorParams") else {
        return;
    };
    for (i, target) in targets {
        if let Some(Value::Array(rgba)) = params[i].get_mut("m_value") {
            for (k, &t) in target.iter().enumerate() {
                if let Some(ch) = rgba.get_mut(k) {
                    *ch = Value::Double(t);
                }
            }
        }
    }
}

/// Whether a tint vector param should be stamped with the brand hue.
///
/// `g_vSelfIllumTint*` (the emissive glow color) is always stamped. `g_vColorTint*`
/// (the base/albedo tint) is stamped only when the material is additive/unlit (the
/// base tint IS the visible effect color, e.g. an additive glow) or when it already
/// carries a real color. A **neutral white/gray base on a solid material** is left
/// alone, so a recolor tints the prop's GLOW (self-illum), not the whole prop: the
/// gravestone stone keeps its vanilla color while its skull / R.I.P. / cracks glow
/// recolors. `g_vSolidOutlineTint` is stamped only when the material draws a solid
/// color outline (`F_SOLID_COLOR_OUTLINE`) and the outline has a real color, so a
/// recolored prop does not keep a vanilla-orange rim while the usual neutral dark
/// outline stays as authored.
fn should_stamp_tint(param: &Value, additive: bool, solid_outline: bool) -> bool {
    let Some(name) = param.get("m_name").and_then(Value::as_str) else {
        return false;
    };
    if name.starts_with("g_vSelfIllumTint") {
        return true;
    }
    let colored = param
        .get("m_value")
        .and_then(Value::as_array)
        .is_some_and(|rgba| !is_neutral_rgb(rgba));
    if name == "g_vSolidOutlineTint" {
        return solid_outline && colored;
    }
    name.starts_with("g_vColorTint") && (additive || colored)
}

/// A material flagged `F_ADDITIVE_BLEND` or `F_UNLIT` (an additive/unlit effect,
/// where the base color tint is the visible color rather than a multiply over a
/// solid albedo).
fn material_is_additive_or_unlit(value: &Value) -> bool {
    material_flag_set(value, &["F_ADDITIVE_BLEND", "F_UNLIT"])
}

/// Whether any of the named `m_intParams` shader flags is non-zero.
fn material_flag_set(value: &Value, flags: &[&str]) -> bool {
    value
        .get("m_intParams")
        .and_then(Value::as_array)
        .is_some_and(|ints| {
            ints.iter().any(|p| {
                p.get("m_name")
                    .and_then(Value::as_str)
                    .is_some_and(|n| flags.contains(&n))
                    && p.get("m_nValue").and_then(Value::as_int).unwrap_or(0) != 0
            })
        })
}

/// True when a tint's RGB is neutral (white/gray: r == g == b within a small
/// tolerance), i.e. it carries no color of its own.
fn is_neutral_rgb(rgba: &[Value]) -> bool {
    let chan = |k: usize| rgba.get(k).and_then(Value::as_f64);
    match (chan(0), chan(1), chan(2)) {
        (Some(r), Some(g), Some(b)) => (r - g).abs() < 1e-6 && (g - b).abs() < 1e-6,
        _ => false,
    }
}

/// If `v` is a numeric array of length 3-4 in Color32 range, return its RGB ints.
fn as_color(v: &Value) -> Option<[i64; 3]> {
    let Value::Array(items) = v else {
        return None;
    };
    if items.len() != 3 && items.len() != 4 {
        return None;
    }
    let mut ch = [0i64; 3];
    for (i, it) in items.iter().enumerate() {
        let n = match it {
            Value::Int(n) if (0..=255).contains(n) => *n,
            Value::UInt(u) if *u <= 255 => i64::try_from(*u).unwrap_or(0),
            _ => return None,
        };
        if i < 3 {
            ch[i] = n;
        }
    }
    Some(ch)
}

/// Apply the shared color set to a Color32 RGB triple (set hue, then scale its
/// saturation and brightness), so a particle param lands on the exact same color
/// as the texture/model recolor.
fn recolored(rgb: [i64; 3], recolor: Recolor) -> [i64; 3] {
    let inp = [
        clamp_channel(rgb[0]),
        clamp_channel(rgb[1]),
        clamp_channel(rgb[2]),
    ];
    let out = set_color(inp, recolor.hue, recolor.saturation, recolor.value);
    [i64::from(out[0]), i64::from(out[1]), i64::from(out[2])]
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_channel(n: i64) -> u8 {
    n.clamp(0, 255) as u8
}

fn collect_rainbow_support_stats(v: &Value, colorish: bool, report: &mut HeroRainbowSupportReport) {
    if colorish {
        if let Some(rgb) = as_color(v) {
            report.color_fields += 1;
            if rgb.iter().copied().max().unwrap_or(0) > 1 {
                report.visible_color_fields += 1;
            }
            return; // a color array has no colorish children
        }
    }

    match v {
        Value::Object(pairs) => {
            if let Some(class) = v.get("_class").and_then(Value::as_str) {
                match class {
                    "C_INIT_RandomColor" => report.random_color_initializers += 1,
                    "C_OP_ColorInterpolate" => report.color_interpolate_ops += 1,
                    _ => {}
                }
            }

            if let Some(input_type) = v.get("m_nType").and_then(Value::as_str) {
                match input_type {
                    "PF_TYPE_COLLECTION_AGE" => report.collection_age_inputs += 1,
                    "PF_TYPE_PARTICLE_AGE_NORMALIZED" => report.particle_age_inputs += 1,
                    _ => {}
                }
            }
            if matches!(
                v.get("m_nInputMode").and_then(Value::as_str),
                Some("PF_INPUT_MODE_LOOPED")
            ) {
                report.looped_inputs += 1;
            }

            if let Some(stops) = v
                .get("m_Gradient")
                .and_then(|g| g.get("m_Stops"))
                .and_then(Value::as_array)
                .filter(|stops| !stops.is_empty())
            {
                report.gradient_fields += 1;
                report.gradient_stops += stops.len();
                if stops.len() > 1 {
                    report.multi_stop_gradient_fields += 1;
                }
                if let Some(interp) = v.get("m_FloatInterp") {
                    match interp.get("m_nType").and_then(Value::as_str) {
                        Some("PF_TYPE_COLLECTION_AGE") => {
                            report.collection_age_gradient_fields += 1;
                        }
                        Some("PF_TYPE_PARTICLE_AGE_NORMALIZED") => {
                            report.particle_age_gradient_fields += 1;
                        }
                        _ => {}
                    }
                    if matches!(
                        interp.get("m_nInputMode").and_then(Value::as_str),
                        Some("PF_INPUT_MODE_LOOPED")
                    ) {
                        report.looped_gradient_fields += 1;
                    }
                }
            }

            for (k, child) in pairs {
                let kl = k.to_lowercase();
                let c = kl.contains("color") || kl.contains("tint");
                collect_rainbow_support_stats(child, c, report);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_rainbow_support_stats(item, false, report);
            }
        }
        _ => {}
    }
}

/// Walk the value tree, building scalar edits for color channels. `path` is the
/// path to the current value; `colorish` is true when reached via a color/tint
/// key. Mirrors `examples/recolor_particles.rs` (the in-game-proven walk).
fn collect_color_edits(
    v: &Value,
    path: &mut Vec<Seg>,
    colorish: bool,
    recolor: Recolor,
    edits: &mut Vec<(Vec<Seg>, i64)>,
) {
    if colorish {
        if let Some(rgb) = as_color(v) {
            let new = recolored(rgb, recolor);
            for (i, &nv) in new.iter().enumerate() {
                if nv != rgb[i] {
                    let mut p = path.clone();
                    p.push(Seg::Index(i));
                    edits.push((p, nv));
                }
            }
            return; // a color array has no colorish children
        }
    }
    match v {
        Value::Object(pairs) => {
            for (k, child) in pairs {
                let kl = k.to_lowercase();
                let c = kl.contains("color") || kl.contains("tint");
                path.push(Seg::Key(k.clone()));
                collect_color_edits(child, path, c, recolor, edits);
                path.pop();
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                path.push(Seg::Index(i));
                collect_color_edits(item, path, false, recolor, edits);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Open the VPKs in resolution priority order: `vpk` first (a skin's overrides
/// win), then the base pak. Mirrors `model::open_vpks`.
fn open_vpks(vpk: &Path, base: Option<&Path>) -> Result<Vec<valve_pak::VPK>> {
    let mut vpks =
        vec![valve_pak::open(vpk).with_context(|| format!("opening {}", vpk.display()))?];
    if let Some(base) = base {
        vpks.push(valve_pak::open(base).with_context(|| format!("opening {}", base.display()))?);
    }
    Ok(vpks)
}

/// Read a VPK entry from the first of `vpks` that contains it.
fn read_entry(vpks: &[valve_pak::VPK], entry: &str) -> Option<Vec<u8>> {
    for vpk in vpks {
        if let Ok(mut vf) = vpk.get_file(entry) {
            if let Ok(bytes) = vf.read_all() {
                return Some(bytes);
            }
        }
    }
    None
}

/// Every entry across `vpks` that ends with `suffix` and starts with any of
/// `prefixes`, de-duplicated and sorted (so a skin override and the base copy of
/// the same path are listed once; [`read_entry`] resolves which wins).
fn list_entries(vpks: &[valve_pak::VPK], prefixes: &[String], suffix: &str) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for vpk in vpks {
        for p in vpk.file_paths() {
            if p.ends_with(suffix) && prefixes.iter().any(|pre| p.starts_with(pre.as_str())) {
                seen.insert(p.clone());
            }
        }
    }
    seen.into_iter().collect()
}

/// The particles a bake walks: everything under the recipe prefixes plus the
/// shared particles listed outside them (those are only ever written as copies).
pub(crate) fn recipe_particle_entries(
    vpks: &[valve_pak::VPK],
    recipe: &HeroRecolorRecipe,
) -> Vec<String> {
    let mut all = list_entries(vpks, &recipe.particle_prefixes, ".vpcf_c");
    all.extend(list_entries(vpks, &recipe.shared_entries, ".vpcf_c"));
    all.sort();
    all.dedup();
    all
}

/// Moves a bake's changed shared entries to hero-local copies and repoints the
/// hero's own particles and pinned materials at them (see [`crate::recolor_isolate`]).
pub(crate) fn isolate_bake(
    vpks: &[valve_pak::VPK],
    recipe: &HeroRecolorRecipe,
    particles: &[String],
    packed: BTreeMap<String, Vec<u8>>,
) -> Result<(BTreeMap<String, Vec<u8>>, IsolationStats)> {
    let candidates: Vec<String> = particles
        .iter()
        .chain(&recipe.material_entries)
        .cloned()
        .collect();
    crate::recolor_isolate::isolate(
        recipe,
        &candidates,
        packed,
        &|e| read_entry(vpks, e),
        &resource_refs,
        &|bytes, map| {
            morphic::repoint_resource_refs(bytes, map)
                .map(|(b, _)| b)
                .map_err(|e| anyhow::anyhow!("repointing references: {e}"))
        },
    )
}

/// The compiled entries a resource's `RERL` names (`x.vpcf` -> `x.vpcf_c`).
fn resource_refs(bytes: &[u8]) -> Vec<String> {
    morphic::resource::Resource::parse(bytes)
        .ok()
        .and_then(|r| r.find_block(*b"RERL").map(morphic::resource::read_rerl))
        .and_then(Result::ok)
        .map(|refs| refs.into_iter().map(|(_, n)| format!("{n}_c")).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paige_recipe_is_pinned() {
        let r = recipe_for("bookworm").expect("paige recipe");
        assert_eq!(r.codename, "bookworm");
        assert_eq!(r.particle_prefixes.len(), 2);
        assert_eq!(r.texture_entries.len(), 12);
        assert_eq!(r.material_entries.len(), 6);
        assert_eq!(r.model_entries.len(), 2);
        // the preview swatch must be one of the recipe's real texture entries
        let preview = r.preview_texture.expect("paige has a preview texture");
        assert!(r.texture_entries.contains(&preview));
        // codename lookup is case-insensitive
        assert!(recipe_for("BOOKWORM").is_some());
        assert!(recipe_for("not_a_hero").is_none());
    }

    #[test]
    fn celeste_recipe_adds_projected_ground_textures() {
        let r = recipe_for("unicorn").expect("celeste recipe");
        assert_eq!(r.codename, "unicorn");
        assert_eq!(
            r.particle_prefixes,
            [
                "particles/abilities/unicorn/",
                "particles/weapon_fx/unicorn/"
            ]
        );
        assert_eq!(r.texture_entries.len(), 7);
        assert!(r
            .texture_entries
            .iter()
            .any(|t| t.contains("unicorn_radiant_flare_ground_projected")));
        assert!(r
            .texture_entries
            .iter()
            .any(|t| t.contains("unicorn_prismatic_shield_ground_warning")));
        assert!(r.model_entries.is_empty(), "{:?}", r.model_entries);
        assert!(r
            .preview_texture
            .as_deref()
            .is_some_and(|t| r.texture_entries.contains(&t.to_string())));
        // codename lookup is case-insensitive
        assert!(recipe_for("UNICORN").is_some());
    }

    #[test]
    fn inferno_recipe_pins_the_body_glow_layers() {
        // The flame hair and arm glow are separate additive materials on the live
        // body model; inferno_body stays vanilla (its tint does not color his fire).
        let r = recipe_for("inferno").expect("inferno recipe");
        assert_eq!(r.codename, "inferno");
        assert_eq!(
            r.particle_prefixes,
            [
                "particles/abilities/inferno/",
                "particles/weapon_fx/inferno/",
                "particles/status_fx/status_fx_inferno_napalm_debuff.vpcf_c",
            ]
        );
        assert_eq!(
            r.material_entries,
            [
                "models/heroes_staging/inferno_v4/materials/inferno_armglow.vmat_c",
                "models/heroes_staging/inferno_v4/materials/inferno_headglow.vmat_c",
            ]
        );
        assert_eq!(r.texture_entries.len(), 4, "{:?}", r.texture_entries);
        assert!(r
            .texture_entries
            .iter()
            .all(|t| t.starts_with("models/heroes_staging/inferno_v4/materials/inferno_")));
        assert!(r.model_entries.is_empty(), "{:?}", r.model_entries);
        assert!(r
            .preview_texture
            .as_deref()
            .is_some_and(|t| r.texture_entries.contains(&t.to_string())));
        assert!(recipe_for("INFERNO").is_some());
    }

    #[test]
    fn particle_only_heroes_are_pinned() {
        // Particle-only heroes have the standard two particle roots and no texture,
        // material, or model extras. Hue is supplied at recolor time, so the recipe
        // itself carries no color.
        for code in ["hornet", "mirage", "shiv", "warden", "wraith"] {
            let r = recipe_for(code).unwrap_or_else(|| panic!("recipe for {code}"));
            assert_eq!(r.codename, code);
            assert_eq!(
                r.particle_prefixes,
                [
                    format!("particles/abilities/{code}/"),
                    format!("particles/weapon_fx/{code}/"),
                ]
            );
            assert!(
                r.texture_entries.is_empty(),
                "{code} should be particle-only"
            );
            assert!(
                r.material_entries.is_empty(),
                "{code} should be particle-only"
            );
            assert!(r.model_entries.is_empty(), "{code} should be particle-only");
            assert!(r.preview_texture.is_none());
        }
        // case-insensitive, and a hero with no pinned recipe still returns None
        assert!(recipe_for("GIGAWATT").is_some());
        assert!(recipe_for("not_a_hero").is_none());
    }

    #[test]
    fn disabled_audited_texture_recipes_are_pinned() {
        // These heroes were added from the local disabled-mod particle scan plus
        // `examples/recolor_assets.rs`. Each has the standard particle roots and
        // at least one hero-specific chromatic texture; shared/default textures
        // from the audit are deliberately excluded.
        for (code, texture_count, material_count, required_marker) in [
            ("fencer", 7, 2, "fencer_ult_gradient_color"),
            ("ghost", 3, 0, "ghost2_clothes_fx_prop_color"),
            ("nano", 2, 0, "nano_ult_ground_dark_proj"),
            ("lash", 2, 0, "lash_cable_material"),
            ("mcginnis", 2, 0, "mcginnis_turret_ambient_goo"),
            ("magician", 2, 2, "magician_hex_ground_projected"),
            ("pocket", 4, 2, "pocket_magic_missile_illum"),
        ] {
            let r = recipe_for(code).unwrap_or_else(|| panic!("recipe for {code}"));
            assert_eq!(r.codename, code);
            assert!(r
                .particle_prefixes
                .contains(&format!("particles/abilities/{code}/")));
            assert!(r
                .particle_prefixes
                .contains(&format!("particles/weapon_fx/{code}/")));
            assert_eq!(r.texture_entries.len(), texture_count, "{code} textures");
            assert!(r
                .texture_entries
                .iter()
                .any(|t| t.contains(required_marker)));
            let preview = r
                .preview_texture
                .unwrap_or_else(|| panic!("{code} has a preview texture"));
            assert!(r.texture_entries.contains(&preview));
            assert_eq!(r.material_entries.len(), material_count, "{code} materials");
            assert!(r.model_entries.is_empty(), "{:?}", r.model_entries);
            assert!(recipe_for(&code.to_uppercase()).is_some());
        }
    }

    #[test]
    fn abrams_recipe_covers_the_bull_namespace_migration() {
        // Abrams' asset basename is migrating to `bull` (his `hero_atlas`
        // record points card art at `bull_card`; charge/leap/passive particles
        // and all weapon FX moved to the `bull` dirs in the 2026-06-11 update,
        // leaving `weapon_fx/abrams/` empty). The recipe must straddle both.
        let r = recipe_for("abrams").expect("abrams recipe");
        assert_eq!(r.codename, "abrams");
        assert_eq!(
            r.particle_prefixes,
            [
                "particles/abilities/abrams/",
                "particles/abilities/bull/",
                "particles/weapon_fx/bull/",
            ]
        );
        assert_eq!(r.texture_entries.len(), 2);
        assert!(r
            .texture_entries
            .iter()
            .any(|t| t.contains("abrams_leap_ground_impact")));
        let preview = r.preview_texture.expect("abrams has a preview texture");
        assert!(r.texture_entries.contains(&preview));
        assert!(r.material_entries.is_empty(), "{:?}", r.material_entries);
        assert!(r.model_entries.is_empty(), "{:?}", r.model_entries);
        assert!(recipe_for("ABRAMS").is_some());
    }

    #[test]
    fn bebop_laser_projected_decals_stay_vanilla_for_prism() {
        for entry in [
            "particles/abilities/bebop/bebop_laser_beam_proj_ground.vpcf_c",
            "particles/abilities/bebop/bebop_laser_beam_end_proj_ground.vpcf_c",
            "particles/abilities/bebop/bebop_laser_beam_end_proj_surface.vpcf_c",
        ] {
            assert!(prism_particle_passthrough("bebop", entry));
        }
        assert!(!prism_particle_passthrough(
            "bebop",
            "particles/abilities/bebop/bebop_laser_beam.vpcf_c"
        ));
        assert!(!prism_particle_passthrough(
            "dynamo",
            "particles/abilities/bebop/bebop_laser_beam_proj_ground.vpcf_c"
        ));
    }

    #[test]
    fn roster_texture_recipes_are_pinned() {
        // Added from the local `pak01` file-tree + ability texture audit. These
        // cover remaining selectable roster namespaces whose particles reference
        // hero-specific chromatic textures. Body albedos/shared defaults are
        // deliberately excluded from the counts.
        for (code, texture_count, material_count, required_marker) in [
            ("archer", 4, 1, "archer_guided_arrow_color"),
            ("digger", 1, 0, "digger_burrow_explode_ground"),
            ("doorman", 1, 7, "doorman_bell_vmat_g_tcolor"),
            ("drifter", 2, 0, "drifter_claw_ground_projected"),
            ("dynamo", 2, 3, "sumo_singularity_ripple"),
            ("familiar", 4, 0, "familiar_spotlight_ground_projected"),
            ("frank", 3, 0, "frank_painaura_model_fade_color"),
            ("haze", 1, 1, "haze_tracer_self_illum"),
            ("kelvin", 2, 0, "kelvin_ice_dome_projected"),
            ("priest", 2, 0, "priest_snaptrap_projectile_aoe"),
            ("tengu", 4, 1, "ivy_entangling_thorns"),
            ("viper", 1, 1, "viper_petrify_symbol"),
            ("viscous", 12, 13, "viscous_ball_vmat_g_tcolor"),
            ("werewolf", 3, 0, "werewolf_cripplingslash_ground"),
        ] {
            let r = recipe_for(code).unwrap_or_else(|| panic!("recipe for {code}"));
            assert_eq!(r.codename, code);
            assert!(r
                .particle_prefixes
                .contains(&format!("particles/abilities/{code}/")));
            assert!(r
                .particle_prefixes
                .contains(&format!("particles/weapon_fx/{code}/")));
            assert_eq!(r.texture_entries.len(), texture_count, "{code} textures");
            assert!(r
                .texture_entries
                .iter()
                .any(|t| t.contains(required_marker)));
            let preview = r
                .preview_texture
                .unwrap_or_else(|| panic!("{code} has a preview texture"));
            assert!(r.texture_entries.contains(&preview));
            assert_eq!(r.material_entries.len(), material_count, "{code} materials");
            assert!(r.model_entries.is_empty(), "{:?}", r.model_entries);
            assert!(recipe_for(&code.to_uppercase()).is_some());
        }
    }

    #[test]
    fn current_selectable_roster_namespaces_are_pinned() {
        // Current local `scripts/heroes.vdata_c` selectable + not disabled/dev
        // namespaces, normalized to the particle codename used by the VFX tree.
        for code in [
            "abrams",
            "archer",
            "astro",
            "bebop",
            "bookworm",
            "chrono",
            "digger",
            "doorman",
            "drifter",
            "dynamo",
            "familiar",
            "fencer",
            "frank",
            "ghost",
            "gigawatt",
            "haze",
            "hornet",
            "inferno",
            "kelvin",
            "lash",
            "magician",
            "mcginnis",
            "mirage",
            "nano",
            "necro",
            "pocket",
            "priest",
            "punkgoat",
            "shiv",
            "tengu",
            "unicorn",
            "vampirebat",
            "viper",
            "viscous",
            "warden",
            "werewolf",
            "wraith",
            "yamato",
        ] {
            assert!(recipe_for(code).is_some(), "missing recipe for {code}");
        }
    }

    #[test]
    fn yamato_recipe_adds_status_particles_and_chromatic_textures() {
        let r = recipe_for("yamato").expect("yamato recipe");
        assert_eq!(r.codename, "yamato");
        assert_eq!(
            r.particle_prefixes,
            [
                "particles/abilities/yamato/",
                "particles/weapon_fx/yamato/",
                "particles/status_fx/status_fx_yamato",
            ]
        );
        assert_eq!(r.texture_entries.len(), 3);
        assert!(r
            .texture_entries
            .iter()
            .any(|t| t.contains("yamato_blade_dash_ground_projected")));
        assert!(r
            .texture_entries
            .iter()
            .any(|t| t.contains("shadow_redemption_complete_status")));
        assert!(r
            .texture_entries
            .iter()
            .any(|t| t.contains("yamoto_shadow_shape_color")));
        assert!(r.material_entries.is_empty(), "{:?}", r.material_entries);
        assert!(r.model_entries.is_empty(), "{:?}", r.model_entries);
        let preview = r.preview_texture.expect("yamato has a preview texture");
        assert!(r.texture_entries.contains(&preview));
        assert!(recipe_for("YAMATO").is_some());
    }

    #[test]
    fn yamato_shadow_form_prism_texture_is_muted() {
        let r = prism_texture_recolor_for(
            "yamato",
            YAMATO_SHADOW_SHAPE_COLOR_TEXTURE,
            0.0,
            PrismTuning::default(),
        );
        assert!((r.hue - 190.0).abs() < 1e-9);
        assert!((r.saturation - 0.45).abs() < 1e-9);
        assert!((r.value - 0.72).abs() < 1e-9);

        let generic = prism_texture_recolor_for(
            "yamato",
            "materials/example.vtex_c",
            0.0,
            PrismTuning::default(),
        );
        assert!((generic.saturation - 1.0).abs() < 1e-9);
        assert!((generic.value - 1.0).abs() < 1e-9);
    }

    #[test]
    fn yamato_shadow_status_textures_use_rainbow_bands() {
        for entry in YAMATO_SHADOW_STATUS_TEXTURES {
            assert!(is_yamato_shadow_status_texture(entry));
        }
        assert!(!is_yamato_shadow_status_texture(
            YAMATO_SHADOW_SHAPE_COLOR_TEXTURE
        ));

        let dt = PrismTuning::default();
        assert_eq!(
            rainbow_status_band_rgb([255, 255, 255], 0.0, 0.0, dt),
            [255, 0, 0]
        );
        assert_eq!(
            rainbow_status_band_rgb([255, 255, 255], 1.0 / 48.0, 0.0, dt),
            [255, 255, 0]
        );
        assert_eq!(rainbow_status_band_rgb([0, 0, 0], 0.5, 0.5, dt), [0, 0, 0]);
    }

    #[test]
    fn graves_recipe_adds_glow_texture_and_tint_materials() {
        // Graves (necro): particles (incl. the held-weapon ambient flame under
        // particles/heroes/) + ability-prop color textures + the stamped effect-tint
        // materials, including the held flaming-hand prop and its aura. NOT particle-only.
        let r = recipe_for("necro").expect("necro recipe");
        assert_eq!(r.codename, "necro");
        assert_eq!(r.particle_prefixes.len(), 3);
        assert!(r
            .particle_prefixes
            .iter()
            .any(|p| p == "particles/heroes/necro/"));
        // ability-prop albedo/transmissive textures (shambler/jar/gravestone)
        assert!(r.texture_entries.len() >= 4, "ability-prop color textures");
        assert!(r.texture_entries.iter().any(|t| t.contains("shambler")));
        assert!(r.texture_entries.iter().any(|t| t.contains("jar_of_dread")));
        assert_eq!(
            r.material_entries.len(),
            7,
            "pickup sphere + jar + necro_hands + flame effect + picker hand effect + \
             2 gravestone (standing + destruction)"
        );
        assert!(r.material_entries.iter().all(|m| m.ends_with(".vmat_c")));
        // the held flaming-hand prop + the gravestone glow are present
        assert!(r
            .material_entries
            .iter()
            .any(|m| m.ends_with("necro_picker_hand_effect.vmat_c")));
        assert!(r
            .material_entries
            .iter()
            .any(|m| m.ends_with("necro_gravestone.vmat_c")));
        assert!(r.model_entries.is_empty(), "{:?}", r.model_entries);
    }

    #[test]
    fn solomon_recipe_adds_status_particles_glow_materials_and_hand_models() {
        let r = recipe_for("chessmaster").expect("chessmaster recipe");
        assert_eq!(
            r.particle_prefixes,
            [
                "particles/abilities/chessmaster/",
                "particles/weapon_fx/chessmaster/",
                "particles/status_fx/status_fx_chessmaster",
            ]
        );
        assert_eq!(r.texture_entries.len(), 9);
        assert_eq!(r.material_entries.len(), 7);
        assert!(r
            .material_entries
            .iter()
            .any(|m| m.ends_with("chessmaster_queen_stomp_impact_sphere.vmat_c")));
        assert_eq!(
            r.model_entries,
            [
                "models/heroes_wip/chessmaster/chessmaster_hand.vmdl_c",
                "models/heroes_wip/chessmaster/chessmaster_knightshield.vmdl_c",
            ]
        );
        let preview = r
            .preview_texture
            .expect("chessmaster has a preview texture");
        assert!(r.texture_entries.contains(&preview));
    }

    #[test]
    fn as_color_accepts_color32_arrays_only() {
        assert_eq!(
            as_color(&Value::Array(vec![
                Value::Int(0),
                Value::Int(255),
                Value::Int(148)
            ])),
            Some([0, 255, 148])
        );
        // RGBA: alpha is ignored, RGB returned
        assert_eq!(
            as_color(&Value::Array(vec![
                Value::Int(10),
                Value::Int(20),
                Value::Int(30),
                Value::Int(255),
            ])),
            Some([10, 20, 30])
        );
        // out of range / wrong length / non-int -> not a color
        assert_eq!(
            as_color(&Value::Array(vec![
                Value::Int(0),
                Value::Int(300),
                Value::Int(0)
            ])),
            None
        );
        assert_eq!(
            as_color(&Value::Array(vec![Value::Int(1), Value::Int(2)])),
            None
        );
    }

    #[test]
    fn recolored_matches_the_documented_purple() {
        // The same fully-saturated green -> hue 280 -> purple the in-game recolor
        // produced, identical to the texture/model `set_hue`. A hue-only recolor
        // (unit saturation + value) reproduces the original behavior.
        assert_eq!(recolored([0, 255, 148], Recolor::hue(280.0)), [170, 0, 255]);
    }

    #[test]
    fn tiling_textures_are_animatable_sprites_are_not() {
        // The real `m_hTexture` names of the inputs the Yamato animation pass would
        // drive: every beam / noise resolves tiling (safe to scroll); the lone flare
        // sprite (and any unknown) does not.
        for tiling in [
            "materials/particle/beam_hotwhite.vtex",
            "materials/particle/beam_jagged_01.vtex",
            "materials/particle/beams/beam_ethereal.vtex",
            "materials/particle/beam_liquid_viscous.vtex",
            "materials/particle/noise_gaussian.vtex",
            "materials/particle/noise/noise_voronoi_tiled/noise_voronoi_tiled_trans.vtex",
            "materials/particle/noise/noise_caustic/noise_caustic_c.vtex",
        ] {
            assert!(is_tiling_particle_texture(tiling), "{tiling} should tile");
        }
        for sprite in [
            "materials/particle/particle_flare_010.vtex",
            "materials/particle/yamato/yamato_power_slash_sheet.vtex",
            "materials/particle/symbols/rune_01.vtex",
        ] {
            assert!(
                !is_tiling_particle_texture(sprite),
                "{sprite} should be treated as a sprite (non-tiling)"
            );
        }
        // Falloff strips that tile along the beam axis only: a TILING root
        // matches their name, but scrolling across the falloff wraps the
        // gradient into a hard seam (the squared-off Rem heal ring). All from
        // real familiar_helpinghand_* renderer inputs.
        for falloff in [
            "materials/particle/beams/beam_edge_02.vtex",
            "materials/particle/beam_smoke_01.vtex",
            "materials/particle/beams/beam_fire_mask.vtex",
            "materials/particle/beams/beam_flame_shape.vtex",
            "materials/particles/lasers/beam_laser_soft_01.vtex",
        ] {
            assert!(
                !is_tiling_particle_texture(falloff),
                "{falloff} is falloff art and must not be scrolled"
            );
        }
    }

    #[test]
    fn non_tiling_inputs_flags_sprite_and_unknown_only() {
        // One renderer with three texture inputs: a tiling beam (animatable), a flare
        // sprite (must be skipped), and an input with no m_hTexture (default-deny).
        let input = |tex: Option<&str>| {
            let mut pairs = Vec::new();
            if let Some(t) = tex {
                pairs.push(("m_hTexture".to_string(), Value::String(t.to_string())));
            }
            Value::Object(pairs)
        };
        let tree = Value::Object(vec![(
            "m_Renderers".to_string(),
            Value::Array(vec![Value::Object(vec![(
                "m_vecTexturesInput".to_string(),
                Value::Array(vec![
                    input(Some("materials/particle/beam_hotwhite.vtex")),
                    input(Some("materials/particle/particle_flare_010.vtex")),
                    input(None),
                ]),
            )])]),
        )]);

        let skip = non_tiling_texture_inputs(&tree);
        let input_path = |j: usize| {
            vec![
                Seg::Key("m_Renderers".to_string()),
                Seg::Index(0),
                Seg::Key("m_vecTexturesInput".to_string()),
                Seg::Index(j),
            ]
        };
        // The beam (index 0) is animatable; the flare (1) and the textureless input
        // (2) are skipped.
        assert_eq!(skip, vec![input_path(1), input_path(2)]);
    }

    #[test]
    fn anim_plan_scales_multipliers_and_skips_falloff_inputs() {
        // Two renderer inputs, each with an offset control authored at -0.1:
        // a genuinely tiling beam (animatable) and a beam_edge falloff strip
        // (must be skipped: scrolling across the falloff wraps the gradient
        // into a hard seam, the squared-off Rem heal ring). The plan must
        // capture the authored value so the boost scales it instead of
        // stamping an absolute.
        let input = |tex: &str| {
            Value::Object(vec![
                ("m_hTexture".to_string(), Value::String(tex.to_string())),
                (
                    "m_TextureControls".to_string(),
                    Value::Object(vec![(
                        "m_flFinalTextureOffset".to_string(),
                        Value::Object(vec![
                            (
                                "m_nType".to_string(),
                                Value::String("PF_TYPE_COLLECTION_AGE".to_string()),
                            ),
                            ("m_flMultFactor".to_string(), Value::Double(-0.1)),
                        ]),
                    )]),
                ),
            ])
        };
        let tree = Value::Object(vec![(
            "m_Renderers".to_string(),
            Value::Array(vec![Value::Object(vec![(
                "m_vecTexturesInput".to_string(),
                Value::Array(vec![
                    input("materials/particle/beam_hotwhite.vtex"),
                    input("materials/particle/beams/beam_edge_02.vtex"),
                ]),
            )])]),
        )]);

        let skip = non_tiling_texture_inputs(&tree);
        let mut plan = PrismAnimPlan::default();
        collect_prism_anim_plan(&tree, &mut Vec::new(), &skip, &mut plan);

        // Only the tiling input's offset is planned, with its authored value.
        assert_eq!(plan.string_paths.len(), 1);
        assert_eq!(plan.mult_paths.len(), 1);
        let (path, original) = &plan.mult_paths[0];
        assert!(path_starts_with(
            path,
            &[
                Seg::Key("m_Renderers".to_string()),
                Seg::Index(0),
                Seg::Key("m_vecTexturesInput".to_string()),
                Seg::Index(0),
            ]
        ));
        assert!((original - -0.1).abs() < 1e-9);
        // The boost preserves the authored sign: -0.1 scales toward -0.4 at
        // full intensity, never a sign-flipped absolute like +2.5.
        let boosted = original * (1.0 + 1.5 * 1.0);
        assert!(boosted < 0.0 && (boosted - -0.25).abs() < 0.16);
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/../morphic/fixtures/material/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap_or_else(|_| panic!("read material fixture {name}"))
    }

    fn tint_rgb(bytes: &[u8], name: &str) -> [f64; 3] {
        let tree = morphic::decode_kv3_resource(bytes).expect("decode stamped material");
        let params = tree
            .get("m_vectorParams")
            .and_then(Value::as_array)
            .expect("m_vectorParams");
        let p = params
            .iter()
            .find(|p| p.get("m_name").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("no {name}"));
        let v = p.get("m_value").and_then(Value::as_array).expect("m_value");
        [
            v[0].as_f64().unwrap(),
            v[1].as_f64().unwrap(),
            v[2].as_f64().unwrap(),
        ]
    }

    fn scaled(rgb: [f64; 3], v: f64) -> [f64; 3] {
        rgb.map(|c| c * v)
    }

    fn max_rgb(rgb: [f64; 3]) -> f64 {
        rgb[0].max(rgb[1]).max(rgb[2])
    }

    fn close(got: [f64; 3], want: [f64; 3]) -> bool {
        (0..3).all(|k| (got[k] - want[k]).abs() < 1e-5)
    }

    /// `material` with the named `m_vectorParams` tint's RGB patched in place.
    fn with_tint(material: &[u8], name: &str, rgb: [f64; 3]) -> Vec<u8> {
        let tree = morphic::decode_kv3_resource(material).expect("decode material");
        let params = tree
            .get("m_vectorParams")
            .and_then(Value::as_array)
            .expect("m_vectorParams");
        let i = params
            .iter()
            .position(|p| p.get("m_name").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("no {name}"));
        let edits: Vec<_> = rgb
            .iter()
            .enumerate()
            .map(|(k, &c)| {
                (
                    vec![
                        Seg::Key("m_vectorParams".to_string()),
                        Seg::Index(i),
                        Seg::Key("m_value".to_string()),
                        Seg::Index(k),
                    ],
                    c,
                )
            })
            .collect();
        morphic::patch_kv3_resource_doubles(material, &edits).expect("patch tint")
    }

    #[test]
    fn stamps_the_brand_hue_at_each_tints_own_brightness() {
        // Hue 328 at full value is a vivid pink: hsv(328,1,1) = (1.0, 0.0, 0.533...).
        let recolor = Recolor::new(328.0, 1.0, 1.0);
        let full = [1.0, 0.0, 0.533_333_3];
        let want = |source: &[u8], name: &str| scaled(full, max_rgb(tint_rgb(source, name)));

        // 1. A neutral WHITE tint (picker_hand_glow g_vColorTint = [1,1,1]) has value
        //    1, so it ends up the full brand color.
        let source = fixture("picker_hand_glow.vmat_c");
        let glow = recolor_material_color_bytes(&source, recolor)
            .expect("stamp glow")
            .expect("glow has a tint");
        assert!(
            close(tint_rgb(&glow, "g_vColorTint1"), full),
            "white aura stamped pink"
        );

        // 2. A blobbed material with colored tints (necro_hands) stamps in place and
        //    stays a compressed, blobbed, engine-loadable block. Its tints are
        //    (0.84, 0.95, 0.15) and (1.0, 0.88, 0.13), so the pink is dimmed to 0.95
        //    and 1.0 of full.
        let source = fixture("necro_hands.vmat_c");
        let hands = recolor_material_color_bytes(&source, recolor)
            .expect("stamp hands")
            .expect("hands has a tint");
        assert!(close(
            tint_rgb(&hands, "g_vColorTint1"),
            want(&source, "g_vColorTint1")
        ));
        assert!(close(
            tint_rgb(&hands, "g_vSelfIllumTint1"),
            want(&source, "g_vSelfIllumTint1")
        ));
        assert!(max_rgb(tint_rgb(&hands, "g_vColorTint1")) < 0.96);
        assert!(
            morphic::kv3_resource_has_blobs(&hands).unwrap(),
            "necro_hands stays blobbed (in-place stamp, not flattened)"
        );

        // 3. The two-blob held-hand material (its g_vColorTint is white-but-stored,
        //    its g_vSelfIllumTint (1.0, 0.95, 0.0) full value) stamps in place too,
        //    after the blob-frame reader fix lets it decode at all.
        let held =
            recolor_material_color_bytes(&fixture("necro_picker_hand_effect.vmat_c"), recolor)
                .expect("stamp held hand")
                .expect("held hand has a tint");
        assert!(close(tint_rgb(&held, "g_vColorTint1"), full));
        assert!(close(tint_rgb(&held, "g_vSelfIllumTint1"), full));
    }

    #[test]
    fn a_dim_tint_stays_dim() {
        // A deliberately dim glow (the Rat King sewage is (0.28, 0.16, 0.09) under a
        // 7.5x self-illum scale) must not be stamped to full value, or it renders
        // several times brighter than vanilla.
        let source = with_tint(
            &fixture("picker_hand_glow.vmat_c"),
            "g_vColorTint1",
            [0.28, 0.16, 0.09],
        );
        let full = [0.666_666_7, 0.0, 1.0]; // hsv(280, 1, 1)
        let stamped = recolor_material_color_bytes(&source, Recolor::new(280.0, 1.0, 1.0))
            .expect("stamp")
            .expect("has a tint");
        assert!(close(
            tint_rgb(&stamped, "g_vColorTint1"),
            scaled(full, 0.28)
        ));

        // A neutral gray keeps its brightness too, and `recolor.value` scales it.
        let gray = with_tint(&source, "g_vColorTint1", [0.5, 0.5, 0.5]);
        let stamped = recolor_material_color_bytes(&gray, Recolor::new(280.0, 1.0, 0.5))
            .expect("stamp")
            .expect("has a tint");
        assert!(close(
            tint_rgb(&stamped, "g_vColorTint1"),
            scaled(full, 0.25)
        ));

        // A brightness lift never takes a normal tint past 1.0.
        let white = fixture("picker_hand_glow.vmat_c");
        let stamped = recolor_material_color_bytes(&white, Recolor::new(280.0, 1.0, 2.0))
            .expect("stamp")
            .expect("has a tint");
        assert!(close(tint_rgb(&stamped, "g_vColorTint1"), full));
    }

    fn blobs_of(value: &Value, out: &mut Vec<Vec<u8>>) {
        match value {
            Value::Binary(b) => out.push(b.clone()),
            Value::Array(items) => items.iter().for_each(|v| blobs_of(v, out)),
            Value::Object(members) => members.iter().for_each(|(_, v)| blobs_of(v, out)),
            _ => {}
        }
    }

    fn tint_members(
        root: &mut morphic::kv3::Node,
    ) -> impl Iterator<Item = &mut Vec<(String, morphic::kv3::Node)>> {
        use morphic::kv3::Val;
        let Val::Object(root) = &mut root.val else {
            panic!("material root is an object");
        };
        let Val::Typed { items, .. } = &mut root
            .iter_mut()
            .find(|(k, _)| k == "m_vectorParams")
            .expect("m_vectorParams")
            .1
            .val
        else {
            panic!("m_vectorParams is a typed object array");
        };
        items.iter_mut().filter_map(|item| match item {
            Val::Object(members)
                if members.iter().any(|(k, n)| {
                    k == "m_name"
                        && matches!(&n.val, Val::String(s)
                            if s.starts_with("g_vColorTint") || s.starts_with("g_vSelfIllumTint"))
                }) =>
            {
                Some(members)
            }
            _ => None,
        })
    }

    /// The wire tags of the named tint's lanes, in order.
    fn lane_tags(material: &[u8], name: &str) -> Vec<u8> {
        use morphic::kv3::Val;
        let data = morphic::kv3_resource_data_block(material).expect("data block");
        let mut doc = morphic::kv3::decode_lossless(&data).expect("lossless decode");
        let members = tint_members(&mut doc.root)
            .find(|m| {
                m.iter()
                    .any(|(_, n)| matches!(&n.val, Val::String(s) if s == name))
            })
            .unwrap_or_else(|| panic!("no {name}"));
        match &members.iter().find(|(k, _)| k == "m_value").unwrap().1.val {
            Val::Array(lanes) => lanes.iter().map(|n| n.tag).collect(),
            other => panic!("{name} is not a generic array: {other:?}"),
        }
    }

    /// `material` with every stampable tint vector rewritten as a generic array
    /// whose 0.0 / 1.0 lanes are tagless `DOUBLE_ZERO` / `DOUBLE_ONE` nodes. No
    /// shipped material has such lanes (they are all typed double arrays), so this
    /// builds the shape the in-place double patch cannot write.
    #[allow(clippy::float_cmp)]
    fn with_tagless_tints(material: &[u8]) -> Vec<u8> {
        use morphic::kv3::{Node, Val};
        let data = morphic::kv3_resource_data_block(material).expect("data block");
        let mut doc = morphic::kv3::decode_lossless(&data).expect("lossless decode");
        for members in tint_members(&mut doc.root) {
            let value = &mut members.iter_mut().find(|(k, _)| k == "m_value").unwrap().1;
            let Val::Typed { items, .. } = &value.val else {
                panic!("tint is a typed double array");
            };
            let lanes = items
                .iter()
                .map(|lane| match lane {
                    Val::F64(d) if *d == 0.0 => Node {
                        tag: 17,
                        flag: None,
                        val: Val::Implied,
                    },
                    Val::F64(d) if *d == 1.0 => Node {
                        tag: 18,
                        flag: None,
                        val: Val::Implied,
                    },
                    Val::F64(d) => Node {
                        tag: 5,
                        flag: None,
                        val: Val::F64(*d),
                    },
                    other => panic!("tint lane is a stored double: {other:?}"),
                })
                .collect();
            *value = Node {
                tag: 8,
                flag: None,
                val: Val::Array(lanes),
            };
        }
        let new_data = morphic::kv3::encode_v5(&doc).expect("encode v5");
        morphic::replace_resource_data_block(material, &new_data).expect("rebuild resource")
    }

    #[test]
    fn stamps_a_blobbed_material_with_tagless_tint_lanes_and_keeps_its_blobs() {
        // necro_picker_hand_effect is blobbed (two dynamic-expression blobs) with a
        // white base tint. With tagless lanes the in-place double patch cannot stamp
        // it; the v5 re-encode must, without touching the blobs, and each param still
        // gets its own brightness (the base tint is full value, the glow is dimmed).
        let dim = with_tint(
            &fixture("necro_picker_hand_effect.vmat_c"),
            "g_vSelfIllumTint1",
            [0.28, 0.16, 0.09],
        );
        let source = with_tagless_tints(&dim);
        assert_eq!(lane_tags(&source, "g_vColorTint1"), [18, 18, 18, 17]);
        let recolor = Recolor::new(328.0, 1.0, 1.0);
        let full = [1.0, 0.0, 0.533_333_3];

        let stamped = recolor_material_color_bytes(&source, recolor)
            .expect("stamp")
            .expect("has a tint");

        // 1.0 and 0.0 stay tagless, 0.533 becomes a stored double.
        assert_eq!(lane_tags(&stamped, "g_vColorTint1"), [18, 17, 5, 17]);
        assert!(close(tint_rgb(&stamped, "g_vColorTint1"), full));
        assert!(close(
            tint_rgb(&stamped, "g_vSelfIllumTint1"),
            scaled(full, 0.28)
        ));
        let mut expected = morphic::decode_kv3_resource(&source).expect("decode source");
        stamp_tint_tree(&mut expected, recolor);
        let got = morphic::decode_kv3_resource(&stamped).expect("decode stamped");
        assert_eq!(got, expected, "only the tint lanes changed");
        let (mut before, mut after) = (Vec::new(), Vec::new());
        blobs_of(&expected, &mut before);
        blobs_of(&got, &mut after);
        assert_eq!(before.len(), 2);
        assert_eq!(before, after, "every blob byte survives");

        let block = morphic::kv3_resource_data_block(&stamped).expect("data block");
        let word = |at: usize| u32::from_le_bytes(block[at..at + 4].try_into().unwrap());
        assert_eq!(word(0), 0x4B56_3305, "still KV3 v5");
        assert_eq!(word(20), 1, "still LZ4");
        assert_eq!(word(56), 2, "same blob count");
        assert_eq!(word(48), word(72) + word(80), "sizeUncompressedTotal");
        assert_eq!(word(52), word(76) + word(84), "sizeCompressedTotal");
    }

    #[test]
    fn keeps_a_solid_prop_base_neutral_and_stamps_only_its_glow() {
        // The gravestone is a SOLID (non-additive) material: its base g_vColorTint is
        // white (the stone) and its g_vSelfIllumTint is the necro yellow-green glow
        // (skull / R.I.P. / cracks). Stamping must leave the stone base its own color
        // and only recolor the glow, so a recolor doesn't paint the whole tombstone.
        let recolor = Recolor::new(205.0, 0.6, 1.0); // sky blue
        let full = [0.4, 0.75, 1.0]; // hsv(205, 0.6, 1.0)
        let source = fixture("necro_gravestone.vmat_c");
        let stamped = recolor_material_color_bytes(&source, recolor)
            .expect("stamp gravestone")
            .expect("gravestone has a tint");

        let base = tint_rgb(&stamped, "g_vColorTint1");
        assert!(
            base.iter().all(|&c| (c - 1.0).abs() < 1e-6),
            "stone base g_vColorTint stays white, got {base:?}"
        );
        let glow = tint_rgb(&stamped, "g_vSelfIllumTint1");
        let want = scaled(full, max_rgb(tint_rgb(&source, "g_vSelfIllumTint1")));
        assert!(
            close(glow, want),
            "glow self-illum stamped to the brand color at its own brightness, got {glow:?}"
        );

        // The solid outline is a dark green here, so it follows the glow, dimmed.
        let outline = tint_rgb(&stamped, "g_vSolidOutlineTint");
        let want = scaled(full, max_rgb(tint_rgb(&source, "g_vSolidOutlineTint")));
        assert!(
            close(outline, want),
            "colored outline stamped, got {outline:?}"
        );
        assert!(max_rgb(outline) < 0.13, "outline stays dark");

        // A neutral outline is left as authored.
        let gray = with_tint(&source, "g_vSolidOutlineTint", [0.1, 0.1, 0.1]);
        let stamped = recolor_material_color_bytes(&gray, recolor)
            .expect("stamp")
            .expect("has a tint");
        assert!(close(
            tint_rgb(&stamped, "g_vSolidOutlineTint"),
            [0.1, 0.1, 0.1]
        ));
    }
}
