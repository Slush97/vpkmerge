//! Model (`.vmdl_c`) orchestration: open a VPK, find compiled models, and hand
//! their bytes to `morphic` for decode. Mirrors [`crate::portrait`]. Today it
//! exposes [`inspect_models`] (a structural read); glTF export lands later
//! (see `docs/vmdl-glb-exporter.md`).

use anyhow::{Context, Result};
use std::path::Path;

pub use morphic::model::{
    BlockSummary, DrawCallInfo, ModelInfo, PrimitiveSelection, RemovedDrawCall, ReplacedMeshGroup,
    ReplacedMeshPart, SegmentBy, VertexTarget,
};

/// Default candidate clips for a bare `--pose`, in priority order. Menu-pose
/// naming is not uniform across Deadlock heroes (Vindicta uses `ui_hero_pose`,
/// Abrams/McGinnis/Haze use `hero_pose` / `hero_roster_pose` / `hero_roster_ready`),
/// so try the curated roster poses first, then fall back to progressively more
/// generic idles, so any hero bakes a sensible still.
pub const DEFAULT_POSE_CLIPS: [&str; 7] = [
    "ui_hero_pose",
    "hero_pose",
    "hero_roster_pose",
    "hero_roster_ready",
    "ui_hero_select",
    "idle_loadout",
    "primary_stand_idle",
];

/// Animation-clip selection for model export. By default every clip the model
/// carries is exported; these narrow that.
#[derive(Debug, Clone, Default)]
pub struct AnimOptions {
    /// Drop all animation clips (export the static mesh + skeleton only).
    pub no_anim: bool,
    /// When non-empty, keep only clips whose name appears here. Ignored if
    /// [`AnimOptions::no_anim`] is set (no-anim wins).
    pub clips: Vec<String>,
    /// Bake a single frame into the mesh as static geometry. When set it wins
    /// over [`AnimOptions::no_anim`] and [`AnimOptions::clips`]: the output is a
    /// plain posed mesh with no skeleton, skin, or clips.
    pub pose: Option<PoseSelection>,
}

/// A single-frame pose to bake into the mesh (see [`morphic::model::bake_pose`]).
#[derive(Debug, Clone, Default)]
pub struct PoseSelection {
    /// Candidate clip names in priority order; the first the model carries wins.
    /// Empty means use [`DEFAULT_POSE_CLIPS`].
    pub clips: Vec<String>,
    /// Frame index to sample (clamped to the clip's range).
    pub frame: usize,
    /// Error out instead of falling back to the static bind pose when no posed
    /// frame is reachable: neither the model nor its base-pak donor carries a
    /// candidate clip, and no loose NM pose clip resolves. Lets a caller tell
    /// "posed" from "would be an unposed bind/T-pose still" and fall back to a 2D
    /// portrait. WIP heroes (`models/heroes_wip/...`) embed no clips in the
    /// `.vmdl_c`; their menu pose is recovered from a loose NM clip
    /// (`clips/*.vnmclip_c`) when present, else they bind-pose.
    pub require: bool,
}

impl AnimOptions {
    /// Applies the selection to a decoded model's clip list in place.
    fn apply(&self, clips: &mut Vec<morphic::model::Clip>) {
        if self.no_anim {
            clips.clear();
        } else if !self.clips.is_empty() {
            clips.retain(|c| self.clips.iter().any(|w| w == &c.name));
        }
    }
}

/// Resolves compiled resource paths (`.vmat_c`, `.vtex_c`) across the open VPKs
/// in order: the skin VPK first, then the base `pak01_dir.vpk`. Skins embed
/// their geometry but reference materials/textures that may live in the base
/// pak, so the model exporter needs both. Implements [`morphic::model::FileResolver`]
/// to keep `morphic` free of VPK I/O.
struct VpkResolver {
    vpks: Vec<valve_pak::VPK>,
}

impl morphic::model::FileResolver for VpkResolver {
    fn resolve(&self, compiled_path: &str) -> Option<Vec<u8>> {
        for vpk in &self.vpks {
            if let Ok(mut vf) = vpk.get_file(compiled_path) {
                if let Ok(bytes) = vf.read_all() {
                    return Some(bytes);
                }
            }
        }
        None
    }
}

/// Decode a `.vmdl_c` at an explicit VPK `entry` path and write it as a textured
/// binary glTF.
///
/// `vpk` is searched first, then `base` (the base `pak01_dir.vpk`), for both the
/// model entry and its materials/textures. A mesh skin ships its own model in
/// `vpk`; a texture-only skin ships no model, so the entry is read from `base`
/// while the skin's overriding textures (in `vpk`) still win on resolution.
pub fn export_model(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
    anim: &AnimOptions,
    out: impl AsRef<Path>,
) -> Result<()> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    if read_entry(&vpks, entry).is_none() {
        anyhow::bail!("model entry {entry} not found in the given VPK(s)");
    }
    export_resolved(vpks, entry, anim, out.as_ref())
}

/// Like [`export_model`] but discovers the hero's body model by codename instead
/// of an explicit entry path: the first `.vmdl_c` under a `models/heroes*`
/// directory whose file name is exactly `<codename>.vmdl_c` (so `hornet.vmdl_c`,
/// not `hornet_backup.vmdl_c` or `hornet_lod1`, and not weapon/prop sub-meshes
/// like `bookworm_sword`). The given VPK is searched first (a mesh skin ships its
/// own model), then the base pak (texture-only skins reuse the base mesh).
pub fn export_hero_model(
    vpk: impl AsRef<Path>,
    codename: &str,
    base: Option<&Path>,
    anim: &AnimOptions,
    out: impl AsRef<Path>,
) -> Result<()> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let entry = discover_hero_entry(&vpks, codename).with_context(|| {
        format!("no body model (`<dir>/{codename}.vmdl_c` under models/heroes*) found in the given VPK(s)")
    })?;
    export_resolved(vpks, &entry, anim, out.as_ref())
}

/// Opens the VPKs in resolution priority order: `vpk` first (a skin's overrides
/// win), then the base pak.
fn open_vpks(vpk: &Path, base: Option<&Path>) -> Result<Vec<valve_pak::VPK>> {
    let mut vpks =
        vec![valve_pak::open(vpk).with_context(|| format!("opening {}", vpk.display()))?];
    if let Some(base) = base {
        vpks.push(valve_pak::open(base).with_context(|| format!("opening {}", base.display()))?);
    }
    Ok(vpks)
}

