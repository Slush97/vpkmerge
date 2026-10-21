//! Per-hero QA for the ability-VFX recolor recipes in [`crate::hero_recolor`].
//!
//! For each pinned hero, against a live pak, this answers four questions:
//!
//! 1. **Integrity.** Does every pinned path still resolve, does every prefix
//!    still hold particles, does every pinned entry actually patch, and does
//!    every patched output decode again?
//! 2. **Safety.** Does every pinned texture/material stay inside the hero? A
//!    pinned asset that another hero's (or a generic) effect also uses bleeds the
//!    recolor onto it. Data channels (normal/roughness/masks, sprite sheets the
//!    particle samples as alpha only), the skin albedo, and UI-signal materials
//!    (placement previews) must not be pinned either.
//! 3. **Coverage.** What color carriers does the hero have that the recipe does
//!    not pin? Hero-named particles outside the recipe prefixes, chromatic sprite
//!    sheets and material color maps, additive/self-illum materials whose color
//!    is a tint constant, and prop models with baked vertex color, all reached by
//!    walking the hero's particles (and its live body model) through their
//!    external-reference lists.
//! 4. **Limits.** Carriers that are shared with other effects and so can't be
//!    recolored in place (generic child particle systems, shared chromatic
//!    sheets). Informational: fixing them needs a copy-and-repoint, not a pin.
//!
//! Bleed is judged on a spawn graph. Every top-level `scripts/*.vdata_c` node is
//! owned by the released heroes, items and neutrals that reach it ([`Owner`]),
//! each owner's nodes name root particles, and the particle child references
//! close those into the set of particles the owner can spawn. A particle under a
//! hero's prefixes that someone else can spawn too must not be recolored in place:
//! the recipe lists it in `shared_entries` and the bake writes a copy under a new
//! path and repoints the hero's own files at it. The plan for that (which
//! particles are copied from which private referrer, which stay vanilla because
//! only a vdata root names them) is reported per hero in [`HeroQa::isolation`].
//!
//! A carrier left out on purpose is recorded in [`REVIEWED`] with a reason, so a
//! clean report means every carrier is pinned or consciously excluded. Review
//! rows that no longer match anything are reported, so the table can't rot.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use morphic::kv3::Value;
use morphic::material::Material;
use morphic::{DecodeOptions, ImageData};
use serde::Serialize;

use crate::hero_recolor::{
    pinned_hero_codenames, recipe_for, recolor_material_color_bytes, recolor_particle_bytes,
    HeroRecolorRecipe,
};
use crate::model::{compiled_resource_path, live_hero_model_from_vdata, open_vpks, read_entry};
use crate::recolor::Recolor;

/// Asset namespaces a recipe codename also appears under. Tokens are matched
/// against `/`, `_` and `.` separated path pieces, so `bull` matches
/// `particles/abilities/bull_drain.vpcf_c` but not `bullet_*`.
const ALIASES: &[(&str, &[&str])] = &[
    ("abrams", &["bull", "atlas"]),
    ("archer", &["orion"]),
    ("astro", &["holliday"]),
    ("bookworm", &["paige"]),
    ("chrono", &["paradox"]),
    ("deadpack", &["airheart"]),
    ("digger", &["krill"]),
    ("dynamo", &["sumo"]),
    ("fencer", &["apollo"]),
    ("frank", &["victor"]),
    ("ghost", &["geist", "ghost2"]),
    ("gigawatt", &["seven"]),
    ("hornet", &["vindicta"]),
    ("inferno", &["infernus"]),
    ("magician", &["sinclair"]),
    ("mcginnis", &["forge"]),
    ("nano", &["calico"]),
    ("necro", &["graves"]),
    ("pocket", &["synth"]),
    ("priest", &["venator"]),
    ("tengu", &["ivy"]),
    ("unicorn", &["celeste"]),
    ("vampirebat", &["mina"]),
];

/// Reviewed decisions: `(codename, entry or path prefix, reason)`. A flagged
/// gap matching a row is reported as excluded instead; a safety finding on a
/// pinned entry matching a row is reported as accepted instead.
const REVIEWED: &[(&str, &str, &str)] = &[
    (
        "doorman",
        "models/heroes_wip/doorman/materials/doorman_door_preview",
        "doorway placement previews (valid blue / invalid red) are a gameplay cue",
    ),
    (
        "doorman",
        "models/heroes_wip/doorman/materials/doorman_luggagetrolley.vmat_c",
        "shared with Solomon's chessmaster_rook_buff_shield_model; needs a copy + repoint",
    ),
    (
        "doorman",
        "models/heroes_wip/doorman/doorman_door.vmdl_c",
        "vertex color is the door's wood tint; only the placement preview group reads it additively",
    ),
    (
        "gigawatt",
        "particles/abilities/gigawatt_lightning_cone",
        "Dead legacy: only the unbound citadel_ability_chain_lightning (modifier_citadel_lightning_bullet, hardcoded in the DLLs) spawns it; no hero binds that ability.",
    ),
    (
        "gigawatt",
        "particles/weapon_fx/tracers/gigawatt_chain_lightning_tracer",
        "Dead legacy: same unbound citadel_ability_chain_lightning (lightning bullet) hardcode; Seven's real tracer is weapon_fx/gigawatt/gigawatt_tracer, already in the prefix.",
    ),
    (
        "inferno",
        "particles/status_fx/status_fx_inferno_flame_dash_debuff.vpcf_c",
        "shared with neutrals and the boss: spawned by citadel_neutral_laser_beam_base, citadel_neutral_attack_lobfire, citadel_neutral_attack_lob_book, modifier_neutral_attack_explode_on_death(_strong) and citadel_ability_tier3boss_laser_beam as well as Flame Dash",
    ),
    (
        "ghost",
        "models/heroes_staging/ghost/materials/ghost2_arm.vmat_c",
        "not live: only the non-enemy ghost_blood_exchange_precast_tgt_hand.vpcf (referenced by nothing) and the ghost.vmdl particle-editor preview render it; the live precast particles spawn only the _enemy hand, which overrides to ghost2_arm_fx.vmat and takes its color from the particle tint",
    ),
    (
        "ghost",
        "models/heroes_staging/ghost/materials/ghost2_arm_color_png_c42e97cd.vtex_c",
        "albedo of the dead ghost2_arm.vmat path above; no live material samples it",
    ),
    (
        "ghost",
        "models/heroes_wip/geist/materials/geist_armglow.vmat_c",
        "always-on spectral arm glow layer of the live body model, skin identity not ability color",
    ),
    (
        "necro",
        "models/heroes_wip/necro/materials/necro_jar_of_dread_color_tga_7f34b26.vtex_c",
        "Jar of Dread prop (ability 1, ability_necro_hauntingskull): skinned to its own jar bone, held in palm_L, and the override material of necro_haunting_skull_container_preview.vpcf_c. Not her body skin.",
    ),
    (
        "yamato",
        "models/heroes_staging/yamato_v2/materials/yamoto_shadow_shape_color_psd_fe3c64a6.vtex_c",
        "Shadow Form only: bound solely by yamato_shadow_shape.vmat, the vmdl 'shadow' material group used for the Shadow Form ult state (MODIFIER_STATE_YAMATO_SHADOW_FORM). The default skin is yamoto_shape_color_png_f53cd055 and stays vanilla.",
    ),
    (
        "dynamo",
        "particles/heroes/dynamo/dynamo_head_ambient",
        "head ambient (stars, smoke, core smoke) from the hero model's particle_cfg_list, m_hAmbientParticle is empty; body decoration, not an ability or weapon effect, same gold as the body self-illum accents that stay vanilla",
    ),
    (
        "archer",
        "particles/heroes/archer/archer_ambient",
        "eye glow and trap-belt ambient (4 files) are spawned only by the unused staging model archer_v2 (live model is heroes_staging/archer/archer.vmdl, no vdata or DLL reference); eye glow is skin",
    ),
    (
        "mcginnis",
        "models/heroes_wip/mcginnis/turret/materials/turret_preview_",
        "turret_preview_good and turret_preview_bad are the placement validity material groups on turret.vmdl (a gameplay cue), same as the doorman previews",
    ),
    (
        "mcginnis",
        "particles/weapon_fx/forge/forge_barrel_charge_circle_tip.vpcf_c",
        "only referenced by models/heroes_staging/engineer/engineer.vmdl_c, which nothing uses; his live model is mcginnis.vmdl and no live clip, vdata node or DLL string spawns it",
    ),
    (
        "mcginnis",
        "particles/weapon_fx/forge/forge_reload_ring.vpcf_c",
        "only referenced by the unused engineer.vmdl_c; live reload clips spawn handle_charge and sparks but not the ring",
    ),
    (
        "chessmaster",
        "models/heroes_wip/chessmaster/materials/chessmaster_glasses_vmat_g_tnprtransmissivecolor_511fdc65.vtex_c",
        "NPR transmissive constant of chessmaster_glasses.vmat, the additive lens layer of the live body model (chessmaster.vmdl): skin identity, not an ability color",
    ),
    (
        "chessmaster",
        "models/heroes_wip/chessmaster/materials/chessmaster_knight_vmat_g_tnprtransmissivecolor_4b47c87b.vtex_c",
        "flat NPR shading constant: identical content (4b47c87b) sits on the king, bishop, queen, knight and rook materials, and every body material carries one too; stone shading of the piece, while its gold glow is pinned through the g_vSelfIllumTint1 stamp",
    ),
    (
        "chessmaster",
        "models/heroes_wip/chessmaster/materials/chessmaster_rook_vmat_g_tnprtransmissivecolor_4b47c87b.vtex_c",
        "same flat NPR shading constant (4b47c87b) as the king, bishop, queen and knight stone pieces; the rook's gold glow is pinned through the g_vSelfIllumTint1 stamp",
    ),
    (
        "punkgoat",
        "models/heroes_wip/punkgoat/punkgoat_chain.vmdl_c",
        "false positive: both vertex color buffers are neutral (white and 0.16 gray, mean sat 0.00 per the vertexcolors tool), so a hue set changes nothing; the chain color is the material tint",
    ),
    (
        "punkgoat",
        "models/heroes_wip/punkgoat/punkgoat_nail.vmdl_c",
        "false positive: vertex color is all neutral white (mean sat 0.00), a hue set is a no-op; the nail color comes from punkgoat_chain.vmat",
    ),
    (
        "abrams",
        "particles/abilities/bull_leap_trail_flame.vpcf_c",
        "child of bull_anim_thruster, which only models/heroes_staging/shiv_ult/shiv_ult.vmdl_c references (rendered by shiv_dash_beast_model, which nothing spawns); the live Abrams leap uses abrams_anim_leap_inair and no vdata, clip or DLL names the thruster",
    ),
    (
        "hornet",
        "models/heroes_staging/hornet_v3/materials/vindicta_glow.vmat_c",
        "cyan additive aura on the always-visible ghost_glow body mesh, while her ability particles are red and orange (hue histogram of constant colors: 89 red or orange vs 4 cyan); also used by models/particle/blend/general_cannon.vmdl_c, so a pin would bleed",
    ),
    (
        "nano",
        "models/heroes_staging/nano/roller/materials/calico_clawball_bomb.vmat_c",
        "shared with plant_neutral_tracer_bullet (live neutral plant tracer) and Rem's familiar_sprite_tracer_bullet; all three render calico_clawball_bomb.vmdl",
    ),
    (
        "nano",
        "models/heroes_staging/nano/roller/materials/calico_clawball_bomb_color_png_8cb21a6b.vtex_c",
        "shared with neutral plant tracer and Rem tracer through calico_clawball_bomb.vmdl; also a prop albedo",
    ),
    (
        "nano",
        "models/heroes_staging/nano/roller/materials/calico_clawball_bomb_vmat_g_tnprtransmissivecolor_b5cf243b.vtex_c",
        "1x1 constant on the clawball material shared with the neutral plant tracer and Rem tracer",
    ),
    (
        "nano",
        "materials/particle/abilities/nano/nano_cat_translucent_vmat_g_tnprtransmissivecolor_ace27dea.vtex_c",
        "only the Cat Tower projectile (ability_nano_proximity_ritual, not bound to hero_nano) and unreferenced Rat King storm particles render nano_catform_model; dormant",
    ),
    (
        "nano",
        "models/heroes_staging/nano/cat_v2/materials/nano_cat_color_png_12bb80af.vtex_c",
        "near-white albedo (5% of texels faint chroma) on the dormant Cat Tower projectile model; a hue set shows nothing, and the live body cat uses a different texture (a6dd0cb3)",
    ),
    (
        "astro",
        "models/abilities/materials/astro_barrelv2_color_png_84c6ba3a.vtex_c",
        "wood and TNT label surface albedo (9% chromatic texels, hue 28); the same texture is on the live body's barrel through astro_barrelv2_noglow.vmat, so it is skin",
    ),
    (
        "bebop",
        "models/heroes_staging/bebop/materials/bebop_hand_projectile_color_png_8d7bfc.vtex_c",
        "dark red panel albedo of the hook fist (chroma 0.10), matches Bebop's own body hand; the ability glow is the self-illum tint pinned via the material",
    ),
    (
        "familiar",
        "models/abilities/familiar/materials/familiar_helper_body.vmat_c",
        "helper NPC creature skin (blue rock albedo, star detail); material is also on hideout_cozy_sleepyguy1/2, the hero-select helper model and ratking_ratswarm_models",
    ),
    (
        "familiar",
        "models/abilities/familiar/materials/familiar_helper_eyes.vmat_c",
        "helper NPC creature eyes; same shared users as familiar_helper_body (hideout props, hero-select helper, ratswarm)",
    ),
    (
        "familiar",
        "models/abilities/familiar/materials/familiar_helper_color_png_35da16bb.vtex_c",
        "albedo of familiar_helper_body.vmat, creature skin shared with hideout props and Rat King's ratswarm",
    ),
    (
        "familiar",
        "models/abilities/familiar/materials/familiar_helper_color_png_65ccae95.vtex_c",
        "albedo of familiar_helper_eyes.vmat, creature skin shared with hideout props and Rat King's ratswarm",
    ),
    (
        "lash",
        "models/heroes_wip/lash/materials/lash_sparkles.vmat_c",
        "12 vertices of gold sparkle on the live body model (lash.vmdl_c): gear glint, skin accent rather than an ability prop",
    ),
    (
        "priest",
        "models/heroes_wip/priest/materials/priest_trap_gem.vmat_c",
        "shared with Silver: the gem model (priest_snaptrap.vmdl) renders in priest_snaptrap_root, which ability_werewolf_netshot also spawns",
    ),
    (
        "priest",
        "models/heroes_wip/priest/materials/priest_trap_gem_vmat_g_tnprtransmissivecolor_3ef99f5e.vtex_c",
        "transmissive map of priest_trap_gem.vmat; same sharing with Silver's Net Shot root",
    ),
    (
        "yamato",
        "materials/particle/abilities/yamato/yamato_shadow_redemption_complete_status.vtex_c",
        "trippy-vfx's BC7 re-encode moves alpha on about 1% of a soft wispy overlay; recolor and both prism paths keep it",
    ),
    (
        "nano",
        "materials/particle/abilities/nano/nano_cat_translucent.vmat_c",
        "dormant: only the Cat Tower projectile (ability_nano_proximity_ritual, not bound to hero_nano) renders it",
    ),
    (
        "nano",
        "models/heroes_staging/nano/cat_statue/materials/cat_statue_color_png_17bb4cac.vtex_c",
        "grey statue albedo variant, 3% faintly tinted texels; a hue set shows nothing",
    ),
    (
        "pocket",
        "models/abilities/pocket_briefcase_model.vmdl_c",
        "vertex color sits on the 'glow' outline shells, a mesh group off by default; pin only if an amber shell shows in game",
    ),
    (
        "werewolf",
        "models/heroes_wip/werewolf/materials/werewolf_transform_eye",
        "transformed-form eye glow (material + albedo): skin identity, the rest of the wolf stays vanilla",
    ),
    (
        "baba",
        "models/heroes_wip/baba/materials/baba_glasses.vmat_c",
        "always-on lens glow on the live body model (60 triangles, additive, static tint, no expression): skin identity, and nothing in her abilities references it; her ability color constants are 82% purple or pink (504 of 617 over the 226 live particles) against its green-white",
    ),
    (
        "artist",
        "models/heroes_wip/artist/materials/artist_rolled_painting_color_png_",
        "rolled painting prop (albedo + transmissive): the color is its artwork, 17% faintly tinted texels",
    ),
    (
        "deadpack",
        "models/heroes_wip/deadpack/materials/deadpack_missile_color_png_",
        "missile paint (albedo + transmissive), 8% faintly tinted trim; its glow is the pinned deadpack_missile tint",
    ),
    (
        "deadpack",
        "models/heroes_wip/deadpack/materials/deadpack_weapon_color_png_f7ef9e26.vtex_c",
        "his gun's paint, borrowed as the fuel globules' albedo; the globule color is the pinned deadpack_globules tint",
    ),
];

/// Hue the bake checks recolor to. Any hue works; the checks are about whether
/// each entry patches and decodes, not about the color.
pub const DEFAULT_PROBE_HUE: f64 = 280.0;

