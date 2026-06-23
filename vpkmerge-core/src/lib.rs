//! Core VPK merging logic. Pure Rust, no UI or framework dependencies.
//!
//! ```no_run
//! use vpkmerge_core::{merge, MergeOptions};
//!
//! let report = merge(
//!     &["mod_a_dir.vpk", "mod_b_dir.vpk"],
//!     "combined_dir.vpk",
//!     &MergeOptions::default(),
//! ).unwrap();
//! println!("Wrote {} entries", report.total_entries);
//! ```

use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub mod model;
pub mod portrait;
pub use model::{
    apply_model_edit_glb, bake_uv_atlas, bake_uv_mask, edit_model_geometry, export_femodel_json,
    export_hero_model, export_model, export_model_buffer_glb, export_model_group_glb,
    hero_model_clips, hero_model_entry, inspect_model_parts, inspect_models, live_hero_entries,
    live_hero_materials, model_clips, model_draw_call_targets, model_uv_segments,
    model_vertex_targets, recolor_models_to_addon, reencode_model_mdat, remove_model_material,
    replace_model_group, replace_model_part, AnimOptions, ClipSummary, DrawCallInfo,
    DrawCallSkinInfo, GeometryEdit, GeometryEditReport, LiveHeroEntry, LiveHeroMaterial,
    MaterialRemovalReport, ModelDrawCallInspection, ModelEntry, ModelInfo, ModelPartInspection,
    ModelPartSelector, ModelRecolorEntry, PartReplacementReport, PoseSelection, RemovedDrawCall,
    ReplacedMeshGroup, ReplacedMeshPart, ResolvedResource, ResolvedTextureParam, SegmentBy,
    SuggestedPartGroup, UvSegmentInfo, VertexTarget, DEFAULT_POSE_CLIPS,
};
pub use portrait::{extract_portraits, PortraitInfo, PortraitVariant};

pub mod soundevents;
pub use soundevents::{EventSummary, SoundEvents};

pub mod catalog;
pub use catalog::{
    build_hero_sound_index, build_voiceline_index, build_voiceline_index_with_captions,
    caption_hash, CaptionDb, HeroSound, HeroSoundCategory, VoiceLine, ENGLISH_CAPTIONS_ENTRY,
};

pub mod texture_catalog;
pub use texture_catalog::{
    build_texture_index, cache_texture_thumbnails, classify_texture, thumbnail_png,
    CachedThumbnail, TextureCategory, TextureEntry, Thumbnail, ThumbnailOutcome,
};

pub mod catalog_cache;
pub use catalog_cache::{BuildFingerprint, CatalogCache, CACHE_SCHEMA_VERSION};

pub mod localization;
pub use localization::{
    build_hero_roster, hero_name_tokens, localization_dir_for_pak, parse_kv_tokens, HeroInfo,
    DEFAULT_LANG,
};

pub mod cubemap;
pub use cubemap::{export_cubemap_hdr, CubemapFaceReport, CUBEMAP_FACE_NAMES};

pub mod recolor;
pub use recolor::{
    inspect_texture, recolor_model_vertex_colors, recolor_texture_hue, recolor_texture_image,
    recolor_texture_preview_png, ModelRecolorStats, Recolor, TextureSummary,
};

pub mod icon;
pub use icon::{build_icon_from_template, png_to_rgba8_image};

pub mod mp3;
pub use mp3::{apply_mp3_gain, trim_mp3};

pub mod soundswap;
pub use soundswap::{
    donor_is_looped, mint_swapped_clip, parse_mp3_params, swap_event_audio, EventSwap, PoolPolicy,
};

pub mod hero_recolor;
pub use hero_recolor::{
    animate_particle_timing_bytes, insert_color_cycle_operator,
    insert_color_cycle_operator_with_tuning, is_tiling_particle_texture,
    loop_animate_particle_bytes, non_tiling_texture_inputs, pinned_hero_codenames,
    prism_recolor_hero_to_addon, prism_recolor_hero_to_addon_tuned, recipe_for,
    recolor_hero_preview_png, recolor_hero_to_addon, recolor_particle_bytes,
    scan_hero_rainbow_support, GradientStop, HeroPrismRecolorReport, HeroRainbowSupportReport,
    HeroRecolorRecipe, HeroRecolorReport, ParticleTimingAnimationStats, PrismAnimationStyle,
    PrismGradient, PrismTuning, MAX_GRADIENT_STOPS, PRISM_PRESET_NAMES,
};

pub mod trippy;
pub use trippy::{
    trippy_ability_vfx_to_addon, trippy_preview_frames, trippy_preview_sprite,
    trippy_skin_to_addon, TrippyAbilityOptions, TrippyAbilityReport, TrippySkinOptions,
    TrippySkinReport, TrippyStyle, TRIPPY_STYLE_NAMES,
};

pub mod vmat_style;
pub use vmat_style::{
    list_materials, patch_vmat_params, style_materials_to_addon, VmatEdit, VmatInfo,
    VmatPatchStats, VmatPreset, VmatStyleReport, VmatTargets,
};

pub mod soul_container;
pub use soul_container::{
    compile_soul_container_prepared_pure_rust, compile_soul_container_source,
    compile_soul_container_source_pure_rust, prepare_soul_container_import,
    ResourceCompilerBackend, SoulContainerBounds, SoulContainerCompileBackend,
    SoulContainerCompileReport, SoulContainerImportOptions, SoulContainerPreparedMaterial,
    SoulContainerPreparedSource, DEFAULT_SOUL_CONTAINER_MODEL_REL,
    DEFAULT_SOUL_CONTAINER_PHYSICS_RADIUS, DEFAULT_SOUL_CONTAINER_TARGET_LARGEST_AXIS,
    DEFAULT_SOURCE_UNITS_PER_BLENDER,
};