/// Reads `entry`, decodes it, textures it via the cross-VPK resolver, and writes
/// the `.glb`. Consumes `vpks` (they move into the resolver).
fn export_resolved(
    vpks: Vec<valve_pak::VPK>,
    entry: &str,
    anim: &AnimOptions,
    out: &Path,
) -> Result<()> {
    let bytes =
        read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;
    let mut model = morphic::model::decode(&bytes).with_context(|| format!("decoding {entry}"))?;

    if let Some(pose) = &anim.pose {
        let candidates: Vec<&str> = if pose.clips.is_empty() {
            DEFAULT_POSE_CLIPS.to_vec()
        } else {
            pose.clips.iter().map(String::as_str).collect()
        };
        // Skin mods ship the mesh + rig but no animation clips (those live in the
        // base game). When this model carries none of the candidate clips, source
        // them from the same entry in the base pak (vpks after the first) and map
        // by bone name. Same hero, same rig, so no cross-hero retargeting.
        let carries_clip = |m: &morphic::model::Model| {
            candidates
                .iter()
                .any(|c| m.animations.iter().any(|a| a.name.eq_ignore_ascii_case(c)))
        };
        let has_own_clip = carries_clip(&model);
        // Decode the base-pak donor once (only when needed): it supplies the menu
        // clip for a clipless skin, and lets --require-pose tell whether a real
        // pose is reachable before baking.
        let donor = if has_own_clip {
            None
        } else {
            read_entry(&vpks[1..], entry)
                .map(|b| {
                    morphic::model::decode(&b)
                        .with_context(|| format!("decoding base clips for {entry}"))
                })
                .transpose()?
        };
        let donor_has_clip = donor.as_ref().is_some_and(carries_clip);
        // Newer WIP heroes embed no clips and have no clip-carrying donor (their
        // base entry is the same clipless model): their menu/idle pose ships as a
        // loose NM clip (`clips/<name>.vnmclip_c` + `<h>.vnmskel_c`) behind an
        // animation graph. Resolve and bake that static single frame directly.
        // It wins over a clipless donor but never over a real embedded/donor clip.
        let nm_posed = if has_own_clip || donor_has_clip {
            None
        } else {
            bake_loose_nm_pose(&vpks, entry, &model, &candidates)
        };
        if pose.require {
            let has_clip = has_own_clip || donor_has_clip || nm_posed.is_some();
            let has_skeleton = !model.skeleton.bones.is_empty();
            if !has_skeleton || !has_clip {
                anyhow::bail!(
                    "{entry} carries no menu/idle pose clip ({}); only a static bind \
                     pose is available and --require-pose was set",
                    candidates.join(", ")
                );
            }
        }
        if let Some(pose_source) = if has_own_clip {
            Some(&model)
        } else if donor_has_clip {
            donor.as_ref()
        } else {
            None
        } {
            emit_secondary_motion_pose_warning(entry, &model, pose_source, &candidates);
        }

        model = if has_own_clip {
            morphic::model::bake_pose(&model, &candidates, pose.frame)
        } else if let Some(nm) = nm_posed {
            nm
        } else if let Some(donor) = &donor {
            morphic::model::bake_pose_from(&model, donor, &candidates, pose.frame)
        } else {
            morphic::model::bake_pose(&model, &candidates, pose.frame)
        };
    } else {
        anim.apply(&mut model.animations);
    }

    let resolver = VpkResolver { vpks };
    let glb = morphic::model::to_glb_textured(&model, &resolver)
        .with_context(|| format!("writing glb for {entry}"))?;

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(out, &glb).with_context(|| format!("writing {}", out.display()))?;
    Ok(())
}

