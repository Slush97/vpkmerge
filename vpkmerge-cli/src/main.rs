use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use vpkmerge_core::{
    edit_model_geometry, export_hero_model, export_model, extract_portraits, inspect_models, merge,
    model_draw_call_targets, model_vertex_targets, split, AnimOptions, CollisionPolicy,
    GeometryEdit, MergeOptions, ModelPartSelector, OverlapPolicy, PathPredicate, PortraitInfo,
    PoseSelection, SoundEvents, SplitOptions, SplitOutput,
};

#[derive(Parser)]
#[command(
    name = "vpkmerge",
    version,
    about = "Combine multiple VPK files into one (or split one into many).",
    long_about = "Combine multiple Valve Pak (VPK) files into one. Built for Deadlock \
                  modding so players can consolidate several mods into a single addon VPK.\n\
                  \n\
                  By default, later inputs win on path collision. Use --strict to refuse to \
                  merge when paths collide.\n\
                  \n\
                  See `vpkmerge split --help` for the inverse operation (one VPK to many).",
    subcommand_negates_reqs = true,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Command>,

    /// Output VPK path. Parent directory is created if missing.
    output: Option<PathBuf>,

    /// Input VPK paths. At least 2 required (validated by the engine).
    #[arg(num_args = 0..)]
    inputs: Vec<PathBuf>,

    /// Error out on any path collision instead of letting later inputs win.
    #[arg(long)]
    strict: bool,

    /// Print each path that gets overridden by a later input.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Route entries from one VPK into N output VPKs by path predicate.
    Split(SplitCmd),

    /// Extract and decode hero portrait/card art from a VPK to PNG.
    Portrait(PortraitCmd),

    /// Inspect compiled models (`.vmdl_c`) in a VPK: block structure,
    /// mesh-part count, embedded geometry, and skeleton/physics presence.
    Model(ModelCmd),

    /// Decode a soundevents (`.vsndevts_c`) file to JSON, optionally edit clip
    /// paths / params and re-emit an uncompressed (loadable) file.
    Soundevents(SoundeventsCmd),

    /// Recolor a texture (`.vtex_c`) by shifting every pixel's hue to a target
    /// (keeping saturation + value), then re-encode in the texture's own
    /// format. Built for the Deadlock ability-VFX recolor: pack the result at
    /// the base entry path and it overrides in place, no `.vmat_c` edit needed.
    Texture(TextureCmd),

    /// Recolor a hero's full ability VFX (particles + color textures + baked
    /// vertex colors) to one hue and pack it into a single addon VPK. The
    /// one-call bridge for a mod manager: composes all three recolor mechanisms
    /// over a built-in per-hero recipe. Run with an unknown codename to list the
    /// pinned set.
    RecolorHero(RecolorHeroCmd),

    /// Recolor a hero's full ability VFX as a static prism/rainbow and pack it
    /// into a single addon VPK. Same composition as `recolor-hero`, but instead
    /// of one target hue it spreads each effect's existing color/tint scalars
    /// across a spectrum (gradient stops become spectral ramps, themed by effect
    /// type) so the VFX reads as a moving rainbow in game. Same pinned heroes as
    /// `recolor-hero`; run `rainbow-scan` first to see which carry the richest
    /// spectrum.
    Prism(PrismCmd),

    /// Paint a hero skin with procedural trippy patterns and VMAT scroll, including
    /// hero-specific weapons when requested.
    TrippySkin(TrippySkinCmd),

    /// Paint and animate a hero's ability/weapon VFX with procedural trippy color
    /// themes, using the same pinned recipes as prism/recolor.
    TrippyVfx(TrippyVfxCmd),

    /// Render the procedural trippy pattern as a sprite-sheet PNG (frames left
    /// to right) for a live UI preview. Pure pattern generation from the same
    /// function the skin/VFX bakes use: reads no VPK, runs in milliseconds.
    TrippyPreview(TrippyPreviewCmd),

    /// Scan pinned hero recipes for rainbow / animated-rainbow VFX support.
    RainbowScan(RainbowScanCmd),

    /// Style hero material shader parameters (`.vmat_c`): gemstone sheen, glass,
    /// solid-ink NPR outlines, unlit, or full-PBR flips, via presets or raw
    /// `--set-*` edits packed into an addon VPK. `--list` surveys the targeted
    /// materials (shader, feature flags, bound texture channels) first.
    Vmat(VmatCmd),
}

#[derive(Args)]
struct SoundeventsCmd {
    /// Path to a `.vsndevts_c` file, or (with --from-vpk) an entry path inside a VPK.
    input: PathBuf,

    /// Read INPUT as an entry path inside this VPK instead of a file on disk
    /// (e.g. `--from-vpk pak01_dir.vpk soundevents/hero/gigawatt.vsndevts_c`).
    #[arg(long, value_name = "VPK")]
    from_vpk: Option<PathBuf>,

    /// After applying edits, re-encode (uncompressed v4) to this loose file.
    #[arg(long, value_name = "FILE")]
    encode: Option<PathBuf>,

    /// After applying edits, re-encode and pack into a standalone VPK at this
    /// path. The encoded file lands at its VPK entry path (see --vpk-entry).
    #[arg(long = "encode-vpk", value_name = "OUT_dir.vpk")]
    encode_vpk: Option<PathBuf>,

    /// Entry path for the file inside --encode-vpk. Defaults to INPUT when
    /// reading with --from-vpk; required for a loose-file INPUT.
    #[arg(long = "vpk-entry", value_name = "PATH")]
    vpk_entry: Option<String>,

    /// Replace a clip path everywhere in the tree: --swap-vsnd OLD=NEW (repeatable).
    #[arg(long = "swap-vsnd", value_name = "OLD=NEW")]
    swap_vsnd: Vec<String>,

    /// Set a numeric field on one event: --set EVENT/FIELD=NUMBER (repeatable),
    /// e.g. --set "Seven.Wpn.Fire/volume=0.25".
    #[arg(long = "set", value_name = "EVENT/FIELD=NUMBER")]
    set: Vec<String>,
}

#[derive(Args)]
struct TextureCmd {
    /// One or more `.vtex_c` files, or (with --from-vpk) entry paths inside a
    /// VPK. Pass several to recolor a whole set into one addon (--encode-vpk).
    #[arg(required = true, num_args = 1..)]
    inputs: Vec<PathBuf>,

    /// Read INPUTS as entry paths inside this VPK instead of files on disk
    /// (e.g. `--from-vpk pak01_dir.vpk models/.../bookworm_dragon_color.vtex_c`).
    #[arg(long, value_name = "VPK")]
    from_vpk: Option<PathBuf>,

    /// Target hue in degrees (0..360). Every pixel is set to this hue while
    /// keeping its original saturation and value, so neutral highlights and
    /// shadows stay neutral. Matches the particle recolor, so the same hue
    /// lands the dragon, the projectile, and the particle params together.
    #[arg(long, value_name = "DEG", allow_hyphen_values = true)]
    hue: f64,

    /// Saturation scale (default 1.0 = keep source). > 1 lifts pale, washed-out
    /// areas toward the picked color; < 1 mutes them toward a pastel.
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    saturation: f64,

    /// Brightness (HSV value) scale (default 1.0 = keep source). > 1 lightens
    /// (e.g. a light/pastel color), < 1 darkens (a deep/ink color).
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    brightness: f64,

    /// Write a PNG preview of the recolored top mip here (the design-intent
    /// color, before the lossy `BCn` re-encode) for an eyeball before committing.
    /// Single input only.
    #[arg(long, value_name = "PNG")]
    preview: Option<PathBuf>,

    /// Write the recolored `.vtex_c` to this loose file. Single input only.
    #[arg(long, value_name = "FILE")]
    encode: Option<PathBuf>,

    /// After recoloring, pack into a standalone addon VPK at this path. With
    /// several INPUTS, all recolored textures land in this one addon, each at
    /// its VPK entry path (see --vpk-entry for the single loose-file case).
    #[arg(long = "encode-vpk", value_name = "OUT_dir.vpk")]
    encode_vpk: Option<PathBuf>,

    /// Entry path for the file inside --encode-vpk. Defaults to INPUT when
    /// reading with --from-vpk; required for a loose-file INPUT. Single input only.
    #[arg(long = "vpk-entry", value_name = "PATH")]
    vpk_entry: Option<String>,
}

#[derive(Args)]
struct RecolorHeroCmd {
    /// Hero model/particle codename to recolor (e.g. `bookworm` for Paige,
    /// `vampirebat` for Mina). Only heroes with a pinned recipe are supported;
    /// an unknown codename lists the pinned set.
    #[arg(long, value_name = "CODENAME")]
    hero: String,

    /// VPK to read the hero's VFX from (a skin VPK, or the base pak itself).
    #[arg(long, value_name = "VPK")]
    vpk: PathBuf,

    /// Base `pak01_dir.vpk` to fall back to for any entry `--vpk` does not ship
    /// (so a texture-only skin still recolors the base mesh/particles).
    #[arg(long, value_name = "VPK")]
    base: Option<PathBuf>,

    /// Target hue in degrees (0..360). Every color is set to this hue while
    /// keeping its saturation and value, so one value lands particles, textures,
    /// and vertex colors on the same color.
    #[arg(long, value_name = "DEG", allow_hyphen_values = true)]
    hue: f64,

    /// Saturation scale (default 1.0 = keep source). > 1 lifts pale, washed-out
    /// areas toward the picked color; < 1 mutes them toward a pastel. Applied to
    /// particles, textures, and vertex colors alike.
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    saturation: f64,

    /// Brightness (HSV value) scale (default 1.0 = keep source). > 1 lightens
    /// (e.g. a light/pastel color), < 1 darkens (a deep/ink color).
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    brightness: f64,

    /// Pack the whole recolored VFX set into this one addon VPK, each entry at
    /// its base path so it overrides the base game in place. Required unless
    /// `--preview-png` is given.
    #[arg(
        long = "encode-vpk",
        value_name = "OUT_dir.vpk",
        required_unless_present = "preview_png"
    )]
    encode_vpk: Option<PathBuf>,

    /// Skip the (slow) full bake and instead write a fast PNG swatch of the
    /// recipe's representative ability texture recolored to this target, for a
    /// live UI preview. Mutually exclusive use with `--encode-vpk` is allowed
    /// (preview wins and the bake is skipped).
    #[arg(long = "preview-png", value_name = "PNG")]
    preview_png: Option<PathBuf>,
}

#[derive(Args)]
struct PrismCmd {
    /// Hero model/particle codename to prism-recolor (e.g. `unicorn` for Celeste,
    /// `yamato` for Yamato). Only heroes with a pinned recipe are supported; an
    /// unknown codename lists the pinned set.
    #[arg(long, value_name = "CODENAME")]
    hero: String,

    /// VPK to read the hero's VFX from (a skin VPK, or the base pak itself).
    #[arg(long, value_name = "VPK")]
    vpk: PathBuf,

    /// Base `pak01_dir.vpk` to fall back to for any entry `--vpk` does not ship
    /// (so a texture-only skin still recolors the base mesh/particles).
    #[arg(long, value_name = "VPK")]
    base: Option<PathBuf>,

    /// Pack the whole prism-recolored VFX set into this one addon VPK, each entry
    /// at its base path so it overrides the base game in place.
    #[arg(long = "encode-vpk", value_name = "OUT_dir.vpk")]
    encode_vpk: PathBuf,

    /// Also animate high-visibility effects (glow/beam/trail/arc/slash/...):
    /// repoint their texture scroll at particle age, boost the scroll, and retime
    /// gradient stops so the spectrum sweeps over each particle's lifetime. Without
    /// this the prism is color-only (still reads as moving on heroes whose
    /// gradients already loop). Byte-faithful and best-effort per file.
    #[arg(long)]
    animated: bool,

