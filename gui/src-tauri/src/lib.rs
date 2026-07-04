use base64::Engine;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Cursor;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

#[derive(Serialize)]
struct ModInfo {
    path: String,
    name: String,
    file_count: usize,
    size_bytes: u64,
    file_paths: Vec<String>,
}

impl From<vpkmerge_core::ModInfo> for ModInfo {
    fn from(m: vpkmerge_core::ModInfo) -> Self {
        ModInfo {
            path: m.path.to_string_lossy().into_owned(),
            name: m.name,
            file_count: m.file_count,
            size_bytes: m.size_bytes,
            file_paths: m.file_paths,
        }
    }
}

#[derive(Serialize)]
struct Conflict {
    path: String,
    owner_indices: Vec<usize>,
}

impl From<vpkmerge_core::Conflict> for Conflict {
    fn from(c: vpkmerge_core::Conflict) -> Self {
        Conflict {
            path: c.path,
            owner_indices: c.owner_indices,
        }
    }
}

#[derive(Serialize)]
struct MergeReport {
    total_entries: usize,
    overridden: usize,
    inputs: usize,
    output_path: String,
}

impl From<vpkmerge_core::MergeReport> for MergeReport {
    fn from(r: vpkmerge_core::MergeReport) -> Self {
        MergeReport {
            total_entries: r.total_entries,
            overridden: r.overridden_paths,
            inputs: r.inputs,
            output_path: r.output_path.to_string_lossy().into_owned(),
        }
    }
}

#[derive(Serialize)]
struct HeroOption {
    codename: String,
    label: String,
    particles: usize,
    textures: usize,
    materials: usize,
    models: usize,
    has_preview: bool,
}

#[derive(Serialize)]
struct HeroPrismReport {
    codename: String,
    particles_total: usize,
    particles_recolored: usize,
    particles_no_color: usize,
    particles_unpatchable: usize,
    gradient_fields: usize,
    color_fields: usize,
    boosted_fields: usize,
    lifted_black_gradient_fields: usize,
    random_range_fields: usize,
    textures_recolored: usize,
    materials_recolored: usize,
    materials_unpatchable: usize,
    models_recolored: usize,
    model_vertices: usize,
    particles_animated: usize,
    texture_age_inputs: usize,
    texture_offset_multipliers: usize,
    gradient_timing_edits: usize,
    total_entries: usize,
    output_path: String,
}

#[derive(Serialize)]
struct HeroRecipeParts {
    codename: String,
    label: String,
    particle_prefixes: Vec<String>,
    texture_entries: Vec<String>,
    material_entries: Vec<String>,
    model_entries: Vec<String>,
    preview_texture: Option<String>,
}

#[derive(Serialize)]
struct SelectOption {
    key: String,
    label: String,
}

#[derive(Serialize)]
struct TrippyPreview {
    frames: Vec<String>,
    width: u32,
    height: u32,
    frame_ms: u32,
}

#[derive(Serialize)]
struct GuiTrippySkinReport {
    codename: String,
    body_textures: usize,
    weapon_textures: usize,
    materials_scrolled: usize,
    texture_placeholders_promoted: usize,
    skipped_shared_textures: usize,
    skipped_unreadable: usize,
    skipped_unpatchable_materials: usize,
    total_entries: usize,
}

impl From<vpkmerge_core::trippy::TrippySkinReport> for GuiTrippySkinReport {
    fn from(report: vpkmerge_core::trippy::TrippySkinReport) -> Self {
        Self {
            codename: report.codename,
            body_textures: report.body_textures,
            weapon_textures: report.weapon_textures,
            materials_scrolled: report.materials_scrolled,
            texture_placeholders_promoted: report.texture_placeholders_promoted,
            skipped_shared_textures: report.skipped_shared_textures,
            skipped_unreadable: report.skipped_unreadable,
            skipped_unpatchable_materials: report.skipped_unpatchable_materials,
            total_entries: report.total_entries,
        }
    }
}