fn emit_secondary_motion_pose_warning(
    entry: &str,
    model: &morphic::model::Model,
    pose_source: &morphic::model::Model,
    candidates: &[&str],
) {
    let Some(report) = morphic::model::secondary_motion_pose_report(model, pose_source, candidates)
    else {
        return;
    };
    if report.vertices_majority_root_secondary == 0 && report.animated_secondary_bone_count > 0 {
        return;
    }

    eprintln!(
        "warning: {entry} pose `{}` has unresolved secondary-motion geometry: {} vertices use cloth/hair-style bones ({} majority; {} root-secondary majority), while the clip animates {}/{} secondary bones",
        report.clip_name,
        report.vertices_with_secondary,
        report.vertices_majority_secondary,
        report.vertices_majority_root_secondary,
        report.animated_secondary_bone_count,
        report.secondary_bone_count,
    );
    for mat in report.materials.iter().take(3) {
        eprintln!(
            "  affected material: {} ({} skinned verts, {} secondary, {} root-secondary majority)",
            mat.material,
            mat.skinned_vertices,
            mat.vertices_with_secondary,
            mat.vertices_majority_root_secondary,
        );
    }
    if !report.top_bones.is_empty() {
        let bones = report
            .top_bones
            .iter()
            .take(5)
            .map(|b| {
                if b.is_root {
                    format!("{}:{:.1} root", b.bone, b.weight_sum)
                } else {
                    format!("{}:{:.1}", b.bone, b.weight_sum)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("  top secondary influences: {bones}");
    }
}

/// One enumerated animation clip, the fields a pose/snapshot authoring UI needs
/// to bound a frame slider, show a clip's length, and preselect a default. The
/// [`ClipSummary::name`] is usable verbatim as `model export --pose <name>` /
/// `--clip <name>`.
#[derive(Debug, Clone)]
pub struct ClipSummary {
    /// Clip name, usable directly in `--pose <name>` / `--clip <name>`.
    pub name: String,
    /// Number of keyframes (the upper bound for `--pose name@N`).
    pub frame_count: usize,
    /// Playback rate.
    pub fps: f32,
    /// Wall-clock length: `(frame_count - 1) / fps` (frame 0 at t=0), or `0.0`
    /// for a single-frame or rateless clip.
    pub duration_seconds: f32,
    /// Whether the engine loops this clip.
    pub looping: bool,
    /// True for the clip a bare `--pose` would bake (the first
    /// [`DEFAULT_POSE_CLIPS`] candidate this model carries). At most one clip in
    /// the list is flagged; none is when the model carries no candidate pose.
    pub default: bool,
}

/// Enumerate the animation clips a model carries at an explicit VPK `entry`, for
/// pose/clip discovery (the read-only companion to [`export_model`]'s
/// `--pose`/`--clip`). Resolution mirrors export: `vpk` first, then `base`. A
/// clipless mesh skin (ships the rig but no clips) falls back to the base-pak
/// donor's clips at the same entry. WIP heroes (`models/heroes_wip/...`) embed no
/// clips and have a clipless donor, so they return an empty vec (`Ok(vec![])`,
/// not an error) - the same models `--pose --require-pose` bails on.
pub fn model_clips(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
) -> Result<Vec<ClipSummary>> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    if read_entry(&vpks, entry).is_none() {
        anyhow::bail!("model entry {entry} not found in the given VPK(s)");
    }
    clips_resolved(&vpks, entry)
}

/// Like [`model_clips`] but discovers the hero's body model by `codename` instead
/// of an explicit entry path (same discovery as [`export_hero_model`]).
pub fn hero_model_clips(
    vpk: impl AsRef<Path>,
    codename: &str,
    base: Option<&Path>,
) -> Result<Vec<ClipSummary>> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let entry = discover_hero_entry(&vpks, codename).with_context(|| {
        format!("no body model (`<dir>/{codename}.vmdl_c` under models/heroes*) found in the given VPK(s)")
    })?;
    clips_resolved(&vpks, &entry)
}

/// Decodes `entry` and summarizes its clips, falling back to the base-pak donor
/// at the same entry when the primary resolution is a clipless mesh skin. Mirrors
/// the donor sourcing in [`export_resolved`]'s pose path.
fn clips_resolved(vpks: &[valve_pak::VPK], entry: &str) -> Result<Vec<ClipSummary>> {
    let bytes =
        read_entry(vpks, entry).with_context(|| format!("model entry {entry} not found"))?;
    let model = morphic::model::decode(&bytes).with_context(|| format!("decoding {entry}"))?;
    let mut anims = model.animations;
    // Clipless mesh skin: the rig is here but the clips live in the base pak at
    // the same entry (vpks after the first). Decode the donor only when needed.
    if anims.is_empty() {
        if let Some(donor_bytes) = read_entry(&vpks[1..], entry) {
            anims = morphic::model::decode(&donor_bytes)
                .with_context(|| format!("decoding base clips for {entry}"))?
                .animations;
        }
    }
    Ok(summarize_clips(&anims))
}

/// Projects decoded clips to [`ClipSummary`]s, flagging the default pose and
/// sorting by name for a stable, UI-friendly order.
#[allow(clippy::cast_precision_loss)] // frame counts are tiny (hundreds at most)
fn summarize_clips(anims: &[morphic::model::Clip]) -> Vec<ClipSummary> {
    // The bare-`--pose` winner: first DEFAULT_POSE_CLIPS candidate the model
    // carries (case-insensitive), matching export's resolution.
    let default_name = DEFAULT_POSE_CLIPS.iter().find_map(|cand| {
        anims
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(cand))
            .map(|a| a.name.clone())
    });
    let mut out: Vec<ClipSummary> = anims
        .iter()
        .map(|c| {
            let duration = if c.fps > 0.0 && c.frame_count > 1 {
                (c.frame_count - 1) as f32 / c.fps
            } else {
                0.0
            };
            ClipSummary {
                name: c.name.clone(),
                frame_count: c.frame_count,
                fps: c.fps,
                duration_seconds: duration,
                looping: c.looping,
                default: default_name.as_deref() == Some(c.name.as_str()),
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Highest `_vN` suffix across a path's segments (`inferno_v4` -> 4,
/// `hornet_v3` -> 3), or 0 when no segment is versioned. Picks the current model
/// dir when several versions ship side by side.
fn version_rank(path: &str) -> u32 {
    path.split('/')
        .filter_map(|seg| {
            seg.rsplit_once("_v")
                .and_then(|(_, n)| n.parse::<u32>().ok())
        })
        .max()
        .unwrap_or(0)
}

/// Finds the hero body model entry for `codename`. The body model's file name is
/// exactly `<codename>.vmdl_c` and it lives under a `models/heroes...` directory.
///
/// VPKs are searched in priority order, so a mesh skin's own model (`vpks[0]`)
/// wins over the base pak. Within one VPK a hero can ship several matching
/// `<codename>.vmdl_c` (e.g. an old `heroes_wip/inferno` beside the current
/// `inferno_v4`). `valve_pak::file_paths()` iterates a `HashMap` (random order),
/// so taking the first match picks an arbitrary version per run; instead pick
/// deterministically: prefer a non-`heroes_wip` dir, then the highest `_vN`,
/// then the lexicographically greatest path.
fn discover_hero_entry(vpks: &[valve_pak::VPK], codename: &str) -> Option<String> {
    let want = format!("{codename}.vmdl_c");
    for vpk in vpks {
        let mut matches: Vec<&String> = vpk
            .file_paths()
            .filter(|p| {
                p.rsplit('/').next().unwrap_or(p.as_str()) == want.as_str() && p.contains("/heroes")
            })
            .collect();
        if matches.is_empty() {
            continue;
        }
        let key = |p: &str| (!p.contains("/heroes_wip/"), version_rank(p));
        matches.sort_by(|a, b| {
            key(b.as_str())
                .cmp(&key(a.as_str()))
                .then_with(|| b.as_str().cmp(a.as_str()))
        });
        return matches.first().map(|p| (*p).clone());
    }
    None
}

/// A geometric transform applied to a model's editable vertex buffers (Tier-0
/// displacement: reshape existing geometry, topology unchanged). Scale is uniform
/// about each buffer's centroid; translation is applied after, in model space.
#[derive(Debug, Clone)]
pub struct GeometryEdit {
    /// Edit only the mesh part with this exact name (e.g. `body`, `gun`); `None`
    /// edits every displacement-editable buffer in the model.
    pub part: Option<String>,
    /// Uniform scale about each buffer's centroid. `1.0` leaves size unchanged.
    pub scale: f32,
    /// Translation in model space, applied after scaling.
    pub translate: [f32; 3],
}

impl Default for GeometryEdit {
    fn default() -> Self {
        Self {
            part: None,
            scale: 1.0,
            translate: [0.0; 3],
        }
    }
}

/// What an [`edit_model_geometry`] run touched.
#[derive(Debug, Clone)]
pub struct GeometryEditReport {
    pub entry: String,
    pub vpk_entry: String,
    /// Names of the mesh parts whose geometry was moved.
    pub edited_parts: Vec<String>,
    pub edited_buffers: usize,
    pub edited_vertices: usize,
}

/// Which VPK supplied a resolved model/material/texture entry.
#[derive(Debug, Clone)]
pub struct ResolvedResource {
    pub path: String,
    pub compiled_path: String,
    /// `mod`, `base`, or `input` when no separate base pak was supplied.
    pub source: String,
}

/// A resolved material texture parameter.
#[derive(Debug, Clone)]
pub struct ResolvedTextureParam {
    pub slot: String,
    pub path: String,
    pub compiled_path: String,
    pub source: Option<String>,
}

/// Bone/skin summary for one draw call.
#[derive(Debug, Clone, Default)]
pub struct DrawCallSkinInfo {
    pub skinned: bool,
    pub bone_weight_count: usize,
    pub used_bone_count: usize,
    pub used_bones: Vec<String>,
}

/// Machine-readable model draw-call inspection record.
#[derive(Debug, Clone)]
pub struct ModelDrawCallInspection {
    pub id: String,
    pub mesh_name: String,
    pub mesh_index: usize,
    pub primitive_index: usize,
    pub data_block: usize,
    pub scene_object: usize,
    pub draw_call: usize,
    pub vertex_buffers: Vec<usize>,
    pub vertex_buffer: usize,
    pub index_buffer: usize,
    pub vertex_blocks: Vec<usize>,
    pub vertex_block: usize,
    pub index_block: usize,
    pub material: String,
    pub material_source: Option<String>,
    pub textures: Vec<ResolvedTextureParam>,
    pub vertex_count: usize,
    pub index_count: usize,
    pub start_index: usize,
    pub base_vertex: u32,
    pub primitive_type: String,
    pub geometry_source: String,
    pub skin: DrawCallSkinInfo,
}

/// A heuristic semantic group Grimoire can display as one candidate card.
#[derive(Debug, Clone)]
pub struct SuggestedPartGroup {
    pub name: String,
    pub label: String,
    pub aliases: Vec<String>,
    pub draw_call_ids: Vec<String>,
    pub mesh_names: Vec<String>,
    pub materials: Vec<String>,
    pub vertex_count: usize,
    pub index_count: usize,
    pub confidence: f32,
}

/// Full model-part inspection payload for JSON consumers.
#[derive(Debug, Clone)]
pub struct ModelPartInspection {
    pub entry: String,
    pub model_source: ResolvedResource,
    pub draw_calls: Vec<ModelDrawCallInspection>,
    pub suggested_groups: Vec<SuggestedPartGroup>,
}

/// Selector shared by grouped export/replacement.
#[derive(Debug, Clone, Default)]
pub struct ModelPartSelector {
    pub group: Option<String>,
    pub materials: Vec<String>,
    pub mesh_parts: Vec<String>,
}

/// One UV region surfaced by `vpkmerge model mask`: its picking id, label, the
/// mesh part it came from, triangle count, UV footprint, and atlas swatch color.
#[derive(Debug, Clone)]
pub struct UvSegmentInfo {
    pub id: usize,
    pub label: String,
    pub mesh: String,
    pub triangles: usize,
    /// Fraction of the texture this region actually covers (`0..1`), measured by
    /// rasterizing at the analysis resolution. Bounded and honest, unlike summed
    /// UV area which tiling/mirrored hero UVs inflate past 1.
    pub coverage: f32,
    /// Summed UV-space triangle area (may exceed 1 with tiling/overlap).
    pub uv_area: f32,
    /// `[min_u, min_v, max_u, max_v]`.
    pub uv_bounds: [f32; 4],
    /// RGB of this region's atlas color, so a legend can show the swatch.
    pub color: [u8; 3],
}

/// Resolves a hero codename to its body model entry (`<dir>/<codename>.vmdl_c`),
/// searching `--vpk` then the optional base pak. Mirrors `export_hero_model`'s
/// discovery so `model mask --hero` accepts the same names as `model export`.
pub fn hero_model_entry(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    codename: &str,
) -> Result<String> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    discover_hero_entry(&vpks, codename)
        .with_context(|| format!("no hero model '{codename}.vmdl_c' found in the given VPK(s)"))
}

fn decode_model_entry(
    vpk: &Path,
    entry: &str,
    base: Option<&Path>,
) -> Result<morphic::model::Model> {
    let vpks = open_vpks(vpk, base)?;
    let bytes =
        read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;
    morphic::model::decode(&bytes).with_context(|| format!("decoding {entry}"))
}

/// Builds the listing rows for a set of segments: measures per-segment texel
/// coverage at `resolution`, then sorts the rows largest-coverage-first (the
/// region a reskinner most likely wants on top). The stable `id`/color follow
/// the segment, so list order can differ from id order.
fn segment_infos(segs: &[morphic::model::Segment], resolution: u32) -> Vec<UvSegmentInfo> {
    let coverage = morphic::model::segment_coverage(segs, resolution);
    let mut infos: Vec<UvSegmentInfo> = segs
        .iter()
        .zip(coverage)
        .map(|(s, cov)| UvSegmentInfo {
            id: s.id,
            label: s.label.clone(),
            mesh: s.mesh.clone(),
            triangles: s.triangle_count(),
            coverage: cov,
            uv_area: s.uv_area(),
            uv_bounds: s.uv_bounds(),
            color: morphic::model::segment_color(s.id),
        })
        .collect();
    infos.sort_by(|a, b| {
        b.coverage
            .partial_cmp(&a.coverage)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    infos
}

fn write_png_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Lists a model's UV regions under the chosen segmentation scheme
/// ([`SegmentBy::Part`], [`SegmentBy::Material`], or [`SegmentBy::Island`]),
/// sorted largest-first. Backs `model mask --list` and the atlas legend.
pub fn model_uv_segments(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
    by: SegmentBy,
    part: Option<&str>,
    resolution: u32,
) -> Result<Vec<UvSegmentInfo>> {
    let model = decode_model_entry(vpk.as_ref(), entry, base)?;
    let segs = morphic::model::segments(&model, by, part);
    Ok(segment_infos(&segs, resolution))
}

/// Bakes a distinct-hue atlas PNG (one color per UV region, for picking by eye)
/// to `out_png` and returns the region metadata so the caller can print a legend.
pub fn bake_uv_atlas(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
    by: SegmentBy,
    part: Option<&str>,
    resolution: u32,
    out_png: impl AsRef<Path>,
) -> Result<Vec<UvSegmentInfo>> {
    let model = decode_model_entry(vpk.as_ref(), entry, base)?;
    let segs = morphic::model::segments(&model, by, part);
    let png = morphic::model::atlas_png(&segs, resolution).context("rendering UV atlas")?;
    write_png_file(out_png.as_ref(), &png)?;
    Ok(segment_infos(&segs, resolution))
}

/// Bakes a white-on-black mask PNG of the `selected` region ids to `out_png` (the
/// region selector the reskin builders consume in place of the AO heuristic).
/// Returns the labels of the baked regions; errors if any id is out of range.
#[allow(clippy::too_many_arguments)]
pub fn bake_uv_mask(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
    by: SegmentBy,
    part: Option<&str>,
    selected: &[usize],
    resolution: u32,
    out_png: impl AsRef<Path>,
) -> Result<Vec<String>> {
    let model = decode_model_entry(vpk.as_ref(), entry, base)?;
    let segs = morphic::model::segments(&model, by, part);
    for &id in selected {
        if id >= segs.len() {
            anyhow::bail!(
                "segment id {id} out of range: model has {} segment(s) (run --list)",
                segs.len()
            );
        }
    }
    let png = morphic::model::mask_png(&segs, selected, resolution).context("rendering UV mask")?;
    write_png_file(out_png.as_ref(), &png)?;
    Ok(selected.iter().map(|&i| segs[i].label.clone()).collect())
}

/// Lists the vertex buffers in a model entry (which mesh parts can be edited, and
/// the block index / vertex count of each). The CLI uses this for `--list` and to
/// resolve a `--part` name to a block index.
pub fn model_vertex_targets(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
) -> Result<Vec<VertexTarget>> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let bytes =
        read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;
    morphic::model::vertex_targets(&bytes).with_context(|| format!("reading targets for {entry}"))
}

/// Machine-readable draw-call/material/group inspection for `model edit
/// --list-drawcalls --json`.
pub fn inspect_model_parts(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
) -> Result<ModelPartInspection> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let has_base = base.is_some();
    let (bytes, source_index) = read_entry_with_source(&vpks, entry)
        .with_context(|| format!("model entry {entry} not found"))?;
    let model = morphic::model::decode(&bytes).with_context(|| format!("decoding {entry}"))?;
    let calls = morphic::model::draw_call_targets(&bytes)
        .with_context(|| format!("reading draw calls for {entry}"))?;
    let geometry_source = source_label(source_index, has_base);

    let mut draw_calls = Vec::with_capacity(calls.len());
    for c in calls {
        let (material_source, textures) = inspect_material_refs(&vpks, has_base, &c.material);
        let skin = skin_info_for(&model, &c);
        draw_calls.push(ModelDrawCallInspection {
            id: draw_call_id(&c),
            mesh_name: c.mesh_name,
            mesh_index: c.mesh_index,
            primitive_index: c.primitive_index,
            data_block: c.data_block,
            scene_object: c.scene_object,
            draw_call: c.draw_call,
            vertex_buffers: c.vertex_buffers,
            vertex_buffer: c.vertex_buffer,
            index_buffer: c.index_buffer,
            vertex_blocks: c.vertex_blocks,
            vertex_block: c.vertex_block,
            index_block: c.index_block,
            material: c.material,
            material_source,
            textures,
            vertex_count: c.vertex_count,
            index_count: c.index_count,
            start_index: c.start_index,
            base_vertex: c.base_vertex,
            primitive_type: c.primitive_type,
            geometry_source: geometry_source.clone(),
            skin,
        });
    }
    let suggested_groups = suggest_part_groups(&draw_calls);

    Ok(ModelPartInspection {
        entry: entry.to_string(),
        model_source: ResolvedResource {
            path: entry.to_string(),
            compiled_path: entry.to_string(),
            source: geometry_source,
        },
        draw_calls,
        suggested_groups,
    })
}

/// Exports selected draw calls as an isolated textured GLB. The model skeleton is
/// retained so skinned parts still have valid `JOINTS_0`; animations are dropped
/// to keep preview assets small.
pub fn export_model_group_glb(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
    selector: &ModelPartSelector,
    out_glb: impl AsRef<Path>,
) -> Result<usize> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let bytes =
        read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;
    let mut model = morphic::model::decode(&bytes).with_context(|| format!("decoding {entry}"))?;
    let calls = inspect_model_parts(vpk, entry, base)?.draw_calls;
    let selected = select_draw_calls(&calls, selector)?;
    let selected_keys: std::collections::BTreeSet<(usize, usize)> = selected
        .iter()
        .map(|c| (c.mesh_index, c.primitive_index))
        .collect();
    let selected_count = selected_keys.len();
    filter_model_to_primitives(&mut model, &selected_keys);
    model.animations.clear();
    if model.meshes.is_empty() {
        anyhow::bail!("group selector matched no exportable geometry");
    }

    let resolver = VpkResolver { vpks };
    let glb = morphic::model::to_glb_textured(&model, &resolver)
        .with_context(|| format!("writing group glb for {entry}"))?;
    let out = out_glb.as_ref();
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(out, &glb).with_context(|| format!("writing {}", out.display()))?;
    Ok(selected_count)
}

/// Replaces the selected semantic group from a donor GLB and packs an addon VPK.
pub fn replace_model_group(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
    selector: &ModelPartSelector,
    glb_bytes: &[u8],
    out_vpk: impl AsRef<Path>,
    vpk_entry: Option<&str>,
) -> Result<(String, ReplacedMeshGroup)> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let bytes =
        read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;
    let calls = inspect_model_parts(vpk, entry, base)?.draw_calls;
    let selected = select_draw_calls(&calls, selector)?;
    let selections: Vec<PrimitiveSelection> = selected
        .iter()
        .map(|c| PrimitiveSelection {
            mesh_index: c.mesh_index,
            primitive_index: c.primitive_index,
        })
        .collect();
    let donors = morphic::model::read_edited_primitives(glb_bytes)
        .with_context(|| "reading donor primitives from glb".to_string())?;
    let (edited, report) = morphic::model::replace_mesh_group(&bytes, &selections, &donors)
        .with_context(|| format!("replacing selected group in {entry}"))?;

    let vpk_entry = vpk_entry.unwrap_or(entry).to_string();
    crate::pack(&[(vpk_entry.as_str(), edited.as_slice())], out_vpk.as_ref())
        .with_context(|| format!("packing edited model into {}", out_vpk.as_ref().display()))?;
    Ok((vpk_entry, report))
}

/// Applies a geometric transform to a model's editable vertex buffers and packs
/// the edited `.vmdl_c` into a standalone addon VPK at `vpk_entry` (defaulting to
/// `entry`, so it overrides the base pak). The geometry analog of
/// `soundevents --encode-vpk`: read from a VPK, edit, re-encode, pack.
///
/// Note: the model's stored bounds (`MDAT` per-mesh AABB) are not recomputed, so
/// large transforms can drift culling bounds; small reshapes are unaffected.
pub fn edit_model_geometry(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
    edit: &GeometryEdit,
    out_vpk: impl AsRef<Path>,
    vpk_entry: Option<&str>,
) -> Result<GeometryEditReport> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let mut bytes =
        read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;

    let targets = morphic::model::vertex_targets(&bytes)
        .with_context(|| format!("reading vertex targets for {entry}"))?;

    // Select the buffers to edit: editable, and matching --part if given.
    let selected: Vec<&VertexTarget> = targets
        .iter()
        .filter(|t| t.editable)
        .filter(|t| edit.part.as_deref().is_none_or(|p| t.mesh_name == p))
        .collect();

    if let Some(part) = &edit.part {
        if !targets.iter().any(|t| &t.mesh_name == part) {
            anyhow::bail!(
                "no mesh part named {part:?} in {entry} (parts: {})",
                part_list(&targets)
            );
        }
    }
    if selected.is_empty() {
        anyhow::bail!(
            "no displacement-editable vertex buffer to edit in {entry} \
             (need meshopt-compressed geometry with a float POSITION)"
        );
    }

    // Read every selected buffer's positions first, then scale them all about a
    // single shared centroid. Scaling each buffer about its own centroid would
    // pull a multi-buffer part (hornet's body is two buffers) or a whole-model
    // edit apart; one shared centroid keeps the selection rigid as it grows.
    let mut buffers: Vec<(usize, &str, Vec<[f32; 3]>)> = Vec::with_capacity(selected.len());
    for t in &selected {
        let positions = morphic::model::read_vertex_positions(&bytes, t.block_index)
            .with_context(|| format!("reading positions for {} buffer", t.mesh_name))?;
        buffers.push((t.block_index, t.mesh_name.as_str(), positions));
    }
    let centroid = shared_centroid(buffers.iter().map(|(_, _, p)| p.as_slice()));

    let mut edited_parts: Vec<String> = Vec::new();
    let mut edited_vertices = 0usize;
    let edited_buffers = buffers.len();
    for (block_index, mesh_name, positions) in &buffers {
        let moved = transform_about(positions, centroid, edit);
        bytes = morphic::model::replace_vertex_positions(&bytes, *block_index, &moved)
            .with_context(|| format!("splicing {mesh_name} buffer"))?;
        edited_vertices += positions.len();
        if !edited_parts.iter().any(|n| n == mesh_name) {
            edited_parts.push((*mesh_name).to_string());
        }
    }

    let vpk_entry = vpk_entry.unwrap_or(entry).to_string();
    crate::pack(&[(vpk_entry.as_str(), bytes.as_slice())], out_vpk.as_ref())
        .with_context(|| format!("packing edited model into {}", out_vpk.as_ref().display()))?;

    Ok(GeometryEditReport {
        entry: entry.to_string(),
        vpk_entry,
        edited_parts,
        edited_buffers,
        edited_vertices,
    })
}