pub mod soul_import_clone;
pub use soul_import_clone::{
    import_clone, import_soul_container_clone, soul_target, urn_target, CloneTarget,
    NormalSynthesis, SoulGlow, SoulImportCloneOptions, SoulImportReport, SoulOrient,
};
pub mod hat_import;
pub use hat_import::{import_hero_hat, HatImportOptions, HatImportReport};

#[derive(Debug, Clone)]
pub struct ModInfo {
    pub path: PathBuf,
    pub name: String,
    pub file_count: usize,
    pub size_bytes: u64,
    pub file_paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Conflict {
    pub path: String,
    /// Indices into the input slice, in input order.
    pub owner_indices: Vec<usize>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CollisionPolicy {
    /// Later inputs override earlier ones (default).
    #[default]
    LastWins,
    /// Earlier inputs win; later duplicates are dropped.
    FirstWins,
    /// Refuse to merge if any path appears in more than one input.
    Error,
}

#[derive(Clone, Debug, Default)]
pub struct MergeOptions {
    pub collision_policy: CollisionPolicy,
    /// Per-path manual overrides: maps an entry path to the index (in
    /// `ordered_inputs`) that should win for that path. Takes precedence
    /// over `collision_policy`. An override pointing at an index that does
    /// not actually own the path causes `merge` to error.
    pub overrides: HashMap<String, usize>,
}

#[derive(Debug, Clone)]
pub struct MergeReport {
    pub total_entries: usize,
    /// Number of distinct paths whose content was overridden by a later input.
    pub overridden_paths: usize,
    pub inputs: usize,
    pub output_path: PathBuf,
}

/// One output bucket: where to write, and the rule that decides which paths
/// from the input belong in it.
#[derive(Clone, Debug)]
pub struct SplitOutput {
    pub path: PathBuf,
    pub predicate: PathPredicate,
}

/// Path matchers. Start with prefix-only (covers ability slots); add more
/// variants if a real use case shows up. Keep this an enum, not a closure,
/// so a `SplitOutput` is serializable from the CLI/JSON layer.
#[derive(Clone, Debug)]
pub enum PathPredicate {
    /// Match if the entry path starts with any of the given prefixes.
    /// Case-sensitive. Empty list matches nothing.
    AnyPrefix(Vec<String>),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlapPolicy {
    /// Each path goes to the FIRST output whose predicate matches it.
    #[default]
    FirstMatch,
    /// Each path goes to EVERY output whose predicate matches it.
    /// Use when you intentionally want the same entry in multiple outputs.
    AllMatches,
    /// Refuse to split if any path matches more than one output.
    Error,
}

#[derive(Clone, Debug, Default)]
pub struct SplitOptions {
    pub overlap_policy: OverlapPolicy,
    /// Optional path for a VPK containing every input entry that no
    /// `SplitOutput` predicate claimed. None = drop unmatched entries silently.
    pub residual_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SplitOutputReport {
    pub path: PathBuf,
    pub entries: usize,
}

#[derive(Debug, Clone)]
pub struct SplitReport {
    pub input_entries: usize,
    pub outputs: Vec<SplitOutputReport>,
    pub residual: Option<SplitOutputReport>,
    /// Number of input entries that landed in zero outputs (and were either
    /// written to residual or dropped depending on options).
    pub unmatched: usize,
}

/// Open a single VPK and return its file list plus a display name.
pub fn inspect<P: AsRef<Path>>(path: P) -> Result<ModInfo> {
    let path = path.as_ref();
    let vpk = valve_pak::open(path).with_context(|| format!("opening {}", path.display()))?;
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let file_paths: Vec<String> = vpk.file_paths().cloned().collect();
    let size_bytes = std::fs::metadata(path).map_or(0, |m| m.len());
    Ok(ModInfo {
        path: path.to_path_buf(),
        name,
        file_count: file_paths.len(),
        size_bytes,
        file_paths,
    })
}

/// Compute the set of paths that appear in more than one input, without
/// performing the merge. Cheaper than a full merge and useful for previewing.
pub fn detect_conflicts<P: AsRef<Path>>(inputs: &[P]) -> Result<Vec<Conflict>> {
    let vpks = open_all(inputs)?;
    let owners = compute_owners(&vpks);
    let mut conflicts: Vec<Conflict> = owners
        .into_iter()
        .filter(|(_, idxs)| idxs.len() > 1)
        .map(|(path, owner_indices)| Conflict {
            path,
            owner_indices,
        })
        .collect();
    conflicts.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(conflicts)
}

/// Combine `ordered_inputs` into a single VPK at `output`, applying
/// `options.collision_policy` when the same path appears in more than one
/// input.
pub fn merge<P: AsRef<Path>, O: AsRef<Path>>(
    ordered_inputs: &[P],
    output: O,
    options: &MergeOptions,
) -> Result<MergeReport> {
    if ordered_inputs.len() < 2 {
        anyhow::bail!("need at least 2 input VPKs to merge");
    }
    let output = output.as_ref().to_path_buf();

    reject_output_equals_input(ordered_inputs, &output)?;
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
    }

    let vpks = open_all(ordered_inputs)?;
    let owners = compute_owners(&vpks);
    validate_overrides(&owners, &options.overrides, ordered_inputs.len())?;
    let winners = resolve_winners(&owners, options.collision_policy, &options.overrides)?;
    let overridden_paths = owners.values().filter(|v| v.len() > 1).count();

    let tmp = tempfile::tempdir().context("creating temp directory")?;
    for (path, idx) in &winners {
        let mut vf = vpks[*idx]
            .get_file(path)
            .with_context(|| format!("locating {path} in input {idx}"))?;
        let bytes = vf.read_all().with_context(|| format!("reading {path}"))?;
        let dst = safe_join(tmp.path(), path)?;
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {}", parent.display()))?;
        }
        std::fs::write(&dst, &bytes).with_context(|| format!("writing {}", dst.display()))?;
    }

    let merged = valve_pak::from_directory(tmp.path()).context("packing merged VPK")?;
    merged
        .save(&output)
        .with_context(|| format!("saving {}", output.display()))?;

    Ok(MergeReport {
        total_entries: winners.len(),
        overridden_paths,
        inputs: ordered_inputs.len(),
        output_path: output,
    })
}

/// Pack in-memory files into a standalone single-archive VPK at `output`.
///
/// Each tuple is `(entry_path, bytes)`; entry paths use `/` and are relative to
/// the VPK root (e.g. `soundevents/hero/gigawatt.vsndevts_c`). The parent
/// directory of `output` is created if missing. The result is a `_dir.vpk` with
/// no chunk files, loadable by Deadlock as an addon and mergeable via [`merge`].
///
/// This is the inverse of the "merge VPK inputs only" pipeline: it gets a
/// generated or edited loose file (e.g. an encoded soundevents resource) into a
/// VPK so it can enter the merge pipeline.
pub fn pack<O: AsRef<Path>>(files: &[(&str, &[u8])], output: O) -> Result<()> {
    let output = output.as_ref();
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating output directory {}", parent.display()))?;
        }
    }