    /// Strength of the animated pass (1.0 = default, 2.0 = harder/faster,
    /// 0.5 = softer). Has no effect unless --animated is set.
    #[arg(
        long = "animation-intensity",
        value_name = "SCALE",
        default_value_t = 1.0
    )]
    animation_intensity: f64,

    /// Animated pass depth: sweep = texture-scroll and timing edits, loop = also
    /// loop age-driven color gradients, cycle = also insert color-cycle operators
    /// for simple constant-color particles.
    #[arg(
        long = "animation-style",
        value_name = "STYLE",
        default_value = "sweep"
    )]
    animation_style: String,

    /// Rotate the whole rainbow's start hue by this many degrees. The per-effect
    /// spectrum spread is unchanged; this just shifts where it begins, so the same
    /// effect reads (say) blue->violet instead of red->orange. Default 0 (no shift).
    #[arg(
        long = "hue-offset",
        value_name = "DEG",
        default_value_t = 0.0,
        allow_hyphen_values = true
    )]
    hue_offset: f64,

    /// Saturation scale on the spectrum (1.0 = engine default; <1 pastels it, >1 is
    /// capped at full saturation).
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    saturation: f64,

    /// Brightness (HSV value) scale on the spectrum (1.0 = engine default; >1
    /// lightens, <1 darkens).
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    brightness: f64,

    /// Spread each effect across a custom gradient instead of the full rainbow.
    /// Either a preset name (fire, ice, toxic, sunset, ocean, neon, gold, void) or
    /// a stop list `pos:hue[:sat],...` (pos 0..1, hue degrees, sat 0..1). The
    /// rotation / saturation / brightness above still apply on top.
    #[arg(long, value_name = "SPEC")]
    gradient: Option<String>,
}

#[derive(Args)]
struct TrippySkinCmd {
    /// Hero model/material codename to repaint (e.g. `chrono` for Paradox).
    #[arg(long, value_name = "CODENAME")]
    hero: String,

    /// VPK to read the hero skin/materials from (a skin VPK, or the base pak).
    #[arg(long, value_name = "VPK")]
    vpk: PathBuf,

    /// Base `pak01_dir.vpk` to fall back to for materials/textures not in `--vpk`.
    #[arg(long, value_name = "VPK")]
    base: Option<PathBuf>,

    /// Pack the generated skin into this addon VPK.
    #[arg(long = "encode-vpk", value_name = "OUT_dir.vpk")]
    encode_vpk: PathBuf,

    /// Procedural style: confetti, liquid, moire, kaleido, holo, glitch, thermal,
    /// gradient, camo, carbon, galaxy, halftone, lava, or vaporwave.
    #[arg(long, value_name = "STYLE", default_value = "confetti")]
    style: String,

    /// Texture blend strength (0 = original texture, 1 = full generated pattern).
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    intensity: f32,

    /// Runtime VMAT UV-scroll speed scale (1 = Paradox prototype speed).
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    scroll: f64,

    /// Pattern phase / hue offset, normalized 0..1.
    #[arg(long, value_name = "T", default_value_t = 0.0)]
    phase: f32,

    /// Target set: all, body, or weapons.
    #[arg(long, value_name = "TARGETS", default_value = "all")]
    targets: String,
}

#[derive(Args)]
struct TrippyVfxCmd {
    /// Hero particle/VFX codename to repaint (e.g. `chrono` for Paradox).
    #[arg(long, value_name = "CODENAME")]
    hero: String,

    /// VPK to read the hero's VFX from (a skin VPK, or the base pak).
    #[arg(long, value_name = "VPK")]
    vpk: PathBuf,

    /// Base `pak01_dir.vpk` to fall back to for entries not in `--vpk`.
    #[arg(long, value_name = "VPK")]
    base: Option<PathBuf>,

    /// Pack the generated ability/weapon VFX into this addon VPK.
    #[arg(long = "encode-vpk", value_name = "OUT_dir.vpk")]
    encode_vpk: PathBuf,

    /// Procedural style: confetti, liquid, moire, kaleido, holo, glitch, thermal,
    /// gradient, camo, carbon, galaxy, halftone, lava, or vaporwave.
    #[arg(long, value_name = "STYLE", default_value = "confetti")]
    style: String,

    /// Texture blend / particle emphasis strength.
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    intensity: f32,

    /// Pattern phase / hue offset, normalized 0..1.
    #[arg(long, value_name = "T", default_value_t = 0.0)]
    phase: f32,

    /// Particle animation strength (1.0 = default, 2.0 = harder/faster,
    /// 0.5 = softer). Use 0 or --animation-style off for static color-only VFX.
    #[arg(
        long = "animation-intensity",
        value_name = "SCALE",
        default_value_t = 1.0
    )]
    animation_intensity: f64,

    /// Animation depth: off, sweep, loop, or cycle. sweep retimes safe texture
    /// scroll/gradient fields; loop also loops color gradients; cycle also inserts
    /// runtime color-cycle operators where safe.
    #[arg(
        long = "animation-style",
        value_name = "STYLE",
        default_value = "cycle"
    )]
    animation_style: String,

    /// Target set: all, abilities, or weapons.
    #[arg(long, value_name = "TARGETS", default_value = "all")]
    targets: String,
}

#[derive(Args)]
struct TrippyPreviewCmd {
    /// Procedural style: confetti, liquid, moire, kaleido, holo, glitch, thermal,
    /// gradient, camo, carbon, galaxy, halftone, lava, or vaporwave.
    #[arg(long, value_name = "STYLE", default_value = "confetti")]
    style: String,

    /// Pattern phase / hue offset, normalized 0..1.
    #[arg(long, value_name = "T", default_value_t = 0.0)]
    phase: f32,

    /// UV-scroll speed scale; advances the phase across the frame loop, so the
    /// loop speed mirrors the runtime scroll the bake would apply.
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    scroll: f32,

    /// Texture blend strength (0 = checkerboard base, 1 = full generated pattern).
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    intensity: f32,

    /// Number of frames in the loop (clamped to 1..=48).
    #[arg(long, value_name = "N", default_value_t = 24)]
    frames: usize,

    /// Tile size in pixels per frame (clamped to 16..=512).
    #[arg(long, value_name = "PX", default_value_t = 256)]
    size: u32,

    /// Write the sprite-sheet PNG here (width = frames x size, height = size).
    #[arg(long, value_name = "PNG")]
    out: PathBuf,
}

#[derive(Args)]
struct RainbowScanCmd {
    /// VPK to scan the hero's VFX from (a skin VPK, or the base pak itself).
    #[arg(long, value_name = "VPK")]
    vpk: PathBuf,

    /// Base `pak01_dir.vpk` to fall back to for entries `--vpk` does not ship.
    #[arg(long, value_name = "VPK")]
    base: Option<PathBuf>,

    /// Hero codename(s) to scan. Defaults to every pinned hero recipe.
    #[arg(long = "hero", value_name = "CODENAME")]
    heroes: Vec<String>,
}

#[derive(Args)]
struct VmatCmd {
    /// VPK to read materials from (a skin VPK, or the base pak itself).
    #[arg(long, value_name = "VPK")]
    vpk: PathBuf,

    /// Base `pak01_dir.vpk` to fall back to for entries `--vpk` does not ship.
    #[arg(long, value_name = "VPK")]
    base: Option<PathBuf>,

    /// Hero codename whose `models/heroes*` materials to target (e.g. `vindicta`;
    /// material paths use display names, not model codenames).
    #[arg(long, value_name = "CODENAME", conflicts_with = "entries")]
    hero: Option<String>,

    /// Explicit `.vmat_c` entry path(s) to target instead of hero discovery.
    #[arg(long = "entry", value_name = "PATH")]
    entries: Vec<String>,

    /// List the targeted materials (shader, active feature flags, bound texture
    /// channels) instead of patching.
    #[arg(long)]
    list: bool,

    /// Curated look: gem (sheen, takes --tint), glass, pbr (NPR lighting off,
    /// real reflections), unlit, or ink (thick solid outline, takes --tint).
    #[arg(long, value_name = "NAME")]
    preset: Option<String>,

    /// Preset color as `R,G,B` (0..1 floats) or `#RRGGBB`.
    #[arg(long, value_name = "COLOR")]
    tint: Option<String>,

    /// Raw int/flag param edit `NAME=VALUE` (e.g. `F_SHEEN=1`). Repeatable.
    #[arg(long = "set-int", value_name = "NAME=V")]
    set_int: Vec<String>,

    /// Raw float param edit `NAME=VALUE`. Repeatable.
    #[arg(long = "set-float", value_name = "NAME=V")]
    set_float: Vec<String>,

    /// Raw vector param edit `NAME=X,Y,Z[,W]` (W defaults to 0). Repeatable.
    #[arg(long = "set-vec", value_name = "NAME=X,Y,Z[,W]")]
    set_vec: Vec<String>,

    /// Dynamic-expression param edit `NAME=EXPR`, compiled to engine bytecode
    /// (e.g. `g_vColorTint1=$ent_health<.4?float3(1,.1,.1):float3(1,1,1)`).
    /// Attributes the expression reads are auto-registered. Repeatable.
    #[arg(long = "set-expr", value_name = "NAME=EXPR")]
    set_expr: Vec<String>,

    /// Edit an *existing* dynamic expression in place via a sed-style
    /// substitution `NAME=s<D>FIND<D>REPLACE<D>` (pick any delimiter `<D>` so
    /// it avoids `/` in the expression), e.g.
    /// `g_flSelfIllumScale1=s|10 * time()|20 * time()|`. The current expression
    /// is decompiled (see `--list`), `FIND` replaced with `REPLACE` everywhere,
    /// and recompiled. Fails if the material has no such expression or `FIND` is
    /// absent. Repeatable.
    #[arg(long = "edit-expr", value_name = "NAME=s/FIND/REPLACE/")]
    edit_expr: Vec<String>,

    /// Target set for hero discovery: all, body, or weapons.
    #[arg(long, value_name = "TARGETS", default_value = "all")]
    targets: String,

    /// Pack the patched materials into this addon VPK.
    #[arg(long = "encode-vpk", value_name = "OUT_dir.vpk")]
    encode_vpk: Option<PathBuf>,
}

#[derive(Args)]
struct PortraitCmd {
    /// Input VPK to read portraits from.
    input: PathBuf,

    /// Directory to write decoded PNGs into (created if missing).
    #[arg(long, value_name = "DIR")]
    out: PathBuf,

    /// Only extract portraits for this hero codename (e.g. "hornet").
    /// Omit to extract every hero in the VPK.
    #[arg(long, value_name = "CODENAME")]
    hero: Option<String>,

    /// Write the JSON manifest to this file instead of stdout.
    #[arg(long, value_name = "FILE")]
    manifest: Option<PathBuf>,
}

#[derive(Args)]
struct ModelCmd {
    /// VPK to inspect (block structure, mesh/skeleton presence). Omit when
    /// using a subcommand such as `export`.
    input: Option<PathBuf>,