/// One model recolored by [`recolor_models_to_addon`].
#[derive(Debug, Clone)]
pub struct ModelRecolorEntry {
    pub entry: String,
    pub stats: crate::recolor::ModelRecolorStats,
}

/// Recolor the baked per-vertex colors of one or more model entries (read from
/// `vpk`, then `base`) to `recolor`, packing them all into one addon VPK at
/// `out`, each at its own entry path so it overrides the base pak in place. The
/// model analog of the multi-entry `vpkmerge texture` recolor (the third VFX
/// recolor mechanism, alongside particle params and texture hue).
///
/// Returns a per-entry report. Errors if an entry is missing, or carries no
/// color-bearing vertex buffer (a likely wrong-model mistake) so a silent
/// no-op addon is never written.
pub fn recolor_models_to_addon(
    vpk: impl AsRef<Path>,
    entries: &[String],
    base: Option<&Path>,
    recolor: crate::recolor::Recolor,
    out: impl AsRef<Path>,
) -> Result<Vec<ModelRecolorEntry>> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let mut packed: Vec<(String, Vec<u8>)> = Vec::with_capacity(entries.len());
    let mut report = Vec::with_capacity(entries.len());
    for entry in entries {
        let bytes =
            read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;
        let (recolored, stats) = crate::recolor::recolor_model_vertex_colors(&bytes, recolor)
            .with_context(|| format!("recoloring {entry}"))?;
        if stats.buffers_recolored == 0 {
            anyhow::bail!("{entry} has no color-bearing vertex buffer to recolor (wrong model?)");
        }
        packed.push((entry.clone(), recolored));
        report.push(ModelRecolorEntry {
            entry: entry.clone(),
            stats,
        });
    }
    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    crate::pack(&refs, out.as_ref())
        .with_context(|| format!("packing recolored models into {}", out.as_ref().display()))?;
    Ok(report)
}