    let tmp = tempfile::tempdir().context("creating temp directory")?;
    for (entry, bytes) in files {
        let dst = safe_join(tmp.path(), entry)?;
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {}", parent.display()))?;
        }
        std::fs::write(&dst, bytes).with_context(|| format!("writing {}", dst.display()))?;
    }

    let packed = valve_pak::from_directory(tmp.path()).context("packing VPK")?;
    packed
        .save(output)
        .with_context(|| format!("saving {}", output.display()))?;
    Ok(())
}

/// Read one entry's raw bytes out of a VPK.
///
/// A small convenience for callers that don't depend on `valve_pak` directly
/// (e.g. the CLI), mirroring what `SoundEvents::from_vpk` does internally.
/// Chunked inputs are transparent: open the `_dir.vpk` and the chunk files are
/// read automatically.
pub fn read_vpk_entry<P: AsRef<Path>>(vpk_path: P, entry: &str) -> Result<Vec<u8>> {
    let vpk_path = vpk_path.as_ref();
    let vpk =
        valve_pak::open(vpk_path).with_context(|| format!("opening {}", vpk_path.display()))?;
    let mut file = vpk
        .get_file(entry)
        .with_context(|| format!("no entry {entry:?} in {}", vpk_path.display()))?;
    file.read_all()
        .with_context(|| format!("reading {entry:?} from {}", vpk_path.display()))
}

/// Read a VO/ability `.vsnd_c` clip out of a VPK and return its playable MP3
/// stream. Combines [`read_vpk_entry`] with [`morphic::extract_vsnd_mp3`]: the
/// audition path for the Foundry Sound tab (pick a voice line in the catalog,
/// play the clip). The returned bytes are a complete MP3 file, no decode needed.
///
/// # Errors
/// Propagates a missing entry, or a clip whose container is not an MP3-backed
/// `CVoiceContainerDefault` (a different codec is reported rather than returned
/// as bogus `.mp3`).
pub fn extract_voiceclip_mp3<P: AsRef<Path>>(vpk_path: P, entry: &str) -> Result<Vec<u8>> {
    // The voice-line index reports clip paths with the soundevents `.vsnd`
    // extension, but the packed VPK entry is the compiled `.vsnd_c`, so accept
    // either and normalize (callers can pass an index path verbatim).
    let normalized = if Path::new(entry)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("vsnd"))
    {
        format!("{entry}_c")
    } else {
        entry.to_owned()
    };
    let bytes = read_vpk_entry(&vpk_path, &normalized)?;
    morphic::extract_vsnd_mp3(&bytes).with_context(|| format!("extracting MP3 from {normalized:?}"))
}

/// Route entries from `input` into N output VPKs according to `outputs`.
/// Reads `input` once. Returns a per-output entry count.
///
/// Entries that no `SplitOutput` predicate claims are written to
/// `options.residual_path` if set, or dropped silently otherwise. Either way,
/// the count appears in `SplitReport.unmatched`.
pub fn split<I: AsRef<Path>>(
    input: I,
    outputs: &[SplitOutput],
    options: &SplitOptions,
) -> Result<SplitReport> {
    let input = input.as_ref();

    let mut all_dst: Vec<&Path> = outputs.iter().map(|o| o.path.as_path()).collect();
    if let Some(r) = &options.residual_path {
        all_dst.push(r.as_path());
    }
    reject_split_path_collisions(input, &all_dst)?;
    for dst in &all_dst {
        if let Some(parent) = dst.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating output directory {}", parent.display()))?;
            }
        }
    }

    let vpk = valve_pak::open(input).with_context(|| format!("opening {}", input.display()))?;
    let all_paths: Vec<String> = vpk.file_paths().cloned().collect();
    let input_entries = all_paths.len();

    if options.overlap_policy == OverlapPolicy::Error {
        let offenders: Vec<&str> = all_paths
            .iter()
            .filter(|p| {
                outputs
                    .iter()
                    .filter(|o| matches_predicate(&o.predicate, p))
                    .count()
                    > 1
            })
            .map(String::as_str)
            .collect();
        if !offenders.is_empty() {
            let sample = offenders
                .iter()
                .take(5)
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "{} path{} matched by more than one output (overlap policy is Error). First few: {}",
                offenders.len(),
                if offenders.len() == 1 { "" } else { "s" },
                sample,
            );
        }
    }

    let mut routes: Vec<Vec<String>> = vec![Vec::new(); outputs.len()];
    let mut residual: Vec<String> = Vec::new();
    let mut unmatched = 0usize;

    for p in &all_paths {
        let mut matched = false;
        for (i, o) in outputs.iter().enumerate() {
            if matches_predicate(&o.predicate, p) {
                matched = true;
                routes[i].push(p.clone());
                if options.overlap_policy == OverlapPolicy::FirstMatch {
                    break;
                }
            }
        }
        if !matched {
            unmatched += 1;
            if options.residual_path.is_some() {
                residual.push(p.clone());
            }
        }
    }

    let mut output_reports = Vec::with_capacity(outputs.len());
    for (i, o) in outputs.iter().enumerate() {
        write_bucket(&vpk, &routes[i], &o.path)?;
        output_reports.push(SplitOutputReport {
            path: o.path.clone(),
            entries: routes[i].len(),
        });
    }

    let residual_report = if let Some(rpath) = &options.residual_path {
        write_bucket(&vpk, &residual, rpath)?;
        Some(SplitOutputReport {
            path: rpath.clone(),
            entries: residual.len(),
        })
    } else {
        None
    };

    Ok(SplitReport {
        input_entries,
        outputs: output_reports,
        residual: residual_report,
        unmatched,
    })
}