const CHROMA_EDGE: u32 = 128;
/// Share of texels with chroma above 0.15 for a texture to count as colored: a
/// hue set visibly moves it (a grey prop with two orange discs qualifies).
const CHROMATIC_SHARE: f32 = 0.03;
const TINT_SATURATION: f32 = 0.2;
/// A source texel this dark is margin: under additive blending it is the
/// transparent part of the silhouette.
const MARGIN_VALUE: f32 = 0.06;
/// A margin texel this bright after a bake is visible.
const LIT_VALUE: f32 = 0.15;
/// Share of the margin a bake may light before the quad reads as a square.
const SQUARE_LIT_SHARE: f32 = 0.02;
/// Alpha change above BC re-encode noise.
const ALPHA_NOISE: f32 = 0.1;
/// Share of texels whose alpha may move before the shape counts as changed.
const ALPHA_DRIFT_SHARE: f32 = 0.01;
/// Longest side of the mip-0 sample grid shape drift is measured on.
const DRIFT_EDGE: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum QaStatus {
    Pass,
    Warn,
    Fail,
}

/// One hero's QA result. `status` is `fail` on any integrity or safety finding,
/// `warn` on an unreviewed gap, a no-op pin or a stale review row.
#[derive(Debug, Clone, Serialize)]
pub struct HeroQa {
    pub codename: String,
    pub status: QaStatus,
    /// Recolored carriers over recolored carriers plus gaps.
    pub coverage: f32,
    pub particles: usize,
    /// Particles the bake recolors in place (patched, and not in `shared_entries`).
    pub particles_recolored: usize,
    /// Shared particles the bake recolors as hero-local copies (patched, listed
    /// in `shared_entries` and reached through a file the bake can repoint).
    pub copies_recolored: usize,
    pub textures_pinned: usize,
    pub materials_pinned: usize,
    pub models_pinned: usize,
    /// Integrity failures: missing paths, empty prefixes, unpatchable or
    /// undecodable entries.
    pub integrity: Vec<Finding>,
    /// Pinned entries that change nothing visible.
    pub noops: Vec<Finding>,
    /// Pinned entries that break a safety rule.
    pub violations: Vec<Finding>,
    /// Safety findings a review row accepts.
    pub accepted: Vec<Finding>,
    /// Color carriers the recipe does not pin.
    pub gaps: Vec<Finding>,
    /// Gaps a review row excludes.
    pub excluded: Vec<Finding>,
    /// Shared carriers that can't be recolored in place.
    pub limits: Vec<Finding>,
    /// Review rows for this hero that matched nothing.
    pub stale_reviews: Vec<String>,
    /// Colored hero-named particles outside the prefixes that nothing spawns
    /// (listed under `limits`).
    pub unreferenced_particles: usize,
    /// Particles under the prefixes that another released hero, an item or a
    /// neutral also spawns, listed in `shared_entries` or not.
    pub prefix_shared: usize,
    /// What the bake must do about every shared entry, so a recipe's
    /// `shared_entries` can be filled mechanically.
    pub isolation: Vec<Isolation>,
    /// The minimal `shared_entries` for this hero: every `isolation` entry, with
    /// runs of files collapsed to a path prefix that covers nothing else.
    pub suggested_shared: Vec<String>,
}

impl HeroQa {
    fn new(recipe: &HeroRecolorRecipe) -> Self {
        Self {
            codename: recipe.codename.clone(),
            status: QaStatus::Pass,
            coverage: 1.0,
            particles: 0,
            particles_recolored: 0,
            copies_recolored: 0,
            textures_pinned: recipe.texture_entries.len(),
            materials_pinned: recipe.material_entries.len(),
            models_pinned: recipe.model_entries.len(),
            integrity: Vec::new(),
            noops: Vec::new(),
            violations: Vec::new(),
            accepted: Vec::new(),
            gaps: Vec::new(),
            excluded: Vec::new(),
            limits: Vec::new(),
            stale_reviews: Vec::new(),
            unreferenced_particles: 0,
            prefix_shared: 0,
            isolation: Vec::new(),
            suggested_shared: Vec::new(),
        }
    }
}

/// One shared entry the bake must not edit in place.
#[derive(Debug, Clone, Serialize)]
pub struct Isolation {
    pub entry: String,
    /// `copy`: a recolored copy, repointed from `referrers`. `vanilla`: the hero
    /// reaches it only through roots the bake cannot repoint (vdata, a clip, a DLL
    /// string) or not at all, so it stays vanilla for the hero too. `pinned`: a
    /// pinned texture, material or model, copied and repointed from `referrers`.
    pub kind: String,
    /// The hero's own files the bake repoints at the copy.
    pub referrers: Vec<String>,
    /// Who else can spawn it: other heroes (by `heroes.vdata_c` key), `item
    /// <key>` or `neutral`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shared_with: Vec<String>,
    /// The nodes and fields that name it for them, with how an owner reaches an
    /// indirectly owned node: `node.field (owner[: ability -[field]-> node])`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spawned_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub entry: String,
    /// Short machine-readable class, e.g. `stale`, `shared`, `particle`.
    pub kind: String,
    /// Evidence: who references it, measured chroma, the error text.
    pub detail: String,
    /// The recipe field a gap belongs in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggest: Option<&'static str>,
}

impl Finding {
    fn new(entry: impl Into<String>, kind: &str, detail: impl Into<String>) -> Self {
        Self {
            entry: entry.into(),
            kind: kind.to_owned(),
            detail: detail.into(),
            suggest: None,
        }
    }

    fn gap(entry: impl Into<String>, kind: &str, detail: String, field: &'static str) -> Self {
        Self {
            suggest: Some(field),
            ..Self::new(entry, kind, detail)
        }
    }
}

/// Chroma of a texture measured at a small mip: mean absolute chroma
/// (`max - min` of RGB) over non-black texels, the share of texels above 0.15,
/// and the chroma-weighted mean hue in degrees.
#[derive(Debug, Clone, Copy)]
struct Chroma {
    mean: f32,
    share: f32,
    hue: f32,
}

impl Chroma {
    fn chromatic(self) -> bool {
        self.share >= CHROMATIC_SHARE
    }
}

impl std::fmt::Display for Chroma {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "chroma {:.2} ({:.0}% of texels), hue {:.0}",
            self.mean,
            self.share * 100.0,
            self.hue
        )
    }
}

/// Model dirs whose models (and meshes) the index reads: hero bodies, ability
/// props and particle models.
const MODEL_DIRS: &[&str] = &["models/heroes", "models/abilities", "models/particle"];

const HEROES_VDATA: &str = "scripts/heroes.vdata_c";
const UNITS_VDATA: &str = "scripts/npc_units.vdata_c";

/// Someone who can make a particle appear in the live game.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Owner {
    /// A released hero, by its `heroes.vdata_c` key suffix (`atlas` is Abrams).
    Hero(String),
    /// A shop item (an `upgrade_*` node).
    Item(String),
    /// Troopers, bosses, jungle camps and the other world entities.
    Neutral,
}

impl Owner {
    fn label(&self) -> String {
        match self {
            Self::Hero(code) => code.clone(),
            Self::Item(key) => format!("item {key}"),
            Self::Neutral => "neutral".to_owned(),
        }
    }

    fn is_hero(&self, names: &[String]) -> bool {
        matches!(self, Self::Hero(code) if names.contains(code))
    }
}

/// How an owner first reaches a particle: the vdata node (`file key`) or model
/// entry that names the root, the field of that node holding the path, and the
/// parent particle when reached as a child.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    parent: Option<String>,
    root: String,
    field: String,
}

/// How an owner reaches a vdata node: through which field of which node (none
/// for the owner's own roots).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Prov {
    parent: Option<usize>,
    field: String,
}

/// `Prov` with the parent as a node label, for reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnerPath {
    parent: Option<String>,
    field: String,
}

/// One top-level node of a `scripts/*.vdata_c` file: an ability, modifier, unit,
/// hero, pickup...
struct VdataNode {
    key: String,
    /// `file key`, which is unique where bare keys are not (`weapon_base`).
    label: String,
    units: bool,
    /// A `hero_*` node of `heroes.vdata_c`.
    definition: bool,
    /// Identifier-like string values anywhere in the node: the candidates for
    /// "this node names that node".
    words: BTreeSet<String>,
    particles: BTreeSet<String>,
    models: BTreeSet<String>,
    /// Word or resource path -> the field path of its first occurrence.
    fields: BTreeMap<String, String>,
}

/// A released hero of `heroes.vdata_c`.
struct HeroDef {
    code: String,
    node: usize,
    /// Keys of the `m_mapBoundAbilities` slots.
    abilities: Vec<String>,
    model: Option<String>,
}

/// External references of every particle, material, and hero/ability/particle
/// model and mesh in the pak, the reverse map, which particles the game can
/// actually spawn, and who can spawn them. Built once per run, mostly from RERL
/// blocks.
#[derive(Default)]
struct RefIndex {
    users: HashMap<String, Vec<String>>,
    /// Sorted.
    particles: Vec<String>,
    /// Particle or material -> everything its RERL references: the links the
    /// bake can repoint.
    refs: HashMap<String, Vec<String>>,
    /// Particles reachable from a vdata string, an animation clip or model
    /// reference, or a path hardcoded in the game DLLs, plus every child system
    /// of one of those, each with what first reached it.
    live: BTreeMap<String, String>,
    /// Particle and model paths named under each top-level vdata node (one
    /// ability, modifier, hero...): ties a code-spawned ability model to the
    /// hero whose particles the same node spawns.
    vdata_nodes: Vec<BTreeSet<String>>,
    /// Every vdata node (`file key`) that spawns a particle, directly or as an
    /// ancestor system.
    spawners: HashMap<String, BTreeSet<String>>,
    /// Models a vdata node names (hero bodies, code-spawned ability models).
    vdata_models: BTreeSet<String>,
    /// Particle -> every owner that can spawn it and how. Particles only DLL
    /// strings, unbound abilities or generic tables name have no owner.
    reach: HashMap<String, BTreeMap<Owner, Step>>,
    /// Vdata node label -> how each owner reaches it.
    ownership: HashMap<String, BTreeMap<Owner, OwnerPath>>,
}

impl RefIndex {
    #[allow(clippy::too_many_lines)]
    fn build(vpks: &[valve_pak::VPK], game_bin: Option<&Path>) -> Self {
        let mut entries: BTreeSet<String> = BTreeSet::new();
        for vpk in vpks {
            for p in vpk.file_paths() {
                let model = (p.ends_with(".vmdl_c") || p.ends_with(".vmesh_c"))
                    && MODEL_DIRS.iter().any(|d| p.starts_with(d));
                if model
                    || p.ends_with(".vpcf_c")
                    || p.ends_with(".vmat_c")
                    || p.ends_with(".vnmclip_c")
                {
                    entries.insert(p.to_ascii_lowercase());
                }
            }
        }
        let mut users: HashMap<String, Vec<String>> = HashMap::new();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        let mut refs: HashMap<String, Vec<String>> = HashMap::new();
        let mut roots: BTreeMap<String, String> = BTreeMap::new();
        let mut entry_particles: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut entry_models: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for entry in &entries {
            let Some(bytes) = read_entry(vpks, entry) else {
                continue;
            };
            let particle = entry.ends_with(".vpcf_c");
            let found = external_refs(&bytes);
            if particle || entry.ends_with(".vmat_c") {
                refs.insert(entry.clone(), found.clone());
            }
            for r in found {
                if r.ends_with(".vpcf_c") {
                    if particle {
                        children.entry(entry.clone()).or_default().push(r.clone());
                    } else {
                        roots.entry(r.clone()).or_insert_with(|| entry.clone());
                        entry_particles
                            .entry(entry.clone())
                            .or_default()
                            .push(r.clone());
                    }
                } else if r.ends_with(".vmdl_c") && !particle {
                    entry_models
                        .entry(entry.clone())
                        .or_default()
                        .push(r.clone());
                }
                users.entry(r).or_default().push(entry.clone());
            }
        }

        let scripts: Vec<String> = vpks
            .iter()
            .flat_map(valve_pak::VPK::file_paths)
            .filter(|p| p.starts_with("scripts/") && p.ends_with(".vdata_c"))
            .cloned()
            .collect();
        let mut nodes: Vec<VdataNode> = Vec::new();
        let mut heroes: Vec<HeroDef> = Vec::new();
        let mut vdata_nodes = Vec::new();
        let mut vdata_models: BTreeSet<String> = BTreeSet::new();
        let mut direct_spawners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for entry in &scripts {
            let Some(Value::Object(top)) =
                read_entry(vpks, entry).and_then(|b| morphic::decode_kv3_resource(&b).ok())
            else {
                continue;
            };
            for (key, node) in &top {
                let mut fields = BTreeMap::new();
                string_fields(node, "", &mut fields);
                let found: BTreeSet<String> = fields
                    .keys()
                    .filter(|k| k.ends_with(".vpcf_c") || k.ends_with(".vmdl_c"))
                    .cloned()
                    .collect();
                let label = format!("{entry} {key}");
                for p in found.iter().filter(|p| p.ends_with(".vpcf_c")) {
                    roots.entry(p.clone()).or_insert_with(|| label.clone());
                    direct_spawners
                        .entry(p.clone())
                        .or_default()
                        .insert(label.clone());
                }
                let definition = entry == HEROES_VDATA && key.starts_with("hero_");
                if definition && released(node) {
                    if let (Some(code), Some(Value::Object(slots))) =
                        (key.strip_prefix("hero_"), node.get("m_mapBoundAbilities"))
                    {
                        let abilities: Vec<String> = slots
                            .iter()
                            .filter_map(|(_, a)| a.as_str().map(str::to_owned))
                            .collect();
                        heroes.push(HeroDef {
                            code: code.to_owned(),
                            node: nodes.len(),
                            abilities,
                            model: node
                                .get("m_strModelName")
                                .and_then(Value::as_str)
                                .filter(|m| !m.is_empty())
                                .map(|m| compiled_resource_path(m).to_ascii_lowercase()),
                        });
                    }
                }
                if found.iter().any(|p| p.ends_with(".vmdl_c")) {
                    vdata_models.extend(found.iter().filter(|p| p.ends_with(".vmdl_c")).cloned());
                    vdata_nodes.push(found.clone());
                }
                if !entry.contains("/tagged_sounds/") {
                    let words = fields
                        .keys()
                        .filter(|k| !k.contains('/'))
                        .cloned()
                        .collect();
                    nodes.push(VdataNode {
                        key: key.clone(),
                        label,
                        units: entry == UNITS_VDATA,
                        definition,
                        words,
                        fields,
                        particles: found
                            .iter()
                            .filter(|p| p.ends_with(".vpcf_c"))
                            .cloned()
                            .collect(),
                        models: found
                            .iter()
                            .filter(|p| p.ends_with(".vmdl_c"))
                            .cloned()
                            .collect(),
                    });
                }
            }
        }
        if let Some(bin) = game_bin {
            for dll in ["client.dll", "server.dll"] {
                if let Ok(bytes) = std::fs::read(bin.join(dll)) {
                    let mut found = BTreeSet::new();
                    dll_particle_paths(&bytes, &mut found);
                    for p in found {
                        roots.entry(p).or_insert_with(|| dll.to_owned());
                    }
                }
            }
        }

        let mut live: BTreeMap<String, String> = BTreeMap::new();
        let mut queue: Vec<(String, String)> = roots.into_iter().collect();
        while let Some((p, source)) = queue.pop() {
            if live.contains_key(&p) {
                continue;
            }
            if let Some(kids) = children.get(&p) {
                let via = format!("child of {p}");
                queue.extend(
                    kids.iter()
                        .filter(|k| !live.contains_key(*k))
                        .map(|k| (k.clone(), via.clone())),
                );
            }
            live.insert(p, source);
        }

        let mut spawners: HashMap<String, BTreeSet<String>> = HashMap::new();
        for (root, labels) in &direct_spawners {
            let mut queue = vec![root.clone()];
            let mut seen: BTreeSet<String> = BTreeSet::new();
            while let Some(p) = queue.pop() {
                if !seen.insert(p.clone()) {
                    continue;
                }
                spawners
                    .entry(p.clone())
                    .or_default()
                    .extend(labels.iter().cloned());
                if let Some(kids) = children.get(&p) {
                    queue.extend(kids.iter().cloned());
                }
            }
        }

        let owners = assign_owners(&nodes, &heroes);
        let ownership = ownership_paths(&nodes, &owners);
        let model_particles = embedded_particles(&entry_particles, &entry_models);
        let reach = particle_reach(
            owner_roots(&nodes, &owners, &heroes, &model_particles),
            &children,
        );

        let particles = entries
            .into_iter()
            .filter(|e| e.ends_with(".vpcf_c"))
            .collect();
        Self {
            users,
            particles,
            refs,
            live,
            vdata_nodes,
            spawners,
            vdata_models,
            reach,
            ownership,
        }
    }

    fn users(&self, entry: &str) -> &[String] {
        self.users.get(entry).map_or(&[], Vec::as_slice)
    }

