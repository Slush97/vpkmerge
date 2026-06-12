//! Procedural "trippy" hero skin repainting.
//!
//! This promotes the Paradox prototype (`examples/reskin_chrono_trippy.rs`) into
//! a reusable core path: generate seamless psychedelic albedo textures, pack them
//! at the hero's existing texture paths, and byte-patch VMAT scroll vectors so the
//! paint flows at runtime. Discovery is conservative: only hero-namespaced model
//! materials/textures are touched, so shared engine textures are not overwritten.

use crate::hero_recolor::{
    animate_particle_timing_bytes, insert_color_cycle_operator_with_tuning,
    is_prism_animation_target, loop_animate_particle_bytes, recolor_material_color_bytes,
    GradientStop, PrismAnimationStyle, PrismGradient, PrismTuning,
};
use crate::{recipe_for, recolor_model_vertex_colors, recolor_particle_bytes, Recolor};
use anyhow::{Context, Result};
use morphic::kv3::{Seg, Value};
use morphic::{Image, ImageData, TextureFormat};
use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::TAU;
use std::path::Path;

pub const TRIPPY_STYLE_NAMES: &[&str] = &[
    "confetti",
    "liquid",
    "moire",
    "kaleido",
    "holo",
    "glitch",
    "thermal",
    "gradient",
    "camo",
    "carbon",
    "galaxy",
    "halftone",
    "lava",
    "vaporwave",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrippyStyle {
    Confetti,
    Liquid,
    Moire,
    Kaleido,
    Holo,
    Glitch,
    Thermal,
    Gradient,
    Camo,
    Carbon,
    Galaxy,
    Halftone,
    Lava,
    Vaporwave,
}

impl TrippyStyle {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Confetti => "confetti",
            Self::Liquid => "liquid",
            Self::Moire => "moire",
            Self::Kaleido => "kaleido",
            Self::Holo => "holo",
            Self::Glitch => "glitch",
            Self::Thermal => "thermal",
            Self::Gradient => "gradient",
            Self::Camo => "camo",
            Self::Carbon => "carbon",
            Self::Galaxy => "galaxy",
            Self::Halftone => "halftone",
            Self::Lava => "lava",
            Self::Vaporwave => "vaporwave",
        }
    }

    pub fn from_name(name: &str) -> Result<Self> {
        match name.to_ascii_lowercase().as_str() {
            "confetti" | "acid" | "rainbow" => Ok(Self::Confetti),
            "liquid" | "marble" | "hololiquid" => Ok(Self::Liquid),
            "moire" | "opart" => Ok(Self::Moire),
            "kaleido" | "kaleidoscope" => Ok(Self::Kaleido),
            "holo" | "foil" | "holographic" => Ok(Self::Holo),
            "glitch" | "crt" => Ok(Self::Glitch),
            "thermal" | "xray" => Ok(Self::Thermal),
            "gradient" | "smooth" => Ok(Self::Gradient),
            "camo" | "camouflage" | "woodland" => Ok(Self::Camo),
            "carbon" | "carbonfiber" | "carbon-fiber" => Ok(Self::Carbon),
            "galaxy" | "nebula" | "cosmos" | "space" => Ok(Self::Galaxy),
            "halftone" | "popart" | "pop-art" | "comic" | "dots" => Ok(Self::Halftone),
            "lava" | "magma" | "molten" => Ok(Self::Lava),
            "vaporwave" | "synthwave" | "retrowave" | "outrun" => Ok(Self::Vaporwave),
            other => anyhow::bail!(
                "unknown trippy style {other:?}; expected one of {}",
                TRIPPY_STYLE_NAMES.join(", ")
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrippySkinOptions {
    pub style: TrippyStyle,
    /// Texture blend, 0.0 = original texture, 1.0 = full generated pattern.
    pub intensity: f32,
    /// Hue/pattern phase offset, normalized 0..1.
    pub phase: f32,
    /// Runtime material scroll scale, 1.0 = Paradox prototype speed.
    pub scroll: f64,
    pub include_body: bool,
    pub include_weapons: bool,
}

impl Default for TrippySkinOptions {
    fn default() -> Self {
        Self {
            style: TrippyStyle::Confetti,
            intensity: 1.0,
            phase: 0.0,
            scroll: 1.0,
            include_body: true,
            include_weapons: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TrippySkinReport {
    pub codename: String,
    pub body_textures: usize,
    pub weapon_textures: usize,
    pub materials_scrolled: usize,
    pub texture_placeholders_promoted: usize,
    pub skipped_shared_textures: usize,
    pub skipped_unreadable: usize,
    pub skipped_unpatchable_materials: usize,
    pub total_entries: usize,
}

#[derive(Debug, Clone)]
pub struct TrippyAbilityOptions {
    pub style: TrippyStyle,
    /// Texture blend strength; particle colors use this as saturation/value
    /// emphasis because particle color fields cannot be partially blended
    /// without a full particle-tree rewrite.
    pub intensity: f32,
    /// Hue/pattern phase offset, normalized 0..1.
    pub phase: f32,
    /// Strength of the optional particle animation pass.
    pub animation_intensity: f64,
    /// sweep = timing/scroll, loop = sweep + loop color gradients, cycle = loop +
    /// inserted runtime color-cycle operators where safe.
    pub animation_style: PrismAnimationStyle,
    pub include_abilities: bool,
    pub include_weapons: bool,
}

impl Default for TrippyAbilityOptions {
    fn default() -> Self {
        Self {
            style: TrippyStyle::Confetti,
            intensity: 1.0,
            phase: 0.0,
            animation_intensity: 1.0,
            animation_style: PrismAnimationStyle::Cycle,
            include_abilities: true,
            include_weapons: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TrippyAbilityReport {
    pub codename: String,
    pub particles_total: usize,
    pub particles_recolored: usize,
    pub particles_animated: usize,
    pub particles_no_color: usize,
    pub particles_unpatchable: usize,
    pub texture_age_inputs: usize,
    pub texture_offset_multipliers: usize,
    pub gradient_timing_edits: usize,
    pub color_gradient_loops: usize,
    pub color_cycle_operators: usize,
    pub textures_painted: usize,
    pub textures_skipped: usize,
    pub materials_recolored: usize,
    pub materials_scrolled: usize,
    pub materials_unpatchable: usize,
    pub models_recolored: usize,
    pub model_vertices: usize,
    pub missing_entries: usize,
    pub total_entries: usize,
}

/// Render procedural trippy pattern frames for a live UI preview.
///
/// The returned PNGs are generated from the same pattern function used by the
/// skin/ability texture bake; advancing `phase` over the frame loop mirrors the
/// runtime UV/color flow without needing the game engine.
#[allow(clippy::cast_precision_loss)]
pub fn trippy_preview_frames(
    style: TrippyStyle,
    phase: f32,
    scroll: f32,
    intensity: f32,
    n_frames: usize,
    size: u32,
) -> Result<Vec<Vec<u8>>> {
    let n_frames = n_frames.clamp(1, 48);
    let size = size.clamp(16, 512);
    let mut out = Vec::with_capacity(n_frames);

    for frame in 0..n_frames {
        let t = frame as f32 / n_frames as f32;
        let frame_phase = phase + t * scroll;
        out.push(trippy_preview_png(style, frame_phase, intensity, size)?);
    }

    Ok(out)
}

/// Render the same preview frames as [`trippy_preview_frames`], composed into a
/// single sprite-sheet PNG with the frames laid out left to right. One file
/// instead of N makes the loop cheap to hand to a UI: draw with a per-frame
/// source offset, or animate via CSS `steps()`.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
pub fn trippy_preview_sprite(
    style: TrippyStyle,
    phase: f32,
    scroll: f32,
    intensity: f32,
    n_frames: usize,
    size: u32,
) -> Result<Vec<u8>> {
    let n_frames = n_frames.clamp(1, 48);
    let size = size.clamp(16, 512);
    let width = size * n_frames as u32;
    let mut pixels = vec![255; (width as usize) * (size as usize) * 4];

    for frame in 0..n_frames {
        let t = frame as f32 / n_frames as f32;
        let frame_phase = phase + t * scroll;
        paint_preview_tile(
            &mut pixels,
            width,
            frame as u32 * size,
            style,
            frame_phase,
            intensity,
            size,
        );
    }

    let image = Image {
        width,
        height: size,
        data: ImageData::Rgba8(pixels),
    };
    morphic::encode_image(&image, TextureFormat::PngRgba8888)
        .context("encoding trippy preview sprite PNG")
}

fn trippy_preview_png(
    style: TrippyStyle,
    phase: f32,
    intensity: f32,
    size: u32,
) -> Result<Vec<u8>> {
    let mut pixels = vec![255; (size as usize) * (size as usize) * 4];
    paint_preview_tile(&mut pixels, size, 0, style, phase, intensity, size);
    let image = Image {
        width: size,
        height: size,
        data: ImageData::Rgba8(pixels),
    };
    morphic::encode_image(&image, TextureFormat::PngRgba8888).context("encoding trippy preview PNG")
}

/// Paint one `size`x`size` preview tile (pattern blended over the checkerboard)
/// into an RGBA8 buffer that is `row_stride` pixels wide, starting at column `x0`.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn paint_preview_tile(
    pixels: &mut [u8],
    row_stride: u32,
    x0: u32,
    style: TrippyStyle,
    phase: f32,
    intensity: f32,
    size: u32,
) {
    let blend = intensity.clamp(0.0, 1.0);
    for y in 0..size {
        let v = y as f32 / size.max(1) as f32;
        for x in 0..size {
            let u = x as f32 / size.max(1) as f32;
            let generated = trippy_pixel(style, u, v, phase);
            let shade = checker_shade(x, y);
            let i = ((y * row_stride + x0 + x) * 4) as usize;
            for k in 0..3 {
                let base = shade;
                pixels[i + k] = (base + (f32::from(generated[k]) - base) * blend)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[allow(clippy::manual_is_multiple_of)]
fn checker_shade(x: u32, y: u32) -> f32 {
    if ((x / 16) + (y / 16)) % 2 == 0 {
        42.0
    } else {
        56.0
    }
}

#[derive(Debug, Clone)]
struct TrippyTarget {
    material_entry: String,
    texture_entry: String,
    weapon: bool,
    placeholder: bool,
}

/// Build a procedural trippy hero skin addon. Textures are discovered from
/// hero-namespaced VMATs under `models/heroes*`; weapon materials are included
/// when `options.include_weapons` is true.
#[allow(
    clippy::too_many_lines,
    clippy::single_match_else,
    clippy::manual_let_else
)]
pub fn trippy_skin_to_addon(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    codename: &str,
    options: &TrippySkinOptions,
    out: impl AsRef<Path>,
) -> Result<TrippySkinReport> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let targets = discover_targets(&vpks, codename, options)?;
    anyhow::ensure!(
        !targets.is_empty(),
        "no hero-specific body/weapon color textures found for {codename:?}"
    );

    let donor = find_donor_texture(&vpks, &targets)
        .context("no real hero color texture was available to use as an encode container")?;
    let mut packed: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut report = TrippySkinReport {
        codename: codename.to_string(),
        ..Default::default()
    };

    for target in &targets {
        let source = if target.placeholder {
            donor.as_slice()
        } else {
            match read_entry(&vpks, &target.texture_entry) {
                Some(bytes) => {
                    packed.insert(
                        target.texture_entry.clone(),
                        repaint_texture(&bytes, target, options)
                            .with_context(|| format!("painting {}", target.texture_entry))?,
                    );
                    if target.weapon {
                        report.weapon_textures += 1;
                    } else {
                        report.body_textures += 1;
                    }
                    continue;
                }
                None => {
                    report.skipped_unreadable += 1;
                    continue;
                }
            }
        };

        packed.insert(
            target.texture_entry.clone(),
            repaint_texture(source, target, options)
                .with_context(|| format!("painting {}", target.texture_entry))?,
        );
        if target.placeholder {
            report.texture_placeholders_promoted += 1;
        }
        if target.weapon {
            report.weapon_textures += 1;
        } else {
            report.body_textures += 1;
        }
    }

    for material in targets
        .iter()
        .map(|t| t.material_entry.as_str())
        .collect::<BTreeSet<_>>()
    {
        let Some(bytes) = read_entry(&vpks, material) else {
            report.skipped_unreadable += 1;
            continue;
        };
        let value = match morphic::decode_kv3_resource(&bytes) {
            Ok(value) => value,
            Err(_) => {
                report.skipped_unpatchable_materials += 1;
                continue;
            }
        };
        let weapon = is_weapon_path(material);
        let edits = trippy_material_edits(&value, options, weapon);
        if edits.is_empty() {
            continue;
        }
        match morphic::patch_kv3_resource_doubles(&bytes, &edits) {
            Ok(new_bytes) => {
                packed.insert(material.to_string(), new_bytes);
                report.materials_scrolled += 1;
            }
            Err(_) => report.skipped_unpatchable_materials += 1,
        }
    }

    let readme = format!(
        "Trippy hero skin\n\
         ================\n\
         Hero: {codename}\n\
         Style: {}\n\
         Intensity: {:.2}\n\
         Scroll: {:.2}\n\
         Targets: body={}, weapons={}\n\
         Generated by vpkmerge. Only hero-namespaced model materials/textures were touched;\n\
         shared engine textures were skipped.\n",
        options.style.as_str(),
        options.intensity,
        options.scroll,
        options.include_body,
        options.include_weapons,
    );
    packed.insert("README.txt".to_string(), readme.into_bytes());

    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    crate::pack(&refs, out.as_ref())
        .with_context(|| format!("packing trippy skin into {}", out.as_ref().display()))?;
    report.total_entries = refs.len();
    Ok(report)
}

/// Build a trippy animated ability/weapon VFX addon for a pinned hero recipe.
///
/// This is the ability-side companion to [`trippy_skin_to_addon`]: color-bearing
/// particle params get a style-sampled hue, explicit ability textures receive the
/// same procedural paint patterns as skins, ability VMAT tint/scroll params are
/// patched when present, and high-visibility particles can receive runtime timing
/// / color-loop / color-cycle animation.
#[allow(clippy::too_many_lines)]
pub fn trippy_ability_vfx_to_addon(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    codename: &str,
    options: &TrippyAbilityOptions,
    out: impl AsRef<Path>,
) -> Result<TrippyAbilityReport> {
    let recipe = recipe_for(codename).with_context(|| {
        format!(
            "no built-in ability-VFX recipe for hero codename {codename:?} (pinned: {})",
            crate::pinned_hero_codenames().join(", ")
        )
    })?;
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let mut packed: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut missing = Vec::new();
    let mut report = TrippyAbilityReport {
        codename: recipe.codename.clone(),
        ..Default::default()
    };

    let particle_entries: Vec<String> = list_entries(&vpks, &recipe.particle_prefixes, ".vpcf_c")
        .into_iter()
        .filter(|entry| vfx_target_allowed(entry, options))
        .collect();
    report.particles_total = particle_entries.len();
    let cycle_tuning = trippy_cycle_tuning(options);

    for entry in &particle_entries {
        let bytes = read_entry(&vpks, entry)
            .with_context(|| format!("reading particle {entry} (listed but unreadable)"))?;
        let mut working = bytes;
        let mut changed = false;
        let mut had_color = false;

        if options.intensity > 0.0 {
            let recolor = trippy_recolor_for(&recipe.codename, entry, 0.0, options);
            match recolor_particle_bytes(&working, recolor) {
                Ok(Some(new_bytes)) => {
                    working = new_bytes;
                    changed = true;
                    had_color = true;
                    report.particles_recolored += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    report.particles_unpatchable += 1;
                    eprintln!("  note: skipping particle color for {entry}: {e:#}");
                    continue;
                }
            }
        }

        let mut animated_this = false;
        if options.animation_intensity > 0.0 && is_prism_animation_target(entry) {
            match animate_particle_timing_bytes(&working, options.animation_intensity) {
                Ok(Some((new_bytes, stats))) => {
                    working = new_bytes;
                    changed = true;
                    animated_this = true;
                    report.texture_age_inputs += stats.age_inputs;
                    report.texture_offset_multipliers += stats.texture_offset_multipliers;
                    report.gradient_timing_edits += stats.gradient_timing_edits;
                }
                Ok(None) => {}
                Err(e) => eprintln!("  note: timing animation skipped for {entry}: {e:#}"),
            }

            if matches!(
                options.animation_style,
                PrismAnimationStyle::Loop | PrismAnimationStyle::Cycle
            ) {
                match loop_animate_particle_bytes(&working) {
                    Ok(Some(new_bytes)) => {
                        working = new_bytes;
                        changed = true;
                        animated_this = true;
                        report.color_gradient_loops += 1;
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("  note: color-gradient loop skipped for {entry}: {e:#}"),
                }
            }

            if options.animation_style == PrismAnimationStyle::Cycle {
                match insert_color_cycle_operator_with_tuning(&working, cycle_tuning) {
                    Ok(Some(new_bytes)) => {
                        working = new_bytes;
                        changed = true;
                        animated_this = true;
                        report.color_cycle_operators += 1;
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("  note: color-cycle insert skipped for {entry}: {e:#}"),
                }
            }
        }

        if animated_this {
            report.particles_animated += 1;
        }
        if changed {
            packed.insert(entry.clone(), working);
        } else if !had_color {
            report.particles_no_color += 1;
        }
    }

    for entry in &recipe.texture_entries {
        if !vfx_target_allowed(entry, options) {
            continue;
        }
        match read_entry(&vpks, entry) {
            Some(bytes) => match repaint_ability_texture(&bytes, options) {
                Ok(new_bytes) => {
                    packed.insert(entry.clone(), new_bytes);
                    report.textures_painted += 1;
                }
                Err(e) => {
                    report.textures_skipped += 1;
                    eprintln!("  note: texture paint skipped for {entry}: {e:#}");
                }
            },
            None => missing.push(entry.clone()),
        }
    }

    for entry in &recipe.material_entries {
        if !vfx_target_allowed(entry, options) {
            continue;
        }
        let Some(bytes) = read_entry(&vpks, entry) else {
            missing.push(entry.clone());
            continue;
        };
        let mut working = bytes;
        let mut changed = false;

        if options.intensity > 0.0 {
            let recolor = trippy_recolor_for(&recipe.codename, entry, 0.33, options);
            match recolor_material_color_bytes(&working, recolor) {
                Ok(Some(new_bytes)) => {
                    working = new_bytes;
                    changed = true;
                    report.materials_recolored += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    report.materials_unpatchable += 1;
                    eprintln!("  note: material tint skipped for {entry}: {e:#}");
                }
            }
        }

        if options.animation_intensity > 0.0 {
            match morphic::decode_kv3_resource(&working) {
                Ok(value) => {
                    let edits = trippy_material_edits(
                        &value,
                        &skin_options_from_ability(options),
                        is_weapon_path(entry),
                    );
                    if !edits.is_empty() {
                        match morphic::patch_kv3_resource_doubles(&working, &edits) {
                            Ok(new_bytes) => {
                                working = new_bytes;
                                changed = true;
                                report.materials_scrolled += 1;
                            }
                            Err(e) => {
                                report.materials_unpatchable += 1;
                                eprintln!("  note: material scroll skipped for {entry}: {e:#}");
                            }
                        }
                    }
                }
                Err(e) => eprintln!("  note: material scroll decode skipped for {entry}: {e:#}"),
            }
        }

        if changed {
            packed.insert(entry.clone(), working);
        }
    }

    for entry in &recipe.model_entries {
        if !vfx_target_allowed(entry, options) || options.intensity <= 0.0 {
            continue;
        }
        match read_entry(&vpks, entry) {
            Some(bytes) => {
                let recolor = trippy_recolor_for(&recipe.codename, entry, 0.66, options);
                let (new_bytes, stats) = recolor_model_vertex_colors(&bytes, recolor)
                    .with_context(|| format!("trippy-recoloring model {entry}"))?;
                if stats.buffers_recolored == 0 {
                    continue;
                }
                packed.insert(entry.clone(), new_bytes);
                report.models_recolored += 1;
                report.model_vertices += stats.vertices;
            }
            None => missing.push(entry.clone()),
        }
    }

    report.missing_entries = missing.len();
    if !missing.is_empty() {
        anyhow::bail!(
            "{} recipe entr{} not found in the given VPK(s) (recipe drift?): {}",
            missing.len(),
            if missing.len() == 1 { "y" } else { "ies" },
            missing.join(", ")
        );
    }

    let readme = format!(
        "Trippy ability VFX\n\
         ==================\n\
         Hero: {}\n\
         Style: {}\n\
         Intensity: {:.2}\n\
         Animation: {:?} @ {:.2}\n\
         Targets: abilities={}, weapons={}\n\
         Generated by vpkmerge. Ability/weapon particle entries come from the pinned hero VFX recipe;\n\
         explicit ability textures are painted with procedural trippy patterns.\n",
        report.codename,
        options.style.as_str(),
        options.intensity,
        options.animation_style,
        options.animation_intensity,
        options.include_abilities,
        options.include_weapons,
    );
    packed.insert("README.txt".to_string(), readme.into_bytes());

    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    crate::pack(&refs, out.as_ref())
        .with_context(|| format!("packing trippy ability VFX into {}", out.as_ref().display()))?;
    report.total_entries = refs.len();
    Ok(report)
}

#[allow(clippy::unnecessary_wraps)]
fn discover_targets(
    vpks: &[valve_pak::VPK],
    codename: &str,
    options: &TrippySkinOptions,
) -> Result<Vec<TrippyTarget>> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut materials = BTreeSet::new();
    for vpk in vpks {
        for path in vpk.file_paths() {
            if path.ends_with(".vmat_c")
                && path.starts_with("models/heroes")
                && hero_path_match(path, codename)
            {
                materials.insert(path.clone());
            }
        }
    }

    for material_entry in materials {
        let weapon = is_weapon_path(&material_entry);
        if (weapon && !options.include_weapons) || (!weapon && !options.include_body) {
            continue;
        }
        let Some(bytes) = read_entry(vpks, &material_entry) else {
            continue;
        };
        let Ok(mat) = morphic::material::parse(&bytes) else {
            continue;
        };
        let Some(texture) = base_color_texture(&mat) else {
            continue;
        };
        let texture_entry = compiled_resource_path(texture);
        if !hero_path_match(&texture_entry, codename) || !texture_entry.starts_with("models/heroes")
        {
            continue;
        }
        let placeholder = read_entry(vpks, &texture_entry)
            .and_then(|b| morphic::inspect(&b).ok())
            .is_some_and(|info| u32::from(info.width) <= 8 || u32::from(info.height) <= 8);
        if seen.insert((material_entry.clone(), texture_entry.clone())) {
            out.push(TrippyTarget {
                material_entry,
                texture_entry,
                weapon,
                placeholder,
            });
        }
    }
    Ok(out)
}

fn base_color_texture(mat: &morphic::material::Material) -> Option<&str> {
    mat.texture("g_tColor")
        .or_else(|| mat.texture("g_tColorA"))
        .or_else(|| mat.texture("g_tBaseColor"))
        .or_else(|| mat.texture("g_tAlbedo"))
}

fn find_donor_texture(vpks: &[valve_pak::VPK], targets: &[TrippyTarget]) -> Option<Vec<u8>> {
    for target in targets {
        if target.placeholder {
            continue;
        }
        let Some(bytes) = read_entry(vpks, &target.texture_entry) else {
            continue;
        };
        let Ok(info) = morphic::inspect(&bytes) else {
            continue;
        };
        if u32::from(info.width) > 8 && u32::from(info.height) > 8 {
            return Some(bytes);
        }
    }
    None
}

fn repaint_texture(
    bytes: &[u8],
    target: &TrippyTarget,
    options: &TrippySkinOptions,
) -> Result<Vec<u8>> {
    let mut image = morphic::decode(bytes).context("decoding trippy texture")?;
    let style = if target.weapon && target.placeholder {
        TrippyStyle::Gradient
    } else {
        options.style
    };
    paint_image(&mut image, style, options.phase, options.intensity)?;
    morphic::replace_mip_chain(bytes, &image).context("re-encoding trippy texture")
}

fn repaint_ability_texture(bytes: &[u8], options: &TrippyAbilityOptions) -> Result<Vec<u8>> {
    let mut image = morphic::decode(bytes).context("decoding trippy ability texture")?;
    paint_image_keep_value(&mut image, options.style, options.phase, options.intensity)?;
    morphic::replace_mip_chain(bytes, &image).context("re-encoding trippy ability texture")
}

fn skin_options_from_ability(options: &TrippyAbilityOptions) -> TrippySkinOptions {
    TrippySkinOptions {
        style: options.style,
        intensity: options.intensity,
        phase: options.phase,
        scroll: options.animation_intensity,
        include_body: true,
        include_weapons: true,
    }
}

fn vfx_target_allowed(entry: &str, options: &TrippyAbilityOptions) -> bool {
    let weapon = entry.contains("/weapon_fx/") || is_weapon_path(entry);
    if weapon {
        options.include_weapons
    } else {
        options.include_abilities
    }
}

fn gradient_stops(triples: &[(f64, f64, f64)]) -> Option<PrismGradient> {
    let stops: Vec<GradientStop> = triples
        .iter()
        .map(|&(position, hue, saturation)| GradientStop {
            position,
            hue,
            saturation,
        })
        .collect();
    PrismGradient::from_stops(&stops)
}

/// The color ramp `trippy-vfx` particle cycles sample for a style, so the
/// animated particles stay on the skin's theme. `None` = full rainbow.
fn trippy_style_gradient(style: TrippyStyle) -> Option<PrismGradient> {
    match style {
        TrippyStyle::Thermal | TrippyStyle::Lava => PrismGradient::preset("fire"),
        TrippyStyle::Liquid => PrismGradient::preset("ocean"),
        TrippyStyle::Moire | TrippyStyle::Glitch => PrismGradient::preset("neon"),
        TrippyStyle::Kaleido => PrismGradient::preset("sunset"),
        TrippyStyle::Holo => {
            gradient_stops(&[(0.0, 190.0, 0.45), (0.48, 305.0, 0.62), (1.0, 55.0, 0.38)])
        }
        TrippyStyle::Camo => {
            gradient_stops(&[(0.0, 95.0, 0.55), (0.5, 130.0, 0.60), (1.0, 60.0, 0.45)])
        }
        TrippyStyle::Carbon => gradient_stops(&[(0.0, 210.0, 0.10), (1.0, 225.0, 0.05)]),
        TrippyStyle::Galaxy => {
            gradient_stops(&[(0.0, 225.0, 0.95), (0.5, 280.0, 1.0), (1.0, 320.0, 0.90)])
        }
        TrippyStyle::Halftone => {
            gradient_stops(&[(0.0, 0.0, 1.0), (0.5, 55.0, 1.0), (1.0, 195.0, 1.0)])
        }
        TrippyStyle::Vaporwave => {
            gradient_stops(&[(0.0, 280.0, 1.0), (0.5, 325.0, 0.95), (1.0, 185.0, 0.95)])
        }
        TrippyStyle::Confetti | TrippyStyle::Gradient => None,
    }
}

fn trippy_cycle_tuning(options: &TrippyAbilityOptions) -> PrismTuning {
    let gradient = trippy_style_gradient(options.style);
    let intensity = f64::from(options.intensity.max(0.0));
    PrismTuning {
        hue_offset: f64::from(options.phase) * 360.0,
        saturation: (0.85 + 0.25 * intensity).clamp(0.25, 1.5),
        brightness: (0.90 + 0.18 * intensity).clamp(0.25, 1.6),
        animation_intensity: options.animation_intensity,
        animation_style: options.animation_style,
        gradient,
    }
}

fn trippy_recolor_for(
    codename: &str,
    entry: &str,
    offset: f32,
    options: &TrippyAbilityOptions,
) -> Recolor {
    let a = hash_str01(&format!("{codename}:{entry}:a"));
    let b = hash_str01(&format!("{entry}:{codename}:b"));
    let rgb = trippy_pixel(
        options.style,
        (a + offset).fract(),
        (b + offset * 0.618).fract(),
        options.phase,
    );
    let (hue, sat, val) = rgb_to_hsv(rgb);
    let intensity = f64::from(options.intensity.max(0.0));
    Recolor::new(
        hue,
        (0.72 + sat * 0.48 * intensity).clamp(0.25, 2.0),
        (0.82 + (val - 0.5) * 0.55 * intensity).clamp(0.25, 1.8),
    )
}

fn trippy_material_edits(
    value: &Value,
    options: &TrippySkinOptions,
    weapon: bool,
) -> Vec<(Vec<Seg>, f64)> {
    let scroll = [0.08 * options.scroll, 0.05 * options.scroll];
    let mut edits = Vec::new();
    edits.extend(scroll_xy_edits(value, "g_vAlbedoScrollSpeed1", scroll));
    edits.extend(scroll_xy_edits(value, "g_vSelfIllumScrollSpeed1", scroll));
    if matches!(options.style, TrippyStyle::Holo) {
        edits.extend(scroll_xy_edits(
            value,
            "g_vNormalAndRoughnessScrollSpeed1",
            [0.12 * options.scroll, 0.07 * options.scroll],
        ));
        edits.extend(float_param_edit(
            value,
            "g_flSelfIllumScale1",
            5.5 * f64::from(options.intensity.max(0.0)),
        ));
        edits.extend(float_param_edit(
            value,
            "g_flSelfIllumFresnelMaskExponent",
            2.5,
        ));
        edits.extend(vcomp_edits(
            value,
            "g_vSelfIllumFresnelMaskTint1",
            &[(0, 1.0), (1, 1.0), (2, 1.0)],
        ));
    }
    if matches!(options.style, TrippyStyle::Lava) {
        edits.extend(float_param_edit(
            value,
            "g_flSelfIllumScale1",
            4.0 * f64::from(options.intensity.max(0.0)),
        ));
        edits.extend(float_param_edit(
            value,
            "g_flSelfIllumFresnelMaskExponent",
            3.0,
        ));
        edits.extend(vcomp_edits(
            value,
            "g_vSelfIllumFresnelMaskTint1",
            &[(0, 1.0), (1, 0.35), (2, 0.08)],
        ));
    }
    if weapon {
        edits.extend(vcomp_edits(
            value,
            "TextureColor1",
            &[(0, 1.0), (1, 1.0), (2, 1.0)],
        ));
    }
    edits
}

/// Paint the trippy pattern while preserving each pixel's original brightness
/// (max RGB channel): the pattern contributes hue and saturation only.
///
/// Ability/particle textures render additively, so a pixel's brightness IS its
/// silhouette: flares, symbols, and falloff strips keep their art inside black
/// margins. The plain [`paint_image`] (full-canvas blend, right for body-skin
/// albedos whose UV islands hide the margins) lights those margins up and the
/// whole quad renders as a square.
#[allow(
    clippy::many_single_char_names,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn paint_image_keep_value(
    image: &mut Image,
    style: TrippyStyle,
    phase: f32,
    intensity: f32,
) -> Result<()> {
    let (w, h) = (image.width, image.height);
    let ImageData::Rgba8(px) = &mut image.data else {
        anyhow::bail!("trippy skin supports LDR (8-bit) textures only");
    };
    let blend = intensity.clamp(0.0, 1.0);
    for y in 0..h {
        let v = y as f32 / h.max(1) as f32;
        for x in 0..w {
            let u = x as f32 / w.max(1) as f32;
            let generated = trippy_pixel(style, u, v, phase);
            let i = ((y * w + x) * 4) as usize;
            let original_max = f32::from(px[i].max(px[i + 1]).max(px[i + 2]));
            let generated_max =
                f32::from(generated[0].max(generated[1]).max(generated[2])).max(1.0);
            // Rescale the pattern so its brightest channel equals the original
            // pixel's brightest channel, then blend as usual. Black stays black.
            let scale = original_max / generated_max;
            for k in 0..3 {
                let original = f32::from(px[i + k]);
                let shaped = f32::from(generated[k]) * scale;
                px[i + k] = (original + (shaped - original) * blend)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok(())
}

#[allow(
    clippy::many_single_char_names,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn paint_image(image: &mut Image, style: TrippyStyle, phase: f32, intensity: f32) -> Result<()> {
    let (w, h) = (image.width, image.height);
    let ImageData::Rgba8(px) = &mut image.data else {
        anyhow::bail!("trippy skin supports LDR (8-bit) textures only");
    };
    let blend = intensity.clamp(0.0, 1.0);
    for y in 0..h {
        let v = y as f32 / h.max(1) as f32;
        for x in 0..w {
            let u = x as f32 / w.max(1) as f32;
            let generated = trippy_pixel(style, u, v, phase);
            let i = ((y * w + x) * 4) as usize;
            for k in 0..3 {
                let original = f32::from(px[i + k]);
                px[i + k] = (original + (f32::from(generated[k]) - original) * blend)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
    }
    Ok(())
}

fn trippy_pixel(style: TrippyStyle, u: f32, v: f32, phase: f32) -> [u8; 3] {
    match style {
        TrippyStyle::Confetti => confetti(u, v, phase),
        TrippyStyle::Liquid => liquid(u, v, phase),
        TrippyStyle::Moire => moire(u, v, phase),
        TrippyStyle::Kaleido => kaleido(u, v, phase),
        TrippyStyle::Holo => holo(u, v, phase),
        TrippyStyle::Glitch => glitch(u, v, phase),
        TrippyStyle::Thermal => thermal(u, v, phase),
        TrippyStyle::Gradient => gradient(u, v, phase),
        TrippyStyle::Camo => camo(u, v, phase),
        TrippyStyle::Carbon => carbon(u, v, phase),
        TrippyStyle::Galaxy => galaxy(u, v, phase),
        TrippyStyle::Halftone => halftone(u, v, phase),
        TrippyStyle::Lava => lava(u, v, phase),
        TrippyStyle::Vaporwave => vaporwave(u, v, phase),
    }
}

#[allow(clippy::cast_sign_loss, clippy::cast_precision_loss)]
fn hash2(i: i64, j: i64) -> f32 {
    let mut h = (i
        .wrapping_mul(374_761_393)
        .wrapping_add(j.wrapping_mul(668_265_263))) as u64;
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xff_ffff) as f32 / 16_777_216.0
}

#[allow(clippy::cast_precision_loss)]
fn hash_str01(s: &str) -> f32 {
    let mut h = 14_695_981_039_346_656_037_u64;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(1_099_511_628_211);
    }
    ((h >> 40) & 0xff_ffff) as f32 / 16_777_216.0
}

#[allow(
    clippy::many_single_char_names,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]
fn vnoise(x: f32, y: f32, p: i64) -> f32 {
    let gx = x * p as f32;
    let gy = y * p as f32;
    let x0 = gx.floor() as i64;
    let y0 = gy.floor() as i64;
    let fx = gx - x0 as f32;
    let fy = gy - y0 as f32;
    let wrap = |a: i64| ((a % p) + p) % p;
    let s = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let ux = s(fx);
    let uy = s(fy);
    let v00 = hash2(wrap(x0), wrap(y0));
    let v10 = hash2(wrap(x0 + 1), wrap(y0));
    let v01 = hash2(wrap(x0), wrap(y0 + 1));
    let v11 = hash2(wrap(x0 + 1), wrap(y0 + 1));
    let a = v00 + (v10 - v00) * ux;
    let b = v01 + (v11 - v01) * ux;
    a + (b - a) * uy
}

fn fbm(x: f32, y: f32, p0: i64, oct: u32) -> f32 {
    let (mut sum, mut amp, mut p, mut norm) = (0.0, 0.5, p0, 0.0);
    for _ in 0..oct {
        sum += amp * vnoise(x, y, p);
        norm += amp;
        amp *= 0.5;
        p *= 2;
    }
    sum / norm
}

#[allow(clippy::many_single_char_names, clippy::cast_possible_truncation)]
fn hsv2rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
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

fn rgb_to_hsv(rgb: [u8; 3]) -> (f64, f64, f64) {
    let r = f64::from(rgb[0]) / 255.0;
    let g = f64::from(rgb[1]) / 255.0;
    let b = f64::from(rgb[2]) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let hue = if d <= f64::EPSILON {
        0.0
    } else if (max - r).abs() <= f64::EPSILON {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if (max - g).abs() <= f64::EPSILON {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };
    let sat = if max <= f64::EPSILON { 0.0 } else { d / max };
    (hue, sat, max)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pack_rgb(rgb: [f32; 3]) -> [u8; 3] {
    [
        (rgb[0] * 255.0).clamp(0.0, 255.0) as u8,
        (rgb[1] * 255.0).clamp(0.0, 255.0) as u8,
        (rgb[2] * 255.0).clamp(0.0, 255.0) as u8,
    ]
}

fn confetti(u: f32, v: f32, phase: f32) -> [u8; 3] {
    let wu = u + 0.12 * fbm(u, v, 6, 4);
    let wv = v + 0.12 * fbm(u + 5.2, v + 1.3, 6, 4);
    let base = fbm(wu, wv, 5, 5);
    let mid = fbm(wu + 2.7, wv + 8.1, 18, 4);
    let fine = fbm(wu + 9.3, wv + 3.4, 40, 3);
    let huef = (base + mid * 1.6 + fine * 2.2).fract();
    let bands = 14.0_f32;
    let hue_q = (huef * bands).floor() / bands + 0.5 / bands;
    let hue = hue_q * 360.0 + phase * 360.0;
    let sat = (0.82 + 0.18 * fine).clamp(0.0, 1.0);
    let val = (0.55 + 0.42 * fbm(wu + 1.1, wv + 6.6, 24, 3)).clamp(0.0, 1.0);
    pack_rgb(hsv2rgb(hue, sat, val))
}

fn liquid(u: f32, v: f32, phase: f32) -> [u8; 3] {
    let warp = 0.5;
    let q0 = fbm(u, v, 4, 5);
    let q1 = fbm(u + 3.1, v + 6.2, 4, 5);
    let r0 = fbm(u + warp * q0 + 1.7, v + warp * q1 + 9.2, 5, 5);
    let r1 = fbm(u + warp * q0 + 8.3, v + warp * q1 + 2.8, 5, 5);
    let f = fbm(u + warp * r0, v + warp * r1, 6, 5);
    let veins = ((f * 11.0 + r0 * 3.0) * TAU).sin() * 0.5 + 0.5;
    let hue = (f * 2.4 + r0 * 0.9 + phase).fract() * 360.0;
    let sat = (0.80 + 0.20 * q1).clamp(0.0, 1.0);
    let val = (0.30 + 0.68 * veins).clamp(0.0, 1.0);
    pack_rgb(hsv2rgb(hue, sat, val))
}

fn moire(u: f32, v: f32, phase: f32) -> [u8; 3] {
    let g = |a: f32, b: f32, ph: f32| (TAU * (a * u + b * v) + ph).sin();
    let p = phase * TAU;
    let m1 = g(9.0, 4.0, p) + g(10.0, 5.0, -p);
    let m2 = g(4.0, 9.0, p * 1.3) + g(5.0, 8.0, -p);
    let field = (m1 * m2) * 0.25 + 0.5;
    let hue = (field * 1.6 + 0.12 * m1 + phase).fract() * 360.0;
    let val = (0.42 + 0.5 * ((field * 6.0).sin() * 0.5 + 0.5)).clamp(0.0, 1.0);
    pack_rgb(hsv2rgb(hue, 0.95, val))
}

fn kaleido(u: f32, v: f32, phase: f32) -> [u8; 3] {
    let g = |a: f32, b: f32| (TAU * (a * u + b * v)).cos();
    let k = 6.0;
    let mandala = (g(k, 0.0) + g(0.0, k) + 0.7 * g(k, k) + 0.7 * g(k, -k)) * 0.25;
    let radial = ((TAU * u).cos() + (TAU * v).cos()) * 0.5;
    let field = mandala * 0.5 + 0.5;
    let hue = (field + 0.35 * radial + phase).fract() * 360.0;
    let sat = (0.85 + 0.15 * radial).clamp(0.0, 1.0);
    let val = (0.45 + 0.5 * (mandala * 0.5 + 0.5)).clamp(0.0, 1.0);
    pack_rgb(hsv2rgb(hue, sat, val))
}

fn holo(u: f32, v: f32, phase: f32) -> [u8; 3] {
    let warp = 0.06 * fbm(u + 1.3, v + 4.1, 6, 3);
    let sweep = (u * 2.0 + v + warp).fract();
    let foil = ((u * 48.0 - v * 48.0) * TAU).sin() * 0.5 + 0.5;
    let hue = (sweep + 0.04 * foil + phase).fract() * 360.0;
    let sat = (0.45 + 0.30 * foil).clamp(0.0, 1.0);
    let val = (0.72 + 0.28 * foil).clamp(0.0, 1.0);
    pack_rgb(hsv2rgb(hue, sat, val))
}

#[allow(clippy::cast_possible_truncation)]
fn glitch(u: f32, v: f32, phase: f32) -> [u8; 3] {
    let band = (v * 28.0).floor() as i64;
    let r = hash2(band, 7);
    let shift = if r > 0.62 { (r - 0.62) * 0.9 } else { 0.0 };
    let warp = 0.15 * fbm(u, v, 10, 3);
    let hue_at = |off: f32| (((u + shift + warp + off) * 2.0 + phase).fract()) * 360.0;
    let rr = hsv2rgb(hue_at(0.0), 0.92, 0.9)[0];
    let gg = hsv2rgb(hue_at(0.010), 0.92, 0.9)[1];
    let bb = hsv2rgb(hue_at(0.020), 0.92, 0.9)[2];
    let scan = 0.55 + 0.45 * ((v * 512.0 * TAU).sin().abs());
    pack_rgb([rr * scan, gg * scan, bb * scan])
}

fn thermal_color(t: f32) -> [f32; 3] {
    const STOPS: &[(f32, [f32; 3])] = &[
        (0.00, [0.0, 0.0, 0.05]),
        (0.22, [0.20, 0.0, 0.45]),
        (0.42, [0.65, 0.0, 0.55]),
        (0.60, [0.95, 0.10, 0.10]),
        (0.76, [1.0, 0.55, 0.0]),
        (0.90, [1.0, 0.95, 0.25]),
        (1.00, [1.0, 1.0, 1.0]),
    ];
    let t = t.clamp(0.0, 1.0);
    for w in STOPS.windows(2) {
        let (a, ca) = w[0];
        let (b, cb) = w[1];
        if t <= b {
            let k = ((t - a) / (b - a)).clamp(0.0, 1.0);
            return [
                ca[0] + (cb[0] - ca[0]) * k,
                ca[1] + (cb[1] - ca[1]) * k,
                ca[2] + (cb[2] - ca[2]) * k,
            ];
        }
    }
    [1.0, 1.0, 1.0]
}

fn thermal(u: f32, v: f32, phase: f32) -> [u8; 3] {
    let warp = 0.25 * fbm(u + 1.7, v + 4.2, 6, 4);
    let heat =
        (fbm(u + warp, v + warp, 6, 5) * 1.5 + 0.25 * fbm(u + 9.0, v + 2.0, 24, 3) + phase).fract();
    pack_rgb(thermal_color(heat))
}

fn gradient(u: f32, v: f32, phase: f32) -> [u8; 3] {
    let hue = ((u * 2.0 + v).fract() + phase).fract() * 360.0;
    pack_rgb(hsv2rgb(hue, 0.95, 0.92))
}

/// Tileable Voronoi over a `p`-cell grid: distance to the nearest and second
/// nearest feature point, plus a per-cell hash id. Cell ids wrap modulo `p`
/// (same trick as [`vnoise`]) so the field is seamless.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
fn voronoi(u: f32, v: f32, p: i64) -> (f32, f32, f32) {
    let gx = u * p as f32;
    let gy = v * p as f32;
    let x0 = gx.floor() as i64;
    let y0 = gy.floor() as i64;
    let wrap = |a: i64| ((a % p) + p) % p;
    let mut f1 = f32::MAX;
    let mut f2 = f32::MAX;
    let mut id = 0.0;
    for dj in -1..=1 {
        for di in -1..=1 {
            let ci = x0 + di;
            let cj = y0 + dj;
            let (wi, wj) = (wrap(ci), wrap(cj));
            let px = ci as f32 + hash2(wi + 101, wj + 17);
            let py = cj as f32 + hash2(wi + 43, wj + 59);
            let d = (gx - px).hypot(gy - py);
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = hash2(wi + 7, wj + 91);
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2, id)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn camo(u: f32, v: f32, phase: f32) -> [u8; 3] {
    // Woodland palette, darkest first so the overlay blobs can reuse slot 0.
    const PALETTE: [[f32; 3]; 4] = [
        [0.11, 0.14, 0.07],
        [0.25, 0.29, 0.13],
        [0.40, 0.34, 0.20],
        [0.55, 0.50, 0.33],
    ];
    let drift = phase * 0.5;
    let n1 = fbm(u + drift, v + 1.3, 4, 4);
    let n2 = fbm(u + 7.3, v + 2.9 + drift, 4, 4);
    // Stretch contrast so the quantized blobs use the whole palette.
    let t = (n1 * 0.6 + n2 * 0.4 - 0.5) * 2.6 + 0.5;
    let mut c = PALETTE[(t.clamp(0.0, 0.999) * 4.0) as usize];
    // Classic woodland: dark disruption blobs punch through every layer.
    if fbm(u + 3.1 - drift, v + 8.8, 5, 4) > 0.62 {
        c = PALETTE[0];
    }
    let speck = (fbm(u + 5.5, v + 0.7, 64, 2) - 0.5) * 0.07;
    pack_rgb([c[0] + speck, c[1] + speck, c[2] + speck])
}

#[allow(clippy::cast_possible_truncation, clippy::many_single_char_names)]
fn carbon(u: f32, v: f32, phase: f32) -> [u8; 3] {
    let cells = 22.0;
    let x = u * cells;
    let y = (v + phase * 0.08) * cells;
    let fx = x - x.floor();
    let fy = y - y.floor();
    let horizontal = (x.floor() as i64 + y.floor() as i64).rem_euclid(2) == 0;
    let across = if horizontal { fy } else { fx };
    let along = if horizontal { fx } else { fy };
    // Cylindrical tow shading: brightest along the center of each bundle.
    let bulge = (TAU * (across - 0.5) * 0.5).cos();
    // Fine fiber striations running along the tow.
    let fiber = ((along * 7.0 + across * 1.5) * TAU * 6.0).sin() * 0.5 + 0.5;
    // A diagonal sheen band sweeps with phase so the weave glints as it scrolls.
    let sheen = ((u + v + phase) * TAU).sin().max(0.0).powi(3);
    let g = 0.035 + bulge * (0.16 * bulge + 0.05 * fiber + 0.18 * sheen);
    pack_rgb([g * 0.95, g, g * 1.08])
}

fn galaxy(u: f32, v: f32, phase: f32) -> [u8; 3] {
    let drift = phase * 0.35;
    let q = fbm(u + drift, v + 1.3, 4, 5);
    let r = fbm(u + 5.8, v - drift + 7.2, 4, 5);
    let neb = fbm(u + 0.55 * q, v + 0.55 * r, 5, 5);
    let dust = fbm(u + 9.4 - drift, v + 3.7, 10, 4);
    let hue = 225.0 + 95.0 * neb + 18.0 * q;
    let sat = (0.78 + 0.22 * r - 0.35 * (dust - 0.5).max(0.0)).clamp(0.0, 1.0);
    let val = (0.04 + 0.62 * neb * neb + 0.16 * (dust - 0.62).max(0.0)).clamp(0.0, 1.0);
    let mut rgb = hsv2rgb(hue, sat, val);

    // Sparse star field: roughly one star per five cells, twinkling with phase.
    let (f1, _, id) = voronoi(u, v, 40);
    if id > 0.80 {
        let twinkle = 0.7 + 0.3 * ((phase * 2.0 + id * 23.0) * TAU).sin();
        let star = (1.0 - f1 * 2.8).clamp(0.0, 1.0).powi(4) * twinkle;
        // The hottest hash ids run warm instead of blue-white.
        let tint = if id > 0.95 {
            [1.0, 0.85, 0.60]
        } else {
            [0.85, 0.92, 1.0]
        };
        for k in 0..3 {
            rgb[k] = (rgb[k] + star * tint[k]).min(1.0);
        }
    }
    pack_rgb(rgb)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn halftone(u: f32, v: f32, phase: f32) -> [u8; 3] {
    // (background, dot color) pairs per pop-art panel.
    const PANELS: [([f32; 3], [f32; 3]); 4] = [
        ([0.96, 0.93, 0.86], [0.82, 0.08, 0.10]),
        ([0.98, 0.84, 0.10], [0.82, 0.08, 0.10]),
        ([0.62, 0.86, 0.94], [0.05, 0.25, 0.65]),
        ([0.97, 0.78, 0.84], [0.08, 0.08, 0.10]),
    ];
    // Diagonal color panels crawl with phase. The 0.5 * v slope keeps the
    // band count integral in both axes so the pattern stays seamless.
    let zone = (((u + 0.5 * v + phase) * 4.0).floor().rem_euclid(4.0)) as usize;
    let (bg, dot) = PANELS[zone];

    // Ben-Day dot grid, rotated 45 degrees like printed comics.
    let cells = 30.0;
    let xr = (u + v) * cells;
    let yr = (u - v) * cells;
    let fx = xr - xr.floor() - 0.5;
    let fy = yr - yr.floor() - 0.5;
    let d = fx.hypot(fy) * 2.0;
    // Dot radius follows a smooth luminance field: the shading ramp.
    let radius = 0.18 + 0.62 * fbm(u + 1.9, v + 4.3, 4, 4);
    let t = ((radius - d) / 0.08).clamp(0.0, 1.0);
    pack_rgb([
        bg[0] + (dot[0] - bg[0]) * t,
        bg[1] + (dot[1] - bg[1]) * t,
        bg[2] + (dot[2] - bg[2]) * t,
    ])
}

fn lava(u: f32, v: f32, phase: f32) -> [u8; 3] {
    // Warp the crack field so the plates read as cooled crust, not a grid.
    let wu = u + 0.10 * fbm(u + 2.2, v + 7.1, 5, 4);
    let wv = v + 0.10 * fbm(u + 6.4, v + 1.8, 5, 4);
    let (f1, f2, id) = voronoi(wu, wv, 6);
    // Plate borders are where the two nearest feature points are equidistant.
    let crack = f2 - f1;
    // Each plate's cracks breathe on their own cycle.
    let pulse = ((phase + id) * TAU).sin() * 0.5 + 0.5;
    let width = 0.16 + 0.05 * pulse;
    let glow = (1.0 - crack / width).clamp(0.0, 1.0).powf(1.6);
    let crust_n = fbm(u + 3.3, v + 5.5, 12, 4);
    let crust = hsv2rgb(12.0 + 14.0 * crust_n, 0.45, 0.05 + 0.10 * crust_n);
    let hot = thermal_color(0.62 + 0.34 * pulse + 0.04 * crust_n);
    pack_rgb([
        crust[0] + (hot[0] - crust[0]) * glow,
        crust[1] + (hot[1] - crust[1]) * glow,
        crust[2] + (hot[2] - crust[2]) * glow,
    ])
}

fn vaporwave(u: f32, v: f32, phase: f32) -> [u8; 3] {
    // Mirror vertically so the texture tiles: band 1 is the "horizon",
    // band 0 the top/bottom edges.
    let band = 1.0 - (2.0 * v - 1.0).abs();
    // Sunset sky: purple at the edges through pink to orange at the horizon.
    let hue = if band < 0.6 {
        280.0 + (band / 0.6) * 50.0
    } else {
        330.0 + ((band - 0.6) / 0.4) * 55.0
    };
    let mut rgb = hsv2rgb(hue, 0.88, 0.18 + 0.62 * band);
    // Retro sun slats: dark horizontal cuts that thicken toward the horizon.
    if band > 0.55 {
        let slat = ((v * 36.0 + phase) * TAU).sin();
        if slat > 0.45 {
            let k = ((slat - 0.45) / 0.55) * 0.55 * ((band - 0.55) / 0.45);
            for c in &mut rgb {
                *c *= 1.0 - k;
            }
        }
    }
    // Neon grid rushing toward the horizon; fades out as it approaches.
    let lx = ((u * 14.0).fract() - 0.5).abs();
    let ly = (((1.0 - band) * 7.0 + phase * 2.0).fract() - 0.5).abs();
    let line = (1.0 - lx / 0.045).max(1.0 - ly / 0.05).clamp(0.0, 1.0) * (1.0 - band * 0.85);
    let glow = line.powf(1.5);
    let neon = hsv2rgb(187.0, 0.85, 1.0);
    for k in 0..3 {
        rgb[k] = (rgb[k] + neon[k] * glow).min(1.0);
    }
    pack_rgb(rgb)
}

fn vector_param_index(v: &Value, name: &str) -> Option<usize> {
    v.get("m_vectorParams")?
        .as_array()?
        .iter()
        .position(|p| p.get("m_name").and_then(Value::as_str) == Some(name))
}

fn float_param_edit(v: &Value, name: &str, val: f64) -> Vec<(Vec<Seg>, f64)> {
    let Some(i) = v
        .get("m_floatParams")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .position(|p| p.get("m_name").and_then(Value::as_str) == Some(name))
        })
    else {
        return Vec::new();
    };
    vec![(
        vec![
            Seg::Key("m_floatParams".to_string()),
            Seg::Index(i),
            Seg::Key("m_flValue".to_string()),
        ],
        val,
    )]
}

fn vcomp_edits(v: &Value, name: &str, comps: &[(usize, f64)]) -> Vec<(Vec<Seg>, f64)> {
    let Some(i) = vector_param_index(v, name) else {
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

fn scroll_xy_edits(v: &Value, name: &str, xy: [f64; 2]) -> Vec<(Vec<Seg>, f64)> {
    vcomp_edits(v, name, &[(0, xy[0]), (1, xy[1])])
}

fn compiled_resource_path(path: &str) -> String {
    if path.ends_with("_c") {
        path.to_string()
    } else {
        format!("{path}_c")
    }
}

pub(crate) fn hero_path_match(path: &str, codename: &str) -> bool {
    let p = path.to_ascii_lowercase();
    let c = codename.to_ascii_lowercase();
    p.contains(&format!("/{c}/"))
        || p.contains(&format!("/{c}_"))
        || p.contains(&format!("_{c}_"))
        || p.contains(&format!("/{c}."))
}

pub(crate) fn is_weapon_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [
        "weapon", "gun", "rifle", "pistol", "shotgun", "cannon", "launcher", "bow", "arrow",
        "blade", "sword", "knife", "staff", "wand", "mace", "club",
    ]
    .iter()
    .any(|needle| p.contains(needle))
}

pub(crate) fn open_vpks(vpk: &Path, base: Option<&Path>) -> Result<Vec<valve_pak::VPK>> {
    let mut vpks =
        vec![valve_pak::open(vpk).with_context(|| format!("opening {}", vpk.display()))?];
    if let Some(base) = base {
        vpks.push(valve_pak::open(base).with_context(|| format!("opening {}", base.display()))?);
    }
    Ok(vpks)
}

pub(crate) fn read_entry(vpks: &[valve_pak::VPK], entry: &str) -> Option<Vec<u8>> {
    for vpk in vpks {
        if let Ok(mut vf) = vpk.get_file(entry) {
            if let Ok(bytes) = vf.read_all() {
                return Some(bytes);
            }
        }
    }
    None
}

fn list_entries(vpks: &[valve_pak::VPK], prefixes: &[String], suffix: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    for vpk in vpks {
        for p in vpk.file_paths() {
            if p.ends_with(suffix) && prefixes.iter().any(|prefix| p.starts_with(prefix)) {
                seen.insert(p.clone());
            }
        }
    }
    seen.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_value_paint_preserves_silhouette() {
        // Additive ability sprites carry their silhouette in brightness: a
        // black margin must stay black at full paint intensity, and a bright
        // core must stay equally bright (only hue/sat may change). 4x4 image:
        // black border, one white pixel and one mid-grey pixel in the middle.
        let mut px = vec![0u8; 4 * 4 * 4];
        for p in px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        let white = (4 + 1) * 4; // pixel (1,1)
        px[white..white + 3].copy_from_slice(&[255, 255, 255]);
        let grey = (2 * 4 + 2) * 4; // pixel (2,2)
        px[grey..grey + 3].copy_from_slice(&[80, 80, 80]);
        let mut image = Image {
            width: 4,
            height: 4,
            data: ImageData::Rgba8(px),
        };
        for style in [TrippyStyle::Moire, TrippyStyle::Lava, TrippyStyle::Holo] {
            let mut painted = image.clone();
            paint_image_keep_value(&mut painted, style, 0.3, 1.0).unwrap();
            let ImageData::Rgba8(out) = &painted.data else {
                panic!("rgba8");
            };
            let ImageData::Rgba8(orig) = &image.data else {
                panic!("rgba8");
            };
            for (i, (o, n)) in orig.chunks_exact(4).zip(out.chunks_exact(4)).enumerate() {
                let omax = o[0].max(o[1]).max(o[2]);
                let nmax = n[0].max(n[1]).max(n[2]);
                assert!(
                    i32::from(omax).abs_diff(i32::from(nmax)) <= 1,
                    "{style:?} pixel {i}: brightness {omax} -> {nmax} (silhouette changed)"
                );
                assert_eq!(o[3], n[3], "{style:?} pixel {i}: alpha changed");
            }
        }
        // The full-canvas painter, by contrast, lights the black margin up
        // (that is the squared-off failure this guards against).
        paint_image(&mut image, TrippyStyle::Lava, 0.3, 1.0).unwrap();
        let ImageData::Rgba8(out) = &image.data else {
            panic!("rgba8");
        };
        assert!(
            out.chunks_exact(4)
                .any(|p| p[0].max(p[1]).max(p[2]) > 0 && p[..3] != [255, 255, 255]),
            "full-canvas paint should differ (sanity check)"
        );
    }

    #[test]
    fn style_aliases_parse() {
        assert_eq!(TrippyStyle::from_name("crt").unwrap(), TrippyStyle::Glitch);
        assert_eq!(
            TrippyStyle::from_name("holographic").unwrap(),
            TrippyStyle::Holo
        );
        assert!(TrippyStyle::from_name("plain").is_err());
    }

    #[test]
    fn all_style_names_round_trip() {
        for name in TRIPPY_STYLE_NAMES {
            let style = TrippyStyle::from_name(name).expect("listed style parses");
            assert_eq!(style.as_str(), *name);
        }
    }

    #[test]
    fn new_style_aliases_parse() {
        for (alias, style) in [
            ("woodland", TrippyStyle::Camo),
            ("carbonfiber", TrippyStyle::Carbon),
            ("nebula", TrippyStyle::Galaxy),
            ("popart", TrippyStyle::Halftone),
            ("magma", TrippyStyle::Lava),
            ("synthwave", TrippyStyle::Vaporwave),
        ] {
            assert_eq!(TrippyStyle::from_name(alias).unwrap(), style);
        }
    }

    #[test]
    fn every_style_renders_a_preview() {
        for name in TRIPPY_STYLE_NAMES {
            let style = TrippyStyle::from_name(name).unwrap();
            let frames = trippy_preview_frames(style, 0.1, 1.0, 1.0, 2, 16).expect("preview");
            for frame in frames {
                assert!(frame.starts_with(b"\x89PNG\r\n\x1a\n"), "{name}");
            }
        }
    }

    #[test]
    fn weapon_paths_are_detected() {
        assert!(is_weapon_path(
            "models/heroes_staging/chrono/chrono_gun/materials/chrono_gun.vmat_c"
        ));
        assert!(!is_weapon_path(
            "models/heroes_staging/chrono/materials/chrono_v2.vmat_c"
        ));
    }

    #[test]
    fn preview_frames_are_pngs() {
        let frames =
            trippy_preview_frames(TrippyStyle::Holo, 0.0, 1.0, 1.0, 3, 32).expect("preview");
        assert_eq!(frames.len(), 3);
        for frame in frames {
            assert!(frame.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
    }

    #[test]
    fn preview_sprite_is_one_png_strip() {
        let sprite =
            trippy_preview_sprite(TrippyStyle::Holo, 0.0, 1.0, 1.0, 4, 32).expect("sprite");
        assert!(sprite.starts_with(b"\x89PNG\r\n\x1a\n"));
        // PNG IHDR: width at byte 16, height at byte 20 (big-endian u32).
        let width = u32::from_be_bytes(sprite[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(sprite[20..24].try_into().unwrap());
        assert_eq!(width, 4 * 32);
        assert_eq!(height, 32);
    }

    #[test]
    fn single_frame_sprite_matches_frame() {
        // A 1-frame sprite paints through the same tile path as a single frame,
        // so the two encodes must be byte-identical.
        let sprite =
            trippy_preview_sprite(TrippyStyle::Liquid, 0.25, 1.0, 0.8, 1, 32).expect("sprite");
        let frames =
            trippy_preview_frames(TrippyStyle::Liquid, 0.25, 1.0, 0.8, 1, 32).expect("frames");
        assert_eq!(sprite, frames[0]);
    }
}