fn matches_predicate(pred: &PathPredicate, path: &str) -> bool {
    match pred {
        PathPredicate::AnyPrefix(prefixes) => prefixes.iter().any(|pref| path.starts_with(pref)),
    }
}

/// Join an untrusted VPK entry path onto `base`, refusing anything that would
/// escape `base`. A VPK is attacker-controllable input: a hand-crafted archive
/// can carry entry paths like `../../foo`, `/etc/cron.d/x`, or `C:\Users\x` that
/// `Path::join` would happily write outside the temp dir (Zip-Slip). Legitimate
/// Source 2 entries are always relative, forward-slash, and never contain `..`,
/// so this rejects only tampered inputs.
fn safe_join(base: &Path, entry: &str) -> Result<PathBuf> {
    if entry.starts_with('/') || entry.starts_with('\\') {
        bail!("refusing absolute VPK entry path: {entry}");
    }
    let mut out = base.to_path_buf();
    let mut pushed = false;
    // Split on both separators: a crafted entry can use either regardless of OS.
    for seg in entry.split(['/', '\\']) {
        match seg {
            "" | "." => {}
            ".." => bail!("refusing VPK entry with parent-dir component: {entry}"),
            _ if seg.contains(':') => {
                bail!("refusing VPK entry with drive/volume component: {entry}")
            }
            _ => {
                out.push(seg);
                pushed = true;
            }
        }
    }
    if !pushed {
        bail!("refusing empty VPK entry path: {entry}");
    }
    Ok(out)
}

fn write_bucket(vpk: &valve_pak::VPK, entries: &[String], output_path: &Path) -> Result<()> {
    let tmp = tempfile::tempdir().context("creating temp directory")?;
    for path in entries {
        let mut vf = vpk
            .get_file(path)
            .with_context(|| format!("locating {path} in input"))?;
        let bytes = vf.read_all().with_context(|| format!("reading {path}"))?;
        let dst = safe_join(tmp.path(), path)?;
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {}", parent.display()))?;
        }
        std::fs::write(&dst, &bytes).with_context(|| format!("writing {}", dst.display()))?;
    }
    let packed = valve_pak::from_directory(tmp.path()).context("packing split VPK")?;
    packed
        .save(output_path)
        .with_context(|| format!("saving {}", output_path.display()))?;
    Ok(())
}

fn reject_split_path_collisions(input: &Path, outputs: &[&Path]) -> Result<()> {
    let canon_input = input.canonicalize().ok();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    for dst in outputs {
        let canon_dst = dst.parent().and_then(|p| {
            if p.as_os_str().is_empty() {
                None
            } else {
                p.canonicalize()
                    .ok()
                    .and_then(|p| dst.file_name().map(|f| p.join(f)))
            }
        });
        let Some(canon_dst) = canon_dst else {
            continue;
        };
        if Some(&canon_dst) == canon_input.as_ref() {
            anyhow::bail!("output path equals input path: {}", canon_dst.display());
        }
        if !seen.insert(canon_dst.clone()) {
            anyhow::bail!("output listed twice: {}", canon_dst.display());
        }
    }
    Ok(())
}

fn open_all<P: AsRef<Path>>(inputs: &[P]) -> Result<Vec<valve_pak::VPK>> {
    inputs
        .iter()
        .map(|p| {
            let p = p.as_ref();
            valve_pak::open(p).with_context(|| format!("opening {}", p.display()))
        })
        .collect()
}

fn compute_owners(vpks: &[valve_pak::VPK]) -> BTreeMap<String, Vec<usize>> {
    let mut owners: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (idx, vpk) in vpks.iter().enumerate() {
        for p in vpk.file_paths() {
            owners.entry(p.clone()).or_default().push(idx);
        }
    }
    owners
}