    #[command(subcommand)]
    action: Option<ModelAction>,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum ModelAction {
    /// Export a model entry to a textured binary glTF (`.glb`).
    Export(ModelExportArgs),

    /// Edit a model and pack the result into an addon VPK. Reshape existing
    /// geometry (vertex displacement via `--scale`/`--translate` or a Blender
    /// `--from-glb`), remove a part by material (`--remove-material`), or replace a
    /// part's geometry wholesale with a new mesh of any vertex/index count
    /// (`--replace-part <MESH> --from-glb <FILE>`, Tier 1d).
    Edit(ModelEditArgs),

    /// Recolor a model's baked per-vertex `COLOR` to a target hue (keeping
    /// saturation + value), re-encode the affected vertex buffers, and pack the
    /// result into an addon VPK. The model half of the Deadlock ability-VFX
    /// recolor: some effects (Paige's ult horse/knight) bake their color into
    /// mesh vertex colors, which the particle/texture recolors don't reach. Pass
    /// several ENTRIES to recolor a whole set into one addon, each overriding its
    /// base entry in place. Mirrors `vpkmerge texture`.
    Recolor(ModelRecolorArgs),
}

#[derive(Args)]
struct ModelRecolorArgs {
    /// One or more VPK-internal model paths (`.vmdl_c`) to recolor, read from
    /// `--vpk`. Several ENTRIES pack into one addon, each at its own path, e.g.
    /// `models/heroes_wip/bookworm/bookworm_horse.vmdl_c`.
    #[arg(required = true, num_args = 1.., value_name = "ENTRY")]
    entries: Vec<String>,

    /// VPK to read the model(s) from (a skin VPK, or the base pak itself).
    #[arg(long, value_name = "VPK")]
    vpk: PathBuf,

    /// Base `pak01_dir.vpk` to fall back to when `--vpk` does not ship a model.
    #[arg(long, value_name = "VPK")]
    base: Option<PathBuf>,

    /// Target hue in degrees (0..360). Every vertex's color is set to this hue
    /// while keeping its saturation and value, so neutral vertices stay neutral.
    /// Matches the particle/texture recolor: the same hue lands all three.
    #[arg(long, value_name = "DEG", allow_hyphen_values = true)]
    hue: f64,

    /// Saturation scale (default 1.0 = keep source). > 1 boosts, < 1 mutes.
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    saturation: f64,

    /// Brightness (HSV value) scale (default 1.0 = keep source). > 1 lightens,
    /// < 1 darkens.
    #[arg(long, value_name = "SCALE", default_value_t = 1.0)]
    brightness: f64,

    /// List each model's color-bearing vertex buffers (mesh part, block, vertex
    /// count) and exit without recoloring.
    #[arg(long)]
    list: bool,

    /// Pack the recolored model(s) into a standalone addon VPK at this path,
    /// each at its base entry path so it overrides in place.
    #[arg(long = "encode-vpk", value_name = "OUT_dir.vpk")]
    encode_vpk: Option<PathBuf>,
}

#[derive(Args)]
struct ModelExportArgs {
    /// VPK containing the `.vmdl_c` (a skin VPK, or the base pak itself).
    #[arg(long, value_name = "VPK")]
    vpk: PathBuf,

    /// VPK-internal model path, e.g.
    /// `models/heroes_staging/hornet_v3/hornet.vmdl_c`. Mutually exclusive with
    /// `--hero`.
    #[arg(
        long,
        value_name = "PATH",
        required_unless_present = "hero",
        conflicts_with = "hero"
    )]
    entry: Option<String>,

    /// Hero codename (e.g. `hornet`, `bookworm`) whose body model
    /// (`<dir>/<codename>.vmdl_c`) is auto-discovered. Mutually exclusive with
    /// `--entry`.
    #[arg(long, value_name = "CODENAME")]
    hero: Option<String>,

    /// Base `pak01_dir.vpk` that referenced materials/textures resolve against
    /// when the skin VPK does not ship them.
    #[arg(long, value_name = "VPK")]
    base: Option<PathBuf>,

    /// Exclude all animation clips (export the static mesh + skeleton only).
    #[arg(long, conflicts_with = "clip")]
    no_anim: bool,

    /// Only export the named clip(s) (e.g. `--clip primary_stand_idle`).
    /// Repeatable. Omit to export every clip the model carries.
    #[arg(long, value_name = "NAME")]
    clip: Vec<String>,

    /// Bake a single frame into the mesh as a static posed `.glb` (no skeleton,
    /// skin, or clips). Optional value `CLIP[@FRAME]`: omit the clip to try the
    /// default menu/idle poses, append `@N` to pick a frame (default 0). E.g.
    /// `--pose`, `--pose ui_hero_pose`, `--pose idle_loadout@5`.
    #[arg(
        long,
        value_name = "CLIP[@FRAME]",
        num_args = 0..=1,
        default_missing_value = "",
        conflicts_with_all = ["no_anim", "clip"]
    )]
    pose: Option<String>,

    /// With `--pose`, error out instead of emitting a static bind/T-pose when the
    /// model carries no menu/idle pose clip (WIP heroes ship the rig but no baked
    /// clips). Lets a caller fall back to a 2D portrait rather than show an
    /// unposed hero.
    #[arg(long, requires = "pose")]
    require_pose: bool,

    /// Output `.glb` path.
    #[arg(long, value_name = "FILE")]
    out: PathBuf,
}

#[derive(Args)]
#[allow(clippy::struct_excessive_bools)]
struct ModelEditArgs {
    /// VPK containing the `.vmdl_c` to edit (a mesh skin, or the base pak itself).
    #[arg(long, value_name = "VPK")]
    vpk: PathBuf,

    /// VPK-internal model path, e.g.
    /// `models/heroes_staging/hornet_v3/hornet.vmdl_c`.
    #[arg(long, value_name = "PATH")]
    entry: String,

    /// Base `pak01_dir.vpk` to read the model from when `--vpk` is a texture-only
    /// skin that does not ship the mesh. Rarely needed for a geometry edit.
    #[arg(long, value_name = "VPK")]
    base: Option<PathBuf>,

    /// List the model's editable vertex buffers (mesh part, block index, vertex
    /// count) and exit without editing.
    #[arg(long)]
    list: bool,

    /// List the model's renderable draw calls (mesh part, material, vertex/index
    /// count) and exit. Use it to find the material for `--remove-material`.
    #[arg(long = "list-drawcalls")]
    list_drawcalls: bool,

    /// Emit machine-readable JSON for `--list-drawcalls`.
    #[arg(long)]
    json: bool,

    /// Remove every draw call whose material contains this string
    /// (case-insensitive), so that part stops rendering, then pack an addon VPK
    /// (needs `--encode-vpk`). E.g. `--remove-material vindicta_dress`. This is a
    /// draw-call-only edit: no vertices change.
    #[arg(long = "remove-material", value_name = "MATERIAL")]
    remove_material: Option<String>,

    /// Diagnostic: re-encode every MDAT block unchanged and pack an addon VPK
    /// (needs `--encode-vpk`). Probes whether the engine accepts our re-encoded
    /// model KV3 blocks at all, independent of any edit.
    #[arg(long = "reencode-mdat")]
    reencode_mdat: bool,

    /// Edit only the mesh part with this name (see `--list`). For the transform
    /// edit, omit to edit every editable part. For `--export-glb` / `--from-glb`
    /// the part must resolve to a single buffer (else pass `--block`).
    #[arg(long, value_name = "NAME")]
    part: Option<String>,

    /// Select draw calls whose material path contains this substring. Repeat for
    /// grouped GLB export/replacement.
    #[arg(long = "material", value_name = "SUBSTRING")]
    material: Vec<String>,

    /// Select a suggested semantic group, e.g. gun, hair, dress, body, hands, legs.
    #[arg(long = "group", value_name = "NAME")]
    group: Option<String>,

    /// Target a specific vertex buffer by its block index (see `--list`).
    /// Disambiguates a multi-buffer part for `--export-glb` / `--from-glb`.
    #[arg(long, value_name = "INDEX")]
    block: Option<usize>,

    /// Export the chosen buffer to a `.glb` (with a `_ORIGID` carrier) for
    /// reshaping in Blender, then re-import with `--from-glb`. Needs `--part`
    /// (single-buffer) or `--block`.
    #[arg(long = "export-glb", value_name = "FILE", conflicts_with_all = ["from_glb", "list"])]
    export_glb: Option<PathBuf>,

    /// Export selected draw calls/materials as one isolated `.glb`.
    #[arg(
        long = "export-group-glb",
        value_name = "FILE",
        conflicts_with_all = ["from_glb", "export_glb", "list"]
    )]
    export_group_glb: Option<PathBuf>,

    /// Apply a Blender-reshaped `.glb` (from `--export-glb`) back onto the buffer
    /// and pack an addon VPK (needs `--encode-vpk`). Topology must be preserved.
    #[arg(long = "from-glb", value_name = "FILE", conflicts_with_all = ["export_glb", "list"])]
    from_glb: Option<PathBuf>,

    /// Replace the named mesh part's geometry with a new mesh from `--from-glb`
    /// (Tier 1d), then pack an addon VPK (needs `--encode-vpk`). The new mesh may
    /// have any vertex/index count but must be skinned to bones the target part
    /// already uses; the part must be single-buffer/single-drawcall (e.g. `gun`).
    #[arg(
        long = "replace-part",
        value_name = "MESH",
        conflicts_with_all = ["export_glb", "list", "list_drawcalls", "remove_material", "reencode_mdat"]
    )]
    replace_part: Option<String>,

    /// Replace a semantic/multi-draw-call group from `--from-glb`, then pack an
    /// addon VPK. The value is interpreted as a suggested group name first, then
    /// as a material/mesh substring fallback.
    #[arg(
        long = "replace-group",
        value_name = "GROUP",
        conflicts_with_all = ["export_glb", "export_group_glb", "list", "list_drawcalls", "remove_material", "reencode_mdat", "replace_part"]
    )]
    replace_group: Option<String>,

    /// Disambiguates which primitive `--from-glb` provides when replacing a part
    /// from a multi-mesh `.glb` (defaults to the only primitive).
    #[arg(long = "glb-mesh", value_name = "NAME")]
    glb_mesh: Option<String>,

    /// Uniform scale about each edited part's centroid (1.0 = unchanged).
    #[arg(long, default_value_t = 1.0, value_name = "S")]
    scale: f32,

    /// Translate edited geometry by `x,y,z` in model space (applied after scale).
    #[arg(long, value_name = "x,y,z")]
    translate: Option<String>,

    /// Output addon VPK. Packs the edited model at `--entry` (or `--vpk-entry`)
    /// so it overrides the base pak.
    #[arg(long = "encode-vpk", value_name = "OUT_dir.vpk")]
    encode_vpk: Option<PathBuf>,

    /// Entry path for the edited model inside `--encode-vpk` (defaults to `--entry`).
    #[arg(long, value_name = "PATH")]
    vpk_entry: Option<String>,
}

#[derive(Args)]
struct SplitCmd {
    /// Input VPK to split.
    input: PathBuf,

    /// JSON plan describing outputs and prefix predicates.
    #[arg(long, value_name = "FILE")]
    plan: PathBuf,

    /// Refuse to split if any path matches more than one output.
    #[arg(long, conflicts_with = "all_matches")]
    strict: bool,

    /// Route each path to EVERY matching output (entries can land in multiple outputs).
    #[arg(long)]
    all_matches: bool,

    /// Optional residual VPK path. Overrides "residual" in the plan if both are set.
    #[arg(long, value_name = "FILE")]
    residual: Option<PathBuf>,