#[derive(Serialize)]
struct GuiTrippyAbilityReport {
    codename: String,
    particles_total: usize,
    particles_recolored: usize,
    particles_animated: usize,
    particles_no_color: usize,
    particles_unpatchable: usize,
    texture_age_inputs: usize,
    texture_offset_multipliers: usize,
    gradient_timing_edits: usize,
    color_gradient_loops: usize,
    color_cycle_operators: usize,
    textures_painted: usize,
    textures_skipped: usize,
    materials_recolored: usize,
    materials_scrolled: usize,
    materials_unpatchable: usize,
    models_recolored: usize,
    model_vertices: usize,
    missing_entries: usize,
    total_entries: usize,
}

impl From<vpkmerge_core::trippy::TrippyAbilityReport> for GuiTrippyAbilityReport {
    fn from(report: vpkmerge_core::trippy::TrippyAbilityReport) -> Self {
        Self {
            codename: report.codename,
            particles_total: report.particles_total,
            particles_recolored: report.particles_recolored,
            particles_animated: report.particles_animated,
            particles_no_color: report.particles_no_color,
            particles_unpatchable: report.particles_unpatchable,
            texture_age_inputs: report.texture_age_inputs,
            texture_offset_multipliers: report.texture_offset_multipliers,
            gradient_timing_edits: report.gradient_timing_edits,
            color_gradient_loops: report.color_gradient_loops,
            color_cycle_operators: report.color_cycle_operators,
            textures_painted: report.textures_painted,
            textures_skipped: report.textures_skipped,
            materials_recolored: report.materials_recolored,
            materials_scrolled: report.materials_scrolled,
            materials_unpatchable: report.materials_unpatchable,
            models_recolored: report.models_recolored,
            model_vertices: report.model_vertices,
            missing_entries: report.missing_entries,
            total_entries: report.total_entries,
        }
    }
}

#[derive(Serialize)]
struct TrippyLockerReport {
    codename: String,
    output_path: String,
    total_entries: usize,
    skin: Option<GuiTrippySkinReport>,
    ability: Option<GuiTrippyAbilityReport>,
}

impl HeroPrismReport {
    fn from_core(report: vpkmerge_core::HeroPrismRecolorReport, output_path: String) -> Self {
        Self {
            codename: report.codename,
            particles_total: report.particles_total,
            particles_recolored: report.particles_recolored,
            particles_no_color: report.particles_no_color,
            particles_unpatchable: report.particles_unpatchable,
            gradient_fields: report.gradient_fields,
            color_fields: report.color_fields,
            boosted_fields: report.boosted_fields,
            lifted_black_gradient_fields: report.lifted_black_gradient_fields,
            random_range_fields: report.random_range_fields,
            textures_recolored: report.textures_recolored,
            materials_recolored: report.materials_recolored,
            materials_unpatchable: report.materials_unpatchable,
            models_recolored: report.models_recolored,
            model_vertices: report.model_vertices,
            particles_animated: report.particles_animated,
            texture_age_inputs: report.texture_age_inputs,
            texture_offset_multipliers: report.texture_offset_multipliers,
            gradient_timing_edits: report.gradient_timing_edits,
            total_entries: report.total_entries,
            output_path,
        }
    }
}