fn resolve_winners(
    owners: &BTreeMap<String, Vec<usize>>,
    policy: CollisionPolicy,
    overrides: &HashMap<String, usize>,
) -> Result<BTreeMap<String, usize>> {
    if policy == CollisionPolicy::Error {
        let conflicts: Vec<&str> = owners
            .iter()
            .filter(|(path, v)| v.len() > 1 && !overrides.contains_key(path.as_str()))
            .map(|(k, _)| k.as_str())
            .collect();
        if !conflicts.is_empty() {
            let sample = conflicts
                .iter()
                .take(5)
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "{} unresolved path conflict{} (collision policy is Error). First few: {}",
                conflicts.len(),
                if conflicts.len() == 1 { "" } else { "s" },
                sample
            );
        }
    }

    Ok(owners
        .iter()
        .map(|(path, idxs)| {
            let winner = if let Some(&idx) = overrides.get(path) {
                idx
            } else {
                match policy {
                    CollisionPolicy::FirstWins | CollisionPolicy::Error => *idxs.first().unwrap(),
                    CollisionPolicy::LastWins => *idxs.last().unwrap(),
                }
            };
            (path.clone(), winner)
        })
        .collect())
}

fn validate_overrides(
    owners: &BTreeMap<String, Vec<usize>>,
    overrides: &HashMap<String, usize>,
    input_count: usize,
) -> Result<()> {
    for (path, &idx) in overrides {
        if idx >= input_count {
            anyhow::bail!(
                "override for {path} points at input index {idx} but only {input_count} inputs were given"
            );
        }
        let Some(idxs) = owners.get(path) else {
            anyhow::bail!("override path {path} does not exist in any input");
        };
        if !idxs.contains(&idx) {
            anyhow::bail!(
                "override for {path} points at input index {idx} which does not contain that path"
            );
        }
    }
    Ok(())
}