    /// Log each path routed to each output.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(serde::Deserialize)]
struct PlanFile {
    outputs: Vec<PlanOutput>,
    #[serde(default)]
    residual: Option<PathBuf>,
}

#[derive(serde::Deserialize)]
struct PlanOutput {
    path: PathBuf,
    prefixes: Vec<String>,
}

#[derive(serde::Serialize)]
struct Manifest {
    input: String,
    portraits: Vec<ManifestEntry>,
}

#[derive(serde::Serialize)]
struct ManifestEntry {
    source_path: String,
    hero_codename: String,
    variant: String,
    width: u32,
    height: u32,
    format_name: String,
    /// Absolute path to the decoded PNG, or null if not decoded.
    output_path: Option<String>,
    /// Why the texture was skipped (null when decoded).
    skipped_reason: Option<String>,
}

impl From<PortraitInfo> for ManifestEntry {
    fn from(p: PortraitInfo) -> Self {
        Self {
            source_path: p.source_path,
            hero_codename: p.hero_codename,
            variant: p.variant.as_str().to_string(),
            width: p.width,
            height: p.height,
            format_name: p.format_name.to_string(),
            output_path: p.output_path.map(|p| p.display().to_string()),
            skipped_reason: p.skipped_reason,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Some(Command::Split(args)) => run_split(args),
        Some(Command::Portrait(args)) => run_portrait(args),
        Some(Command::Model(args)) => run_model(args),
        Some(Command::Soundevents(args)) => run_soundevents(args),
        Some(Command::Texture(args)) => run_texture(args),
        Some(Command::RecolorHero(args)) => run_recolor_hero(&args),
        Some(Command::Prism(args)) => run_prism(&args),
        Some(Command::TrippySkin(args)) => run_trippy_skin(&args),
        Some(Command::TrippyVfx(args)) => run_trippy_vfx(&args),
        Some(Command::TrippyPreview(args)) => run_trippy_preview(&args),
        Some(Command::RainbowScan(args)) => run_rainbow_scan(&args),
        Some(Command::Vmat(args)) => run_vmat(&args),
        None => run_merge(cli),
    }
}

/// Parses a `--pose` value `CLIP[@FRAME]` into a [`PoseSelection`]. An empty clip
/// part means "use the default candidate poses"; the frame defaults to 0.
fn parse_pose(spec: &str) -> Result<PoseSelection> {
    let (clip, frame) = match spec.split_once('@') {
        Some((c, f)) => (
            c,
            f.parse::<usize>()
                .with_context(|| format!("--pose frame `{f}` is not a non-negative integer"))?,
        ),
        None => (spec, 0),
    };
    let clips = if clip.is_empty() {
        Vec::new()
    } else {
        vec![clip.to_string()]
    };
    Ok(PoseSelection {
        clips,
        frame,
        require: false,
    })
}

fn run_model(args: ModelCmd) -> Result<()> {
    match args.action {
        Some(ModelAction::Export(e)) => return run_model_export(&e),
        Some(ModelAction::Edit(e)) => return run_model_edit(e),
        Some(ModelAction::Recolor(e)) => return run_model_recolor(&e),
        None => {}
    }

    let input = args.input.context(
        "model: provide a VPK to inspect, or use `model export --vpk <vpk> --entry <path> --out <file.glb>`",
    )?;
    let models = inspect_models(&input)
        .with_context(|| format!("inspecting models in {}", input.display()))?;

    if models.is_empty() {
        println!("{}: no .vmdl_c entries found", input.display());
        return Ok(());
    }

    for m in &models {
        let i = &m.info;
        println!("\n{}", m.path);
        println!(
            "  mesh parts: {}   index buffers: {}   vertex bytes: {}",
            i.mesh_parts, i.index_buffers, i.vertex_bytes
        );
        println!(
            "  embedded geometry: {}   skeleton/anim: {}   physics: {}",
            i.has_embedded_geometry, i.has_skeleton_anim, i.has_physics
        );

        // Collapse the block table into a "KINDxCOUNT (bytes)" histogram so a
        // model with 8 MVTX/MIDX pairs reads at a glance instead of 29 lines.
        let mut counts: std::collections::BTreeMap<&str, (usize, u64)> =
            std::collections::BTreeMap::new();
        for b in &i.blocks {
            let e = counts.entry(b.kind.as_str()).or_insert((0, 0));
            e.0 += 1;
            e.1 += u64::from(b.size);
        }
        let histogram: Vec<String> = counts
            .iter()
            .map(|(k, (n, sz))| format!("{k}x{n} ({sz}B)"))
            .collect();
        println!("  blocks: {}", histogram.join("  "));
    }

    Ok(())
}

fn run_model_export(e: &ModelExportArgs) -> Result<()> {
    let pose = e.pose.as_deref().map(parse_pose).transpose()?.map(|mut p| {
        p.require = e.require_pose;
        p
    });
    let anim = AnimOptions {
        no_anim: e.no_anim,
        clips: e.clip.clone(),
        pose,
    };
    match (&e.entry, &e.hero) {
        (Some(entry), _) => export_model(&e.vpk, entry, e.base.as_deref(), &anim, &e.out)
            .with_context(|| format!("exporting {entry} from {}", e.vpk.display()))?,
        (None, Some(hero)) => export_hero_model(&e.vpk, hero, e.base.as_deref(), &anim, &e.out)
            .with_context(|| format!("exporting hero {hero} from {}", e.vpk.display()))?,
        (None, None) => anyhow::bail!("model export: provide --entry or --hero"),
    }
    println!("wrote {}", e.out.display());
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn run_model_edit(e: ModelEditArgs) -> Result<()> {
    // --list: enumerate vertex buffers and exit.
    if e.list {
        return run_model_list_targets(&e);
    }

    // --list-drawcalls: enumerate renderable draw calls and exit.
    if e.list_drawcalls {
        return run_model_list_drawcalls(&e);
    }

    // --remove-material: drop matching draw calls and pack an addon VPK.
    if e.remove_material.is_some() {
        return run_model_remove_material(&e);
    }

    // --reencode-mdat: diagnostic identity re-encode of every MDAT block.
    if e.reencode_mdat {
        return run_model_reencode_mdat(&e);
    }

    // --export-group-glb: write selected draw calls/materials to an isolated glb.
    if let Some(out_glb) = &e.export_group_glb {
        let selector = group_selector(&e, e.group.as_deref());
        let count = vpkmerge_core::export_model_group_glb(
            &e.vpk,
            &e.entry,
            e.base.as_deref(),
            &selector,
            out_glb,
        )
        .with_context(|| format!("exporting selected group of {}", e.entry))?;
        println!(
            "wrote {} ({} draw call(s) from {})",
            out_glb.display(),
            count,
            e.entry
        );
        return Ok(());
    }

    // --export-glb: write the chosen buffer to a glb for Blender editing.
    if let Some(out_glb) = &e.export_glb {
        let block = resolve_edit_block(&e)?;
        vpkmerge_core::export_model_buffer_glb(&e.vpk, &e.entry, e.base.as_deref(), block, out_glb)
            .with_context(|| format!("exporting buffer {block} of {}", e.entry))?;
        println!("wrote {} (block {block} of {})", out_glb.display(), e.entry);
        return Ok(());
    }

    // --replace-part: splice a new mesh (from --from-glb) over a part and pack.
    if let Some(mesh_name) = &e.replace_part {
        return run_model_replace_part(&e, mesh_name);
    }

    // --replace-group: splice donor primitives over selected draw calls.
    if let Some(group) = &e.replace_group {
        return run_model_replace_group(&e, group);
    }

    // --from-glb: apply a Blender-reshaped glb back and pack an addon VPK.
    if let Some(in_glb) = &e.from_glb {
        let block = resolve_edit_block(&e)?;
        let out = e.encode_vpk.as_ref().context(
            "model edit --from-glb: provide --encode-vpk <OUT_dir.vpk> for the edited model",
        )?;
        let glb_bytes =
            std::fs::read(in_glb).with_context(|| format!("reading {}", in_glb.display()))?;
        let vpk_entry = vpkmerge_core::apply_model_edit_glb(
            &e.vpk,
            &e.entry,
            e.base.as_deref(),
            block,
            &glb_bytes,
            out,
            e.vpk_entry.as_deref(),
        )
        .with_context(|| format!("applying {} to {}", in_glb.display(), e.entry))?;
        println!(
            "wrote {}: 1 entry ({vpk_entry}) reshaped from {}",
            out.display(),
            in_glb.display()
        );
        return Ok(());
    }

    // Guard against a no-op (which would just repack the model unchanged): no
    // scale change and no translate given.
    if (e.scale - 1.0).abs() < f32::EPSILON && e.translate.is_none() {
        anyhow::bail!("model edit: nothing to do (set --scale and/or --translate, or use --list)");
    }

    let translate = e
        .translate
        .as_deref()
        .map(parse_translate)
        .transpose()?
        .unwrap_or([0.0; 3]);

    let out = e.encode_vpk.context(
        "model edit: provide --encode-vpk <OUT_dir.vpk> to write the edited model (or --list)",
    )?;

    let edit = GeometryEdit {
        part: e.part.clone(),
        scale: e.scale,
        translate,
    };
    let report = edit_model_geometry(
        &e.vpk,
        &e.entry,
        e.base.as_deref(),
        &edit,
        &out,
        e.vpk_entry.as_deref(),
    )
    .with_context(|| format!("editing {} from {}", e.entry, e.vpk.display()))?;

    eprintln!(
        "edited {} buffer(s) across part(s) [{}], {} vertices moved",
        report.edited_buffers,
        report.edited_parts.join(", "),
        report.edited_vertices,
    );
    println!(
        "wrote {}: 1 entry ({}) edited model",
        out.display(),
        report.vpk_entry,
    );
    Ok(())
}

/// `model edit --list`: print the model's vertex buffers (mesh part, block index,
/// vertex count, stride, editability) so a user can pick a `--block`/`--part`.
fn run_model_list_targets(e: &ModelEditArgs) -> Result<()> {
    let targets = model_vertex_targets(&e.vpk, &e.entry, e.base.as_deref())
        .with_context(|| format!("listing targets for {}", e.entry))?;
    println!("{}: {} vertex buffer(s)", e.entry, targets.len());
    for t in &targets {
        println!(
            "  {:<16} block {:<3} {:>7} verts  stride {:<3} {}",
            t.mesh_name,
            t.block_index,
            t.vertex_count,
            t.stride,
            if t.editable {
                "editable"
            } else {
                "not editable"
            },
        );
    }
    Ok(())
}

/// `model edit --list-drawcalls`: print the renderable draw calls (mesh part,
/// material, vertex/index counts) so a user can find a `--remove-material` target.
fn run_model_list_drawcalls(e: &ModelEditArgs) -> Result<()> {
    if e.json {
        let inspection = vpkmerge_core::inspect_model_parts(&e.vpk, &e.entry, e.base.as_deref())
            .with_context(|| format!("listing draw calls for {}", e.entry))?;
        let json = serde_json::to_string_pretty(&inspection_json(&inspection))
            .context("serializing JSON")?;
        println!("{json}");
        return Ok(());
    }

    let calls = model_draw_call_targets(&e.vpk, &e.entry, e.base.as_deref())
        .with_context(|| format!("listing draw calls for {}", e.entry))?;
    println!("{}: {} renderable draw call(s)", e.entry, calls.len());
    for c in &calls {
        println!(
            "  {:<16} {:>7} verts {:>8} idx  {}",
            c.mesh_name, c.vertex_count, c.index_count, c.material
        );
    }
    Ok(())
}

fn inspection_json(i: &vpkmerge_core::ModelPartInspection) -> serde_json::Value {
    use serde_json::json;
    json!({
        "entry": &i.entry,
        "model_source": {
            "path": &i.model_source.path,
            "compiled_path": &i.model_source.compiled_path,
            "source": &i.model_source.source,
        },
        "draw_calls": i.draw_calls.iter().map(|c| json!({
            "id": &c.id,
            "mesh_part_name": &c.mesh_name,
            "mesh_index": c.mesh_index,
            "primitive_index": c.primitive_index,
            "data_block": c.data_block,
            "scene_object": c.scene_object,
            "draw_call": c.draw_call,
            "vertex_buffers": &c.vertex_buffers,
            "vertex_buffer": c.vertex_buffer,
            "index_buffer": c.index_buffer,
            "vertex_blocks": &c.vertex_blocks,
            "vertex_block": c.vertex_block,
            "index_block": c.index_block,
            "material": &c.material,
            "material_source": &c.material_source,
            "textures": c.textures.iter().map(|t| json!({
                "slot": &t.slot,
                "path": &t.path,
                "compiled_path": &t.compiled_path,
                "source": &t.source,
            })).collect::<Vec<_>>(),
            "vertex_count": c.vertex_count,
            "index_count": c.index_count,
            "start_index": c.start_index,
            "base_vertex": c.base_vertex,
            "primitive_type": &c.primitive_type,
            "primitive_identifier": &c.id,
            "geometry_source": &c.geometry_source,
            "skin": {
                "skinned": c.skin.skinned,
                "bone_weight_count": c.skin.bone_weight_count,
                "used_bone_count": c.skin.used_bone_count,
                "used_bones": &c.skin.used_bones,
            },
        })).collect::<Vec<_>>(),
        "suggested_groups": i.suggested_groups.iter().map(|g| json!({
            "name": &g.name,
            "label": &g.label,
            "aliases": &g.aliases,
            "draw_call_ids": &g.draw_call_ids,
            "mesh_part_names": &g.mesh_names,
            "materials": &g.materials,
            "vertex_count": g.vertex_count,
            "index_count": g.index_count,
            "confidence": g.confidence,
            "selector": {
                "group": &g.name,
                "materials": &g.materials,
                "mesh_parts": &g.mesh_names,
            },
        })).collect::<Vec<_>>(),
    })
}

/// `model edit --remove-material`: drop every draw call whose material matches,
/// then pack the edited model into the `--encode-vpk` addon VPK.
fn run_model_remove_material(e: &ModelEditArgs) -> Result<()> {
    let material = e
        .remove_material
        .as_deref()
        .expect("caller checks remove_material is set");
    let out = e.encode_vpk.as_ref().context(
        "model edit --remove-material: provide --encode-vpk <OUT_dir.vpk> for the edited model",
    )?;
    let report = vpkmerge_core::remove_model_material(
        &e.vpk,
        &e.entry,
        e.base.as_deref(),
        material,
        out,
        e.vpk_entry.as_deref(),
    )
    .with_context(|| format!("removing {material:?} from {}", e.entry))?;

    eprintln!("removed {} draw call(s):", report.removed.len());
    for r in &report.removed {
        eprintln!(
            "  {:<16} {:>8} idx  {}",
            r.mesh_name, r.index_count, r.material
        );
    }
    println!(
        "wrote {}: 1 entry ({}) with {} draw call(s) removed",
        out.display(),
        report.vpk_entry,
        report.removed.len(),
    );
    Ok(())
}

/// `model edit --replace-part`: splice a new mesh (from `--from-glb`) over the
/// named part and pack the edited model into the `--encode-vpk` addon VPK.
fn run_model_replace_part(e: &ModelEditArgs, mesh_name: &str) -> Result<()> {
    let in_glb = e
        .from_glb
        .as_ref()
        .context("model edit --replace-part: provide --from-glb <FILE.glb> with the new mesh")?;
    let out = e.encode_vpk.as_ref().context(
        "model edit --replace-part: provide --encode-vpk <OUT_dir.vpk> for the edited model",
    )?;
    let glb_bytes =
        std::fs::read(in_glb).with_context(|| format!("reading {}", in_glb.display()))?;

    let report = vpkmerge_core::replace_model_part(
        &e.vpk,
        &e.entry,
        e.base.as_deref(),
        mesh_name,
        &glb_bytes,
        e.glb_mesh.as_deref(),
        out,
        e.vpk_entry.as_deref(),
    )
    .with_context(|| format!("replacing part {mesh_name:?} in {}", e.entry))?;

    let r = &report.replaced;
    eprintln!(
        "replaced part {:?} ({}): {} -> {} verts, {} -> {} idx (stride {}, idx width {})",
        r.mesh_name,
        r.material,
        r.old_vertex_count,
        r.new_vertex_count,
        r.old_index_count,
        r.new_index_count,
        r.stride,
        r.index_size,
    );
    println!(
        "wrote {}: 1 entry ({}) with part {:?} replaced",
        out.display(),
        report.vpk_entry,
        r.mesh_name,
    );
    Ok(())
}

/// `model edit --replace-group`: splice donor primitives over a semantic group
/// and pack the edited model into the `--encode-vpk` addon VPK.
fn run_model_replace_group(e: &ModelEditArgs, group: &str) -> Result<()> {
    let in_glb = e
        .from_glb
        .as_ref()
        .context("model edit --replace-group: provide --from-glb <FILE.glb> with donor geometry")?;
    let out = e.encode_vpk.as_ref().context(
        "model edit --replace-group: provide --encode-vpk <OUT_dir.vpk> for the edited model",
    )?;
    let glb_bytes =
        std::fs::read(in_glb).with_context(|| format!("reading {}", in_glb.display()))?;
    let selector = group_selector(e, Some(group));

    let (vpk_entry, report) = vpkmerge_core::replace_model_group(
        &e.vpk,
        &e.entry,
        e.base.as_deref(),
        &selector,
        &glb_bytes,
        out,
        e.vpk_entry.as_deref(),
    )
    .with_context(|| format!("replacing group {group:?} in {}", e.entry))?;

    eprintln!(
        "replaced {} draw call(s) across {} mesh part(s):",
        report.replaced_draw_calls,
        report.replaced_parts.len()
    );
    for r in &report.replaced_parts {
        eprintln!(
            "  {:<16} {} -> {} verts, {} -> {} idx (stride {}, idx width {})",
            r.mesh_name,
            r.old_vertex_count,
            r.new_vertex_count,
            r.old_index_count,
            r.new_index_count,
            r.stride,
            r.index_size,
        );
    }
    println!(
        "wrote {}: 1 entry ({}) with group {:?} replaced",
        out.display(),
        vpk_entry,
        group,
    );
    Ok(())
}

fn group_selector(e: &ModelEditArgs, group: Option<&str>) -> ModelPartSelector {
    let mut mesh_parts = Vec::new();
    if let Some(part) = &e.part {
        mesh_parts.push(part.clone());
    }
    ModelPartSelector {
        group: group.map(str::to_string),
        materials: e.material.clone(),
        mesh_parts,
    }
}

/// `model edit --reencode-mdat`: diagnostic identity re-encode of every MDAT block
/// (re-emit uncompressed, byte-faithful, no edit), packed to `--encode-vpk`.
fn run_model_reencode_mdat(e: &ModelEditArgs) -> Result<()> {
    let out = e.encode_vpk.as_ref().context(
        "model edit --reencode-mdat: provide --encode-vpk <OUT_dir.vpk> for the re-encoded model",
    )?;
    let count = vpkmerge_core::reencode_model_mdat(
        &e.vpk,
        &e.entry,
        e.base.as_deref(),
        out,
        e.vpk_entry.as_deref(),
    )
    .with_context(|| format!("re-encoding MDAT of {}", e.entry))?;
    println!(
        "wrote {}: {count} MDAT block(s) re-encoded unchanged (identity diagnostic)",
        out.display()
    );
    Ok(())
}

/// Resolves the single vertex buffer (block index) a glb export/import targets,
/// from `--block` (exact) or `--part` (must name a single-buffer editable part).
fn resolve_edit_block(e: &ModelEditArgs) -> Result<usize> {
    let targets = model_vertex_targets(&e.vpk, &e.entry, e.base.as_deref())
        .with_context(|| format!("reading buffers of {}", e.entry))?;

    if let Some(b) = e.block {
        let t = targets
            .iter()
            .find(|t| t.block_index == b)
            .with_context(|| format!("no vertex buffer at block {b} (see --list)"))?;
        if !t.editable {
            anyhow::bail!("block {b} ({}) is not displacement-editable", t.mesh_name);
        }
        return Ok(b);
    }

    let part = e.part.as_deref().context(
        "specify --part <name> or --block <index> to choose the buffer to edit (see --list)",
    )?;
    let hits: Vec<&vpkmerge_core::VertexTarget> = targets
        .iter()
        .filter(|t| t.mesh_name == part && t.editable)
        .collect();
    match hits.len() {
        0 => anyhow::bail!("no editable buffer in part {part:?} (see --list)"),
        1 => Ok(hits[0].block_index),
        _ => {
            let blocks: Vec<String> = hits.iter().map(|t| t.block_index.to_string()).collect();
            anyhow::bail!(
                "part {part:?} has {} buffers (blocks {}); pass --block <index>",
                hits.len(),
                blocks.join(", ")
            )
        }
    }
}

/// Parses a `--translate` value `x,y,z` into a 3-float vector.
fn parse_translate(spec: &str) -> Result<[f32; 3]> {
    let parts: Vec<&str> = spec.split(',').collect();
    if parts.len() != 3 {
        anyhow::bail!("--translate expects x,y,z (three comma-separated numbers), got {spec:?}");
    }
    let mut out = [0.0f32; 3];
    for (i, p) in parts.iter().enumerate() {
        out[i] = p
            .trim()
            .parse()
            .with_context(|| format!("--translate component {:?} is not a number", p.trim()))?;
    }
    Ok(out)
}

/// Recolor one or more models' baked per-vertex colors to a hue and pack them
/// into one addon VPK (`--list` to just see each model's color buffers).
fn run_model_recolor(e: &ModelRecolorArgs) -> Result<()> {
    let base = e.base.as_deref();

    if e.list {
        for entry in &e.entries {
            let targets = model_vertex_targets(&e.vpk, entry, base)
                .with_context(|| format!("listing vertex buffers for {entry}"))?;
            let color: Vec<_> = targets.iter().filter(|t| t.has_color).collect();
            println!("{entry}: {} color-bearing vertex buffer(s)", color.len());
            for t in color {
                println!(
                    "  mesh {:<20} block {:>3}  {} verts",
                    t.mesh_name, t.block_index, t.vertex_count
                );
            }
        }
        return Ok(());
    }

    let out = e.encode_vpk.as_ref().context(
        "model recolor: provide --encode-vpk <OUT_dir.vpk> to write the recolored model(s) \
         (or --list to inspect)",
    )?;

    let recolor = vpkmerge_core::Recolor::new(e.hue, e.saturation, e.brightness);
    let report = vpkmerge_core::recolor_models_to_addon(&e.vpk, &e.entries, base, recolor, out)?;
    let total_verts: usize = report.iter().map(|r| r.stats.vertices).sum();
    for r in &report {
        eprintln!(
            "  {} buffer(s), {} color lane(s), {} verts  {}",
            r.stats.buffers_recolored, r.stats.color_lanes, r.stats.vertices, r.entry
        );
    }
    eprintln!(
        "wrote {}: {} model(s) recolored to hue {} deg ({} verts), each overriding its base entry in place",
        out.display(),
        report.len(),
        e.hue,
        total_verts
    );
    Ok(())
}

fn run_merge(cli: Cli) -> Result<()> {
    let Some(output) = cli.output else {
        anyhow::bail!("missing OUTPUT. Run `vpkmerge --help` for usage.");
    };

    if cli.verbose && !cli.strict {
        let conflicts = vpkmerge_core::detect_conflicts(&cli.inputs)?;
        for c in &conflicts {
            let winner_idx = *c.owner_indices.last().unwrap();
            let winner = cli.inputs[winner_idx].file_name().map_or_else(
                || cli.inputs[winner_idx].display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            println!("override: {} <- {}", c.path, winner);
        }
    }

    let policy = if cli.strict {
        CollisionPolicy::Error
    } else {
        CollisionPolicy::LastWins
    };

    let report = merge(
        &cli.inputs,
        &output,
        &MergeOptions {
            collision_policy: policy,
            ..Default::default()
        },
    )?;

    println!(
        "wrote {}: {} entries, {} paths overridden from {} inputs",
        report.output_path.display(),
        report.total_entries,
        report.overridden_paths,
        report.inputs,
    );
    Ok(())
}

fn run_split(args: SplitCmd) -> Result<()> {
    let plan = read_plan(&args.plan)?;
    let residual_path = args.residual.or(plan.residual);

    let outputs: Vec<SplitOutput> = plan
        .outputs
        .into_iter()
        .map(|o| SplitOutput {
            path: o.path,
            predicate: PathPredicate::AnyPrefix(o.prefixes),
        })
        .collect();

    let policy = if args.strict {
        OverlapPolicy::Error
    } else if args.all_matches {
        OverlapPolicy::AllMatches
    } else {
        OverlapPolicy::FirstMatch
    };

    if args.verbose {
        log_routing(&args.input, &outputs, policy, residual_path.as_deref())?;
    }

    let report = split(
        &args.input,
        &outputs,
        &SplitOptions {
            overlap_policy: policy,
            residual_path,
        },
    )?;

    println!(
        "split {}: {} input entries",
        args.input.display(),
        report.input_entries
    );
    for o in &report.outputs {
        println!("  {:<6} {}  {} entries", "out", o.path.display(), o.entries);
    }
    if let Some(r) = &report.residual {
        println!(
            "  {:<6} {}  {} entries",
            "resid",
            r.path.display(),
            r.entries
        );
    } else if report.unmatched > 0 {
        println!(
            "  {:<6} (dropped, no residual configured)  {} entries",
            "drop", report.unmatched
        );
    }
    Ok(())
}

fn run_portrait(args: PortraitCmd) -> Result<()> {
    let PortraitCmd {
        input,
        out,
        hero,
        manifest,
    } = args;
    let portraits = extract_portraits(&input, hero.as_deref(), &out)?;

    let decoded = portraits.iter().filter(|p| p.output_path.is_some()).count();
    let skipped = portraits.len() - decoded;
    eprintln!(
        "{}: {} portrait{} found, {decoded} decoded, {skipped} skipped",
        input.display(),
        portraits.len(),
        if portraits.len() == 1 { "" } else { "s" },
    );
    for p in &portraits {
        match (&p.output_path, &p.skipped_reason) {
            (Some(out), _) => eprintln!(
                "  {:<13} {:>4}x{:<4} {:<12} -> {}",
                p.hero_codename,
                p.width,
                p.height,
                p.variant.as_str(),
                out.display()
            ),
            (None, Some(reason)) => eprintln!(
                "  {:<13} {:<12} skipped: {reason}",
                p.hero_codename,
                p.variant.as_str()
            ),
            (None, None) => {}
        }
    }

    let manifest_data = Manifest {
        input: input.display().to_string(),
        portraits: portraits.into_iter().map(ManifestEntry::from).collect(),
    };
    let json = serde_json::to_string_pretty(&manifest_data).context("serializing manifest")?;
    if let Some(path) = &manifest {
        std::fs::write(path, &json).with_context(|| format!("writing {}", path.display()))?;
        eprintln!("manifest: {}", path.display());
    } else {
        println!("{json}");
    }
    Ok(())
}

fn run_soundevents(args: SoundeventsCmd) -> Result<()> {
    let SoundeventsCmd {
        input,
        from_vpk,
        encode,
        encode_vpk,
        vpk_entry,
        swap_vsnd,
        set,
    } = args;

    let label = match &from_vpk {
        Some(vpk) => format!("{} @ {}", input.display(), vpk.display()),
        None => input.display().to_string(),
    };

    let mut se = match &from_vpk {
        Some(vpk) => SoundEvents::from_vpk(vpk, &input.to_string_lossy())?,
        None => SoundEvents::from_file(&input)?,
    };

    // Apply edits (Phase 2). All edits log to stderr.
    for spec in &swap_vsnd {
        let (from, to) = spec
            .split_once('=')
            .with_context(|| format!("--swap-vsnd expects OLD=NEW, got {spec:?}"))?;
        let n = se.swap_vsnd(from, to);
        eprintln!("swap-vsnd: {n} path(s) rewritten {from} -> {to}");
    }
    for spec in &set {
        let (lhs, num) = spec
            .rsplit_once('=')
            .with_context(|| format!("--set expects EVENT/FIELD=NUMBER, got {spec:?}"))?;
        let (event, field) = lhs
            .rsplit_once('/')
            .with_context(|| format!("--set expects EVENT/FIELD=NUMBER, got {spec:?}"))?;
        let value: f64 = num
            .parse()
            .with_context(|| format!("--set value must be a number, got {num:?}"))?;
        if !se.set_event_field(event, field, value) {
            anyhow::bail!("--set: no event named {event:?} in {label}");
        }
        eprintln!("set: {event}/{field} = {value}");
    }

    // Human-readable summary to stderr.
    let summaries = se.summaries();
    eprintln!(
        "{label}: {} event{}",
        summaries.len(),
        if summaries.len() == 1 { "" } else { "s" }
    );
    for s in &summaries {
        let base = s
            .base
            .as_deref()
            .map_or(String::new(), |b| format!("  base={b}"));
        let vol = s.volume.map_or(String::new(), |v| format!("  volume={v}"));
        eprintln!(
            "  {:<34} {} sound{}{base}{vol}",
            s.name,
            s.vsnd_count,
            if s.vsnd_count == 1 { "" } else { "s" }
        );
    }

    // Outputs: a loose file (--encode), a standalone VPK (--encode-vpk), or
    // (if neither) the decoded JSON on stdout. --encode and --encode-vpk can be
    // combined; encode once and reuse the bytes.
    if encode.is_some() || encode_vpk.is_some() {
        let bytes = se.encode()?;
        if let Some(out) = &encode {
            std::fs::write(out, &bytes).with_context(|| format!("writing {}", out.display()))?;
            eprintln!(
                "wrote {}: {} bytes uncompressed (original was {} bytes)",
                out.display(),
                bytes.len(),
                se.original_len()
            );
        }
        if let Some(out) = &encode_vpk {
            let entry = match (&vpk_entry, &from_vpk) {
                (Some(e), _) => e.clone(),
                (None, Some(_)) => input.to_string_lossy().into_owned(),
                (None, None) => anyhow::bail!(
                    "--encode-vpk needs an entry path for a loose-file input: pass \
                     --vpk-entry <PATH> (or read with --from-vpk, which defaults the \
                     entry to INPUT)"
                ),
            };
            vpkmerge_core::pack(&[(entry.as_str(), bytes.as_slice())], out)?;
            eprintln!(
                "wrote {}: 1 entry ({entry}) {} bytes uncompressed",
                out.display(),
                bytes.len(),
            );
        }
    } else {
        let json = serde_json::to_string_pretty(&se.to_json()).context("serializing JSON")?;
        println!("{json}");
    }
    Ok(())
}

/// Recolor every entry in `inputs` (read from `vpk`) to `recolor` and pack the
/// whole set into one addon at `out`, each at its own VPK entry path.
fn recolor_textures_to_addon(
    inputs: &[PathBuf],
    vpk: &Path,
    recolor: vpkmerge_core::Recolor,
    out: &Path,
) -> Result<()> {
    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();
    for input in inputs {
        let entry = input.to_string_lossy().into_owned();
        let bytes = vpkmerge_core::read_vpk_entry(vpk, &entry)?;
        let summary = vpkmerge_core::inspect_texture(&bytes)
            .with_context(|| format!("{entry} is not a readable .vtex_c"))?;
        let recolored = vpkmerge_core::recolor_texture_hue(&bytes, recolor)?;
        eprintln!(
            "  {} {}x{} {}mip -> {} bytes  {entry}",
            summary.format,
            summary.width,
            summary.height,
            summary.mip_count,
            recolored.len()
        );
        packed.push((entry, recolored));
    }
    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, out)?;
    eprintln!(
        "wrote {}: {} textures recolored to hue {} deg, each overriding its base entry in place",
        out.display(),
        packed.len(),
        recolor.hue,
    );
    Ok(())
}

fn run_texture(args: TextureCmd) -> Result<()> {
    let TextureCmd {
        inputs,
        from_vpk,
        hue,
        saturation,
        brightness,
        preview,
        encode,
        encode_vpk,
        vpk_entry,
    } = args;

    let recolor = vpkmerge_core::Recolor::new(hue, saturation, brightness);

    // Batch: recolor several entries into one addon (each at its own path).
    if inputs.len() > 1 {
        if preview.is_some() || encode.is_some() || vpk_entry.is_some() {
            anyhow::bail!(
                "--preview/--encode/--vpk-entry apply to a single input; with multiple \
                 INPUTS pass only --encode-vpk <OUT_dir.vpk>"
            );
        }
        let out = encode_vpk.as_ref().context(
            "multiple INPUTS need --encode-vpk <OUT_dir.vpk> to pack them into one addon",
        )?;
        let vpk = from_vpk.as_ref().context(
            "multiple INPUTS must be VPK entry paths: pass --from-vpk <VPK> (each entry packs \
             back at its own path)",
        )?;
        return recolor_textures_to_addon(&inputs, vpk, recolor, out);
    }

    let input = &inputs[0];
    let label = match &from_vpk {
        Some(vpk) => format!("{} @ {}", input.display(), vpk.display()),
        None => input.display().to_string(),
    };

    let bytes = match &from_vpk {
        Some(vpk) => vpkmerge_core::read_vpk_entry(vpk, &input.to_string_lossy())?,
        None => std::fs::read(input).with_context(|| format!("reading {}", input.display()))?,
    };

    let summary = vpkmerge_core::inspect_texture(&bytes)
        .with_context(|| format!("{label} is not a readable .vtex_c"))?;
    eprintln!(
        "{label}: {} {}x{}, {} mip{} -> hue {hue} deg",
        summary.format,
        summary.width,
        summary.height,
        summary.mip_count,
        if summary.mip_count == 1 { "" } else { "s" }
    );

    // Preview is the design-intent color (pre re-encode); cheap, so do it first.
    if let Some(out) = &preview {
        let png = vpkmerge_core::recolor_texture_preview_png(&bytes, recolor)?;
        std::fs::write(out, &png).with_context(|| format!("writing preview {}", out.display()))?;
        eprintln!("wrote preview {} ({} bytes PNG)", out.display(), png.len());
    }

    // Outputs: a loose .vtex_c (--encode), a standalone addon VPK (--encode-vpk),
    // or (if neither, and no --preview) a dry run that just confirms it recolors.
    if encode.is_some() || encode_vpk.is_some() {
        let recolored = vpkmerge_core::recolor_texture_hue(&bytes, recolor)?;
        if let Some(out) = &encode {
            std::fs::write(out, &recolored)
                .with_context(|| format!("writing {}", out.display()))?;
            eprintln!(
                "wrote {}: {} bytes (recolored .vtex_c)",
                out.display(),
                recolored.len()
            );
        }
        if let Some(out) = &encode_vpk {
            let entry = match (&vpk_entry, &from_vpk) {
                (Some(e), _) => e.clone(),
                (None, Some(_)) => input.to_string_lossy().into_owned(),
                (None, None) => anyhow::bail!(
                    "--encode-vpk needs an entry path for a loose-file input: pass \
                     --vpk-entry <PATH> (or read with --from-vpk, which defaults the \
                     entry to INPUT)"
                ),
            };
            vpkmerge_core::pack(&[(entry.as_str(), recolored.as_slice())], out)?;
            eprintln!(
                "wrote {}: 1 entry ({entry}) overrides the base texture in place",
                out.display()
            );
        }
    } else if preview.is_none() {
        // Dry run: prove the recolor path works end to end without writing.
        vpkmerge_core::recolor_texture_hue(&bytes, recolor)?;
        eprintln!(
            "dry run OK (recolor succeeds); pass --encode <FILE>, --encode-vpk <OUT_dir.vpk>, \
             or --preview <PNG> to write output"
        );
    }
    Ok(())
}

/// Recolor a hero's full ability VFX to one target and pack it into a single
/// addon (or, with `--preview-png`, just render a fast swatch and exit).
fn run_recolor_hero(args: &RecolorHeroCmd) -> Result<()> {
    let recolor = vpkmerge_core::Recolor::new(args.hue, args.saturation, args.brightness);

    // Fast path: render the recipe's representative texture as a PNG swatch (no
    // bake, no re-encode), for a live UI preview.
    if let Some(png_out) = &args.preview_png {
        let png = vpkmerge_core::recolor_hero_preview_png(
            &args.vpk,
            args.base.as_deref(),
            &args.hero,
            recolor,
        )
        .with_context(|| format!("previewing hero {} recolor", args.hero))?;
        std::fs::write(png_out, &png)
            .with_context(|| format!("writing preview {}", png_out.display()))?;
        eprintln!(
            "wrote preview {} ({} bytes PNG) for hero {} at hue {} sat {} val {}",
            png_out.display(),
            png.len(),
            args.hero,
            args.hue,
            args.saturation,
            args.brightness,
        );
        return Ok(());
    }

    let out = args.encode_vpk.as_ref().context(
        "recolor-hero: provide --encode-vpk <OUT_dir.vpk> to bake the addon (or --preview-png \
         <PNG> for a fast swatch)",
    )?;
    let report = vpkmerge_core::recolor_hero_to_addon(
        &args.vpk,
        args.base.as_deref(),
        &args.hero,
        recolor,
        out,
    )
    .with_context(|| format!("recoloring hero {} to hue {}", args.hero, args.hue))?;

    eprintln!(
        "{}: {} particle(s) recolored ({} color-free skipped, {} unpatchable left vanilla), {} texture(s), {} material tint(s) ({} left vanilla), {} model(s) ({} verts)",
        report.codename,
        report.particles_recolored,
        report.particles_no_color,
        report.particles_unpatchable,
        report.textures_recolored,
        report.materials_recolored,
        report.materials_unpatchable,
        report.models_recolored,
        report.model_vertices,
    );
    if report.particles_unpatchable > 0 || report.materials_unpatchable > 0 {
        eprintln!(
            "  warning: {} color-bearing particle(s) and {} material(s) could not be patched in \
             place (a non-v5 KV3 block, or a ZSTD-compressed binary-blob section) and were left \
             vanilla; this hero's recolor is PARTIAL.",
            report.particles_unpatchable, report.materials_unpatchable,
        );
    }
    println!(
        "wrote {}: {} entries, hero {} recolored to hue {} deg (overrides the base in place)",
        out.display(),
        report.total_entries,
        report.codename,
        args.hue,
    );
    Ok(())
}

fn run_prism(args: &PrismCmd) -> Result<()> {
    let gradient = match &args.gradient {
        Some(spec) => Some(
            vpkmerge_core::PrismGradient::from_spec(spec)
                .map_err(|e| anyhow::anyhow!("--gradient: {e}"))?,
        ),
        None => None,
    };
    let animation_style = match args.animation_style.to_ascii_lowercase().as_str() {
        "sweep" => vpkmerge_core::PrismAnimationStyle::Sweep,
        "loop" | "loops" => vpkmerge_core::PrismAnimationStyle::Loop,
        "cycle" | "cycles" => vpkmerge_core::PrismAnimationStyle::Cycle,
        other => anyhow::bail!("--animation-style must be sweep, loop, or cycle (got {other:?})"),
    };
    let tuning = vpkmerge_core::PrismTuning {
        hue_offset: args.hue_offset,
        saturation: args.saturation,
        brightness: args.brightness,
        animation_intensity: args.animation_intensity,
        animation_style,
        gradient,
    };
    let report = vpkmerge_core::prism_recolor_hero_to_addon_tuned(
        &args.vpk,
        args.base.as_deref(),
        &args.hero,
        args.animated,
        tuning,
        &args.encode_vpk,
    )
    .with_context(|| format!("prism-recoloring hero {}", args.hero))?;

    eprintln!(
        "{}: {}/{} particle(s) prism-recolored ({} color-free, {} unpatchable left vanilla), {} gradient field(s), {} color field(s) ({} boosted, {} black-lifted, {} random-range), {} texture(s), {} material tint(s) ({} left vanilla), {} model(s) ({} verts)",
        report.codename,
        report.particles_recolored,
        report.particles_total,
        report.particles_no_color,
        report.particles_unpatchable,
        report.gradient_fields,
        report.color_fields,
        report.boosted_fields,
        report.lifted_black_gradient_fields,
        report.random_range_fields,
        report.textures_recolored,
        report.materials_recolored,
        report.materials_unpatchable,
        report.models_recolored,
        report.model_vertices,
    );
    if args.animated {
        eprintln!(
            "  animated: {} high-visibility particle(s) retimed ({} texture-age input(s), {} scroll multiplier(s), {} gradient timing edit(s), {} color-gradient loop(s), {} color-cycle operator(s))",
            report.particles_animated,
            report.texture_age_inputs,
            report.texture_offset_multipliers,
            report.gradient_timing_edits,
            report.color_gradient_loops,
            report.color_cycle_operators,
        );
    }
    if report.particles_unpatchable > 0 || report.materials_unpatchable > 0 {
        eprintln!(
            "  warning: {} color-bearing particle(s) and {} material(s) could not be patched in \
             place (a non-v5 KV3 block, or a ZSTD-compressed binary-blob section) and were left \
             vanilla; this hero's prism is PARTIAL.",
            report.particles_unpatchable, report.materials_unpatchable,
        );
    }
    println!(
        "wrote {}: {} entries, hero {} recolored as a prism spectrum (overrides the base in place)",
        args.encode_vpk.display(),
        report.total_entries,
        report.codename,
    );
    Ok(())
}

fn run_trippy_preview(args: &TrippyPreviewCmd) -> Result<()> {
    let style = vpkmerge_core::TrippyStyle::from_name(&args.style)
        .map_err(|e| anyhow::anyhow!("--style: {e:#}"))?;
    let frames = args.frames.clamp(1, 48);
    let size = args.size.clamp(16, 512);
    let sprite = vpkmerge_core::trippy_preview_sprite(
        style,
        args.phase,
        args.scroll,
        args.intensity,
        frames,
        size,
    )
    .with_context(|| format!("rendering {} trippy preview sprite", style.as_str()))?;
    std::fs::write(&args.out, &sprite)
        .with_context(|| format!("writing sprite {}", args.out.display()))?;
    println!(
        "wrote {}: {} frame(s) @ {}px ({} bytes PNG, {} style)",
        args.out.display(),
        frames,
        size,
        sprite.len(),
        style.as_str(),
    );
    Ok(())
}

fn run_trippy_skin(args: &TrippySkinCmd) -> Result<()> {
    let style = vpkmerge_core::TrippyStyle::from_name(&args.style)
        .map_err(|e| anyhow::anyhow!("--style: {e:#}"))?;
    let targets = args.targets.to_ascii_lowercase();
    let (include_body, include_weapons) = match targets.as_str() {
        "all" | "body,weapons" | "body,weapon" | "weapons,body" | "weapon,body" => (true, true),
        "body" | "skin" => (true, false),
        "weapon" | "weapons" => (false, true),
        other => anyhow::bail!("--targets must be all, body, or weapons (got {other:?})"),
    };
    let options = vpkmerge_core::TrippySkinOptions {
        style,
        intensity: args.intensity,
        phase: args.phase,
        scroll: args.scroll,
        include_body,
        include_weapons,
    };
    let report = vpkmerge_core::trippy_skin_to_addon(
        &args.vpk,
        args.base.as_deref(),
        &args.hero,
        &options,
        &args.encode_vpk,
    )
    .with_context(|| format!("building trippy skin for {}", args.hero))?;

    eprintln!(
        "{}: {} body texture(s), {} weapon texture(s), {} material(s) scrolled, {} placeholder texture(s) promoted",
        report.codename,
        report.body_textures,
        report.weapon_textures,
        report.materials_scrolled,
        report.texture_placeholders_promoted,
    );
    if report.skipped_unreadable > 0 || report.skipped_unpatchable_materials > 0 {
        eprintln!(
            "  warning: {} unreadable target(s), {} unpatchable material(s) skipped",
            report.skipped_unreadable, report.skipped_unpatchable_materials,
        );
    }
    println!(
        "wrote {}: {} entries, hero {} painted with {} trippy skin",
        args.encode_vpk.display(),
        report.total_entries,
        report.codename,
        style.as_str(),
    );
    Ok(())
}

fn run_trippy_vfx(args: &TrippyVfxCmd) -> Result<()> {
    let style = vpkmerge_core::TrippyStyle::from_name(&args.style)
        .map_err(|e| anyhow::anyhow!("--style: {e:#}"))?;
    let targets = args.targets.to_ascii_lowercase();
    let (include_abilities, include_weapons) = match targets.as_str() {
        "all" | "abilities,weapons" | "ability,weapon" | "weapons,abilities" | "weapon,ability" => {
            (true, true)
        }
        "ability" | "abilities" | "vfx" => (true, false),
        "weapon" | "weapons" => (false, true),
        other => anyhow::bail!("--targets must be all, abilities, or weapons (got {other:?})"),
    };
    let (animation_style, animation_intensity) =
        parse_trippy_vfx_animation(&args.animation_style, args.animation_intensity)?;
    let options = vpkmerge_core::TrippyAbilityOptions {
        style,
        intensity: args.intensity,
        phase: args.phase,
        animation_intensity,
        animation_style,
        include_abilities,
        include_weapons,
    };
    let report = vpkmerge_core::trippy_ability_vfx_to_addon(
        &args.vpk,
        args.base.as_deref(),
        &args.hero,
        &options,
        &args.encode_vpk,
    )
    .with_context(|| format!("building trippy ability VFX for {}", args.hero))?;

    eprintln!(
        "{}: {}/{} particle(s) trippy-recolored ({} color-free, {} unpatchable), {} texture(s) painted, {} material tint(s), {} material scroll(s), {} model(s) ({} verts)",
        report.codename,
        report.particles_recolored,
        report.particles_total,
        report.particles_no_color,
        report.particles_unpatchable,
        report.textures_painted,
        report.materials_recolored,
        report.materials_scrolled,
        report.models_recolored,
        report.model_vertices,
    );
    if animation_intensity > 0.0 {
        eprintln!(
            "  animated: {} particle(s) ({} texture-age input(s), {} scroll multiplier(s), {} gradient timing edit(s), {} color-gradient loop(s), {} color-cycle operator(s))",
            report.particles_animated,
            report.texture_age_inputs,
            report.texture_offset_multipliers,
            report.gradient_timing_edits,
            report.color_gradient_loops,
            report.color_cycle_operators,
        );
    }
    if report.textures_skipped > 0 || report.materials_unpatchable > 0 {
        eprintln!(
            "  warning: {} texture(s) and {} material(s) could not be patched and were skipped",
            report.textures_skipped, report.materials_unpatchable,
        );
    }
    println!(
        "wrote {}: {} entries, hero {} painted with {} trippy ability VFX",
        args.encode_vpk.display(),
        report.total_entries,
        report.codename,
        style.as_str(),
    );
    Ok(())
}

fn parse_trippy_vfx_animation(
    style: &str,
    intensity: f64,
) -> Result<(vpkmerge_core::PrismAnimationStyle, f64)> {
    let style = style.to_ascii_lowercase();
    match style.as_str() {
        "off" | "none" | "static" => Ok((vpkmerge_core::PrismAnimationStyle::Sweep, 0.0)),
        "sweep" => Ok((vpkmerge_core::PrismAnimationStyle::Sweep, intensity)),
        "loop" | "loops" => Ok((vpkmerge_core::PrismAnimationStyle::Loop, intensity)),
        "cycle" | "cycles" => Ok((vpkmerge_core::PrismAnimationStyle::Cycle, intensity)),
        other => {
            anyhow::bail!("--animation-style must be off, sweep, loop, or cycle (got {other:?})")
        }
    }
}

/// Parses `NAME=REST`, failing with the flag name on a missing `=`.
fn parse_name_eq<'a>(spec: &'a str, flag: &str) -> Result<(&'a str, &'a str)> {
    spec.split_once('=')
        .ok_or_else(|| anyhow::anyhow!("{flag}: expected NAME=VALUE, got {spec:?}"))
}

/// Parses a preset color: `R,G,B` floats 0..=1, or `#RRGGBB` / `RRGGBB` sRGB.
fn parse_tint(spec: &str) -> Result<[f64; 3]> {
    let hex = spec.strip_prefix('#').unwrap_or(spec);
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        let chan = |i: usize| -> Result<f64> {
            Ok(f64::from(u8::from_str_radix(&hex[i..i + 2], 16)?) / 255.0)
        };
        return Ok([chan(0)?, chan(2)?, chan(4)?]);
    }
    let parts: Vec<f64> = spec
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("--tint: expected R,G,B floats or #RRGGBB, got {spec:?}"))?;
    anyhow::ensure!(
        parts.len() == 3,
        "--tint: expected 3 components, got {}",
        parts.len()
    );
    Ok([parts[0], parts[1], parts[2]])
}