/// Exports one vertex buffer (by global block index) of a model to a `.glb` for
/// reshaping in Blender: a single triangle mesh carrying a `_ORIGID` per-vertex
/// attribute that maps the edit back to the original buffer (see
/// [`apply_model_edit_glb`]). The model is read from `vpk` (then `base`).
pub fn export_model_buffer_glb(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
    block_index: usize,
    out_glb: impl AsRef<Path>,
) -> Result<()> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let bytes =
        read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;
    let glb = morphic::model::export_buffer_for_edit(&bytes, block_index)
        .with_context(|| format!("exporting buffer at block {block_index} of {entry}"))?;
    let out = out_glb.as_ref();
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(out, &glb).with_context(|| format!("writing {}", out.display()))?;
    Ok(())
}

/// Applies a Blender-reshaped `.glb` (from [`export_model_buffer_glb`]) back onto
/// a model: maps the edited positions to the original buffer via `_ORIGID`,
/// splices, and packs a standalone addon VPK at `vpk_entry` (default `entry`).
pub fn apply_model_edit_glb(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
    block_index: usize,
    glb_bytes: &[u8],
    out_vpk: impl AsRef<Path>,
    vpk_entry: Option<&str>,
) -> Result<String> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let bytes =
        read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;
    let edited = morphic::model::apply_edited_glb(&bytes, block_index, glb_bytes)
        .with_context(|| format!("applying edited glb to block {block_index} of {entry}"))?;
    let vpk_entry = vpk_entry.unwrap_or(entry).to_string();
    crate::pack(&[(vpk_entry.as_str(), edited.as_slice())], out_vpk.as_ref())
        .with_context(|| format!("packing edited model into {}", out_vpk.as_ref().display()))?;
    Ok(vpk_entry)
}