    /// Owners that can spawn `particle` and are not the hero named by `names`.
    fn foreign_owners(&self, particle: &str, names: &[String]) -> Vec<(&Owner, &Step)> {
        self.reach
            .get(particle)
            .into_iter()
            .flatten()
            .filter(|(o, _)| !o.is_hero(names))
            .collect()
    }

    /// The hero's own way to `particle`, if its abilities or body spawn it.
    fn hero_owner(&self, particle: &str, names: &[String]) -> Option<(&Owner, &Step)> {
        self.reach
            .get(particle)?
            .iter()
            .find(|(o, _)| o.is_hero(names))
    }

    /// `root > parent > ... > particle` for how `owner` reaches `particle`.
    fn chain(&self, particle: &str, owner: &Owner) -> String {
        let mut path = vec![file_name(particle).to_owned()];
        let mut at = particle;
        let mut root = "";
        while let Some(step) = self.reach.get(at).and_then(|m| m.get(owner)) {
            let Some(parent) = &step.parent else {
                root = short_root(&step.root);
                break;
            };
            path.push(file_name(parent).to_owned());
            at = parent;
        }
        path.push(root.to_owned());
        path.reverse();
        path.join(" > ")
    }

    /// How `owner` reaches the vdata node that names a root particle, when it
    /// is not simply the owner's own node or (for a hero) one of its bound
    /// abilities: `hero_x -[field]-> ability_a -[field]-> node`.
    fn explain(&self, root: &str, owner: &Owner) -> Option<String> {
        let mut at = root.split_once(" via ").map_or(root, |(label, _)| label);
        let mut hops: Vec<(&str, &str)> = Vec::new();
        while let Some(OwnerPath {
            parent: Some(parent),
            field,
        }) = self.ownership.get(at).and_then(|m| m.get(owner))
        {
            hops.push((at, field));
            at = parent;
        }
        if hops.len() <= usize::from(matches!(owner, Owner::Hero(_))) {
            return None;
        }
        let mut out = short_root(at).to_owned();
        for (node, field) in hops.iter().rev() {
            let _ = write!(
                out,
                " -[{}]-> {}",
                field.trim_start_matches('.'),
                short_root(node)
            );
        }
        Some(out)
    }

    /// Whether a particle, model or mesh can actually draw: a live particle, a
    /// model a vdata node names or a live particle renders, or a mesh of such a
    /// model. Anything else (materials, unknown kinds) is assumed to draw.
    fn renders(&self, entry: &str) -> bool {
        if entry.ends_with(".vpcf_c") {
            self.live.contains_key(entry)
        } else if entry.ends_with(".vmdl_c") {
            self.vdata_models.contains(entry)
                || self
                    .users(entry)
                    .iter()
                    .any(|u| u.ends_with(".vpcf_c") && self.live.contains_key(u))
        } else if entry.ends_with(".vmesh_c") {
            self.users(entry)
                .iter()
                .any(|u| u.ends_with(".vmdl_c") && self.renders(u))
        } else {
            true
        }
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The node key of a `file key` label, or a model entry path as is.
fn short_root(root: &str) -> &str {
    if root.starts_with("scripts/") {
        root.split_once(' ').map_or(root, |(_, key)| key)
    } else {
        root
    }
}

/// Who owns each vdata node.
///
/// A released hero owns its definition, its bound abilities and, transitively,
/// every node whose key appears as a string value in a node it owns (summoned
/// units, modifiers, projectiles, misc entities). Items (`upgrade_*`) own their
/// closures the same way. Edges into hero definitions are not followed: an item
/// lists the heroes it is disabled on in `m_vecDisabledOnHeroes`, and following
/// that blacklist would hand the item a hero's whole kit. A node several owners
/// reach is owned by all of them.
///
/// A node nobody reaches is spawned by code under its own name (`CLASS_*` and
/// entity classnames in the DLLs; vdata never links to it). Those in
/// `npc_units.vdata_c`, or whose key says neutral, belong to the one released
/// hero all their hero-named resources sit under (Graves' ghouls, `McGinnis`'
/// sentry, Paradox's time wall, Rat King's rats), else to the neutrals
/// (troopers, camps, bosses, and neutral modifiers that reuse hero effects), and
/// own their closures too. A key that says neutral wins over the assets: a
/// jungle rat wears Rat King's model. Everything else (abilities of heroes in
/// development, pickups, generic tables) has no owner and is nobody's bleed.
fn assign_owners(nodes: &[VdataNode], heroes: &[HeroDef]) -> Vec<BTreeMap<Owner, Prov>> {
    let mut by_key: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        by_key.entry(n.key.as_str()).or_default().push(i);
    }
    let edges: Vec<Vec<(usize, &str)>> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mut out: BTreeMap<usize, &str> = BTreeMap::new();
            for (word, field) in n.words.iter().filter_map(|w| Some((w, n.fields.get(w)?))) {
                for &j in by_key.get(word.as_str()).into_iter().flatten() {
                    if j != i && !nodes[j].definition {
                        out.entry(j).or_insert(field.as_str());
                    }
                }
            }
            out.into_iter().collect()
        })
        .collect();
    let mut bound: HashMap<&str, Vec<&str>> = HashMap::new();
    for hero in heroes {
        for ability in &hero.abilities {
            bound
                .entry(ability.as_str())
                .or_default()
                .push(hero.code.as_str());
        }
    }
    let claim = Claims {
        nodes,
        edges: &edges,
        bound: &bound,
    };
    let mut owners: Vec<BTreeMap<Owner, Prov>> = vec![BTreeMap::new(); nodes.len()];
    for hero in heroes {
        let mut roots = vec![hero.node];
        roots.extend(
            hero.abilities
                .iter()
                .filter_map(|a| by_key.get(a.as_str()))
                .flatten(),
        );
        claim.grant(&mut owners, &Owner::Hero(hero.code.clone()), roots);
    }
    for (i, n) in nodes.iter().enumerate() {
        if n.key.starts_with("upgrade_") {
            claim.grant(&mut owners, &Owner::Item(n.key.clone()), vec![i]);
        }
    }

    let hero_names: Vec<(&str, Vec<String>)> = heroes
        .iter()
        .map(|h| (h.code.as_str(), names_for(&h.code)))
        .collect();
    let mut seen: HashSet<usize> = HashSet::new();
    for i in 0..nodes.len() {
        let unit = |n: &VdataNode| n.units || n.key.contains("neutral");
        if !owners[i].is_empty() || !unit(&nodes[i]) || seen.contains(&i) {
            continue;
        }
        // Nodes that reference each other (a jungle creature and its variants)
        // are one entity; it is spawned by code only if nothing else reaches it.
        let forward = claim.reachable([i]);
        let cluster: BTreeSet<usize> = forward
            .iter()
            .copied()
            .filter(|&j| claim.reachable([j]).contains(&i))
            .collect();
        seen.extend(&cluster);
        let reached_from_outside = (0..nodes.len())
            .any(|k| !cluster.contains(&k) && edges[k].iter().any(|(j, _)| cluster.contains(j)));
        if reached_from_outside {
            continue;
        }
        let named: Vec<&str> = hero_names
            .iter()
            .filter(|(_, names)| {
                cluster.iter().any(|&k| {
                    nodes[k]
                        .particles
                        .iter()
                        .chain(&nodes[k].models)
                        .any(|p| path_owned_by(names, p))
                })
            })
            .map(|(code, _)| *code)
            .collect();
        let neutral_named = cluster.iter().any(|&k| nodes[k].key.contains("neutral"));
        let owner = match named[..] {
            [code] if !neutral_named => Owner::Hero(code.to_owned()),
            _ => Owner::Neutral,
        };
        claim.grant(&mut owners, &owner, cluster.into_iter().collect());
    }
    owners
}

/// The vdata reference graph with the rule for who may claim a node.
///
/// An ability bound (`m_mapBoundAbilities`) to released heroes belongs to
/// exactly those heroes: references from other owners are not claims. Sinclair's
/// copy-an-ultimate lists every hero's ultimate, and an animgraph source name
/// such as Apollo's `m_strAG2SourceName = citadel_ability_power_slash` is not a
/// spawn, so following either would hand one hero another's abilities.
struct Claims<'a> {
    nodes: &'a [VdataNode],
    edges: &'a [Vec<(usize, &'a str)>],
    bound: &'a HashMap<&'a str, Vec<&'a str>>,
}

impl Claims<'_> {
    fn may_claim(&self, owner: &Owner, node: usize) -> bool {
        self.bound.get(self.nodes[node].key.as_str()).is_none_or(
            |heroes| matches!(owner, Owner::Hero(code) if heroes.contains(&code.as_str())),
        )
    }

    /// Nodes `roots` reach along references any owner may follow, the roots included.
    fn reachable(&self, roots: impl IntoIterator<Item = usize>) -> HashSet<usize> {
        let mut seen: HashSet<usize> = HashSet::new();
        let mut queue: Vec<usize> = roots.into_iter().collect();
        while let Some(i) = queue.pop() {
            if seen.insert(i) {
                queue.extend(self.edges[i].iter().map(|(j, _)| *j));
            }
        }
        seen
    }

    /// Gives `owner` the closure of `roots`, shortest reference chain first.
    fn grant(&self, owners: &mut [BTreeMap<Owner, Prov>], owner: &Owner, roots: Vec<usize>) {
        let mut seen: HashSet<usize> = HashSet::new();
        let mut queue: VecDeque<(usize, Prov)> = roots
            .into_iter()
            .map(|r| {
                (
                    r,
                    Prov {
                        parent: None,
                        field: String::new(),
                    },
                )
            })
            .collect();
        while let Some((i, prov)) = queue.pop_front() {
            if !seen.insert(i) {
                continue;
            }
            owners[i].entry(owner.clone()).or_insert(prov);
            for (j, field) in &self.edges[i] {
                if self.may_claim(owner, *j) {
                    queue.push_back((
                        *j,
                        Prov {
                            parent: Some(i),
                            field: (*field).to_owned(),
                        },
                    ));
                }
            }
        }
    }
}

/// Each node's owners with how they reach it, by node label.
fn ownership_paths(
    nodes: &[VdataNode],
    owners: &[BTreeMap<Owner, Prov>],
) -> HashMap<String, BTreeMap<Owner, OwnerPath>> {
    nodes
        .iter()
        .zip(owners)
        .map(|(n, owned)| {
            let paths = owned
                .iter()
                .map(|(o, prov)| {
                    let path = OwnerPath {
                        parent: prov.parent.map(|p| nodes[p].label.clone()),
                        field: prov.field.clone(),
                    };
                    (o.clone(), path)
                })
                .collect();
            (n.label.clone(), paths)
        })
        .collect()
}

/// Particles a model or clip embeds (RERL), through the models it embeds in
/// turn: whoever spawns the model draws them.
fn embedded_particles(
    direct: &BTreeMap<String, Vec<String>>,
    models: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for entry in direct.keys().chain(models.keys()) {
        let mut seen: HashSet<&String> = HashSet::new();
        let mut queue = vec![entry];
        let mut particles: BTreeSet<String> = BTreeSet::new();
        while let Some(m) = queue.pop() {
            if seen.insert(m) {
                particles.extend(direct.get(m).into_iter().flatten().cloned());
                queue.extend(models.get(m).into_iter().flatten());
            }
        }
        if !particles.is_empty() {
            out.insert(entry.clone(), particles.into_iter().collect());
        }
    }
    out
}

/// Root particles per owner with the node or model that names each, and the
/// field holding the path: the particles of every node the owner holds (plus
/// those of the models such a node names), and the particles a released hero's
/// body model and clips spawn.
fn owner_roots(
    nodes: &[VdataNode],
    owners: &[BTreeMap<Owner, Prov>],
    heroes: &[HeroDef],
    entry_particles: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<Owner, BTreeMap<String, (String, String)>> {
    let mut roots: BTreeMap<Owner, BTreeMap<String, (String, String)>> = BTreeMap::new();
    for (node, owned) in nodes.iter().zip(owners) {
        let field = |resource: &String| node.fields.get(resource).cloned().unwrap_or_default();
        for owner in owned.keys() {
            let own = roots.entry(owner.clone()).or_default();
            for p in &node.particles {
                own.entry(p.clone())
                    .or_insert_with(|| (node.label.clone(), field(p)));
            }
            for model in &node.models {
                for p in entry_particles.get(model).into_iter().flatten() {
                    own.entry(p.clone())
                        .or_insert_with(|| (format!("{} via {model}", node.label), field(model)));
                }
            }
        }
    }
    for hero in heroes {
        let Some(model) = hero.model.as_deref() else {
            continue;
        };
        let dir = model
            .rsplit_once('/')
            .map_or(String::new(), |(d, _)| format!("{d}/"));
        let own = roots.entry(Owner::Hero(hero.code.clone())).or_default();
        for (entry, ps) in entry_particles {
            if entry == model || (!dir.is_empty() && entry.starts_with(&dir)) {
                for p in ps {
                    own.entry(p.clone())
                        .or_insert_with(|| (entry.clone(), String::new()));
                }
            }
        }
    }
    roots
}

/// Every particle each owner can spawn, closed over child systems, with how it
/// got there (shortest chain first).
fn particle_reach(
    roots: BTreeMap<Owner, BTreeMap<String, (String, String)>>,
    children: &HashMap<String, Vec<String>>,
) -> HashMap<String, BTreeMap<Owner, Step>> {
    let mut reach: HashMap<String, BTreeMap<Owner, Step>> = HashMap::new();
    for (owner, roots) in roots {
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, Step)> = roots
            .into_iter()
            .map(|(p, (root, field))| {
                (
                    p,
                    Step {
                        parent: None,
                        root,
                        field,
                    },
                )
            })
            .collect();
        while let Some((p, step)) = queue.pop_front() {
            if !seen.insert(p.clone()) {
                continue;
            }
            for kid in children.get(&p).into_iter().flatten() {
                if !seen.contains(kid) {
                    queue.push_back((
                        kid.clone(),
                        Step {
                            parent: Some(p.clone()),
                            root: step.root.clone(),
                            field: step.field.clone(),
                        },
                    ));
                }
            }
            reach.entry(p).or_default().insert(owner.clone(), step);
        }
    }
    reach
}

/// A QA run over one or more heroes.
#[derive(Debug, Clone, Serialize)]
pub struct QaRun {
    pub heroes: Vec<HeroQa>,
    /// Selectable heroes in `scripts/heroes.vdata_c` that no recipe covers, as
    /// `codename (Display Name)`.
    pub unpinned_selectable: Vec<String>,
}

/// Audits each codename (all pinned heroes when empty) against `vpk`, with
/// `base` as the fallback pak for entries `vpk` does not ship.
pub fn audit_heroes(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    codenames: &[String],
    probe_hue: f64,
) -> Result<QaRun> {
    let roster_pak = base.unwrap_or(vpk.as_ref());
    let unpinned_selectable = crate::localization::build_hero_roster(roster_pak, None, "english")
        .map(|roster| {
            roster
                .into_iter()
                .filter(|h| h.selectable)
                .filter(|h| {
                    !pinned_hero_codenames()
                        .iter()
                        .any(|c| names_for(c).contains(&h.codename))
                })
                .map(|h| format!("{} ({})", h.codename, h.name))
                .collect()
        })
        .unwrap_or_default();
    let heroes = audit_pinned(vpk.as_ref(), base, codenames, probe_hue)?;
    Ok(QaRun {
        heroes,
        unpinned_selectable,
    })
}

fn audit_pinned(
    vpk: &Path,
    base: Option<&Path>,
    codenames: &[String],
    probe_hue: f64,
) -> Result<Vec<HeroQa>> {
    let vpks = open_vpks(vpk, base)?;
    // `game/citadel/pak01_dir.vpk` -> `game/citadel/bin/win64`, where the DLLs
    // that spawn particles by hardcoded path live.
    let game_bin = base
        .unwrap_or(vpk)
        .parent()
        .map(|d| d.join("bin").join("win64"))
        .filter(|d| d.is_dir());
    let index = RefIndex::build(&vpks, game_bin.as_deref());
    let all: Vec<String> = if codenames.is_empty() {
        pinned_hero_codenames()
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    } else {
        codenames.iter().map(|c| c.to_ascii_lowercase()).collect()
    };
    let recipes: Vec<HeroRecolorRecipe> = pinned_hero_codenames()
        .iter()
        .filter_map(|c| recipe_for(c))
        .collect();
    all.iter()
        .map(|code| {
            let recipe = recipe_for(code).with_context(|| {
                format!(
                    "no recolor recipe for {code:?} (pinned: {})",
                    pinned_hero_codenames().join(", ")
                )
            })?;
            Ok(audit_hero(
                &vpks,
                &index,
                &recipe,
                &recipes,
                Recolor::hue(probe_hue),
            ))
        })
        .collect()
}

struct Hero<'a> {
    recipe: &'a HeroRecolorRecipe,
    names: Vec<String>,
    body_model: Option<String>,
    pinned: BTreeSet<String>,
    /// Models the hero's particles render (props, the body, preview models).
    reached: BTreeSet<String>,
}