fn vmat_edits(args: &VmatCmd) -> Result<Vec<vpkmerge_core::VmatEdit>> {
    let tint = args.tint.as_deref().map(parse_tint).transpose()?;
    let mut edits = Vec::new();
    if let Some(preset) = &args.preset {
        edits.extend(vpkmerge_core::VmatPreset::from_name(preset)?.edits(tint));
    }
    for spec in &args.set_int {
        let (name, v) = parse_name_eq(spec, "--set-int")?;
        edits.push(vpkmerge_core::VmatEdit::Int {
            name: name.to_string(),
            value: v.parse().with_context(|| format!("--set-int {spec:?}"))?,
        });
    }
    for spec in &args.set_float {
        let (name, v) = parse_name_eq(spec, "--set-float")?;
        edits.push(vpkmerge_core::VmatEdit::Float {
            name: name.to_string(),
            value: v.parse().with_context(|| format!("--set-float {spec:?}"))?,
        });
    }
    for spec in &args.set_vec {
        let (name, v) = parse_name_eq(spec, "--set-vec")?;
        let comps: Vec<f64> = v
            .split(',')
            .map(|p| p.trim().parse::<f64>())
            .collect::<std::result::Result<_, _>>()
            .with_context(|| format!("--set-vec {spec:?}"))?;
        anyhow::ensure!(
            comps.len() == 3 || comps.len() == 4,
            "--set-vec {spec:?}: expected 3 or 4 components"
        );
        let mut value = [0.0; 4];
        value[..comps.len()].copy_from_slice(&comps);
        edits.push(vpkmerge_core::VmatEdit::Vector {
            name: name.to_string(),
            value,
        });
    }
    for spec in &args.set_expr {
        let (name, src) = parse_name_eq(spec, "--set-expr")?;
        edits.push(
            vpkmerge_core::VmatEdit::expr(name, src)
                .with_context(|| format!("--set-expr {spec:?}"))?,
        );
    }
    for spec in &args.edit_expr {
        let (name, find, replace) =
            parse_edit_expr(spec).with_context(|| format!("--edit-expr {spec:?}"))?;
        edits.push(vpkmerge_core::VmatEdit::EditExpr {
            name: name.to_string(),
            find,
            replace,
        });
    }
    Ok(edits)
}