fn reject_output_equals_input<P: AsRef<Path>>(inputs: &[P], output: &Path) -> Result<()> {
    let canonical_output = output.parent().and_then(|p| {
        if p.as_os_str().is_empty() {
            None
        } else {
            p.canonicalize()
                .ok()
                .and_then(|p| output.file_name().map(|f| p.join(f)))
        }
    });
    let Some(canonical_output) = canonical_output else {
        return Ok(());
    };
    let mut seen = HashSet::new();
    for input in inputs {
        if let Ok(canon) = input.as_ref().canonicalize() {
            if canon == canonical_output {
                anyhow::bail!("output path equals input path: {}", canon.display());
            }
            if !seen.insert(canon.clone()) {
                anyhow::bail!("input listed twice: {}", canon.display());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{tempdir, TempDir};

    #[test]
    fn safe_join_rejects_path_traversal() {
        let base = Path::new("/tmp/vpkmerge-work");

        // Parent-dir escapes, with either separator, leading or interior.
        assert!(safe_join(base, "../escape.txt").is_err());
        assert!(safe_join(base, "a/b/../../../escape.txt").is_err());
        assert!(safe_join(base, "..\\escape.txt").is_err());
        // Absolute paths (POSIX root and UNC).
        assert!(safe_join(base, "/etc/cron.d/evil.cfg").is_err());
        assert!(safe_join(base, "\\\\server\\share\\evil").is_err());
        // Windows drive prefix.
        assert!(safe_join(base, "C:/Users/me/evil.lnk").is_err());
        // Nothing usable left.
        assert!(safe_join(base, "").is_err());
        assert!(safe_join(base, "/").is_err());
    }

    #[test]
    fn safe_join_accepts_legitimate_entries() {
        let base = Path::new("/tmp/vpkmerge-work");

        // A normal Source 2 asset path stays under base.
        let nested = safe_join(base, "materials/heroes/foo.vmat_c").unwrap();
        assert!(nested.starts_with(base));
        assert_eq!(
            nested.strip_prefix(base).unwrap(),
            Path::new("materials/heroes/foo.vmat_c")
        );

        // A root-level file is fine too.
        let root = safe_join(base, "readme.txt").unwrap();
        assert!(root.starts_with(base));
        assert_eq!(root.strip_prefix(base).unwrap(), Path::new("readme.txt"));

        // Redundant "." / doubled slashes collapse but stay contained.
        let messy = safe_join(base, "./a//b/c.txt").unwrap();
        assert_eq!(messy.strip_prefix(base).unwrap(), Path::new("a/b/c.txt"));
    }

    fn make_vpk(path: &Path, files: &[(&str, &[u8])]) -> Result<()> {
        let src = tempdir()?;
        for (rel, bytes) in files {
            let p = src.path().join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(p, bytes)?;
        }
        let vpk = valve_pak::from_directory(src.path())?;
        vpk.save(path)?;
        Ok(())
    }

    fn read_entry(vpk_path: &Path, entry: &str) -> Result<Vec<u8>> {
        let vpk = valve_pak::open(vpk_path)?;
        let mut f = vpk.get_file(entry)?;
        f.read_all()
    }

    struct Fixture {
        dir: TempDir,
        a: PathBuf,
        b: PathBuf,
        out: PathBuf,
    }

    fn two_inputs() -> Result<Fixture> {
        let tmp = tempdir()?;
        let a = tmp.path().join("a_dir.vpk");
        let b = tmp.path().join("b_dir.vpk");
        let out = tmp.path().join("merged_dir.vpk");
        make_vpk(
            &a,
            &[
                ("only_a/file.txt", b"a-only"),
                ("shared/file.txt", b"from a"),
            ],
        )?;
        make_vpk(
            &b,
            &[
                ("only_b/file.txt", b"b-only"),
                ("shared/file.txt", b"from b"),
            ],
        )?;
        Ok(Fixture {
            dir: tmp,
            a,
            b,
            out,
        })
    }

    #[test]
    fn last_wins() -> Result<()> {
        let fx = two_inputs()?;
        let report = merge(&[&fx.a, &fx.b], &fx.out, &MergeOptions::default())?;
        assert_eq!(report.total_entries, 3);
        assert_eq!(report.overridden_paths, 1);
        assert_eq!(report.inputs, 2);
        assert_eq!(read_entry(&fx.out, "shared/file.txt")?, b"from b");
        assert_eq!(read_entry(&fx.out, "only_a/file.txt")?, b"a-only");
        assert_eq!(read_entry(&fx.out, "only_b/file.txt")?, b"b-only");
        Ok(())
    }

    #[test]
    fn first_wins() -> Result<()> {
        let fx = two_inputs()?;
        let opts = MergeOptions {
            collision_policy: CollisionPolicy::FirstWins,
            ..Default::default()
        };
        merge(&[&fx.a, &fx.b], &fx.out, &opts)?;
        assert_eq!(read_entry(&fx.out, "shared/file.txt")?, b"from a");
        Ok(())
    }

    #[test]
    fn error_policy_rejects_conflicts() -> Result<()> {
        let fx = two_inputs()?;
        let opts = MergeOptions {
            collision_policy: CollisionPolicy::Error,
            ..Default::default()
        };
        let result = merge(&[&fx.a, &fx.b], &fx.out, &opts);
        assert!(result.is_err(), "expected error policy to reject");
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("conflict"), "msg = {msg}");
        Ok(())
    }

    #[test]
    fn override_beats_policy() -> Result<()> {
        // LastWins would pick B; override forces A.
        let fx = two_inputs()?;
        let mut overrides = HashMap::new();
        overrides.insert("shared/file.txt".to_string(), 0);
        let opts = MergeOptions {
            collision_policy: CollisionPolicy::LastWins,
            overrides,
        };
        merge(&[&fx.a, &fx.b], &fx.out, &opts)?;
        assert_eq!(read_entry(&fx.out, "shared/file.txt")?, b"from a");
        Ok(())
    }

    #[test]
    fn override_resolves_strict_conflict() -> Result<()> {
        // Error policy normally rejects; override resolves the only conflict.
        let fx = two_inputs()?;
        let mut overrides = HashMap::new();
        overrides.insert("shared/file.txt".to_string(), 0);
        let opts = MergeOptions {
            collision_policy: CollisionPolicy::Error,
            overrides,
        };
        merge(&[&fx.a, &fx.b], &fx.out, &opts)?;
        assert_eq!(read_entry(&fx.out, "shared/file.txt")?, b"from a");
        Ok(())
    }

    #[test]
    fn override_pointing_at_non_owner_errors() -> Result<()> {
        // only_a/file.txt is in input 0; pointing the override at input 1 must fail.
        let fx = two_inputs()?;
        let mut overrides = HashMap::new();
        overrides.insert("only_a/file.txt".to_string(), 1);
        let opts = MergeOptions {
            overrides,
            ..Default::default()
        };
        let err = merge(&[&fx.a, &fx.b], &fx.out, &opts).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("does not contain"), "msg = {msg}");
        Ok(())
    }

    #[test]
    fn override_for_nonexistent_path_errors() -> Result<()> {
        let fx = two_inputs()?;
        let mut overrides = HashMap::new();
        overrides.insert("nope/missing.txt".to_string(), 0);
        let opts = MergeOptions {
            overrides,
            ..Default::default()
        };
        let err = merge(&[&fx.a, &fx.b], &fx.out, &opts).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("does not exist"), "msg = {msg}");
        Ok(())
    }

    #[test]
    fn inspect_reports_size() -> Result<()> {
        let tmp = tempdir()?;
        let p = tmp.path().join("x_dir.vpk");
        make_vpk(&p, &[("a.txt", b"hi")])?;
        let info = inspect(&p)?;
        assert!(info.size_bytes > 0, "size_bytes should be populated");
        assert_eq!(info.size_bytes, std::fs::metadata(&p)?.len());
        Ok(())
    }

    #[test]
    fn detect_conflicts_reports_overlap() -> Result<()> {
        let fx = two_inputs()?;
        let conflicts = detect_conflicts(&[&fx.a, &fx.b])?;
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].path, "shared/file.txt");
        assert_eq!(conflicts[0].owner_indices, vec![0, 1]);
        Ok(())
    }

    #[test]
    fn rejects_too_few_inputs() {
        let tmp = tempdir().unwrap();
        let out = tmp.path().join("out_dir.vpk");
        let err = merge::<&Path, _>(&[], &out, &MergeOptions::default()).unwrap_err();
        assert!(format!("{err:#}").contains("at least 2"));
    }

    #[test]
    fn rejects_output_equals_input() -> Result<()> {
        let fx = two_inputs()?;
        // Try to merge into one of the inputs.
        let err = merge(&[&fx.a, &fx.b], &fx.a, &MergeOptions::default()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("equals input"), "msg = {msg}");
        Ok(())
    }

    #[test]
    fn creates_missing_parent_dir() -> Result<()> {
        let fx = two_inputs()?;
        let nested = fx.dir.path().join("does/not/exist/yet/out_dir.vpk");
        merge(&[&fx.a, &fx.b], &nested, &MergeOptions::default())?;
        assert!(nested.exists());
        Ok(())
    }

    fn make_abilities_fixture(path: &Path) -> Result<()> {
        make_vpk(
            path,
            &[
                ("sounds/abilities/abrams/a2_charge/x.vsnd_c", b"a2x"),
                ("sounds/abilities/abrams/a2_charge/y.vsnd_c", b"a2y"),
                ("sounds/abilities/abrams/a4_leap/z.vsnd_c", b"a4z"),
                ("other/unrelated.txt", b"misc"),
            ],
        )
    }

    fn prefix_output(path: PathBuf, prefixes: &[&str]) -> SplitOutput {
        SplitOutput {
            path,
            predicate: PathPredicate::AnyPrefix(
                prefixes.iter().map(|s| (*s).to_string()).collect(),
            ),
        }
    }

    fn entry_set(vpk_path: &Path) -> Result<Vec<String>> {
        let mut paths: Vec<String> = valve_pak::open(vpk_path)?.file_paths().cloned().collect();
        paths.sort();
        Ok(paths)
    }

    #[test]
    fn split_routes_by_prefix() -> Result<()> {
        let tmp = tempdir()?;
        let input = tmp.path().join("in_dir.vpk");
        make_abilities_fixture(&input)?;
        let out_a2 = tmp.path().join("a2_dir.vpk");
        let out_a4 = tmp.path().join("a4_dir.vpk");
        let outputs = vec![
            prefix_output(out_a2.clone(), &["sounds/abilities/abrams/a2_"]),
            prefix_output(out_a4.clone(), &["sounds/abilities/abrams/a4_"]),
        ];
        let report = split(&input, &outputs, &SplitOptions::default())?;
        assert_eq!(report.input_entries, 4);
        assert_eq!(report.outputs[0].entries, 2);
        assert_eq!(report.outputs[1].entries, 1);
        assert_eq!(report.unmatched, 1);
        assert!(report.residual.is_none());
        assert_eq!(
            entry_set(&out_a2)?,
            vec![
                "sounds/abilities/abrams/a2_charge/x.vsnd_c".to_string(),
                "sounds/abilities/abrams/a2_charge/y.vsnd_c".to_string(),
            ]
        );
        assert_eq!(
            entry_set(&out_a4)?,
            vec!["sounds/abilities/abrams/a4_leap/z.vsnd_c".to_string()]
        );
        Ok(())
    }

    #[test]
    fn split_residual_collects_unmatched() -> Result<()> {
        let tmp = tempdir()?;
        let input = tmp.path().join("in_dir.vpk");
        make_abilities_fixture(&input)?;
        let out_a2 = tmp.path().join("a2_dir.vpk");
        let residual = tmp.path().join("residual_dir.vpk");
        let outputs = vec![prefix_output(out_a2, &["sounds/abilities/abrams/a2_"])];
        let opts = SplitOptions {
            residual_path: Some(residual.clone()),
            ..Default::default()
        };
        let report = split(&input, &outputs, &opts)?;
        assert_eq!(report.unmatched, 2);
        let res = report.residual.as_ref().expect("residual report");
        assert_eq!(res.entries, 2);
        assert_eq!(
            entry_set(&residual)?,
            vec![
                "other/unrelated.txt".to_string(),
                "sounds/abilities/abrams/a4_leap/z.vsnd_c".to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn split_no_residual_drops_unmatched() -> Result<()> {
        let tmp = tempdir()?;
        let input = tmp.path().join("in_dir.vpk");
        make_abilities_fixture(&input)?;
        let out_a2 = tmp.path().join("a2_dir.vpk");
        let outputs = vec![prefix_output(out_a2, &["sounds/abilities/abrams/a2_"])];
        let report = split(&input, &outputs, &SplitOptions::default())?;
        assert_eq!(report.unmatched, 2);
        assert!(report.residual.is_none());
        Ok(())
    }

    #[test]
    fn split_empty_output_still_writes_vpk() -> Result<()> {
        let tmp = tempdir()?;
        let input = tmp.path().join("in_dir.vpk");
        make_abilities_fixture(&input)?;
        let empty = tmp.path().join("empty_dir.vpk");
        let outputs = vec![prefix_output(empty.clone(), &["no/such/prefix/"])];
        let report = split(&input, &outputs, &SplitOptions::default())?;
        assert_eq!(report.outputs[0].entries, 0);
        assert!(
            empty.exists(),
            "empty bucket should still produce a VPK file"
        );
        let paths: Vec<String> = valve_pak::open(&empty)?.file_paths().cloned().collect();
        assert!(paths.is_empty());
        Ok(())
    }

    #[test]
    fn split_first_match_wins() -> Result<()> {
        let tmp = tempdir()?;
        let input = tmp.path().join("in_dir.vpk");
        make_abilities_fixture(&input)?;
        let earlier = tmp.path().join("earlier_dir.vpk");
        let later = tmp.path().join("later_dir.vpk");
        // Both predicates match every "sounds/abilities/abrams/a2_charge/*" entry.
        let outputs = vec![
            prefix_output(earlier.clone(), &["sounds/abilities/abrams/a2_charge/"]),
            prefix_output(later.clone(), &["sounds/abilities/abrams/a2_"]),
        ];
        let report = split(&input, &outputs, &SplitOptions::default())?;
        assert_eq!(report.outputs[0].entries, 2);
        assert_eq!(report.outputs[1].entries, 0);
        assert!(entry_set(&later)?.is_empty());
        Ok(())
    }

    #[test]
    fn split_all_matches_duplicates() -> Result<()> {
        let tmp = tempdir()?;
        let input = tmp.path().join("in_dir.vpk");
        make_abilities_fixture(&input)?;
        let earlier = tmp.path().join("earlier_dir.vpk");
        let later = tmp.path().join("later_dir.vpk");
        let outputs = vec![
            prefix_output(earlier.clone(), &["sounds/abilities/abrams/a2_charge/"]),
            prefix_output(later.clone(), &["sounds/abilities/abrams/a2_"]),
        ];
        let opts = SplitOptions {
            overlap_policy: OverlapPolicy::AllMatches,
            ..Default::default()
        };
        let report = split(&input, &outputs, &opts)?;
        assert_eq!(report.outputs[0].entries, 2);
        assert_eq!(report.outputs[1].entries, 2);
        assert_eq!(entry_set(&earlier)?, entry_set(&later)?);
        Ok(())
    }

    #[test]
    fn split_error_policy_rejects_overlap() -> Result<()> {
        let tmp = tempdir()?;
        let input = tmp.path().join("in_dir.vpk");
        make_abilities_fixture(&input)?;
        let outputs = vec![
            prefix_output(
                tmp.path().join("earlier_dir.vpk"),
                &["sounds/abilities/abrams/a2_charge/"],
            ),
            prefix_output(
                tmp.path().join("later_dir.vpk"),
                &["sounds/abilities/abrams/a2_"],
            ),
        ];
        let opts = SplitOptions {
            overlap_policy: OverlapPolicy::Error,
            ..Default::default()
        };
        let err = split(&input, &outputs, &opts).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("sounds/abilities/abrams/a2_charge/"),
            "msg = {msg}"
        );
        Ok(())
    }

    #[test]
    fn split_rejects_output_equals_input() -> Result<()> {
        let tmp = tempdir()?;
        let input = tmp.path().join("in_dir.vpk");
        make_abilities_fixture(&input)?;
        let outputs = vec![prefix_output(input.clone(), &["sounds/"])];
        let err = split(&input, &outputs, &SplitOptions::default()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("equals input"), "msg = {msg}");
        Ok(())
    }

    #[test]
    fn split_creates_missing_parent_dirs() -> Result<()> {
        let tmp = tempdir()?;
        let input = tmp.path().join("in_dir.vpk");
        make_abilities_fixture(&input)?;
        let nested = tmp.path().join("does/not/exist/a2_dir.vpk");
        let outputs = vec![prefix_output(
            nested.clone(),
            &["sounds/abilities/abrams/a2_"],
        )];
        split(&input, &outputs, &SplitOptions::default())?;
        assert!(nested.exists());
        Ok(())
    }

    #[test]
    fn stable_split_then_merge() -> Result<()> {
        let tmp = tempdir()?;
        let input = tmp.path().join("in_dir.vpk");
        make_abilities_fixture(&input)?;
        let a2 = tmp.path().join("a2_dir.vpk");
        let a4 = tmp.path().join("a4_dir.vpk");
        let residual = tmp.path().join("residual_dir.vpk");
        let outputs = vec![
            prefix_output(a2.clone(), &["sounds/abilities/abrams/a2_"]),
            prefix_output(a4.clone(), &["sounds/abilities/abrams/a4_"]),
        ];
        let opts = SplitOptions {
            residual_path: Some(residual.clone()),
            ..Default::default()
        };
        split(&input, &outputs, &opts)?;

        let recombined = tmp.path().join("recombined_dir.vpk");
        merge(
            &[&a2, &a4, &residual],
            &recombined,
            &MergeOptions::default(),
        )?;
        assert_eq!(entry_set(&input)?, entry_set(&recombined)?);
        Ok(())
    }

    #[test]
    fn pack_writes_single_entry_at_path() -> Result<()> {
        let tmp = tempdir()?;
        let out = tmp.path().join("packed_dir.vpk");
        pack(
            &[("soundevents/hero/gigawatt.vsndevts_c", b"encoded-bytes")],
            &out,
        )?;
        assert_eq!(
            entry_set(&out)?,
            vec!["soundevents/hero/gigawatt.vsndevts_c".to_string()]
        );
        assert_eq!(
            read_entry(&out, "soundevents/hero/gigawatt.vsndevts_c")?,
            b"encoded-bytes"
        );
        Ok(())
    }

    #[test]
    fn pack_creates_missing_parent_dir() -> Result<()> {
        let tmp = tempdir()?;
        let nested = tmp.path().join("does/not/exist/packed_dir.vpk");
        pack(&[("a/b.txt", b"x")], &nested)?;
        assert!(nested.exists());
        Ok(())
    }

    #[test]
    fn packed_vpk_merges_cleanly_when_disjoint() -> Result<()> {
        // A packed loose file enters the merge pipeline like any other VPK.
        let tmp = tempdir()?;
        let chunk = tmp.path().join("sndevts_chunk_dir.vpk");
        pack(
            &[("soundevents/hero/gigawatt.vsndevts_c", b"edited")],
            &chunk,
        )?;
        let other = tmp.path().join("other_dir.vpk");
        make_vpk(&other, &[("materials/foo.txt", b"foo")])?;
        let out = tmp.path().join("combined_dir.vpk");
        merge(&[&chunk, &other], &out, &MergeOptions::default())?;
        assert_eq!(
            entry_set(&out)?,
            vec![
                "materials/foo.txt".to_string(),
                "soundevents/hero/gigawatt.vsndevts_c".to_string(),
            ]
        );
        assert_eq!(
            read_entry(&out, "soundevents/hero/gigawatt.vsndevts_c")?,
            b"edited"
        );
        Ok(())
    }

    #[test]
    fn stable_entry_set() -> Result<()> {
        // Same inputs merged twice produce the same set of entries.
        // (Byte-exact order isn't guaranteed: valve_pak::from_directory walks
        // the filesystem in OS-dependent order. Set equality is what's
        // semantically required.)
        let fx = two_inputs()?;
        let out1 = fx.dir.path().join("m1_dir.vpk");
        let out2 = fx.dir.path().join("m2_dir.vpk");
        merge(&[&fx.a, &fx.b], &out1, &MergeOptions::default())?;
        merge(&[&fx.a, &fx.b], &out2, &MergeOptions::default())?;
        let mut p1: Vec<_> = valve_pak::open(&out1)?.file_paths().cloned().collect();
        let mut p2: Vec<_> = valve_pak::open(&out2)?.file_paths().cloned().collect();
        p1.sort();
        p2.sort();
        assert_eq!(p1, p2);
        Ok(())
    }
}