impl Hero<'_> {
    fn owns(&self, path: &str) -> bool {
        path_owned_by(&self.names, path)
    }

    fn in_prefixes(&self, path: &str) -> bool {
        self.recipe
            .particle_prefixes
            .iter()
            .any(|p| path.starts_with(p.as_str()))
    }

    fn direct(&self, user: &str) -> bool {
        self.in_prefixes(user)
            || self.owns(user)
            || self.pinned.contains(user)
            || self.body_model.as_deref() == Some(user)
            || self.reached.contains(user)
    }

    /// Whether a resource that uses a pinned asset belongs to this hero. A
    /// particle or model must be the hero's directly. A material is judged by
    /// what renders it, whatever its name: `pocket_satchel_projected.vmat` is
    /// also drawn by neutral telegraphs, while `ice_dome.vmat` is only drawn by
    /// the hero's dome model.
    /// What outside the hero actually renders through `user`: the user itself
    /// when it is a particle or model the hero doesn't own (or a particle of the
    /// hero that someone else spawns too), or the foreign renderers of a material
    /// (`material via renderer`). Particles nothing spawns are skipped: they never
    /// draw.
    fn foreign_renderers(&self, index: &RefIndex, user: &str) -> Vec<String> {
        let foreign = |u: &str| {
            (!self.direct(u)
                || (u.ends_with(".vpcf_c") && !index.foreign_owners(u, &self.names).is_empty()))
                && index.renders(u)
        };
        if !user.ends_with(".vmat_c") {
            return if foreign(user) {
                vec![user.to_owned()]
            } else {
                Vec::new()
            };
        }
        let up = index.users(user);
        if up.is_empty() {
            return if self.owns(user) || self.pinned.contains(user) {
                Vec::new()
            } else {
                vec![user.to_owned()]
            };
        }
        up.iter()
            .filter(|u| foreign(u))
            .map(|u| format!("{user} via {u}"))
            .collect()
    }
}

/// Every name a hero goes by, from either side of [`ALIASES`]: a recipe
/// codename or the key `heroes.vdata_c` uses (`atlas`, `forge`).
fn names_for(codename: &str) -> Vec<String> {
    let mut names = vec![codename.to_owned()];
    for (c, extra) in ALIASES
        .iter()
        .filter(|(c, extra)| *c == codename || extra.contains(&codename))
    {
        for n in std::iter::once(c).chain(extra.iter()) {
            if !names.iter().any(|x| x == n) {
                names.push((*n).to_owned());
            }
        }
    }
    names
}

fn path_owned_by(names: &[String], path: &str) -> bool {
    path.split(['/', '_', '.'])
        .any(|t| names.iter().any(|n| n == t))
}

/// What the bake does about one hero's shared entries (`isolate_bake`).
#[derive(Debug, Default, PartialEq, Eq)]
struct IsolationPlan {
    /// Shared entry -> the files the bake repoints at its copy.
    copies: BTreeMap<String, BTreeSet<String>>,
    /// Shared entries the bake would write but no private file reaches (a vdata
    /// node, a clip or a DLL string names them): they stay vanilla for the hero.
    vanilla: BTreeSet<String>,
}

impl IsolationPlan {
    /// Shared entries the bake writes a copy of, or would if something reached them.
    fn needed(&self) -> impl Iterator<Item = &String> {
        self.copies.keys().chain(&self.vanilla)
    }
}

/// Mirrors the bake. Shared entries are never written at their own path, only
/// as copies, and a copy exists only when the bake changed the entry or it
/// leads, through links between shared entries, to one it changed (an unchanged
/// shared leaf keeps pointing at the vanilla file). `private` files (walked
/// particles and pinned materials that are not shared) are edited in place, so
/// the bake can repoint them; a copy is reached from a private file or from
/// another copy. `refs` are the RERL references of particles and materials, the
/// only files the bake repoints.
fn plan_isolation(
    refs: &HashMap<String, Vec<String>>,
    private: &BTreeSet<String>,
    shared: &BTreeSet<String>,
    changed: &BTreeSet<String>,
) -> IsolationPlan {
    let mut leads: BTreeSet<&String> = shared.iter().filter(|s| changed.contains(*s)).collect();
    loop {
        let more: Vec<&String> = shared
            .iter()
            .filter(|s| !leads.contains(s))
            .filter(|s| {
                refs.get(*s)
                    .into_iter()
                    .flatten()
                    .any(|k| leads.contains(k))
            })
            .collect();
        if more.is_empty() {
            break;
        }
        leads.extend(more);
    }
    let mut copies: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut queue: VecDeque<&String> = private.iter().collect();
    while let Some(parent) = queue.pop_front() {
        for kid in refs.get(parent).into_iter().flatten() {
            if let Some(kid) = leads.get(kid).copied() {
                if !copies.contains_key(kid) {
                    queue.push_back(kid);
                }
                copies
                    .entry(kid.clone())
                    .or_default()
                    .insert(parent.clone());
            }
        }
    }
    let vanilla = leads
        .into_iter()
        .filter(|s| !copies.contains_key(*s))
        .cloned()
        .collect();
    IsolationPlan { copies, vanilla }
}

/// The shortest `shared_entries` that covers `required` and nothing else:
/// `universe` is every particle in the pak (sorted), and a path prefix replaces
/// a run of files only when every particle under it is required. Entries outside
/// the universe (textures, materials, models) are kept verbatim.
fn collapse_prefixes(required: &BTreeSet<String>, universe: &[String]) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut covered: BTreeSet<&String> = BTreeSet::new();
    for path in required {
        if covered.contains(path) {
            continue;
        }
        if universe.binary_search(path).is_err() {
            out.insert(path.clone());
            continue;
        }
        let cuts = path
            .char_indices()
            .filter_map(|(i, c)| match c {
                '/' | '_' | '-' => Some(i + 1),
                '.' => Some(i),
                _ => None,
            })
            .chain([path.len()]);
        let best = cuts
            .map(|n| &path[..n])
            .find(|prefix| {
                let from = universe.partition_point(|u| u.as_str() < *prefix);
                universe[from..]
                    .iter()
                    .take_while(|u| u.starts_with(prefix))
                    .all(|u| required.contains(u))
            })
            .unwrap_or(path);
        let hits: Vec<&String> = required.iter().filter(|r| r.starts_with(best)).collect();
        if hits.len() > 1 {
            out.insert(best.to_owned());
            covered.extend(hits);
        } else {
            out.insert(path.clone());
        }
    }
    out.into_iter().collect()
}

/// `node.field (owner)` for each distinct foreign spawner; an owner that
/// reaches the node indirectly says how: `(owner: ability -[field]-> node)`.
fn foreign_labels(index: &RefIndex, foreign: &[(&Owner, &Step)]) -> BTreeSet<String> {
    foreign
        .iter()
        .map(|(owner, step)| {
            let key = short_root(&step.root);
            let field = step.field.trim_start_matches('.');
            let named = if field.is_empty() {
                key.to_owned()
            } else {
                format!("{key}.{field}")
            };
            match index.explain(&step.root, owner) {
                Some(how) => format!("{named} ({}: {how})", owner.label()),
                None => format!("{named} ({})", owner.label()),
            }
        })
        .collect()
}

fn foreign_summary(index: &RefIndex, foreign: &[(&Owner, &Step)]) -> String {
    shown(foreign_labels(index, foreign).iter().map(String::as_str))
}

/// The first three items comma-joined, then `...` when there are more.
fn shown<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    let all: Vec<&str> = items.into_iter().collect();
    let head = all.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    if all.len() > 3 {
        format!("{head}, ...")
    } else {
        head
    }
}

fn reviewed(codename: &str, entry: &str) -> Option<&'static str> {
    REVIEWED
        .iter()
        .find(|(c, p, _)| *c == codename && entry.starts_with(p))
        .map(|(_, _, why)| *why)
}