/// Parse `NAME=s<D>FIND<D>REPLACE<D>` (sed-style; `<D>` is any single
/// delimiter char). The trailing delimiter is optional.
fn parse_edit_expr(spec: &str) -> Result<(&str, String, String)> {
    let (name, cmd) = parse_name_eq(spec, "--edit-expr")?;
    let mut chars = cmd.chars();
    anyhow::ensure!(
        chars.next() == Some('s'),
        "--edit-expr command must start with 's' (a sed-style substitution)"
    );
    let delim = chars
        .next()
        .context("--edit-expr: substitution needs a delimiter, e.g. s|find|replace|")?;
    let parts: Vec<&str> = chars.as_str().split(delim).collect();
    let (find, replace) = match parts.as_slice() {
        [find, replace] | [find, replace, ""] => (*find, *replace),
        _ => anyhow::bail!(
            "--edit-expr: delimiter {delim:?} must appear exactly twice \
             (s{delim}FIND{delim}REPLACE{delim}); pick a delimiter not used in the text"
        ),
    };
    anyhow::ensure!(!find.is_empty(), "--edit-expr: FIND must not be empty");
    Ok((name, find.to_string(), replace.to_string()))
}

fn run_vmat(args: &VmatCmd) -> Result<()> {
    let targets = if let Some(hero) = &args.hero {
        let t = args.targets.to_ascii_lowercase();
        let (include_body, include_weapons) = match t.as_str() {
            "all" => (true, true),
            "body" | "skin" => (true, false),
            "weapon" | "weapons" => (false, true),
            other => anyhow::bail!("--targets must be all, body, or weapons (got {other:?})"),
        };
        vpkmerge_core::VmatTargets::Hero {
            codename: hero.clone(),
            include_body,
            include_weapons,
        }
    } else if !args.entries.is_empty() {
        vpkmerge_core::VmatTargets::Entries(args.entries.clone())
    } else {
        anyhow::bail!("pass --hero CODENAME or one or more --entry PATH");
    };

    if args.list {
        let infos = vpkmerge_core::list_materials(&args.vpk, args.base.as_deref(), &targets)?;
        anyhow::ensure!(!infos.is_empty(), "no materials matched");
        for info in &infos {
            println!("{} [{}]", info.entry, info.shader);
            if !info.flags.is_empty() {
                let flags: Vec<String> = info
                    .flags
                    .iter()
                    .map(|(n, v)| {
                        if *v == 1 {
                            n.clone()
                        } else {
                            format!("{n}={v}")
                        }
                    })
                    .collect();
                println!("  flags: {}", flags.join(" "));
            }
            for (slot, path) in &info.textures {
                println!("  {slot} -> {path}");
            }
            for (name, expr) in &info.expressions {
                println!("  expr {name} = {expr}");
            }
        }
        println!("{} material(s)", infos.len());
        return Ok(());
    }

    let edits = vmat_edits(args)?;
    anyhow::ensure!(
        !edits.is_empty(),
        "nothing to do: pass --preset and/or --set-int/--set-float/--set-vec/--set-expr/--edit-expr (or --list)"
    );
    let Some(out) = &args.encode_vpk else {
        anyhow::bail!("--encode-vpk OUT_dir.vpk is required when patching");
    };

    let report = vpkmerge_core::style_materials_to_addon(
        &args.vpk,
        args.base.as_deref(),
        &targets,
        &edits,
        out,
    )?;
    eprintln!(
        "{} material(s) patched ({} params set, {} inserted); skipped: {} non-pbr, {} unreadable",
        report.materials_patched,
        report.params_set,
        report.params_inserted,
        report.skipped_non_pbr,
        report.skipped_unreadable,
    );
    for (entry, param) in &report.failed_params {
        eprintln!("  warning: could not apply {param} on {entry}");
    }
    println!(
        "wrote {}: {} material(s) styled",
        out.display(),
        report.materials_patched
    );
    Ok(())
}