/// What a [`remove_model_material`] run dropped and where it was packed.
#[derive(Debug, Clone)]
pub struct MaterialRemovalReport {
    pub entry: String,
    pub vpk_entry: String,
    /// Each draw call removed (its mesh, material, and vertex/index counts).
    pub removed: Vec<RemovedDrawCall>,
}

/// Lists a model's renderable (LOD0) draw calls: the mesh part, the material each
/// renders, and the vertex/index counts. The CLI uses this for `--list-drawcalls`
/// so a user can find the exact material string to pass to
/// [`remove_model_material`].
pub fn model_draw_call_targets(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
) -> Result<Vec<DrawCallInfo>> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let bytes =
        read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;
    morphic::model::draw_call_targets(&bytes)
        .with_context(|| format!("reading draw calls for {entry}"))
}

/// Removes every draw call whose material contains `material` (case-insensitive
/// substring) from the model, across all LODs, and packs the edited `.vmdl_c`
/// into a standalone addon VPK at `vpk_entry` (defaulting to `entry`, so it
/// overrides the base pak). This is a draw-call-only edit (Tier 1a): the shared
/// vertex/index buffers are untouched, so the targeted part simply stops
/// rendering.
///
/// Errors if `material` matches no draw call (so a typo fails loudly rather than
/// repacking the model unchanged); use [`model_draw_call_targets`] to discover
/// the exact names.
pub fn remove_model_material(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
    material: &str,
    out_vpk: impl AsRef<Path>,
    vpk_entry: Option<&str>,
) -> Result<MaterialRemovalReport> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let bytes =
        read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;

    let (edited, removed) = morphic::model::remove_draw_calls_by_material(&bytes, material)
        .with_context(|| {
            format!("removing draw calls matching {material:?} from {entry} (try --list-drawcalls)")
        })?;

    let vpk_entry = vpk_entry.unwrap_or(entry).to_string();
    crate::pack(&[(vpk_entry.as_str(), edited.as_slice())], out_vpk.as_ref())
        .with_context(|| format!("packing edited model into {}", out_vpk.as_ref().display()))?;

    Ok(MaterialRemovalReport {
        entry: entry.to_string(),
        vpk_entry,
        removed,
    })
}

/// What a [`replace_model_part`] run swapped and where it was packed.
#[derive(Debug, Clone)]
pub struct PartReplacementReport {
    pub entry: String,
    pub vpk_entry: String,
    pub replaced: ReplacedMeshPart,
}

/// Replaces an existing mesh part's geometry (Tier 1d): reads a new mesh from a
/// `.glb` (one primitive; `glb_mesh` picks it out of a multi-mesh export) and
/// splices it over the model's `mesh_name` part in place, then packs the edited
/// `.vmdl_c` into a standalone addon VPK at `vpk_entry` (default `entry`, so it
/// overrides the base pak).
///
/// The new mesh may have any vertex/index count; it must be skinned against the
/// model skeleton (glTF joint indices == model bone indices) and bound to bones the
/// target part already uses (its bone palette is reused, not grown). The target
/// part must have exactly one vertex buffer, one index buffer, and one draw call
/// (e.g. the gun); errors loudly otherwise. See `docs/handoff-model-edit.md` (T1d).
#[allow(clippy::too_many_arguments)] // VPK in, mesh selectors, addon VPK out
pub fn replace_model_part(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
    mesh_name: &str,
    glb_bytes: &[u8],
    glb_mesh: Option<&str>,
    out_vpk: impl AsRef<Path>,
    vpk_entry: Option<&str>,
) -> Result<PartReplacementReport> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let bytes =
        read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;

    let (vb, indices) = morphic::model::read_edited_mesh(glb_bytes, glb_mesh)
        .with_context(|| "reading the replacement mesh from the glb".to_string())?;
    let (edited, replaced) = morphic::model::replace_mesh_part(&bytes, mesh_name, &vb, &indices)
        .with_context(|| format!("replacing mesh part {mesh_name:?} in {entry}"))?;

    let vpk_entry = vpk_entry.unwrap_or(entry).to_string();
    crate::pack(&[(vpk_entry.as_str(), edited.as_slice())], out_vpk.as_ref())
        .with_context(|| format!("packing edited model into {}", out_vpk.as_ref().display()))?;

    Ok(PartReplacementReport {
        entry: entry.to_string(),
        vpk_entry,
        replaced,
    })
}

/// Diagnostic: re-encodes every `MDAT` block of the model unchanged and packs the
/// result into a standalone addon VPK. Used to probe whether the engine accepts our
/// re-encoded model KV3 blocks at all, independent of any edit (see
/// [`morphic::model::reencode_all_mdat_identity`]). Returns the number of blocks
/// re-encoded.
pub fn reencode_model_mdat(
    vpk: impl AsRef<Path>,
    entry: &str,
    base: Option<&Path>,
    out_vpk: impl AsRef<Path>,
    vpk_entry: Option<&str>,
) -> Result<usize> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let bytes =
        read_entry(&vpks, entry).with_context(|| format!("model entry {entry} not found"))?;
    let (reencoded, count) = morphic::model::reencode_all_mdat_identity(&bytes)
        .with_context(|| format!("re-encoding MDAT blocks of {entry}"))?;
    let vpk_entry = vpk_entry.unwrap_or(entry);
    crate::pack(&[(vpk_entry, reencoded.as_slice())], out_vpk.as_ref()).with_context(|| {
        format!(
            "packing re-encoded model into {}",
            out_vpk.as_ref().display()
        )
    })?;
    Ok(count)
}

/// Mean position over all the given buffers (the shared scale pivot).
// The vertex count -> f32 cast is an averaging divisor; precision loss at
// realistic mesh sizes does not meaningfully move the centroid.
#[allow(clippy::cast_precision_loss)]
fn shared_centroid<'a>(buffers: impl Iterator<Item = &'a [[f32; 3]]>) -> [f32; 3] {
    let mut sum = [0.0f32; 3];
    let mut count = 0usize;
    for positions in buffers {
        for p in positions {
            for k in 0..3 {
                sum[k] += p[k];
            }
        }
        count += positions.len();
    }
    if count == 0 {
        return [0.0; 3];
    }
    let n = count as f32;
    [sum[0] / n, sum[1] / n, sum[2] / n]
}

/// Scale each position about `centroid`, then translate.
fn transform_about(
    positions: &[[f32; 3]],
    centroid: [f32; 3],
    edit: &GeometryEdit,
) -> Vec<[f32; 3]> {
    positions
        .iter()
        .map(|p| {
            [
                (p[0] - centroid[0]) * edit.scale + centroid[0] + edit.translate[0],
                (p[1] - centroid[1]) * edit.scale + centroid[1] + edit.translate[1],
                (p[2] - centroid[2]) * edit.scale + centroid[2] + edit.translate[2],
            ]
        })
        .collect()
}