#[allow(clippy::too_many_lines)]
fn audit_hero(
    vpks: &[valve_pak::VPK],
    index: &RefIndex,
    recipe: &HeroRecolorRecipe,
    recipes: &[HeroRecolorRecipe],
    probe: Recolor,
) -> HeroQa {
    let lower = |s: &String| s.to_ascii_lowercase();
    let pinned: BTreeSet<String> = recipe
        .texture_entries
        .iter()
        .chain(&recipe.material_entries)
        .chain(&recipe.model_entries)
        .map(lower)
        .collect();
    let names = names_for(&recipe.codename);
    // The vdata key can be an alias (`hero_synth` is Pocket, `hero_atlas` Abrams).
    let body_model = names
        .iter()
        .find_map(|n| live_hero_model_from_vdata(vpks, n).ok())
        .map(|(m, _)| compiled_resource_path(&m).to_ascii_lowercase());
    let mut hero = Hero {
        recipe,
        names,
        body_model,
        pinned,
        reached: BTreeSet::new(),
    };
    let mut qa = HeroQa::new(recipe);

    let listed: Vec<String> = recipe.shared_entries.iter().map(lower).collect();
    let is_listed = |path: &str| listed.iter().any(|l| path.starts_with(l.as_str()));

    // ---- integrity: prefixes + particles
    let particles: Vec<&String> = index
        .particles
        .iter()
        .filter(|p| hero.in_prefixes(p))
        .collect();
    for prefix in &recipe.particle_prefixes {
        if !particles.iter().any(|p| p.starts_with(prefix.as_str())) {
            qa.integrity.push(Finding::new(
                prefix,
                "empty-prefix",
                "no .vpcf_c under this prefix",
            ));
        }
    }
    qa.particles = particles.len();
    let mut patchable: BTreeSet<String> = BTreeSet::new();
    let mut child_particles: BTreeSet<String> = BTreeSet::new();
    let mut sprite_use: BTreeMap<String, bool> = BTreeMap::new();
    let mut materials: BTreeSet<String> = BTreeSet::new();
    let mut models: BTreeSet<String> = BTreeSet::new();
    for entry in &particles {
        let Some(bytes) = read_entry(vpks, entry) else {
            continue;
        };
        // Coverage only follows particles the game spawns; a dead one renders
        // none of what it references.
        let live = index.live.contains_key(entry.as_str());
        if live {
            for r in external_refs(&bytes) {
                if r.ends_with(".vpcf_c") {
                    child_particles.insert(r);
                } else if r.ends_with(".vmat_c") {
                    materials.insert(r);
                } else if r.ends_with(".vmdl_c") {
                    models.insert(r);
                }
            }
        }
        match morphic::decode_kv3_resource(&bytes) {
            Ok(tree) if live => sprite_textures(&tree, &mut sprite_use),
            Ok(_) => {}
            Err(e) => {
                qa.integrity
                    .push(Finding::new(*entry, "undecodable", format!("{e}")));
                continue;
            }
        }
        match recolor_particle_bytes(&bytes, probe) {
            Ok(Some(out)) => {
                patchable.insert((*entry).clone());
                if !is_listed(entry) {
                    qa.particles_recolored += 1;
                }
                if let Err(e) = morphic::decode_kv3_resource(&out) {
                    qa.integrity.push(Finding::new(
                        *entry,
                        "output-undecodable",
                        format!("recolored particle no longer decodes: {e}"),
                    ));
                }
            }
            Ok(None) => {}
            Err(e) => qa
                .integrity
                .push(Finding::new(*entry, "unpatchable", format!("{e:#}"))),
        }
    }

    // Ability models the game spawns from vdata rather than from a particle
    // (Kelvin's dome blocker): a vdata node that spawns the hero's particles
    // also names its models.
    for node in &index.vdata_nodes {
        if node
            .iter()
            .any(|p| p.ends_with(".vpcf_c") && hero.in_prefixes(p))
        {
            models.extend(node.iter().filter(|p| p.ends_with(".vmdl_c")).cloned());
        }
    }
    hero.reached.clone_from(&models);
    let hero = hero;

    // Shared entries: a particle under the prefixes that another released hero,
    // an item or a neutral can also spawn, and a pinned texture or material
    // something outside the hero uses, must not be recolored in place. The
    // recipe lists them in `shared_entries` and the bake copies the ones it
    // changes under a hero-local path, repointing the files it edits in place
    // (`private`: walked particles and pinned materials that are not shared).
    // What no private file reaches stays vanilla for the hero too.
    let mut safety: Vec<Finding> = Vec::new();
    let foreign_of = |p: &str| index.foreign_owners(p, &hero.names);
    let shared_in_prefix: BTreeSet<String> = particles
        .iter()
        .filter(|p| !foreign_of(p).is_empty())
        .map(|p| (*p).clone())
        .collect();
    qa.prefix_shared = shared_in_prefix.len();
    let pins: Vec<String> = recipe
        .texture_entries
        .iter()
        .chain(&recipe.material_entries)
        .map(lower)
        .collect();
    let pinned_shared: BTreeSet<String> = pins
        .iter()
        .filter(|k| {
            let (foreign, others) = sharing(&hero, index, recipes, k);
            !foreign.is_empty() || !others.is_empty()
        })
        .cloned()
        .collect();
    let listed_particles: BTreeSet<&String> =
        index.particles.iter().filter(|p| is_listed(p)).collect();
    let shared: BTreeSet<String> = shared_in_prefix
        .iter()
        .cloned()
        .chain(listed_particles.iter().map(|p| (*p).clone()))
        .chain(pinned_shared.iter().cloned())
        .chain(pins.iter().filter(|k| is_listed(k)).cloned())
        .collect();
    let changed: BTreeSet<String> = shared
        .iter()
        .filter(|s| {
            !s.ends_with(".vpcf_c")
                || patchable.contains(*s)
                || (!hero.in_prefixes(s) && carries_particle_color(vpks, s, probe))
        })
        .cloned()
        .collect();
    let private: BTreeSet<String> = particles
        .iter()
        .map(|p| (*p).clone())
        .chain(recipe.material_entries.iter().map(lower))
        .filter(|p| !shared.contains(p))
        .collect();
    let plan = plan_isolation(&index.refs, &private, &shared, &changed);
    let mut required: BTreeSet<String> = BTreeSet::new();
    for entry in plan.needed().filter(|e| e.ends_with(".vpcf_c")) {
        let foreign = foreign_of(entry);
        if foreign.is_empty() {
            continue;
        }
        let in_prefix = shared_in_prefix.contains(entry);
        if in_prefix {
            required.insert(entry.clone());
        }
        let spawners = foreign_summary(index, &foreign);
        let hero_way = index
            .hero_owner(entry, &hero.names)
            .map(|(owner, _)| index.chain(entry, owner));
        let copy = plan.copies.get(entry);
        qa.isolation.push(Isolation {
            entry: entry.clone(),
            kind: if copy.is_some() { "copy" } else { "vanilla" }.to_owned(),
            referrers: copy.into_iter().flatten().cloned().collect(),
            shared_with: foreign
                .iter()
                .map(|(owner, _)| owner.label())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            spawned_by: foreign_labels(index, &foreign).into_iter().collect(),
        });
        let stays = match &hero_way {
            Some(way) => format!("the hero reaches it only through {way}, which the bake cannot repoint, so it stays vanilla"),
            None => "no vdata node, body model or clip of the hero names it, so it stays vanilla"
                .to_owned(),
        };
        if !is_listed(entry) {
            let fix = match copy {
                Some(refs) => format!(
                    "list it in shared_entries: the bake copies it and repoints {}",
                    shown(refs.iter().map(String::as_str))
                ),
                None => format!("list it in shared_entries: {stays}"),
            };
            safety.push(Finding::new(
                entry,
                "bleed",
                format!("also spawned by {spawners}; {fix}"),
            ));
        } else if copy.is_none() {
            qa.limits.push(Finding::new(
                entry,
                "vanilla-for-hero",
                format!("also spawned by {spawners}; {stays}"),
            ));
        }
    }
    let iso = Isolator {
        hero: &hero,
        index,
        recipes,
        listed: &listed,
        plan: &plan,
        shared: &shared,
    };

    // ---- integrity + safety: pinned textures
    let mut material_cache: HashMap<String, Option<Material>> = HashMap::new();
    let mut load_material = |entry: &str| -> Option<Material> {
        material_cache
            .entry(entry.to_owned())
            .or_insert_with(|| {
                read_entry(vpks, entry).and_then(|b| morphic::material::parse(&b).ok())
            })
            .clone()
    };
    let body_materials: BTreeSet<String> = hero
        .body_model
        .as_deref()
        .map(|m| model_materials(vpks, m))
        .unwrap_or_default();
    let skin_textures: BTreeSet<String> = body_materials
        .iter()
        .filter_map(|m| load_material(m))
        .filter(|m| !material_glows(m))
        .flat_map(|m| {
            m.texture_params
                .into_iter()
                .filter(|(slot, _)| slot == "g_tColor")
                .map(|(_, t)| compiled_resource_path(&t).to_ascii_lowercase())
        })
        .collect();

    for entry in &recipe.texture_entries {
        let key = entry.to_ascii_lowercase();
        let Some(bytes) = read_entry(vpks, entry) else {
            qa.integrity
                .push(Finding::new(entry, "stale", stale_detail(vpks, entry)));
            continue;
        };
        let source = top_rgba(&bytes);
        let format =
            morphic::inspect(&bytes).map_or_else(|_| "?".to_owned(), |i| format!("{:?}", i.format));
        for (path, baked) in texture_bakes(&recipe.codename, entry, &bytes, probe) {
            let out = match baked {
                Ok(out) => out,
                Err(e) => {
                    qa.integrity
                        .push(Finding::new(entry, "unpatchable", format!("{path}: {e:#}")));
                    continue;
                }
            };
            if let Err(e) = morphic::decode(&out) {
                qa.integrity.push(Finding::new(
                    entry,
                    "output-undecodable",
                    format!("{path} output no longer decodes: {e}"),
                ));
                continue;
            }
            let (Some(src), Some(dst)) = (source.as_ref(), top_rgba(&out)) else {
                continue;
            };
            let drift = silhouette_drift(src, &dst);
            if drift.lit > SQUARE_LIT_SHARE {
                safety.push(Finding::new(
                    entry,
                    "squares",
                    format!(
                        "{path} lights {:.0}% of the black margin ({} texels): the quad renders as a square",
                        drift.lit * 100.0,
                        drift.margin
                    ),
                ));
            }
            if drift.alpha > ALPHA_DRIFT_SHARE {
                safety.push(Finding::new(
                    entry,
                    "alpha",
                    format!(
                        "{path} moves alpha on {:.1}% of texels ({format} re-encode): the cut-out/fade shape changes",
                        drift.alpha * 100.0
                    ),
                ));
            }
        }
        match texture_chroma(&bytes) {
            Some(c) if !c.chromatic() => qa.noops.push(Finding::new(
                entry,
                "achromatic",
                format!("{c}: a hue set leaves it unchanged"),
            )),
            None => qa
                .noops
                .push(Finding::new(entry, "achromatic", "undecodable for chroma")),
            _ => {}
        }
        if let Some(materials) = inert_selfillum(index, &key, &mut load_material) {
            qa.noops.push(Finding::new(
                entry,
                "inert-selfillum",
                format!(
                    "only used by self-illum slots of material(s) whose g_flSelfIllumScale is a static 0 with no expression ({}): it never renders",
                    shown(materials.iter().map(String::as_str))
                ),
            ));
        }
        if sprite_use.get(&key) == Some(&false) {
            safety.push(Finding::new(
                entry,
                "data-channel",
                "every hero particle samples it as a single channel (alpha/mask), not color",
            ));
        }
        for user in index.users(&key) {
            if user.ends_with(".vmat_c") {
                if let Some(slot) = load_material(user).and_then(|m| data_slot_for(&m, &key)) {
                    safety.push(Finding::new(
                        entry,
                        "data-channel",
                        format!("bound as data slot {slot} in {user}"),
                    ));
                }
            }
        }
        if index.users(&key).iter().all(|u| orphan_material(index, u)) {
            qa.noops.push(Finding::new(
                entry,
                "orphan",
                "no particle, or material something renders, references it; the game never samples it",
            ));
        }
        if skin_textures.contains(&key) {
            safety.push(Finding::new(
                entry,
                "skin",
                "the live body model's albedo (g_tColor of a solid material)",
            ));
        }
        iso.pinned(entry, &key, &mut safety, &mut qa, &mut required);
    }

    // ---- integrity + safety: pinned materials
    for entry in &recipe.material_entries {
        let key = entry.to_ascii_lowercase();
        let Some(bytes) = read_entry(vpks, entry) else {
            qa.integrity
                .push(Finding::new(entry, "stale", stale_detail(vpks, entry)));
            continue;
        };
        match recolor_material_color_bytes(&bytes, probe) {
            Ok(Some(out)) => {
                if let Err(e) = morphic::material::parse(&out) {
                    qa.integrity.push(Finding::new(
                        entry,
                        "output-undecodable",
                        format!("stamped material no longer decodes: {e}"),
                    ));
                }
            }
            Ok(None) => qa.noops.push(Finding::new(
                entry,
                "no-tint",
                "no stampable g_vColorTint / g_vSelfIllumTint",
            )),
            Err(e) => qa
                .integrity
                .push(Finding::new(entry, "unpatchable", format!("{e:#}"))),
        }
        if let Some(m) = load_material(&key) {
            let dynamic: Vec<&String> = m.dynamic_params.keys().filter(|k| is_tint(k)).collect();
            let tints = m.vector_params.keys().filter(|k| is_tint(k)).count();
            if !dynamic.is_empty() && dynamic.len() == tints {
                qa.noops.push(Finding::new(
                    entry,
                    "dynamic-tint",
                    format!("{dynamic:?} driven by an expression; the static stamp is overridden"),
                ));
            }
        }
        if index.users(&key).is_empty() {
            qa.noops.push(Finding::new(
                entry,
                "orphan",
                "no particle, model or mesh references it; the game never renders it",
            ));
        }
        if is_ui_signal(&key) {
            safety.push(Finding::new(
                entry,
                "ui-signal",
                "placement/validity cue; recoloring it can read as the wrong state",
            ));
        }
        iso.pinned(entry, &key, &mut safety, &mut qa, &mut required);
    }

    // ---- integrity: pinned models
    for entry in &recipe.model_entries {
        let Some(bytes) = read_entry(vpks, entry) else {
            qa.integrity
                .push(Finding::new(entry, "stale", stale_detail(vpks, entry)));
            continue;
        };
        match crate::recolor::recolor_model_vertex_colors(&bytes, probe) {
            Ok((_, s)) if s.buffers_recolored == 0 => qa.noops.push(Finding::new(
                entry,
                "colorless",
                "no color-bearing vertex buffer",
            )),
            Ok(_) => {}
            Err(e) => qa
                .integrity
                .push(Finding::new(entry, "unpatchable", format!("{e:#}"))),
        }
    }

    if let Some(preview) = &recipe.preview_texture {
        if !recipe.texture_entries.contains(preview) {
            qa.integrity.push(Finding::new(
                preview,
                "preview",
                "preview_texture is not one of texture_entries",
            ));
        }
    }

    // ---- coverage
    let mut gaps: Vec<Finding> = Vec::new();

    // Hero-named particles outside the prefixes, and child systems the hero's
    // particles pull in from elsewhere.
    let claimed_by_other = |p: &str| {
        recipes
            .iter()
            .filter(|r| r.codename != recipe.codename)
            .any(|r| {
                r.particle_prefixes
                    .iter()
                    .any(|pre| p.starts_with(pre.as_str()))
            })
    };
    let mut outside: BTreeSet<String> = index
        .particles
        .iter()
        .filter(|p| hero.owns(p) && !hero.in_prefixes(p) && !claimed_by_other(p))
        .cloned()
        .collect();
    let children: Vec<String> = child_particles
        .iter()
        .filter(|p| !hero.in_prefixes(p) && !outside.contains(*p))
        .cloned()
        .collect();
    let mut unreferenced = 0usize;
    for entry in std::mem::take(&mut outside) {
        if !carries_particle_color(vpks, &entry, probe) {
            continue;
        }
        if let Some(source) = index.live.get(&entry) {
            // Only this hero's spawners make it a gap; a hero-named file another
            // hero, an item, a neutral or an unbound (in-development) ability
            // spawns belongs to them.
            let ours = !index.spawners.contains_key(&entry)
                || index.hero_owner(&entry, &hero.names).is_some();
            if ours {
                gaps.push(Finding::gap(
                    &entry,
                    "particle",
                    format!("live hero-named particle outside the prefixes; spawned via {source}"),
                    "particle_prefixes",
                ));
            } else {
                qa.limits.push(Finding::new(
                    &entry,
                    "not-hero-spawned",
                    format!("hero-named but spawned only by someone else's or unbound abilities, via {source}"),
                ));
            }
        } else {
            unreferenced += 1;
            qa.limits.push(Finding::new(
                &entry,
                "unreferenced",
                "hero-named particle with color outside the prefixes that no vdata, clip, \
                 model, DLL or live particle reaches (legacy?)",
            ));
        }
    }
    qa.unreferenced_particles = unreferenced;
    for entry in children {
        if !carries_particle_color(vpks, &entry, probe) {
            continue;
        }
        if claimed_by_other(&entry) {
            qa.limits.push(Finding::new(
                &entry,
                "shared-child",
                "child particle owned by another hero's recipe",
            ));
        } else {
            qa.limits.push(Finding::new(
                &entry,
                "shared-child",
                "generic child particle with color; recoloring in place bleeds to other effects",
            ));
        }
    }

    // Materials the hero renders: particle refs, prop models, the body model.
    let mut prop_models: Vec<String> = Vec::new();
    for model in &models {
        if hero.body_model.as_deref() != Some(model.as_str()) && hero.owns(model) {
            materials.extend(model_materials(vpks, model));
            prop_models.push(model.clone());
        }
    }
    materials.extend(body_materials.iter().cloned());

    let mut color_textures: BTreeMap<String, String> = BTreeMap::new();
    for (tex, color) in &sprite_use {
        if *color {
            color_textures.insert(
                tex.clone(),
                "sampled as color by the hero's particles".into(),
            );
        }
    }
    for mat_entry in &materials {
        let Some(m) = load_material(mat_entry) else {
            continue;
        };
        let glow = material_glows(&m);
        let body = body_materials.contains(mat_entry);
        let layer = flag(&m, "F_ADDITIVE_BLEND") || flag(&m, "F_UNLIT");
        let tinted = static_tint_saturation(&m).filter(|s| *s >= TINT_SATURATION);
        let candidate = glow && hero.owns(mat_entry) && !hero.pinned.contains(mat_entry);
        if let (true, Some(sat)) = (candidate, tinted) {
            if !body {
                gaps.push(Finding::gap(
                    mat_entry,
                    "material",
                    format!("additive/self-illum glow with a tint constant (saturation {sat:.2})"),
                    "material_entries",
                ));
            } else if layer {
                gaps.push(Finding::gap(
                    mat_entry,
                    "body-glow",
                    format!("additive glow layer on the body model (tint saturation {sat:.2})"),
                    "material_entries",
                ));
            } else {
                qa.limits.push(Finding::new(
                    mat_entry,
                    "body-accent",
                    format!(
                        "self-illum accent on a body surface (tint saturation {sat:.2}); \
                         skin identity unless it reads as ability color"
                    ),
                ));
            }
        }
        // A solid surface's albedo (prop wood, the skin) is surface color, not an
        // ability color. On the body only additive layers count: a self-illum
        // mask on the head still leaves the head albedo skin.
        if !glow || (body && !layer) {
            continue;
        }
        for (slot, tex) in &m.texture_params {
            if is_color_slot(slot) {
                color_textures
                    .entry(compiled_resource_path(tex).to_ascii_lowercase())
                    .or_insert_with(|| format!("{slot} of {mat_entry}"));
            }
        }
    }
    for (tex, via) in &color_textures {
        if hero.pinned.contains(tex)
            || tex.starts_with("materials/default/")
            || inert_selfillum(index, tex, &mut load_material).is_some()
        {
            continue;
        }
        let Some(c) = read_entry(vpks, tex).and_then(|b| texture_chroma(&b)) else {
            continue;
        };
        if !c.chromatic() {
            continue;
        }
        if hero.owns(tex) {
            gaps.push(Finding::gap(
                tex,
                "texture",
                format!("{c}; {via}"),
                "texture_entries",
            ));
        } else {
            qa.limits.push(Finding::new(
                tex,
                "shared-sheet",
                format!("{c}; {via}; shared, needs a copy + repoint to recolor"),
            ));
        }
    }

    for model in &prop_models {
        if hero.pinned.contains(model) {
            continue;
        }
        let Some(bytes) = read_entry(vpks, model) else {
            continue;
        };
        // Vertex color is an effect color only through an additive/unlit
        // material that reads it; a solid prop's tinted wood (even one with a
        // masked self-illum strip) is surface color.
        let shows_vertex_color = model_materials(vpks, model)
            .iter()
            .filter_map(|r| load_material(r))
            .any(|m| {
                (flag(&m, "F_ADDITIVE_BLEND") || flag(&m, "F_UNLIT")) && flag(&m, "F_VERTEX_COLOR")
            });
        if !shows_vertex_color {
            continue;
        }
        if let Ok((_, s)) = crate::recolor::recolor_model_vertex_colors(&bytes, probe) {
            if s.buffers_recolored > 0 {
                gaps.push(Finding::gap(
                    model,
                    "model",
                    format!("{} vertices of baked chromatic vertex color", s.vertices),
                    "model_entries",
                ));
            }
        }
    }

    // A texture/material candidate something outside the hero also renders
    // would fail the shared check if pinned: report it as a limit instead.
    let (gaps, shared): (Vec<Finding>, Vec<Finding>) = gaps.into_iter().partition(|f| {
        f.kind == "particle"
            || index
                .users(&f.entry)
                .iter()
                .all(|u| hero.foreign_renderers(index, u).is_empty())
    });
    for f in shared {
        let foreign: Vec<String> = index
            .users(&f.entry)
            .iter()
            .flat_map(|u| hero.foreign_renderers(index, u))
            .take(2)
            .collect();
        qa.limits.push(Finding::new(
            f.entry,
            "shared-candidate",
            format!(
                "{}; also rendered by {}, so pinning it would bleed",
                f.detail,
                foreign.join(", ")
            ),
        ));
    }

    // ---- reviews
    let mut used_reviews: BTreeSet<&str> = BTreeSet::new();
    for f in gaps {
        if let Some(why) = reviewed(&recipe.codename, &f.entry) {
            used_reviews.insert(review_key(&recipe.codename, &f.entry));
            qa.excluded.push(Finding {
                detail: format!("{} (reviewed: {why})", f.detail),
                ..f
            });
        } else {
            qa.gaps.push(f);
        }
    }
    for f in safety {
        if let Some(why) = reviewed(&recipe.codename, &f.entry) {
            used_reviews.insert(review_key(&recipe.codename, &f.entry));
            qa.accepted.push(Finding {
                detail: format!("{} (reviewed: {why})", f.detail),
                ..f
            });
        } else {
            qa.violations.push(f);
        }
    }
    for (c, p, _) in REVIEWED {
        if *c == recipe.codename && !used_reviews.contains(p) {
            qa.stale_reviews.push((*p).to_owned());
        }
    }

    for pattern in &recipe.shared_entries {
        let pat = pattern.to_ascii_lowercase();
        let hits = qa
            .isolation
            .iter()
            .filter(|i| i.entry.starts_with(&pat))
            .count();
        let private_hits: Vec<&str> = particles
            .iter()
            .filter(|p| p.starts_with(&pat) && foreign_of(p).is_empty())
            .map(|p| p.as_str())
            .collect();
        if !index.particles.iter().any(|p| p.starts_with(&pat))
            && !index.users.keys().any(|u| u.starts_with(&pat))
        {
            qa.integrity.push(Finding::new(
                pattern,
                "stale",
                "shared_entries pattern matches nothing in the pak",
            ));
        } else if hits == 0 {
            qa.noops.push(Finding::new(
                pattern,
                "isolation-unneeded",
                "nothing under it needs isolating: no foreign renderer uses what the bake changes there",
            ));
        } else if !private_hits.is_empty() {
            qa.noops.push(Finding::new(
                pattern,
                "isolation-unneeded",
                format!(
                    "also covers {} particle(s) only this hero spawns ({}): the bake would not edit them in place",
                    private_hits.len(),
                    shown(private_hits.iter().copied())
                ),
            ));
        }
    }
    qa.copies_recolored = plan
        .copies
        .keys()
        .filter(|c| c.ends_with(".vpcf_c") && is_listed(c) && changed.contains(*c))
        .count();
    qa.isolation.sort_by(|a, b| a.entry.cmp(&b.entry));
    qa.suggested_shared = collapse_prefixes(&required, &index.particles);

    let carriers = qa.particles_recolored
        + qa.copies_recolored
        + recipe.texture_entries.len()
        + recipe.material_entries.len()
        + recipe.model_entries.len();
    #[allow(clippy::cast_precision_loss)]
    {
        qa.coverage = carriers as f32 / (carriers + qa.gaps.len()).max(1) as f32;
    }
    qa.status = if !qa.integrity.is_empty() || !qa.violations.is_empty() {
        QaStatus::Fail
    } else if !qa.gaps.is_empty() || !qa.noops.is_empty() || !qa.stale_reviews.is_empty() {
        QaStatus::Warn
    } else {
        QaStatus::Pass
    };
    qa
}

/// The materials that make `texture` inert: every live use of it is a
/// `g_tSelfIllum*` slot of a material whose `g_flSelfIllumScale*` is a static 0
/// no dynamic expression or `$SELFILLUM` render attribute drives (a decal the
/// particle feeds through `$SELFILLUM` does render). `None` when any live use is
/// something else.
fn inert_selfillum(
    index: &RefIndex,
    texture: &str,
    load: &mut dyn FnMut(&str) -> Option<Material>,
) -> Option<Vec<String>> {
    let mut inert = Vec::new();
    for user in index.users(texture) {
        if !user.ends_with(".vmat_c") {
            return None;
        }
        let users = index.users(user);
        if orphan_material(index, user)
            || (!users.is_empty() && !users.iter().any(|u| index.renders(u)))
        {
            continue;
        }
        let m = load(user)?;
        let on_selfillum = |slot: &String| slot.to_ascii_lowercase().starts_with("g_tselfillum");
        let slots: Vec<&String> = m
            .texture_params
            .iter()
            .filter(|(_, t)| compiled_resource_path(t).eq_ignore_ascii_case(texture))
            .map(|(slot, _)| slot)
            .collect();
        let scales: Vec<f32> = m
            .float_params
            .iter()
            .filter(|(k, _)| k.starts_with("g_flSelfIllumScale"))
            .map(|(_, v)| *v)
            .collect();
        let driven = m
            .dynamic_params
            .keys()
            .any(|k| k.starts_with("g_flSelfIllumScale"))
            || m.render_attributes_used
                .iter()
                .any(|a| a.to_ascii_uppercase().contains("SELFILLUM"));
        if slots.is_empty()
            || !slots.iter().all(|s| on_selfillum(s))
            || scales.is_empty()
            || scales.iter().any(|v| v.abs() > f32::EPSILON)
            || driven
        {
            return None;
        }
        inert.push(user.clone());
    }
    (!inert.is_empty()).then_some(inert)
}