fn run_rainbow_scan(args: &RainbowScanCmd) -> Result<()> {
    let heroes: Vec<String> = if args.heroes.is_empty() {
        vpkmerge_core::pinned_hero_codenames()
            .iter()
            .map(|s| (*s).to_string())
            .collect()
    } else {
        args.heroes.clone()
    };

    println!(
        "{:<12} {:>5} {:>5} {:>5} {:>5} {:>7} {:>6} {:>6} {:>5} {:>5} {:>5} {:>5} {:>3} {:>3} {:>3}  mode",
        "hero",
        "vpcf",
        "patch",
        "none",
        "err",
        "colors",
        "grad",
        "multi",
        "age",
        "loop",
        "rand",
        "fade",
        "tex",
        "mat",
        "mdl",
    );
    for hero in heroes {
        let report =
            vpkmerge_core::scan_hero_rainbow_support(&args.vpk, args.base.as_deref(), &hero)
                .with_context(|| format!("scanning rainbow support for hero {hero}"))?;
        let age_grad = report.collection_age_gradient_fields + report.particle_age_gradient_fields;
        println!(
            "{:<12} {:>5} {:>5} {:>5} {:>5} {:>7} {:>6} {:>6} {:>5} {:>5} {:>5} {:>5} {:>3} {:>3} {:>3}  {}",
            report.codename,
            report.particles_total,
            report.particles_patchable,
            report.particles_color_free,
            report.particles_unpatchable + report.particles_decode_failed,
            report.visible_color_fields,
            report.gradient_fields,
            report.multi_stop_gradient_fields,
            age_grad,
            report.looped_gradient_fields,
            report.random_color_initializers,
            report.color_interpolate_ops,
            report.texture_entries,
            report.material_entries,
            report.model_entries,
            rainbow_scan_mode(&report),
        );
    }
    println!(
        "\nmode: looped = existing looped gradient color inputs; animated = age/lifetime gradients; strong = many static gradients; static = color constants only"
    );
    Ok(())
}