fn part_list(targets: &[VertexTarget]) -> String {
    let mut names: Vec<&str> = targets.iter().map(|t| t.mesh_name.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    names.join(", ")
}

fn inspect_material_refs(
    vpks: &[valve_pak::VPK],
    has_base: bool,
    material: &str,
) -> (Option<String>, Vec<ResolvedTextureParam>) {
    let compiled = compiled_resource_path(material);
    let Some((bytes, source)) = read_entry_with_source(vpks, &compiled) else {
        return (None, Vec::new());
    };
    let material_source = Some(source_label(source, has_base));
    let Ok(mat) = morphic::material::parse(&bytes) else {
        return (material_source, Vec::new());
    };
    let mut textures = Vec::new();
    for (slot, path) in mat.texture_params {
        let compiled_path = compiled_resource_path(&path);
        let source =
            read_entry_with_source(vpks, &compiled_path).map(|(_, i)| source_label(i, has_base));
        textures.push(ResolvedTextureParam {
            slot,
            path,
            compiled_path,
            source,
        });
    }
    textures.sort_by(|a, b| a.slot.cmp(&b.slot));
    (material_source, textures)
}

fn skin_info_for(model: &morphic::model::Model, call: &DrawCallInfo) -> DrawCallSkinInfo {
    let Some(part) = model
        .meshes
        .iter()
        .find(|m| m.mesh_index == call.mesh_index)
    else {
        return DrawCallSkinInfo::default();
    };
    let mut used = std::collections::BTreeSet::new();
    for &vb_i in &call.vertex_buffers {
        let Some(vb) = part.vertex_buffers.get(vb_i) else {
            continue;
        };
        let weights = (vb.weights.len() == vb.element_count).then_some(vb.weights.as_slice());
        for (row, joints) in vb.joints.iter().enumerate() {
            for (lane, &bone) in joints.iter().enumerate() {
                let significant = weights.map_or(lane == 0, |w| w[row][lane] > 0.0);
                if significant {
                    used.insert(usize::from(bone));
                }
            }
        }
    }
    let used_bones: Vec<String> = used
        .iter()
        .filter_map(|&i| model.skeleton.bones.get(i).map(|b| b.name.clone()))
        .collect();
    DrawCallSkinInfo {
        skinned: part.bone_weight_count > 0 || !used_bones.is_empty(),
        bone_weight_count: part.bone_weight_count,
        used_bone_count: used_bones.len(),
        used_bones,
    }
}

#[allow(clippy::too_many_lines)]
fn suggest_part_groups(calls: &[ModelDrawCallInspection]) -> Vec<SuggestedPartGroup> {
    const GROUPS: &[(&str, &str, &[&str], &[&str])] = &[
        (
            "gun",
            "Gun / weapon",
            &["weapon"],
            &[
                "gun", "weapon", "rifle", "pistol", "revolver", "shotgun", "smg", "bow",
                "crossbow", "sword", "knife", "blade",
            ],
        ),
        (
            "hair",
            "Hair",
            &[],
            &["hair", "bang", "pony", "braid", "beard"],
        ),
        (
            "dress",
            "Dress / skirt",
            &["skirt"],
            &["dress", "skirt", "robe", "coat", "cape", "cloth"],
        ),
        (
            "body",
            "Body / outfit",
            &["outfit"],
            &[
                "body",
                "torso",
                "chest",
                "outfit",
                "suit",
                "uniform",
                "armor",
                "skinmaterial",
            ],
        ),
        (
            "gloves",
            "Gloves / hands",
            &["hands"],
            &["glove", "hand", "arm", "sleeve"],
        ),
        (
            "shoes",
            "Shoes / legs",
            &["legs"],
            &["shoe", "boot", "leg", "foot", "pants", "heel"],
        ),
        (
            "accessories",
            "Accessories",
            &["accessory"],
            &[
                "accessory",
                "accessories",
                "acc",
                "hat",
                "mask",
                "belt",
                "bag",
                "ring",
                "ear",
                "neck",
                "glasses",
                "prop",
            ],
        ),
    ];

    let mut groups = Vec::new();
    for (name, label, aliases, tokens) in GROUPS {
        let mut matched: Vec<&ModelDrawCallInspection> = calls
            .iter()
            .filter(|c| {
                let hay = format!("{} {}", c.mesh_name, c.material).to_ascii_lowercase();
                tokens.iter().any(|t| hay.contains(t))
            })
            .collect();
        if matched.is_empty() {
            continue;
        }
        matched.sort_by(|a, b| a.id.cmp(&b.id));
        matched.dedup_by(|a, b| a.id == b.id);
        let mut mesh_names: Vec<String> = matched.iter().map(|c| c.mesh_name.clone()).collect();
        mesh_names.sort();
        mesh_names.dedup();
        let mut materials: Vec<String> = matched.iter().map(|c| c.material.clone()).collect();
        materials.sort();
        materials.dedup();
        groups.push(SuggestedPartGroup {
            name: (*name).to_string(),
            label: (*label).to_string(),
            aliases: aliases.iter().map(|a| (*a).to_string()).collect(),
            draw_call_ids: matched.iter().map(|c| c.id.clone()).collect(),
            mesh_names,
            materials,
            vertex_count: matched.iter().map(|c| c.vertex_count).sum(),
            index_count: matched.iter().map(|c| c.index_count).sum(),
            confidence: 0.7,
        });
    }
    groups
}

fn select_draw_calls<'a>(
    calls: &'a [ModelDrawCallInspection],
    selector: &ModelPartSelector,
) -> Result<Vec<&'a ModelDrawCallInspection>> {
    let group = selector.group.as_deref().map(normalize_selector);
    let materials: Vec<String> = selector
        .materials
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let mesh_parts: Vec<String> = selector
        .mesh_parts
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    if group.is_none() && materials.is_empty() && mesh_parts.is_empty() {
        anyhow::bail!("provide --group, --material, or --part for grouped model selection");
    }

    let groups = suggest_part_groups(calls);
    let mut ids = std::collections::BTreeSet::new();
    if let Some(group) = &group {
        for g in &groups {
            let matches_name = normalize_selector(&g.name) == *group
                || g.aliases.iter().any(|a| normalize_selector(a) == *group);
            if matches_name {
                ids.extend(g.draw_call_ids.iter().cloned());
            }
        }
    }

    let mut selected: Vec<&ModelDrawCallInspection> = calls
        .iter()
        .filter(|c| {
            ids.contains(&c.id)
                || materials
                    .iter()
                    .any(|m| c.material.to_ascii_lowercase().contains(m))
                || mesh_parts
                    .iter()
                    .any(|p| c.mesh_name.to_ascii_lowercase().contains(p))
                || group.as_ref().is_some_and(|g| {
                    let hay = format!("{} {}", c.mesh_name, c.material).to_ascii_lowercase();
                    hay.contains(g)
                })
        })
        .collect();
    selected.sort_by(|a, b| a.id.cmp(&b.id));
    selected.dedup_by(|a, b| a.id == b.id);
    if selected.is_empty() {
        anyhow::bail!("group selector matched no draw calls");
    }
    Ok(selected)
}