/// A material nothing indexed renders. Only trusted for hero, ability and
/// particle materials, whose users (particles and the models in
/// [`MODEL_DIRS`]) are all indexed.
fn orphan_material(index: &RefIndex, entry: &str) -> bool {
    entry.ends_with(".vmat_c")
        && index.users(entry).is_empty()
        && (MODEL_DIRS.iter().any(|d| entry.starts_with(d))
            || entry.starts_with("materials/particle")
            || entry.starts_with("materials/models/particle"))
}

fn review_key(codename: &str, entry: &str) -> &'static str {
    REVIEWED
        .iter()
        .find(|(c, p, _)| *c == codename && entry.starts_with(p))
        .map_or("", |(_, p, _)| *p)
}

/// Resources outside the hero that use `key`, and the other recipes that pin it.
fn sharing<'a>(
    hero: &Hero,
    index: &RefIndex,
    recipes: &'a [HeroRecolorRecipe],
    key: &str,
) -> (Vec<String>, Vec<&'a str>) {
    let foreign = index
        .users(key)
        .iter()
        .flat_map(|u| hero.foreign_renderers(index, u))
        .collect();
    let others = recipes
        .iter()
        .filter(|r| r.codename != hero.recipe.codename)
        .filter(|r| {
            r.texture_entries
                .iter()
                .chain(&r.material_entries)
                .chain(&r.model_entries)
                .any(|p| p.eq_ignore_ascii_case(key))
        })
        .map(|r| r.codename.as_str())
        .collect();
    (foreign, others)
}

/// Judges pinned textures and materials that something outside the hero also
/// uses (or that another recipe pins). Unlisted, that is a `shared` violation:
/// the in-place recolor bleeds. Listed in `shared_entries`, the bake writes a
/// copy and repoints a referrer at it, which works only if the plan reaches the
/// entry from a file the bake edits anyway: a walked particle, a pinned
/// material, or another copy.
struct Isolator<'a> {
    hero: &'a Hero<'a>,
    index: &'a RefIndex,
    recipes: &'a [HeroRecolorRecipe],
    listed: &'a [String],
    plan: &'a IsolationPlan,
    shared: &'a BTreeSet<String>,
}

impl Isolator<'_> {
    fn is_listed(&self, path: &str) -> bool {
        self.listed.iter().any(|l| path.starts_with(l.as_str()))
    }

    /// Why a user of a shared asset cannot carry a repoint to its copy.
    fn blocker(&self, user: &str) -> &'static str {
        if self.shared.contains(user) {
            "a shared entry that stays vanilla"
        } else if user.ends_with(".vpcf_c") {
            "a particle outside the prefixes"
        } else if user.ends_with(".vmat_c") {
            "an unpinned material"
        } else if user.ends_with(".vmdl_c") || user.ends_with(".vmesh_c") {
            "a model, which the bake does not repoint"
        } else {
            "not a file the bake edits"
        }
    }

    fn pinned(
        &self,
        entry: &str,
        key: &str,
        safety: &mut Vec<Finding>,
        qa: &mut HeroQa,
        required: &mut BTreeSet<String>,
    ) {
        let (foreign, others) = sharing(self.hero, self.index, self.recipes, key);
        if !self.hero.owns(key) && foreign.is_empty() && self.index.users(key).is_empty() {
            safety.push(Finding::new(
                entry,
                "unattributed",
                "path does not name the hero and nothing in the pak references it",
            ));
        }
        if foreign.is_empty() && others.is_empty() {
            return;
        }
        let referrers: Vec<String> = self
            .plan
            .copies
            .get(key)
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        if !referrers.is_empty() {
            required.insert(key.to_owned());
            qa.isolation.push(Isolation {
                entry: key.to_owned(),
                kind: "pinned".to_owned(),
                referrers: referrers.clone(),
                shared_with: Vec::new(),
                spawned_by: Vec::new(),
            });
        }
        if !self.is_listed(key) {
            let unfixable = if referrers.is_empty() {
                "; listing it would not help, nothing the bake can repoint reaches it"
            } else {
                ""
            };
            if !foreign.is_empty() {
                safety.push(Finding::new(
                    entry,
                    "shared",
                    format!(
                        "also used by {} resource(s) outside the hero: {}{unfixable}",
                        foreign.len(),
                        shown(foreign.iter().map(String::as_str))
                    ),
                ));
            }
            for other in others {
                safety.push(Finding::new(
                    entry,
                    "shared",
                    format!("also pinned by the {other} recipe"),
                ));
            }
        } else if referrers.is_empty() {
            let users = self.index.users(key);
            safety.push(Finding::new(
                entry,
                "unreachable-isolation",
                format!(
                    "listed in shared_entries, but nothing the bake can repoint reaches it: {}",
                    if users.is_empty() {
                        "no file references it".to_owned()
                    } else {
                        shown(
                            users
                                .iter()
                                .map(|u| format!("{u} ({})", self.blocker(u)))
                                .collect::<Vec<_>>()
                                .iter()
                                .map(String::as_str),
                        )
                    }
                ),
            ));
        } else {
            qa.limits.push(Finding::new(
                entry,
                "isolated",
                format!(
                    "shared with {}; the bake copies it and repoints {}",
                    shown(
                        foreign
                            .iter()
                            .map(String::as_str)
                            .chain(others.iter().copied())
                    ),
                    shown(referrers.iter().map(String::as_str))
                ),
            ));
        }
    }
}

/// For a missing pinned entry, the most likely successor: same path with a
/// different trailing content hash.
fn stale_detail(vpks: &[valve_pak::VPK], entry: &str) -> String {
    let stem = |p: &str| -> String {
        let no_ext = p.rsplit_once('.').map_or(p, |(h, _)| h);
        match no_ext.rsplit_once('_') {
            Some((head, tail))
                if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_hexdigit()) =>
            {
                head.to_owned()
            }
            _ => no_ext.to_owned(),
        }
    };
    let want = stem(entry);
    let ext = entry.rsplit_once('.').map_or("", |(_, e)| e);
    let successor = vpks
        .iter()
        .flat_map(valve_pak::VPK::file_paths)
        .find(|p| p.ends_with(ext) && stem(p) == want);
    match successor {
        Some(s) => format!("missing from the pak; successor: {s}"),
        None => "missing from the pak; no successor with the same stem".to_owned(),
    }
}

fn carries_particle_color(vpks: &[valve_pak::VPK], entry: &str, probe: Recolor) -> bool {
    read_entry(vpks, entry)
        .and_then(|b| recolor_particle_bytes(&b, probe).ok())
        .flatten()
        .is_some()
}

/// Records every sprite texture a particle samples and whether any use reads
/// color (an RGB channel mode) rather than a single data channel, a
/// distortion/normal map, or the input of a refraction/screen-warp renderer.
fn sprite_textures(v: &Value, out: &mut BTreeMap<String, bool>) {
    sprite_textures_in(v, false, out);
}

fn sprite_textures_in(v: &Value, distort: bool, out: &mut BTreeMap<String, bool>) {
    match v {
        Value::Object(pairs) => {
            let distort = match v.get("_class").and_then(Value::as_str) {
                Some(class) if class.starts_with("C_OP_Render") => {
                    v.get("m_bRefract").and_then(Value::as_bool) == Some(true)
                        || ["Warp", "Refract", "Distort"]
                            .iter()
                            .any(|k| class.contains(k))
                }
                _ => distort,
            };
            if let Some(tex) = v.get("m_hTexture").and_then(Value::as_str) {
                let channels = v
                    .get("m_nTextureChannels")
                    .and_then(Value::as_str)
                    .unwrap_or("SPRITECARD_TEXTURE_CHANNEL_MIX_RGBA");
                let kind = v
                    .get("m_nTextureType")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let path = compiled_resource_path(tex).to_ascii_lowercase();
                let file = path.rsplit('/').next().unwrap_or(&path);
                let color = !distort
                    && channels.contains("RGB")
                    && !kind.contains("UVDISTORTION")
                    && !kind.contains("NORMAL")
                    && !file.contains("normal");
                *out.entry(path).or_insert(false) |= color;
            }
            for (_, child) in pairs {
                sprite_textures_in(child, distort, out);
            }
        }
        Value::Array(items) => items
            .iter()
            .for_each(|i| sprite_textures_in(i, distort, out)),
        _ => {}
    }
}

fn flag(m: &Material, name: &str) -> bool {
    m.int_params.get(name).copied().unwrap_or(0) != 0
}

/// Additive, unlit, self-illuminated at a nonzero scale, or not a surface
/// shader at all (projected decals, cables): a material whose color reads as an
/// effect color rather than a surface color.
fn material_glows(m: &Material) -> bool {
    m.shader_name != "pbr.vfx"
        || flag(m, "F_ADDITIVE_BLEND")
        || flag(m, "F_UNLIT")
        || (flag(m, "F_SELF_ILLUM")
            && m.float_params
                .get("g_flSelfIllumScale1")
                .copied()
                .unwrap_or(1.0)
                > 0.0)
}

fn is_tint(name: &str) -> bool {
    name.starts_with("g_vColorTint") || name.starts_with("g_vSelfIllumTint")
}

/// Highest HSV saturation among the tints the stamp would write, ignoring any
/// a dynamic expression drives (those follow the particle color).
fn static_tint_saturation(m: &Material) -> Option<f32> {
    m.vector_params
        .iter()
        .filter(|(name, _)| !m.dynamic_params.contains_key(*name))
        .filter(|(name, _)| is_tint(name))
        .map(|(_, v)| {
            let max = v[0].max(v[1]).max(v[2]);
            let min = v[0].min(v[1]).min(v[2]);
            if max <= 1e-6 {
                0.0
            } else {
                (max - min) / max
            }
        })
        .reduce(f32::max)
}

/// Texture slots that carry color: albedo/tint maps, detail and sheen layers,
/// self-illum and transmissive color. Masks and normals are data.
fn is_color_slot(slot: &str) -> bool {
    let s = slot.to_ascii_lowercase();
    let colorish = ["color", "detail", "sheen", "selfillum", "transmissive"]
        .iter()
        .any(|k| s.contains(k));
    colorish && !s.contains("mask") && !s.contains("normal")
}

fn data_slot_for(m: &Material, texture: &str) -> Option<String> {
    m.texture_params
        .iter()
        .find(|(slot, t)| {
            compiled_resource_path(t).eq_ignore_ascii_case(texture) && !is_color_slot(slot)
        })
        .map(|(slot, _)| slot.clone())
}

fn is_ui_signal(entry: &str) -> bool {
    let file = entry.rsplit('/').next().unwrap_or(entry);
    ["preview_bad", "_invalid", "placement_bad"]
        .iter()
        .any(|k| file.contains(k))
}

/// Every bake path that writes a pinned texture, run on it: the single-hue
/// recolor, the static and animated prism, and the trippy repaint.
fn texture_bakes(
    codename: &str,
    entry: &str,
    bytes: &[u8],
    probe: Recolor,
) -> Vec<(&'static str, Result<Vec<u8>>)> {
    let tuning = crate::hero_recolor::PrismTuning::default();
    let trippy = crate::trippy::TrippyAbilityOptions::default();
    vec![
        ("recolor", crate::recolor::recolor_texture_hue(bytes, probe)),
        (
            "prism",
            crate::hero_recolor::prism_recolor_texture_bytes(codename, entry, bytes, false, tuning),
        ),
        (
            "prism --animated",
            crate::hero_recolor::prism_recolor_texture_bytes(codename, entry, bytes, true, tuning),
        ),
        (
            "trippy-vfx",
            crate::trippy::repaint_ability_texture(bytes, &trippy),
        ),
    ]
}

/// How a bake changed a texture's shape, measured at a small mip.
struct Drift {
    /// Texels dark in the source (the margin that is the silhouette under
    /// additive blending).
    margin: usize,
    /// Share of those margin texels the bake lit up.
    lit: f32,
    /// Share of all texels whose alpha moved by more than BC noise.
    alpha: f32,
}

#[allow(clippy::cast_precision_loss)]
fn silhouette_drift(src: &Rgba, dst: &Rgba) -> Drift {
    let value = |p: &[f32; 4]| p[0].max(p[1]).max(p[2]);
    let (mut margin, mut lit, mut moved) = (0usize, 0usize, 0usize);
    for (s, d) in src.px.iter().zip(&dst.px) {
        if value(s) < MARGIN_VALUE {
            margin += 1;
            if value(d) > LIT_VALUE {
                lit += 1;
            }
        }
        if (s[3] - d[3]).abs() > ALPHA_NOISE {
            moved += 1;
        }
    }
    Drift {
        margin,
        lit: if margin == 0 {
            0.0
        } else {
            lit as f32 / margin as f32
        },
        alpha: moved as f32 / src.px.len().max(1) as f32,
    }
}

/// Decoded texels as RGBA floats in 0..1.
struct Rgba {
    px: Vec<[f32; 4]>,
}

/// The smallest mip whose longer edge is still at least [`CHROMA_EDGE`]:
/// enough for chroma statistics.
fn small_rgba(bytes: &[u8]) -> Option<Rgba> {
    let info = morphic::inspect(bytes).ok()?;
    let mut mip = 0u8;
    while mip + 1 < info.mip_count {
        let (w, h) = info.actual_mip_dims(mip + 1);
        if w.max(h) < CHROMA_EDGE {
            break;
        }
        mip += 1;
    }
    decode_rgba(bytes, &info, mip, 1)
}

/// Mip 0 sampled on a grid of at most [`DRIFT_EDGE`] per side. Shape drift is
/// measured here: the bakes regenerate the lower mips from mip 0, so a lower
/// mip differs from Valve's authored one even when the bake kept the shape.
fn top_rgba(bytes: &[u8]) -> Option<Rgba> {
    let info = morphic::inspect(bytes).ok()?;
    let (w, h) = info.actual_mip_dims(0);
    let stride = (w.max(h) / DRIFT_EDGE).max(1);
    decode_rgba(bytes, &info, 0, stride)
}

fn decode_rgba(bytes: &[u8], info: &morphic::TextureInfo, mip: u8, stride: u32) -> Option<Rgba> {
    let image = morphic::decode_at(
        bytes,
        &DecodeOptions {
            mip,
            slice: 0,
            face: 0,
        },
    )
    .ok()?;
    let (w, h) = info.actual_mip_dims(mip);
    let image = morphic::crop_to_actual(&image, w, h);
    let width = image.width as usize;
    let stride = stride as usize;
    let keep = |i: usize| (i % width).is_multiple_of(stride) && (i / width).is_multiple_of(stride);
    let px = match &image.data {
        ImageData::Rgba8(p) => p
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(i, _)| keep(*i))
            .map(|(_, c)| [0, 1, 2, 3].map(|i| f32::from(c[i]) / 255.0))
            .collect(),
        ImageData::Rgba16F(p) => p
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(i, _)| keep(*i))
            .map(|(_, c)| [0, 1, 2, 3].map(|i| c[i].to_f32().clamp(0.0, 1.0)))
            .collect(),
    };
    Some(Rgba { px })
}

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn texture_chroma(bytes: &[u8]) -> Option<Chroma> {
    let image = small_rgba(bytes)?;
    let (mut n, mut sum, mut over, mut hx, mut hy) = (0usize, 0f32, 0usize, 0f32, 0f32);
    for [r, g, b, _] in image.px {
        let max = r.max(g).max(b);
        if max < 0.08 {
            continue;
        }
        let min = r.min(g).min(b);
        let chroma = max - min;
        n += 1;
        sum += chroma;
        if chroma > 0.15 {
            over += 1;
        }
        if chroma > 0.0 {
            let hue = if (max - r).abs() < f32::EPSILON {
                ((g - b) / chroma).rem_euclid(6.0)
            } else if (max - g).abs() < f32::EPSILON {
                (b - r) / chroma + 2.0
            } else {
                (r - g) / chroma + 4.0
            } * 60.0;
            hx += chroma * hue.to_radians().cos();
            hy += chroma * hue.to_radians().sin();
        }
    }
    if n == 0 {
        return Some(Chroma {
            mean: 0.0,
            share: 0.0,
            hue: 0.0,
        });
    }
    Some(Chroma {
        mean: sum / n as f32,
        share: over as f32 / n as f32,
        hue: hy.atan2(hx).to_degrees().rem_euclid(360.0),
    })
}