fn rainbow_scan_mode(r: &vpkmerge_core::HeroRainbowSupportReport) -> &'static str {
    if r.particles_patchable == 0 {
        "none"
    } else if r.looped_gradient_fields > 0 {
        "looped"
    } else if r.collection_age_gradient_fields + r.particle_age_gradient_fields > 0 {
        "animated"
    } else if r.multi_stop_gradient_fields >= 12 {
        "strong"
    } else if r.gradient_fields > 0 {
        "gradient"
    } else {
        "static"
    }
}

fn read_plan(path: &Path) -> Result<PlanFile> {
    let bytes = std::fs::read(path).with_context(|| format!("reading plan {}", path.display()))?;
    let plan: PlanFile = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing plan {}", path.display()))?;
    Ok(plan)
}

fn log_routing(
    input: &Path,
    outputs: &[SplitOutput],
    policy: OverlapPolicy,
    residual: Option<&Path>,
) -> Result<()> {
    let info = vpkmerge_core::inspect(input)?;
    for path in &info.file_paths {
        let mut hits: Vec<&Path> = Vec::new();
        for o in outputs {
            if predicate_matches(&o.predicate, path) {
                hits.push(&o.path);
                if policy == OverlapPolicy::FirstMatch {
                    break;
                }
            }
        }
        if hits.is_empty() {
            if let Some(r) = residual {
                println!("residual: {path}  ->  {}", r.display());
            } else {
                println!("dropped : {path}");
            }
        } else {
            for h in hits {
                println!("route   : {path}  ->  {}", h.display());
            }
        }
    }
    Ok(())
}

fn predicate_matches(pred: &PathPredicate, path: &str) -> bool {
    match pred {
        PathPredicate::AnyPrefix(prefixes) => prefixes.iter().any(|p| path.starts_with(p)),
    }
}