#[tauri::command]
async fn pick_vpk_files(app: AppHandle) -> Vec<String> {
    app.dialog()
        .file()
        .add_filter("VPK files", &["vpk"])
        .blocking_pick_files()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[tauri::command]
async fn pick_output_path(app: AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .add_filter("VPK file", &["vpk"])
        .set_file_name("merged_dir.vpk")
        .blocking_save_file()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
async fn add_mod(path: String) -> Result<ModInfo, String> {
    vpkmerge_core::inspect(&path)
        .map(Into::into)
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn detect_conflicts(ordered_paths: Vec<String>) -> Result<Vec<Conflict>, String> {
    vpkmerge_core::detect_conflicts(&ordered_paths)
        .map(|cs| cs.into_iter().map(Into::into).collect())
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn merge_vpks(
    ordered_paths: Vec<String>,
    output_path: String,
    policy: Option<String>,
    overrides: Option<HashMap<String, usize>>,
) -> Result<MergeReport, String> {
    let collision_policy = match policy.as_deref() {
        None | Some("last_wins") => vpkmerge_core::CollisionPolicy::LastWins,
        Some("first_wins") => vpkmerge_core::CollisionPolicy::FirstWins,
        Some("strict") => vpkmerge_core::CollisionPolicy::Error,
        Some(other) => return Err(format!("unknown policy: {other}")),
    };
    vpkmerge_core::merge(
        &ordered_paths,
        &output_path,
        &vpkmerge_core::MergeOptions {
            collision_policy,
            overrides: overrides.unwrap_or_default(),
            ..Default::default()
        },
    )
    .map(Into::into)
    .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn path_exists(path: String) -> bool {
    std::path::Path::new(&path).exists()
}

/// Best-effort resolution of the local Deadlock `pak01_dir.vpk`. Checks the
/// standard Steam install roots on Linux and Windows. Returns `None` if no
/// candidate exists; the frontend then falls back to asking the user.
#[tauri::command]
async fn default_deadlock_vpk_path() -> Option<String> {
    default_deadlock_vpk_candidate().map(|p| p.to_string_lossy().into_owned())
}

fn default_deadlock_vpk_candidate() -> Option<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if cfg!(target_os = "linux") {
        if let Ok(home) = std::env::var("HOME") {
            candidates.push(
                format!("{home}/.steam/steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk")
                    .into(),
            );
            candidates.push(
                format!(
                    "{home}/.local/share/Steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk"
                )
                .into(),
            );
        }
    } else if cfg!(target_os = "windows") {
        candidates.push(
            "C:\\Program Files (x86)\\Steam\\steamapps\\common\\Deadlock\\game\\citadel\\pak01_dir.vpk"
                .into(),
        );
        candidates.push(
            "C:\\Program Files\\Steam\\steamapps\\common\\Deadlock\\game\\citadel\\pak01_dir.vpk"
                .into(),
        );
    }
    candidates.into_iter().find(|p| p.exists())
}

#[tauri::command]
async fn default_addon_output_path() -> Option<String> {
    let base = default_deadlock_vpk_candidate()?;
    let citadel_dir = base.parent()?;
    let addons = citadel_dir.join("addons");
    let mut max_slot = 1u32;
    if let Ok(read_dir) = std::fs::read_dir(&addons) {
        for entry in read_dir.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(slot) = name
                .strip_prefix("pak")
                .and_then(|s| s.strip_suffix("_dir.vpk"))
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            max_slot = max_slot.max(slot);
        }
    }
    Some(
        addons
            .join(format!("pak{:02}_dir.vpk", max_slot + 1))
            .to_string_lossy()
            .into_owned(),
    )
}

#[tauri::command]
async fn supported_hero_options() -> Vec<HeroOption> {
    vpkmerge_core::pinned_hero_codenames()
        .iter()
        .filter_map(|codename| {
            let recipe = vpkmerge_core::recipe_for(codename)?;
            Some(HeroOption {
                codename: (*codename).to_string(),
                label: hero_label(codename).to_string(),
                particles: recipe.particle_prefixes.len(),
                textures: recipe.texture_entries.len(),
                materials: recipe.material_entries.len(),
                models: recipe.model_entries.len(),
                has_preview: recipe.preview_texture.is_some(),
            })
        })
        .collect()
}

#[tauri::command]
async fn hero_recipe_parts(hero: String) -> Result<HeroRecipeParts, String> {
    let recipe = vpkmerge_core::recipe_for(&hero).ok_or_else(|| {
        format!(
            "no built-in ability-VFX recipe for hero codename {hero:?} (pinned: {})",
            vpkmerge_core::pinned_hero_codenames().join(", ")
        )
    })?;
    Ok(HeroRecipeParts {
        label: hero_label(&recipe.codename).to_string(),
        codename: recipe.codename,
        particle_prefixes: recipe.particle_prefixes,
        texture_entries: recipe.texture_entries,
        material_entries: recipe.material_entries,
        model_entries: recipe.model_entries,
        preview_texture: recipe.preview_texture,
    })
}

#[tauri::command]
async fn trippy_style_options() -> Vec<SelectOption> {
    vpkmerge_core::trippy::TRIPPY_STYLE_NAMES
        .iter()
        .map(|key| SelectOption {
            key: (*key).to_string(),
            label: title_label(key),
        })
        .collect()
}

#[tauri::command]
async fn trippy_animation_options() -> Vec<SelectOption> {
    ["off", "sweep", "loop", "cycle"]
        .into_iter()
        .map(|key| SelectOption {
            key: key.to_string(),
            label: title_label(key),
        })
        .collect()
}

#[tauri::command]
async fn trippy_preview(
    style: String,
    phase: Option<f32>,
    scroll: Option<f32>,
    intensity: Option<f32>,
    frames: Option<usize>,
    size: Option<u32>,
) -> Result<TrippyPreview, String> {
    let style =
        vpkmerge_core::trippy::TrippyStyle::from_name(&style).map_err(|e| format!("{e:#}"))?;
    let size = size.unwrap_or(192).clamp(16, 512);
    let frame_bytes = vpkmerge_core::trippy::trippy_preview_frames(
        style,
        phase.unwrap_or(0.0),
        scroll.unwrap_or(1.0),
        intensity.unwrap_or(1.0),
        frames.unwrap_or(18),
        size,
    )
    .map_err(|e| format!("{e:#}"))?;
    let frames = frame_bytes
        .into_iter()
        .map(|bytes| {
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            format!("data:image/png;base64,{b64}")
        })
        .collect();
    Ok(TrippyPreview {
        frames,
        width: size,
        height: size,
        frame_ms: 90,
    })
}

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
#[tauri::command]
async fn build_trippy_addon(
    vpk_path: String,
    base_path: Option<String>,
    hero: String,
    style: String,
    intensity: Option<f32>,
    phase: Option<f32>,
    scroll: Option<f64>,
    animation_style: String,
    animation_intensity: Option<f64>,
    include_body: bool,
    include_skin_weapons: bool,
    include_abilities: bool,
    include_vfx_weapons: bool,
    output_path: String,
) -> Result<TrippyLockerReport, String> {
    let style =
        vpkmerge_core::trippy::TrippyStyle::from_name(&style).map_err(|e| format!("{e:#}"))?;
    let (animation_style, animation_intensity) =
        parse_trippy_animation(&animation_style, animation_intensity.unwrap_or(1.0))?;
    let base = base_path
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from);
    let wants_skin = include_body || include_skin_weapons;
    let wants_ability = include_abilities || include_vfx_weapons;
    if !wants_skin && !wants_ability {
        return Err("select at least one skin or VFX target".to_string());
    }

    let skin_options = vpkmerge_core::trippy::TrippySkinOptions {
        style,
        intensity: intensity.unwrap_or(1.0),
        phase: phase.unwrap_or(0.0),
        scroll: scroll.unwrap_or(1.0),
        include_body,
        include_weapons: include_skin_weapons,
    };
    let ability_options = vpkmerge_core::trippy::TrippyAbilityOptions {
        style,
        intensity: intensity.unwrap_or(1.0),
        phase: phase.unwrap_or(0.0),
        animation_intensity,
        animation_style,
        include_abilities,
        include_weapons: include_vfx_weapons,
    };

    let mut skin = None;
    let mut ability = None;
    let total_entries;

    if wants_skin && wants_ability {
        let tmp = tempfile::tempdir().map_err(|e| format!("create temp dir: {e}"))?;
        let skin_vpk = tmp.path().join("trippy_skin_dir.vpk");
        let ability_vpk = tmp.path().join("trippy_vfx_dir.vpk");
        let skin_report = vpkmerge_core::trippy::trippy_skin_to_addon(
            &vpk_path,
            base.as_deref(),
            &hero,
            &skin_options,
            &skin_vpk,
        )
        .map_err(|e| format!("{e:#}"))?;
        let ability_report = vpkmerge_core::trippy::trippy_ability_vfx_to_addon(
            &vpk_path,
            base.as_deref(),
            &hero,
            &ability_options,
            &ability_vpk,
        )
        .map_err(|e| format!("{e:#}"))?;
        let report = vpkmerge_core::merge(
            &[skin_vpk.as_path(), ability_vpk.as_path()],
            &output_path,
            &vpkmerge_core::MergeOptions {
                collision_policy: vpkmerge_core::CollisionPolicy::LastWins,
                overrides: HashMap::new(),
                ..Default::default()
            },
        )
        .map_err(|e| format!("{e:#}"))?;
        total_entries = report.total_entries;
        skin = Some(skin_report.into());
        ability = Some(ability_report.into());
    } else if wants_skin {
        let skin_report = vpkmerge_core::trippy::trippy_skin_to_addon(
            &vpk_path,
            base.as_deref(),
            &hero,
            &skin_options,
            &output_path,
        )
        .map_err(|e| format!("{e:#}"))?;
        total_entries = skin_report.total_entries;
        skin = Some(skin_report.into());
    } else {
        let ability_report = vpkmerge_core::trippy::trippy_ability_vfx_to_addon(
            &vpk_path,
            base.as_deref(),
            &hero,
            &ability_options,
            &output_path,
        )
        .map_err(|e| format!("{e:#}"))?;
        total_entries = ability_report.total_entries;
        ability = Some(ability_report.into());
    }

    Ok(TrippyLockerReport {
        codename: hero,
        output_path,
        total_entries,
        skin,
        ability,
    })
}

fn hero_label(codename: &str) -> &str {
    match codename {
        "bookworm" => "Paige",
        "necro" => "Graves",
        "inferno" => "Infernus",
        "yamato" => "Yamato",
        "abrams" => "Abrams",
        "archer" => "Grey Talon",
        "astro" => "Holliday",
        "bebop" => "Bebop",
        "digger" => "Mo & Krill",
        "doorman" => "Doorman",
        "drifter" => "Drifter",
        "dynamo" => "Dynamo",
        "familiar" => "Familiar",
        "fencer" => "Apollo",
        "frank" => "Victor",
        "ghost" => "Lady Geist",
        "haze" => "Haze",
        "hornet" => "Vindicta",
        "kelvin" => "Kelvin",
        "nano" => "Calico",
        "lash" => "Lash",
        "mcginnis" => "McGinnis",
        "magician" => "Sinclair",
        "mirage" => "Mirage",
        "pocket" => "Pocket",
        "priest" => "Priest",
        "punkgoat" => "Billy",
        "shiv" => "Shiv",
        "tengu" => "Ivy",
        "unicorn" => "Celeste",
        "gigawatt" => "Seven",
        "vampirebat" => "Mina",
        "viper" => "Vyper",
        "viscous" => "Viscous",
        "warden" => "Warden",
        "werewolf" => "Werewolf",
        "wraith" => "Wraith",
        _ => codename,
    }
}

fn title_label(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn parse_trippy_animation(
    style: &str,
    intensity: f64,
) -> Result<(vpkmerge_core::PrismAnimationStyle, f64), String> {
    match style.to_ascii_lowercase().as_str() {
        "off" | "none" | "static" => Ok((vpkmerge_core::PrismAnimationStyle::Sweep, 0.0)),
        "sweep" => Ok((vpkmerge_core::PrismAnimationStyle::Sweep, intensity)),
        "loop" | "loops" => Ok((vpkmerge_core::PrismAnimationStyle::Loop, intensity)),
        "cycle" | "cycles" => Ok((vpkmerge_core::PrismAnimationStyle::Cycle, intensity)),
        other => Err(format!(
            "animation style must be off, sweep, loop, or cycle (got {other:?})"
        )),
    }
}

#[tauri::command]
async fn build_hero_prism_vpk(
    vpk_path: String,
    base_path: Option<String>,
    hero: String,
    animated: bool,
    output_path: String,
) -> Result<HeroPrismReport, String> {
    let base = base_path
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from);
    let report = vpkmerge_core::prism_recolor_hero_to_addon(
        &vpk_path,
        base.as_deref(),
        &hero,
        animated,
        &output_path,
    )
    .map_err(|e| format!("{e:#}"))?;
    Ok(HeroPrismReport::from_core(report, output_path))
}

#[derive(Serialize)]
struct TexturePreview {
    /// `data:image/png;base64,...` URL, ready to drop into `<img :src>`.
    data_url: String,
    /// Displayed (post-downscale) dimensions.
    width: u32,
    height: u32,
    /// Original texture dimensions before any downscale.
    orig_width: u32,
    orig_height: u32,
    /// `VTexFormat` name (e.g. "BC7", "RGBA8888").
    format: String,
    /// True if the texture has the `CUBE_TEXTURE` flag; callers may then
    /// re-invoke with `face` in `1..=5` to see other faces. Face ordering
    /// is `[+X, -X, +Y, -Y, +Z, -Z]`; default (no face arg) is `+X`.
    is_cubemap: bool,
    /// Number of mip levels the source texture has. Callers may pass `mip`
    /// in `0..mip_count` to see lower-detail versions.
    mip_count: u8,
}

#[tauri::command]
async fn preview_texture(
    vpk_path: String,
    entry: String,
    max_dim: Option<u32>,
    face: Option<u8>,
    mip: Option<u8>,
) -> Result<TexturePreview, String> {
    let cap = max_dim.unwrap_or(256).max(16);
    let face = face.unwrap_or(0);
    let mip = mip.unwrap_or(0);
    let vpk = valve_pak::open(&vpk_path).map_err(|e| format!("open vpk: {e}"))?;
    let mut vf = vpk
        .get_file(&entry)
        .map_err(|e| format!("entry not found: {e}"))?;
    let bytes = vf.read_all().map_err(|e| format!("read entry: {e}"))?;

    let info = morphic::inspect(&bytes).map_err(|e| format!("inspect: {e}"))?;
    let is_cubemap = info.flags.contains(morphic::TextureFlags::CUBE_TEXTURE);
    let mip_count = info.mip_count;
    let img = match morphic::decode_at(
        &bytes,
        &morphic::DecodeOptions {
            mip,
            slice: 0,
            face,
        },
    ) {
        Ok(img) => img,
        Err(morphic::DecodeError::Unimplemented(fmt)) => {
            return Err(format!("preview not supported for format: {}", fmt.name()));
        }
        Err(e) => return Err(format!("decode: {e}")),
    };

    let raw_rgba = match img.data {
        morphic::ImageData::Rgba8(buf) => buf,
        morphic::ImageData::Rgba16F(buf) => tonemap_rgba_f16_to_u8(&buf),
    };
    let buffer: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> =
        image::ImageBuffer::from_raw(img.width, img.height, raw_rgba)
            .ok_or_else(|| "decoded buffer size mismatch".to_string())?;

    let dyn_img = image::DynamicImage::ImageRgba8(buffer);
    let (orig_w, orig_h) = (img.width, img.height);
    let downscaled = if orig_w > cap || orig_h > cap {
        dyn_img.resize(cap, cap, image::imageops::FilterType::Triangle)
    } else {
        dyn_img
    };

    let (w, h) = (downscaled.width(), downscaled.height());
    let mut png_bytes: Vec<u8> = Vec::new();
    downscaled
        .write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .map_err(|e| format!("png encode: {e}"))?;

    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Ok(TexturePreview {
        data_url: format!("data:image/png;base64,{b64}"),
        width: w,
        height: h,
        orig_width: orig_w,
        orig_height: orig_h,
        format: info.format.name().to_string(),
        is_cubemap,
        mip_count,
    })
}

// Reinhard tone-map + sRGB-ish 1/2.2 gamma so HDR (BC6H, future RGBA16F)
// textures look like reasonable LDR previews in the conflict modal. Negative
// values are clamped to 0; alpha is a direct clamp to [0, 1]. This matches
// the spirit of what VRF's tone-mapped PNG output produces for the same
// fixtures, though it's not bit-exact with VRF (which is fine, this is
// purely for human preview).
fn tonemap_rgba_f16_to_u8(buf: &[half::f16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len());
    for px in buf.chunks_exact(4) {
        for c in &px[..3] {
            let v = c.to_f32().max(0.0);
            let tonemapped = v / (1.0 + v);
            let gamma = tonemapped.powf(1.0 / 2.2);
            out.push(float_to_u8(gamma));
        }
        out.push(float_to_u8(px[3].to_f32()));
    }
    out
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn float_to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(r: f32, g: f32, b: f32, a: f32) -> [half::f16; 4] {
        [
            half::f16::from_f32(r),
            half::f16::from_f32(g),
            half::f16::from_f32(b),
            half::f16::from_f32(a),
        ]
    }

    #[test]
    fn tonemap_black_stays_black() {
        let out = tonemap_rgba_f16_to_u8(&rgba(0.0, 0.0, 0.0, 1.0));
        assert_eq!(out, vec![0, 0, 0, 255]);
    }

    #[test]
    fn tonemap_alpha_is_direct_clamp() {
        // Color channels tone-map (so 0.0 -> 0), alpha passes straight through.
        let out = tonemap_rgba_f16_to_u8(&rgba(0.0, 0.0, 0.0, 0.5));
        assert_eq!(out[3], 128); // round(0.5 * 255) = 128
        let saturated = tonemap_rgba_f16_to_u8(&rgba(0.0, 0.0, 0.0, 4.0));
        assert_eq!(saturated[3], 255);
        let neg_alpha = tonemap_rgba_f16_to_u8(&rgba(0.0, 0.0, 0.0, -1.0));
        assert_eq!(neg_alpha[3], 0);
    }

    #[test]
    fn tonemap_compresses_bright_values_below_white() {
        // Even a "100x brighter than mid-grey" input must not produce pure
        // white, that's the whole point of Reinhard: 100 / (1 + 100) ≈ 0.99,
        // then gamma 1/2.2 ≈ 0.995, * 255 ≈ 253.
        let out = tonemap_rgba_f16_to_u8(&rgba(100.0, 100.0, 100.0, 1.0));
        assert!(
            out[0] >= 250 && out[0] < 255,
            "bright but not saturated, got {}",
            out[0]
        );
        assert_eq!(out[3], 255);
    }

    #[test]
    fn tonemap_negative_color_clamps_to_zero() {
        let out = tonemap_rgba_f16_to_u8(&rgba(-1.0, -0.5, -10.0, 1.0));
        assert_eq!(&out[..3], &[0, 0, 0]);
    }

    #[test]
    fn tonemap_mid_grey_is_reasonable() {
        // 0.5 -> 0.5/1.5 = 0.333... -> ^(1/2.2) ≈ 0.605 -> *255 ≈ 154
        let out = tonemap_rgba_f16_to_u8(&rgba(0.5, 0.5, 0.5, 1.0));
        for c in &out[..3] {
            assert!(*c >= 150 && *c <= 158, "mid-grey ~154, got {c}");
        }
    }
}

#[tauri::command]
async fn save_text_file(
    app: AppHandle,
    default_name: String,
    content: String,
) -> Result<Option<String>, String> {
    let Some(path) = app
        .dialog()
        .file()
        .set_file_name(&default_name)
        .blocking_save_file()
        .and_then(|p| p.into_path().ok())
    else {
        return Ok(None);
    };
    std::fs::write(&path, content.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn reveal_in_folder(path: String) -> Result<(), String> {
    use std::process::Command;
    let result = if cfg!(target_os = "linux") {
        let p = std::path::Path::new(&path);
        let target = if p.is_file() {
            p.parent().unwrap_or(p)
        } else {
            p
        };
        Command::new("xdg-open").arg(target).spawn()
    } else if cfg!(target_os = "windows") {
        Command::new("explorer")
            .arg(format!("/select,{path}"))
            .spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").args(["-R", &path]).spawn()
    } else {
        return Err("Unsupported OS".into());
    };
    result.map(|_| ()).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            pick_vpk_files,
            pick_output_path,
            add_mod,
            detect_conflicts,
            merge_vpks,
            path_exists,
            reveal_in_folder,
            preview_texture,
            save_text_file,
            default_deadlock_vpk_path,
            default_addon_output_path,
            supported_hero_options,
            hero_recipe_parts,
            trippy_style_options,
            trippy_animation_options,
            trippy_preview,
            build_hero_prism_vpk,
            build_trippy_addon
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