/// Materials a model renders: its own RERL plus that of each external mesh
/// (`.vmesh_c`) it references, where hero bodies keep their draw calls.
fn model_materials(vpks: &[valve_pak::VPK], model: &str) -> BTreeSet<String> {
    let Some(bytes) = read_entry(vpks, model) else {
        return BTreeSet::new();
    };
    let refs = external_refs(&bytes);
    let mut out: BTreeSet<String> = refs
        .iter()
        .filter(|r| r.ends_with(".vmat_c"))
        .cloned()
        .collect();
    for mesh in refs.iter().filter(|r| r.ends_with(".vmesh_c")) {
        if let Some(b) = read_entry(vpks, mesh) {
            out.extend(
                external_refs(&b)
                    .into_iter()
                    .filter(|r| r.ends_with(".vmat_c")),
            );
        }
    }
    out
}

/// A `hero_*` node of a hero players can pick: `EHeroDevState_Release`, or the
/// legacy `m_bPlayerSelectable` flag on older builds.
fn released(node: &Value) -> bool {
    match node.get("m_eHeroDevelopmentState").and_then(Value::as_str) {
        Some(state) => state == "EHeroDevState_Release",
        None => node.get("m_bPlayerSelectable").and_then(Value::as_bool) == Some(true),
    }
}

/// Every string in a vdata tree a node could name another node or a resource
/// by (an identifier-like word, or a particle or model path), lowercase compiled
/// for paths, with the field path of its first occurrence (`.m_Foo.m_Bar[].m_Baz`).
fn string_fields(v: &Value, path: &str, out: &mut BTreeMap<String, String>) {
    match v {
        Value::String(s) => {
            let word = !s.is_empty()
                && s.len() <= 96
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b));
            let compiled = compiled_resource_path(s).to_ascii_lowercase();
            let resource = compiled.ends_with(".vpcf_c") || compiled.ends_with(".vmdl_c");
            let key = if word {
                Some(s.clone())
            } else if resource {
                Some(compiled)
            } else {
                None
            };
            if let Some(key) = key {
                out.entry(key).or_insert_with(|| path.to_owned());
            }
        }
        Value::Object(pairs) => pairs
            .iter()
            .for_each(|(k, c)| string_fields(c, &format!("{path}.{k}"), out)),
        Value::Array(items) => items
            .iter()
            .for_each(|c| string_fields(c, &format!("{path}[]"), out)),
        _ => {}
    }
}

/// `particles/....vpcf` ASCII runs in a game binary: particles the code spawns
/// by hardcoded path.
fn dll_particle_paths(bytes: &[u8], out: &mut BTreeSet<String>) {
    let needle = b"particles/";
    let mut i = 0;
    while let Some(off) = bytes[i..].windows(needle.len()).position(|w| w == needle) {
        let start = i + off;
        let end = bytes[start..]
            .iter()
            .position(|&b| !(b.is_ascii_alphanumeric() || b"/_.-".contains(&b)))
            .map_or(bytes.len(), |n| start + n);
        if let Ok(s) = std::str::from_utf8(&bytes[start..end]) {
            let path = compiled_resource_path(s).to_ascii_lowercase();
            if path.ends_with(".vpcf_c") {
                out.insert(path);
            }
        }
        i = end.max(start + 1);
    }
}