fn normalize_selector(s: &str) -> String {
    s.to_ascii_lowercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

fn filter_model_to_primitives(
    model: &mut morphic::model::Model,
    selected: &std::collections::BTreeSet<(usize, usize)>,
) {
    for part in &mut model.meshes {
        let mut primitive_index = 0usize;
        part.primitives.retain(|_| {
            let keep = selected.contains(&(part.mesh_index, primitive_index));
            primitive_index += 1;
            keep
        });
    }
    model.meshes.retain(|m| !m.primitives.is_empty());
}

fn draw_call_id(c: &DrawCallInfo) -> String {
    format!(
        "mesh{}:prim{}:mdat{}:so{}:dc{}",
        c.mesh_index, c.primitive_index, c.data_block, c.scene_object, c.draw_call
    )
}

fn compiled_resource_path(path: &str) -> String {
    if path.ends_with("_c") {
        path.to_string()
    } else {
        format!("{path}_c")
    }
}

fn source_label(index: usize, has_base: bool) -> String {
    match (index, has_base) {
        (0, true) => "mod".to_string(),
        (0, false) => "input".to_string(),
        _ => "base".to_string(),
    }
}

/// Reads a VPK entry from the first of `vpks` that contains it.
fn read_entry(vpks: &[valve_pak::VPK], entry: &str) -> Option<Vec<u8>> {
    read_entry_with_source(vpks, entry).map(|(bytes, _)| bytes)
}

fn read_entry_with_source(vpks: &[valve_pak::VPK], entry: &str) -> Option<(Vec<u8>, usize)> {
    for (index, vpk) in vpks.iter().enumerate() {
        if let Ok(mut vf) = vpk.get_file(entry) {
            if let Ok(bytes) = vf.read_all() {
                return Some((bytes, index));
            }
        }
    }
    None
}

/// Resolves and bakes a WIP hero's loose NM menu/idle pose. The hero's `.vmdl_c`
/// embeds no clips; its pose lives in `<model_dir>/clips/<name>.vnmclip_c` with a
/// sibling `<h>.vnmskel_c`. For each candidate clip name this tries the bare name
/// and a `<stem>_<name>` variant (Mina prefixes the codename, e.g.
/// `vampirebat_hero_pose`), reads the `.vnmskel_c` the clip references, and bakes
/// the static pose onto `model`. `None` when no loose pose clip resolves.
fn bake_loose_nm_pose(
    vpks: &[valve_pak::VPK],
    entry: &str,
    model: &morphic::model::Model,
    candidates: &[&str],
) -> Option<morphic::model::Model> {
    let (dir, file) = entry.rsplit_once('/')?;
    let stem = file.strip_suffix(".vmdl_c").unwrap_or(file);
    for cand in candidates {
        for name in [(*cand).to_string(), format!("{stem}_{cand}")] {
            let clip_path = format!("{dir}/clips/{name}.vnmclip_c");
            let Some(clip_bytes) = read_entry(vpks, &clip_path) else {
                continue;
            };
            let Ok(pose) = morphic::model::decode_nm_pose(&clip_bytes) else {
                continue;
            };
            let skel_path = nm_skeleton_entry(&pose.skeleton_ref, dir, stem);
            let Some(skel_bytes) = read_entry(vpks, &skel_path) else {
                continue;
            };
            let Ok(skel) = morphic::model::decode_nm_skeleton(&skel_bytes) else {
                continue;
            };
            if let Ok(posed) = morphic::model::bake_nm_pose(model, &skel, &pose) {
                return Some(posed);
            }
        }
    }
    None
}

/// Compiled `.vnmskel_c` entry path for a clip's `m_skeleton` reference (append
/// `_c` to the uncompiled path), falling back to the conventional
/// `<dir>/<stem>.vnmskel_c` when the reference is absent.
fn nm_skeleton_entry(skeleton_ref: &str, dir: &str, stem: &str) -> String {
    if skeleton_ref.is_empty() {
        format!("{dir}/{stem}.vnmskel_c")
    } else if skeleton_ref.ends_with("_c") {
        skeleton_ref.to_string()
    } else {
        format!("{skeleton_ref}_c")
    }
}

/// A compiled model found inside a VPK, with its structural summary.
#[derive(Debug, Clone)]
pub struct ModelEntry {
    /// VPK-internal path (e.g. `models/heroes_staging/hornet_v3/hornet.vmdl_c`).
    pub path: String,
    pub info: ModelInfo,
}

/// Find every `.vmdl_c` in a VPK and summarize its block structure.
pub fn inspect_models(vpk_path: impl AsRef<Path>) -> Result<Vec<ModelEntry>> {
    let vpk_path = vpk_path.as_ref();
    let vpk =
        valve_pak::open(vpk_path).with_context(|| format!("opening {}", vpk_path.display()))?;

    let paths: Vec<String> = vpk
        .file_paths()
        .filter(|p| p.ends_with(".vmdl_c"))
        .cloned()
        .collect();

    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let mut vf = vpk
            .get_file(&path)
            .with_context(|| format!("locating {path}"))?;
        let bytes = vf.read_all().with_context(|| format!("reading {path}"))?;
        let info = morphic::model::inspect(&bytes).with_context(|| format!("parsing {path}"))?;
        out.push(ModelEntry { path, info });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    // The transform arithmetic here is exact (scale 2.0, integer inputs), so the
    // tests assert exact float-array equality deliberately.
    #![allow(clippy::float_cmp)]

    use super::*;

    fn clip(name: &str, fps: f32, frame_count: usize, looping: bool) -> morphic::model::Clip {
        morphic::model::Clip {
            name: name.to_string(),
            fps,
            frame_count,
            looping,
            tracks: Vec::new(),
        }
    }

    #[test]
    fn summarize_clips_computes_duration_sorts_and_flags_default() {
        // `ui_hero_pose` outranks `primary_stand_idle` in DEFAULT_POSE_CLIPS, so it
        // is the flagged default even though both are present and it is the shorter
        // clip. Output is sorted by name regardless of input order.
        let anims = vec![
            clip("primary_stand_idle", 30.0, 91, true),
            clip("ui_hero_pose", 30.0, 1, false),
            clip("ability_cast", 30.0, 31, false),
        ];
        let out = summarize_clips(&anims);
        assert_eq!(
            out.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["ability_cast", "primary_stand_idle", "ui_hero_pose"]
        );
        let idle = out.iter().find(|c| c.name == "primary_stand_idle").unwrap();
        // (91 - 1) / 30 = 3.0s; single-frame poses are 0.0s.
        assert_eq!(idle.duration_seconds, 3.0);
        assert!(!idle.default);
        let pose = out.iter().find(|c| c.name == "ui_hero_pose").unwrap();
        assert_eq!(pose.duration_seconds, 0.0);
        assert!(pose.default);
        assert_eq!(out.iter().filter(|c| c.default).count(), 1);
    }

    #[test]
    fn summarize_clips_flags_no_default_without_a_candidate_pose() {
        let anims = vec![clip("ability_cast", 30.0, 31, false)];
        let out = summarize_clips(&anims);
        assert!(out.iter().all(|c| !c.default));
        assert!(summarize_clips(&[]).is_empty());
    }

    #[test]
    fn shared_centroid_is_the_mean_across_buffers() {
        let a = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        let b = [[0.0, 4.0, 0.0], [2.0, 4.0, 0.0]];
        let c = shared_centroid([a.as_slice(), b.as_slice()].into_iter());
        assert_eq!(c, [1.0, 2.0, 0.0]);
    }

    /// Two buffers scaled about one shared centroid stay rigid relative to each
    /// other (the bug a per-buffer centroid would introduce): a point and the
    /// centroid keep the same scaled offset regardless of which buffer it is in.
    #[test]
    fn scale_about_shared_centroid_keeps_buffers_aligned() {
        let centroid = [1.0, 2.0, 0.0];
        let edit = GeometryEdit {
            part: None,
            scale: 2.0,
            translate: [0.0; 3],
        };
        // A vertex at the centroid stays put under a pure scale.
        let at_centroid = transform_about(&[centroid], centroid, &edit);
        assert_eq!(at_centroid[0], centroid);
        // A vertex 1 unit from the centroid moves to 2 units (scale 2x), same
        // rule in any buffer.
        let off = transform_about(&[[2.0, 2.0, 0.0]], centroid, &edit);
        assert_eq!(off[0], [3.0, 2.0, 0.0]);
    }

    #[test]
    fn translate_is_uniform_and_centroid_independent() {
        let edit = GeometryEdit {
            part: None,
            scale: 1.0,
            translate: [10.0, -5.0, 1.0],
        };
        let out = transform_about(
            &[[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]],
            [99.0, 99.0, 99.0],
            &edit,
        );
        assert_eq!(out, vec![[10.0, -5.0, 1.0], [11.0, -4.0, 2.0]]);
    }
}