/// Every resource a compiled resource references, from its RERL block, as
/// lowercased compiled entry paths (`materials/x.vmat` -> `materials/x.vmat_c`).
///
/// Layout (VRF `ResourceExtRefList`): a `u32` offset to the entries, counted
/// from the start of the block, then a `u32` count; each 16-byte entry is a
/// `u64` id followed by an `i64` offset, counted from that field, to a
/// NUL-terminated path.
fn external_refs(bytes: &[u8]) -> Vec<String> {
    let Ok(resource) = morphic::resource::Resource::parse(bytes) else {
        return Vec::new();
    };
    let Some(block) = resource.find_block(*b"RERL") else {
        return Vec::new();
    };
    let u32_at = |o: usize| -> Option<usize> {
        block
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let (Some(start), Some(count)) = (u32_at(0), u32_at(4)) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let field = start + i * 16 + 8;
        let Some(raw) = block.get(field..field + 8) else {
            break;
        };
        let rel = i64::from_le_bytes(raw.try_into().unwrap_or([0; 8]));
        let Some(at) = i64::try_from(field)
            .ok()
            .and_then(|f| usize::try_from(f + rel).ok())
        else {
            continue;
        };
        let Some(tail) = block.get(at..) else {
            continue;
        };
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        if let Ok(name) = std::str::from_utf8(&tail[..end]) {
            if !name.is_empty() {
                out.push(compiled_resource_path(name).to_ascii_lowercase());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_matches_whole_path_tokens() {
        let names = names_for("abrams");
        assert!(path_owned_by(
            &names,
            "particles/abilities/bull_drain.vpcf_c"
        ));
        assert!(path_owned_by(&names, "particles/abilities/abrams/x.vpcf_c"));
        assert!(!path_owned_by(
            &names,
            "particles/abilities/bullet_amp_wall.vpcf_c"
        ));
        let names = names_for("doorman");
        assert!(path_owned_by(
            &names,
            "particles/status_fx/status_fx_doorman_hotel_debuff.vpcf_c"
        ));
    }

    #[test]
    fn review_rows_name_pinned_heroes() {
        for (code, path, why) in REVIEWED {
            assert!(recipe_for(code).is_some(), "{code} has no recipe");
            assert!(!path.is_empty() && !why.is_empty());
        }
    }

    #[test]
    fn ui_signal_names() {
        assert!(is_ui_signal(
            "models/heroes_wip/doorman/materials/doorman_door_preview_bad.vmat_c"
        ));
        assert!(!is_ui_signal(
            "models/heroes_wip/doorman/materials/doorman_door_portal.vmat_c"
        ));
    }

    #[test]
    fn color_slots() {
        assert!(is_color_slot("g_tColor"));
        assert!(is_color_slot("g_tSelfIllum"));
        assert!(!is_color_slot("g_tSelfIllumMask"));
        assert!(!is_color_slot("g_tNormalRoughness"));
        assert!(!is_color_slot("g_tTintMask"));
        assert!(is_color_slot("g_tDetail"));
        assert!(is_color_slot("g_tSheen"));
        assert!(!is_color_slot("g_tDetailMask"));
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn kids(edges: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        edges
            .iter()
            .map(|(p, ks)| {
                (
                    (*p).to_owned(),
                    ks.iter().map(|k| (*k).to_owned()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn shared_children_of_private_particles_are_copied_downward() {
        let refs = kids(&[
            ("a", &["b", "priv2"]),
            ("b", &["c", "generic"]),
            ("generic", &["d"]),
            ("priv2", &["e"]),
        ]);
        let private = set(&["a", "priv2"]);
        let shared = set(&["b", "c", "d", "e", "x", "y"]);
        let changed = set(&["c", "d", "e", "y"]);
        let plan = plan_isolation(&refs, &private, &shared, &changed);
        assert_eq!(plan.copies.keys().collect::<Vec<_>>(), ["b", "c", "e"]);
        assert_eq!(plan.copies["b"], set(&["a"]));
        assert_eq!(plan.copies["c"], set(&["b"]));
        assert_eq!(plan.copies["e"], set(&["priv2"]));
        assert_eq!(
            plan.vanilla,
            set(&["d", "y"]),
            "d hangs off an unlisted generic and y has no referrer; x changes nothing"
        );
    }

    #[test]
    fn an_unchanged_shared_leaf_keeps_pointing_at_the_vanilla_file() {
        let refs = kids(&[("a", &["s"]), ("s", &["leaf"])]);
        let plan = plan_isolation(&refs, &set(&["a"]), &set(&["s", "leaf"]), &set(&["s"]));
        assert_eq!(plan.copies.keys().collect::<Vec<_>>(), ["s"]);
        assert!(plan.vanilla.is_empty());
    }

    #[test]
    fn an_unchanged_shared_file_is_copied_when_it_leads_to_a_changed_one() {
        let refs = kids(&[("a", &["mid"]), ("mid", &["tex"])]);
        let plan = plan_isolation(&refs, &set(&["a"]), &set(&["mid", "tex"]), &set(&["tex"]));
        assert_eq!(plan.copies["mid"], set(&["a"]));
        assert_eq!(plan.copies["tex"], set(&["mid"]));
    }

    #[test]
    fn a_shared_particle_two_private_files_spawn_has_both_referrers() {
        let refs = kids(&[("a", &["s"]), ("b", &["s"])]);
        let plan = plan_isolation(&refs, &set(&["a", "b"]), &set(&["s"]), &set(&["s"]));
        assert_eq!(plan.copies["s"], set(&["a", "b"]));
        assert!(plan.vanilla.is_empty());
    }

    #[test]
    fn shared_cycles_terminate() {
        let refs = kids(&[("a", &["s"]), ("s", &["t"]), ("t", &["s"])]);
        let plan = plan_isolation(&refs, &set(&["a"]), &set(&["s", "t"]), &set(&["t"]));
        assert_eq!(plan.copies["s"], set(&["a", "t"]));
        assert_eq!(plan.copies["t"], set(&["s"]));
    }

    fn universe(paths: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = paths.iter().map(|s| (*s).to_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn a_run_of_required_files_collapses_to_a_prefix_that_covers_nothing_else() {
        let all = universe(&[
            "particles/abilities/h/h_blast.vpcf_c",
            "particles/abilities/h/h_blast_core.vpcf_c",
            "particles/abilities/h/h_blast_ring.vpcf_c",
            "particles/abilities/h/h_blaster.vpcf_c",
            "particles/abilities/h/h_dash.vpcf_c",
            "particles/abilities/h/h_dash_trail.vpcf_c",
            "particles/abilities/h/h_zap.vpcf_c",
        ]);
        let required = set(&[
            "particles/abilities/h/h_blast.vpcf_c",
            "particles/abilities/h/h_blast_core.vpcf_c",
            "particles/abilities/h/h_blast_ring.vpcf_c",
            "particles/abilities/h/h_dash.vpcf_c",
            "particles/abilities/h/h_dash_trail.vpcf_c",
            "models/x/pinned.vmat_c",
        ]);
        assert_eq!(
            collapse_prefixes(&required, &all),
            [
                "models/x/pinned.vmat_c",
                "particles/abilities/h/h_blast.vpcf_c",
                "particles/abilities/h/h_blast_",
                "particles/abilities/h/h_dash",
            ]
        );
    }

    #[test]
    fn a_prefix_that_would_cover_a_private_file_is_not_used() {
        let all = universe(&[
            "particles/abilities/h/h_a.vpcf_c",
            "particles/abilities/h/h_b.vpcf_c",
            "particles/abilities/h/h_c.vpcf_c",
        ]);
        let required = set(&[
            "particles/abilities/h/h_a.vpcf_c",
            "particles/abilities/h/h_b.vpcf_c",
        ]);
        assert_eq!(
            collapse_prefixes(&required, &all),
            [
                "particles/abilities/h/h_a.vpcf_c",
                "particles/abilities/h/h_b.vpcf_c"
            ]
        );
    }

    #[test]
    fn a_single_required_file_stays_an_exact_path() {
        let all = universe(&["particles/p/a.vpcf_c", "particles/q/b.vpcf_c"]);
        assert_eq!(
            collapse_prefixes(&set(&["particles/p/a.vpcf_c"]), &all),
            ["particles/p/a.vpcf_c"]
        );
    }

    fn vnode(file: &str, key: &str, words: &[&str], particles: &[&str]) -> VdataNode {
        VdataNode {
            key: key.to_owned(),
            label: format!("{file} {key}"),
            units: file == UNITS_VDATA,
            definition: file == HEROES_VDATA && key.starts_with("hero_"),
            words: set(words),
            particles: set(particles),
            models: BTreeSet::new(),
            fields: words
                .iter()
                .map(|w| ((*w).to_owned(), format!(".m_{w}")))
                .chain(
                    particles
                        .iter()
                        .map(|p| ((*p).to_owned(), ".m_Particle".to_owned())),
                )
                .collect(),
        }
    }

    fn hero(code: &str, node: usize, abilities: &[&str]) -> HeroDef {
        HeroDef {
            code: code.to_owned(),
            node,
            abilities: abilities.iter().map(|a| (*a).to_owned()).collect(),
            model: None,
        }
    }

    fn owned(owners: &[BTreeMap<Owner, Prov>], i: usize) -> Vec<String> {
        owners[i].keys().map(Owner::label).collect()
    }

    const ABILITIES: &str = "scripts/abilities.vdata_c";

    #[test]
    fn ownership_follows_key_references_from_bound_abilities() {
        let nodes = vec![
            vnode(HEROES_VDATA, "hero_necro", &["ability_necro_wall"], &[]),
            vnode(ABILITIES, "ability_necro_wall", &["necro_ghoul_mod"], &[]),
            vnode(ABILITIES, "necro_ghoul_mod", &["npc_necro_skele"], &[]),
            vnode(
                UNITS_VDATA,
                "npc_necro_skele",
                &[],
                &["particles/abilities/necro/boom.vpcf_c"],
            ),
            vnode(
                ABILITIES,
                "ability_unbound",
                &[],
                &["particles/abilities/necro/boom.vpcf_c"],
            ),
        ];
        let owners = assign_owners(&nodes, &[hero("necro", 0, &["ability_necro_wall"])]);
        for (i, node) in nodes.iter().enumerate().take(4) {
            assert_eq!(owned(&owners, i), ["necro"], "{}", node.key);
        }
        assert!(
            owners[4].is_empty(),
            "an ability no released hero binds has no owner"
        );
    }

    #[test]
    fn a_node_several_owners_reach_is_shared_by_all_of_them() {
        let nodes = vec![
            vnode(HEROES_VDATA, "hero_a", &["ability_a"], &[]),
            vnode(HEROES_VDATA, "hero_b", &["ability_b"], &[]),
            vnode(ABILITIES, "ability_a", &["shared_mod"], &[]),
            vnode(ABILITIES, "ability_b", &["shared_mod"], &[]),
            vnode(ABILITIES, "shared_mod", &[], &[]),
            vnode(ABILITIES, "upgrade_item", &["shared_mod"], &[]),
        ];
        let heroes = [hero("a", 0, &["ability_a"]), hero("b", 1, &["ability_b"])];
        let owners = assign_owners(&nodes, &heroes);
        assert_eq!(owned(&owners, 4), ["a", "b", "item upgrade_item"]);
        assert_eq!(owned(&owners, 3), ["b"]);
    }

    #[test]
    fn hero_definitions_are_never_followed_so_an_item_blacklist_owns_nothing() {
        let nodes = vec![
            vnode(HEROES_VDATA, "hero_a", &["ability_a"], &[]),
            vnode(ABILITIES, "ability_a", &[], &[]),
            vnode(ABILITIES, "upgrade_burst", &["hero_a"], &[]),
        ];
        let owners = assign_owners(&nodes, &[hero("a", 0, &["ability_a"])]);
        assert_eq!(owned(&owners, 1), ["a"]);
        assert_eq!(owned(&owners, 0), ["a"]);
        assert_eq!(owned(&owners, 2), ["item upgrade_burst"]);
    }

    #[test]
    fn code_spawned_units_go_to_the_hero_their_assets_name_else_the_neutrals() {
        let nodes = vec![
            vnode(HEROES_VDATA, "hero_necro", &[], &[]),
            vnode(HEROES_VDATA, "hero_forge", &[], &[]),
            vnode(
                UNITS_VDATA,
                "npc_necro_skele",
                &[],
                &["particles/abilities/necro/boom.vpcf_c"],
            ),
            vnode(
                UNITS_VDATA,
                "neutral_trooper",
                &["neutral_melee"],
                &["particles/generic/hit.vpcf_c"],
            ),
            vnode(
                ABILITIES,
                "neutral_melee",
                &[],
                &["particles/abilities/mcginnis/flare.vpcf_c"],
            ),
            vnode(
                ABILITIES,
                "citadel_neutral_shield",
                &[],
                &[
                    "particles/abilities/necro/a.vpcf_c",
                    "particles/abilities/mcginnis/b.vpcf_c",
                ],
            ),
            vnode(
                ABILITIES,
                "citadel_neutral_beam",
                &[],
                &["particles/abilities/necro/a.vpcf_c"],
            ),
            vnode(
                UNITS_VDATA,
                "npc_two_heroes",
                &[],
                &[
                    "particles/abilities/necro/a.vpcf_c",
                    "particles/abilities/mcginnis/b.vpcf_c",
                ],
            ),
            vnode(
                ABILITIES,
                "pickup",
                &[],
                &["particles/abilities/necro/a.vpcf_c"],
            ),
        ];
        let heroes = [hero("necro", 0, &[]), hero("forge", 1, &[])];
        let owners = assign_owners(&nodes, &heroes);
        assert_eq!(owned(&owners, 2), ["necro"]);
        assert_eq!(owned(&owners, 3), ["neutral"]);
        assert_eq!(
            owned(&owners, 4),
            ["neutral"],
            "children of a neutral unit are neutral even when they name a hero's effect"
        );
        assert_eq!(
            owned(&owners, 5),
            ["neutral"],
            "several heroes' effects on a neutral modifier"
        );
        assert_eq!(owned(&owners, 6), ["neutral"], "the key says neutral");
        assert_eq!(owned(&owners, 7), ["neutral"]);
        assert!(owners[8].is_empty(), "not a unit and not neutral: nobody's");
    }

    #[test]
    fn jungle_variants_that_reference_each_other_are_neutral_whatever_they_wear() {
        let rat = "particles/abilities/ratking/ratking_nibble_swarm.vpcf_c";
        let nodes = vec![
            vnode(HEROES_VDATA, "hero_ratking", &[], &[]),
            vnode(
                UNITS_VDATA,
                "npc_neutral_bug",
                &["npc_neutral_bug_rat"],
                &[],
            ),
            vnode(
                UNITS_VDATA,
                "npc_neutral_bug_rat",
                &["npc_neutral_bug"],
                &[rat],
            ),
            vnode(UNITS_VDATA, "npc_ratking_rat", &[], &[rat]),
            vnode(UNITS_VDATA, "npc_yakuza_gangster", &[], &[]),
            vnode(ABILITIES, "yakuza_summon", &["npc_yakuza_gangster"], &[]),
        ];
        let owners = assign_owners(&nodes, &[hero("ratking", 0, &[])]);
        assert_eq!(owned(&owners, 1), ["neutral"]);
        assert_eq!(owned(&owners, 2), ["neutral"]);
        assert_eq!(owned(&owners, 3), ["ratking"]);
        assert!(
            owners[4].is_empty(),
            "summoned by an unbound ability: nobody's"
        );
    }

    #[test]
    fn models_a_node_names_bring_their_embedded_particles_to_the_owner() {
        let minion = "models/heroes_wip/ratking/ratking_minion.vmdl_c";
        let sub = "models/heroes_wip/ratking/sub.vmdl_c";
        let mut rat = vnode(UNITS_VDATA, "neutral_rat", &[], &[]);
        rat.models = set(&[minion]);
        let embedded = embedded_particles(
            &BTreeMap::from([
                (minion.to_owned(), vec!["p/ambient.vpcf_c".to_owned()]),
                (sub.to_owned(), vec!["p/sub.vpcf_c".to_owned()]),
                (
                    "models/heroes_wip/h/clips/run.vnmclip_c".to_owned(),
                    vec!["p/dust.vpcf_c".to_owned()],
                ),
                (
                    "models/heroes_wip/other/clips/run.vnmclip_c".to_owned(),
                    vec!["p/other.vpcf_c".to_owned()],
                ),
            ]),
            &BTreeMap::from([(minion.to_owned(), vec![sub.to_owned()])]),
        );
        let mut h = hero("h", 0, &[]);
        h.model = Some("models/heroes_wip/h/h.vmdl_c".to_owned());
        let roots = owner_roots(&[rat], &[set_of(Owner::Neutral)], &[h], &embedded);
        let neutral: Vec<&String> = roots[&Owner::Neutral].keys().collect();
        assert_eq!(neutral, ["p/ambient.vpcf_c", "p/sub.vpcf_c"]);
        let hero_roots: Vec<&String> = roots[&Owner::Hero("h".to_owned())].keys().collect();
        assert_eq!(
            hero_roots,
            ["p/dust.vpcf_c"],
            "its clips, not another hero's"
        );
    }

    fn set_of(owner: Owner) -> BTreeMap<Owner, Prov> {
        BTreeMap::from([(
            owner,
            Prov {
                parent: None,
                field: String::new(),
            },
        )])
    }

    fn decal(slot: &str, scale: f32, drive: Option<&str>) -> Material {
        let mut m = Material::default();
        m.texture_params
            .insert(slot.to_owned(), "materials/p/glow.vtex".to_owned());
        m.float_params
            .insert("g_flSelfIllumScale".to_owned(), scale);
        match drive {
            Some("expression") => {
                m.dynamic_params.insert(
                    "g_flSelfIllumScale".to_owned(),
                    morphic::material::DynamicExpr {
                        source: "$SELFILLUM".to_owned(),
                        decompiled: true,
                        byte_len: 0,
                        attributes: vec!["SELFILLUM".to_owned()],
                        hash: String::new(),
                        error: None,
                    },
                );
            }
            Some(attribute) => m.render_attributes_used.push(attribute.to_owned()),
            None => {}
        }
        m
    }

    fn inert(slot: &str, scale: f32, drive: Option<&str>, extra_user: Option<&str>) -> bool {
        let mat = "materials/p/decal.vmat_c";
        let mut users = vec![("materials/p/glow.vtex_c".to_owned(), vec![mat.to_owned()])];
        users.push((mat.to_owned(), vec!["particles/p/a.vpcf_c".to_owned()]));
        if let Some(u) = extra_user {
            users[0].1.push(u.to_owned());
        }
        let index = RefIndex {
            users: users.into_iter().collect(),
            live: BTreeMap::from([("particles/p/a.vpcf_c".to_owned(), "vdata".to_owned())]),
            ..RefIndex::default()
        };
        let m = decal(slot, scale, drive);
        inert_selfillum(&index, "materials/p/glow.vtex_c", &mut |_| Some(m.clone())).is_some()
    }

    #[test]
    fn a_selfillum_texture_behind_a_static_zero_scale_is_inert() {
        assert!(inert("g_tSelfIllum", 0.0, None, None));
        assert!(!inert("g_tSelfIllum", 1.0, None, None), "scale on");
        assert!(
            !inert("g_tSelfIllum", 0.0, Some("expression"), None),
            "an expression drives the scale"
        );
        assert!(
            !inert("g_tSelfIllum", 0.0, Some("$SELFILLUM"), None),
            "the particle feeds $SELFILLUM"
        );
        assert!(!inert("g_tColor", 0.0, None, None), "color slot");
        assert!(
            !inert("g_tSelfIllum", 0.0, None, Some("particles/p/a.vpcf_c")),
            "a particle samples it directly"
        );
    }

    fn judge_pinned(shared_entries: &[&str]) -> (HeroQa, Vec<Finding>, BTreeSet<String>) {
        let list = |items: &[&str]| items.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        let recipe = HeroRecolorRecipe {
            codename: "h".to_owned(),
            particle_prefixes: list(&["particles/abilities/h/"]),
            texture_entries: list(&["materials/t.vtex_c", "materials/u.vtex_c"]),
            material_entries: list(&["materials/m1.vmat_c"]),
            model_entries: Vec::new(),
            shared_entries: list(shared_entries),
            preview_texture: None,
        };
        let mine = "particles/abilities/h/a.vpcf_c";
        let theirs = "particles/generic/foreign.vpcf_c";
        let refs = kids(&[
            (mine, &["materials/m1.vmat_c", "materials/m2.vmat_c"]),
            (theirs, &["materials/m1.vmat_c", "materials/m2.vmat_c"]),
            ("materials/m1.vmat_c", &["materials/t.vtex_c"]),
            ("materials/m2.vmat_c", &["materials/u.vtex_c"]),
        ]);
        let mut users: HashMap<String, Vec<String>> = HashMap::new();
        for (from, tos) in &refs {
            for to in tos {
                users.entry(to.clone()).or_default().push(from.clone());
            }
        }
        let index = RefIndex {
            users,
            refs,
            live: [mine, theirs]
                .into_iter()
                .map(|p| (p.to_owned(), "vdata".to_owned()))
                .collect(),
            ..RefIndex::default()
        };
        let hero = Hero {
            recipe: &recipe,
            names: vec!["h".to_owned()],
            body_model: None,
            pinned: recipe
                .texture_entries
                .iter()
                .chain(&recipe.material_entries)
                .cloned()
                .collect(),
            reached: BTreeSet::new(),
        };
        let listed = |p: &str| recipe.shared_entries.iter().any(|l| p.starts_with(l));
        let pins: Vec<String> = hero.pinned.iter().cloned().collect();
        let shared: BTreeSet<String> = pins
            .iter()
            .filter(|k| {
                let (foreign, others) = sharing(&hero, &index, &[], k);
                !foreign.is_empty() || !others.is_empty() || listed(k)
            })
            .cloned()
            .collect();
        let private: BTreeSet<String> = [mine.to_owned(), "materials/m1.vmat_c".to_owned()]
            .into_iter()
            .filter(|p| !shared.contains(p))
            .collect();
        let plan = plan_isolation(&index.refs, &private, &shared, &shared);
        let iso = Isolator {
            hero: &hero,
            index: &index,
            recipes: &[],
            listed: &recipe.shared_entries,
            plan: &plan,
            shared: &shared,
        };
        let mut qa = HeroQa::new(&recipe);
        let mut safety = Vec::new();
        let mut required = BTreeSet::new();
        for entry in &recipe.texture_entries {
            iso.pinned(entry, entry, &mut safety, &mut qa, &mut required);
        }
        (qa, safety, required)
    }

    fn kinds(findings: &[Finding]) -> Vec<(&str, &str)> {
        findings
            .iter()
            .map(|f| (f.kind.as_str(), f.entry.as_str()))
            .collect()
    }

    #[test]
    fn a_pinned_asset_is_isolated_through_a_pinned_material_and_blocked_by_an_unpinned_one() {
        let (qa, safety, required) = judge_pinned(&["materials/"]);
        assert_eq!(
            kinds(&safety),
            [("unreachable-isolation", "materials/u.vtex_c")]
        );
        assert!(safety[0].detail.contains("an unpinned material"));
        assert_eq!(kinds(&qa.limits), [("isolated", "materials/t.vtex_c")]);
        assert_eq!(required, set(&["materials/t.vtex_c"]));
        assert_eq!(qa.isolation.len(), 1);
        assert_eq!(qa.isolation[0].kind, "pinned");
        assert_eq!(qa.isolation[0].referrers, ["materials/m1.vmat_c"]);
    }

    #[test]
    fn an_unlisted_shared_pin_bleeds_but_still_reports_what_would_isolate_it() {
        let (qa, safety, required) = judge_pinned(&[]);
        assert_eq!(
            kinds(&safety),
            [
                ("shared", "materials/t.vtex_c"),
                ("shared", "materials/u.vtex_c")
            ]
        );
        assert!(qa.limits.is_empty());
        assert_eq!(required, set(&["materials/t.vtex_c"]));
    }

    #[test]
    fn an_ability_bound_to_a_hero_is_never_claimed_through_another_owners_references() {
        let nodes = vec![
            vnode(HEROES_VDATA, "hero_a", &["ability_a", "shared_ult"], &[]),
            vnode(HEROES_VDATA, "hero_b", &["ability_b", "shared_ult"], &[]),
            vnode(HEROES_VDATA, "hero_c", &["ability_c"], &[]),
            vnode(ABILITIES, "ability_a", &["ability_c", "wall_mod"], &[]),
            vnode(ABILITIES, "ability_b", &[], &[]),
            vnode(ABILITIES, "ability_c", &[], &["p/c.vpcf_c"]),
            vnode(ABILITIES, "shared_ult", &[], &["p/ult.vpcf_c"]),
            vnode(ABILITIES, "wall_mod", &[], &["p/wall.vpcf_c"]),
            vnode(ABILITIES, "upgrade_copy", &["ability_c", "wall_mod"], &[]),
        ];
        let heroes = [
            hero("a", 0, &["ability_a", "shared_ult"]),
            hero("b", 1, &["ability_b", "shared_ult"]),
            hero("c", 2, &["ability_c"]),
        ];
        let owners = assign_owners(&nodes, &heroes);
        assert_eq!(
            owned(&owners, 5),
            ["c"],
            "a lists c's ability by name, an animgraph source or a copy-an-ult list"
        );
        assert_eq!(owned(&owners, 6), ["a", "b"], "bound to both");
        assert_eq!(
            owned(&owners, 7),
            ["a", "item upgrade_copy"],
            "an unbound modifier is claimed by whoever references it"
        );
    }

    #[test]
    fn indirect_ownership_is_explained_with_the_fields_that_link_it() {
        let nodes = vec![
            vnode(HEROES_VDATA, "hero_a", &["ability_a"], &[]),
            vnode(ABILITIES, "ability_a", &["summon_mod"], &[]),
            vnode(ABILITIES, "summon_mod", &["npc_pet"], &[]),
            vnode(UNITS_VDATA, "npc_pet", &[], &["p/pet.vpcf_c"]),
        ];
        let owners = assign_owners(&nodes, &[hero("a", 0, &["ability_a"])]);
        let index = RefIndex {
            ownership: ownership_paths(&nodes, &owners),
            ..RefIndex::default()
        };
        let a = Owner::Hero("a".to_owned());
        let pet = "scripts/npc_units.vdata_c npc_pet";
        assert_eq!(
            index.explain(pet, &a).as_deref(),
            Some("ability_a -[m_summon_mod]-> summon_mod -[m_npc_pet]-> npc_pet")
        );
        assert_eq!(
            index.explain("scripts/abilities.vdata_c ability_a", &a),
            None,
            "a bound ability is the hero's own"
        );
    }

    fn reach_index(roots: &[(Owner, &str, &str)], children: &[(&str, &[&str])]) -> RefIndex {
        let mut by_owner: BTreeMap<Owner, BTreeMap<String, (String, String)>> = BTreeMap::new();
        for (o, p, root) in roots {
            by_owner.entry(o.clone()).or_default().insert(
                (*p).to_owned(),
                ((*root).to_owned(), ".m_Particle".to_owned()),
            );
        }
        let children = kids(children);
        RefIndex {
            reach: particle_reach(by_owner, &children),
            refs: children,
            ..RefIndex::default()
        }
    }

    #[test]
    fn reach_closes_over_children_and_tells_hero_from_foreign() {
        let fencer = Owner::Hero("fencer".to_owned());
        let paige = Owner::Hero("bookworm".to_owned());
        let index = reach_index(
            &[
                (
                    paige.clone(),
                    "p/aoe.vpcf_c",
                    "scripts/abilities.vdata_c ability_aoe",
                ),
                (
                    fencer.clone(),
                    "p/cyl.vpcf_c",
                    "scripts/abilities.vdata_c ability_ult",
                ),
                (
                    Owner::Neutral,
                    "p/ring.vpcf_c",
                    "scripts/npc_units.vdata_c neutral_x",
                ),
            ],
            &[
                ("p/aoe.vpcf_c", &["p/cyl.vpcf_c"]),
                ("p/cyl.vpcf_c", &["p/ring.vpcf_c"]),
            ],
        );
        let names = vec!["bookworm".to_owned(), "paige".to_owned()];
        assert_eq!(index.foreign_owners("p/aoe.vpcf_c", &names), []);
        let cyl = index.foreign_owners("p/cyl.vpcf_c", &names);
        assert_eq!(cyl.len(), 1);
        assert_eq!(*cyl[0].0, fencer);
        assert_eq!(index.foreign_owners("p/ring.vpcf_c", &names).len(), 2);
        assert_eq!(
            index.chain("p/ring.vpcf_c", &paige),
            "ability_aoe > aoe.vpcf_c > cyl.vpcf_c > ring.vpcf_c"
        );
        assert_eq!(
            index
                .hero_owner("p/cyl.vpcf_c", &names)
                .map(|(o, _)| o.clone()),
            Some(paige)
        );
        assert!(index.hero_owner("p/none.vpcf_c", &names).is_none());
    }

    #[test]
    fn private_shared_and_vanilla_follow_from_who_reaches_what() {
        let me = Owner::Hero("me".to_owned());
        let you = Owner::Hero("you".to_owned());
        let index = reach_index(
            &[
                (
                    me.clone(),
                    "p/me/root.vpcf_c",
                    "scripts/abilities.vdata_c mine",
                ),
                (
                    me.clone(),
                    "p/me/named.vpcf_c",
                    "scripts/abilities.vdata_c mine",
                ),
                (
                    you.clone(),
                    "p/me/named.vpcf_c",
                    "scripts/abilities.vdata_c yours",
                ),
                (
                    you.clone(),
                    "p/me/leaf.vpcf_c",
                    "scripts/abilities.vdata_c yours",
                ),
            ],
            &[("p/me/root.vpcf_c", &["p/me/leaf.vpcf_c"])],
        );
        let names = vec!["me".to_owned()];
        let prefix = ["p/me/leaf.vpcf_c", "p/me/named.vpcf_c", "p/me/root.vpcf_c"];
        let (shared, private): (Vec<&str>, Vec<&str>) = prefix
            .into_iter()
            .partition(|p| !index.foreign_owners(p, &names).is_empty());
        let (shared, private) = (set(&shared), set(&private));
        assert_eq!(shared, set(&["p/me/leaf.vpcf_c", "p/me/named.vpcf_c"]));
        assert_eq!(private, set(&["p/me/root.vpcf_c"]));
        let plan = plan_isolation(&index.refs, &private, &shared, &shared);
        assert_eq!(plan.copies["p/me/leaf.vpcf_c"], set(&["p/me/root.vpcf_c"]));
        assert_eq!(
            plan.vanilla,
            set(&["p/me/named.vpcf_c"]),
            "named by my own vdata node, not repointable"
        );
    }
}
