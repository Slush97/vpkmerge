// Build a custom soul container that is a faithful CLONE of an imported GLB.
//
// This is the library form of the `soul_import_clone` example, promoted so the
// `vpkmerge soul-container import` subcommand (and Grimoire, via the bundled
// binary) can build a soul-container override VPK from a user `.glb` with no
// terminal env-var dance. The pipeline is unchanged from the proven prototype;
// only the inputs differ: the three `SOUL_*` env vars are now explicit options.
//
// APPROACH (proven in-game): ONE clean material with an ATLASED albedo + a
// single draw call -- NOT N draw calls. Source 2 soul containers all render
// through one shader (pbr.vfx NPR toon); shipped multi-material mods differ only
// in their albedo TEXTURE. So packing every GLB material group into one atlas
// albedo (each group's UVs remapped into its atlas cell) is visually identical to
// N materials and uses the single-draw-call path confirmed to load in-game.
//
// The material is a committed clean DONOR (a shipped soul material: pbr.vfx, NPR
// toon on, NO solid outline, NO self-illum) with only g_tColor repointed to the
// atlas. Vanilla soul_container.vmat is never touched (a morphic re-emit renders
// the red error shader).
//
// PARTICLES: the orb's gold "soul" look is 3 entity-attached particles. We ship
// recolored copies (recolor_particle_bytes -> the import's dominant hue) so the
// glow matches the model instead of staying default gold.
//
// Output: fresh model + 1 material + 1 atlas texture + 3 recolored particles.

// Atlas layout, UV remapping, geometry fitting, and colour conversion convert
// between float and fixed-width integer lanes with clamped, bounded inputs; the
// truncation, sign loss, wrap, precision loss, and infallible widening on these
// casts are intentional.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_lossless
)]

use anyhow::{anyhow, Context, Result};
use gltf::mesh::Mode;
use morphic::kv3::{Seg, Value as Kv3};
use morphic::model::{replace_mesh_part_uncompressed, set_model_material, VertexBuffer};
use morphic::{replace_mip_chain, Image, ImageData};
use serde_json::Value as Json;
use std::collections::HashMap;
use std::path::Path;

use crate::{recolor_particle_bytes, Recolor};

const MODEL: &str = "models/props_gameplay/soul_container/soul_container.vmdl_c";
const MAT_DIR: &str = "models/props_gameplay/soul_container/materials";
const MESH_NAME: &str = "soul_container";
const DONOR_VMAT: &[u8] = include_bytes!("../../morphic/fixtures/soul/soul_material_donor.vmat_c");
const DEFAULT_NORMAL: &str = "materials/default/default_normal_tga_7be61377.vtex";
pub(crate) const FLAT_DONOR: &str = "panorama/images/hud/zipline_icon_psd.vtex_c";
pub(crate) const COLOR_DONOR: &str = "dev/helper/testgrid_color_tga_2d6cc34.vtex_c";
// A 2048x2048 BC7 donor for high-res atlases (4x the 512 donor's linear res). The
// atlas is spliced into a same-size BCn donor, so the donor's dimensions set the
// atlas resolution. Used for single-material props (the urn) whose source texture
// is large; falls back to the 512 donor if absent.
pub(crate) const COLOR_DONOR_2K: &str =
    "models/dev/calibration_tool/materials/calibration_tool_color_psd_588eed2b.vtex_c";
// A 4096x4096 BC7 sRGB color donor (a stock gameplay prop albedo), for props whose
// source texture is 4096 (e.g. AI-generated meshes) so the atlas maps 1:1 with no
// downscale. Heavier (~21 MB texture); falls back to the 2048/512 donors if absent.
pub(crate) const COLOR_DONOR_4K: &str =
    "models/props_gameplay/wooden_crate_03/materials/wooden_crate_03_color_png_9f61ca0d.vtex_c";
const IDENTITY3: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
const IDENTITY4: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];
const ATLAS_GUTTER_PX: u32 = 4;

// The 3 soul-glow particles (base game) the orb entity attaches; we recolor them.
const PARTICLES: [&str; 3] = [
    "particles/generic/holding_gold_neutral_model.vpcf_c",
    "particles/generic/holding_gold_neutral_model_glow.vpcf_c",
    "particles/generic/holding_gold_neutral_embers.vpcf_c",
];

/// Coordinate convention to apply to the imported GLB before fitting. `Auto`
/// guesses Y-up vs Z-up from which gives the taller silhouette, but it is not
/// reliable for cube-like props; callers should default to [`SoulOrient::YUp`]
/// and offer manual correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoulOrient {
    YUp,
    ZUp,
    FlipY,
    Auto,
}

/// What to do with the orb's soul-glow particles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoulGlow {
    /// Recolor the gold glow to the import's dominant hue (default).
    Recolor,
    /// Ship the particles unchanged (gold) so the glow is isolated from the model.
    Base,
    /// Don't ship particles; the base game's gold glow plays over the new model.
    Off,
}

/// Inputs for [`import_soul_container_clone`].
#[derive(Debug, Clone)]
pub struct SoulImportCloneOptions {
    /// Material/texture basename used inside the VPK (e.g. `togetic`). The mesh
    /// always overrides the single canonical soul-container model path.
    pub name: String,
    pub orient: SoulOrient,
    /// Extra Euler rotation in degrees `[X, Y, Z]`, applied after `orient`.
    pub rotate: Option<[f32; 3]>,
    /// Facing yaw in degrees, applied as a turn about Source-Z (up) through the
    /// orb center after fitting. Distinct from `rotate` (pre-swizzle GLB-space
    /// Euler): this is unambiguous final-space yaw, the knob the Grimoire slider
    /// drives. The rotation is baked into geometry, so it survives the
    /// particle's orientation ops (see `orient_upright`).
    pub yaw: f32,
    /// Apply psyduck's upright-orientation recipe to the model soul-glow
    /// particle so the orb stands still and upright instead of tumbling/spinning
    /// with the control point. Default true; pair with `yaw` for a stable facing.
    pub orient_upright: bool,
    /// After fitting, lift the mesh so its lowest Source-Z point sits at the
    /// model origin/floor instead of centering it on the stock orb.
    pub ground: bool,
    pub glow: SoulGlow,
}

impl Default for SoulImportCloneOptions {
    fn default() -> Self {
        Self {
            name: "custom_soul".to_string(),
            orient: SoulOrient::YUp,
            rotate: None,
            yaw: 0.0,
            orient_upright: true,
            ground: false,
            glow: SoulGlow::Recolor,
        }
    }
}

/// Which model the clone edits, and where the result is packed. The default
/// targets the soul container; [`urn_target`] retargets the same pipeline at the
/// carryable Idol/urn objective (`idol_urn.vmdl_c`).
///
/// `envelope_model` is read from the pak and is the model actually edited (it must
/// be a modern split-buffer `.vmdl_c` morphic can decode). `output_model` is where
/// the edited bytes are packed in the VPK, which can differ from the envelope: the
/// urn is a legacy monolithic-VBIB model morphic cannot edit in place, so its slot
/// is overridden with a soul-container-derived model instead. A `.vmdl_c` carries
/// no self path, so the engine treats whatever sits at `output_model` as that model.
#[derive(Debug, Clone)]
pub struct CloneTarget {
    /// Modern, editable `.vmdl_c` entry read from the pak (the envelope).
    pub envelope_model: String,
    /// Embedded mesh-part name inside the envelope to replace.
    pub mesh_name: String,
    /// Entry path the edited model is packed at (overrides this slot in-game).
    pub output_model: String,
    /// Directory under which the new material + atlas texture entries are written.
    pub mat_dir: String,
    /// Entity-attached particles to ship (recolored per [`SoulGlow`]). Empty for
    /// targets like the urn that have no soul-glow particles of their own.
    pub particles: Vec<String>,
    /// Largest-axis Source-units span to fit the import to. `None` fits to the
    /// envelope model's own bounds (right for the soul container; for the urn the
    /// envelope is soul-sized, so an explicit span sets the in-game size).
    pub target_span: Option<f32>,
    /// Preferred atlas resolution in pixels. `2048` selects the high-res BC7 donor
    /// (props with a large source texture); anything else uses the 512 donor. The
    /// real size is clamped to an available donor at build time.
    pub atlas_px: u32,
    /// When set, synthesize a `g_tNormalRoughness` map for the import and bind it in
    /// place of the flat default normal. The flat default gives a relief-less,
    /// matte-looking surface (roughness 0.5, no microdetail) that reads as soft/blurry
    /// next to a real prop; a synthesized normal (height-from-albedo) plus a tuned
    /// roughness restores surface relief and specular pop. `None` keeps the legacy
    /// flat-default behavior (right for the glowing soul orb, which wants no relief).
    pub synth_normal: Option<NormalSynthesis>,
}

/// Parameters for synthesizing a `g_tNormalRoughness` map from the import's albedo.
/// The normal is a tangent-space bump derived from albedo luminance (Sobel height);
/// roughness is packed into the B channel (Deadlock `pbr.vfx`: R,G = normal.xy,
/// B = roughness, A = 1).
#[derive(Debug, Clone, Copy)]
pub struct NormalSynthesis {
    /// Bump strength. Higher = steeper synthesized relief. ~1.0 is a subtle,
    /// safe default; the source albedo's own contrast scales the effect.
    pub strength: f32,
    /// Uniform roughness packed into B, `0.0` (mirror) .. `1.0` (fully matte).
    /// The flat default is effectively `0.5`; metal/ceramic props read crisper
    /// glossier around `0.3..0.4`.
    pub roughness: f32,
}

/// The default target: edit and override the soul container in place, shipping its
/// three soul-glow particles.
#[must_use]
pub fn soul_target() -> CloneTarget {
    CloneTarget {
        envelope_model: MODEL.to_string(),
        mesh_name: MESH_NAME.to_string(),
        output_model: MODEL.to_string(),
        mat_dir: MAT_DIR.to_string(),
        particles: PARTICLES.iter().map(|s| (*s).to_string()).collect(),
        target_span: None,
        atlas_px: 512,
        // The soul orb is an emissive glowing prop; a relief normal would fight its
        // look. Keep the flat default (legacy behavior).
        synth_normal: None,
    }
}

/// Retarget the clone pipeline at the carryable Idol/urn objective: build into the
/// soul-container envelope (the urn's own format is not editable), pack at the urn's
/// model + material paths, ship no particles. `span` sets the in-game size in Source
/// units (the urn is bigger than a soul orb, so the soul envelope's bounds are not used).
#[must_use]
pub fn urn_target(span: f32) -> CloneTarget {
    CloneTarget {
        envelope_model: MODEL.to_string(),
        mesh_name: MESH_NAME.to_string(),
        output_model: "models/props_gameplay/idol_urn/idol_urn.vmdl_c".to_string(),
        mat_dir: "models/props_gameplay/idol_urn/materials".to_string(),
        particles: Vec::new(),
        target_span: Some(span),
        atlas_px: 4096,
        // Solid prop: give it real surface relief + glossy metal/ceramic roughness so
        // it doesn't read as a soft, flat, matte blob (the flat-default look that made
        // the first urn imports look "blurry" despite a high-res albedo).
        synth_normal: Some(NormalSynthesis {
            strength: 1.0,
            roughness: 0.35,
        }),
    }
}

/// Diagnostics from a successful build, surfaced to the CLI/UI and stored in the
/// imported mod's metadata so the transform is reproducible.
#[derive(Debug, Clone)]
pub struct SoulImportReport {
    /// Human-readable orientation applied, e.g. `y-up` or `auto:z-up + rotate(0,90,0)`.
    pub orient_label: String,
    pub prim_count: usize,
    pub group_count: usize,
    pub vert_count: usize,
    pub tri_count: usize,
    pub atlas_px: u32,
    pub atlas_cols: u32,
    pub atlas_rows: u32,
    /// Uniform scale applied to fit the import to the soul-container's span.
    pub fit_scale: f32,
    /// The vanilla soul-container's largest-axis span (Source units) the mesh was
    /// fit to (~12.65).
    pub target_span: f32,
    /// The import's largest-axis span before fitting (post-orientation).
    pub source_span: f32,
    /// Facing yaw (degrees) baked into the geometry about Source-Z.
    pub yaw: f32,
    /// Whether the model particle was patched upright (psyduck recipe).
    pub upright: bool,
    /// Dominant-hue degrees the glow was recolored to (meaningful only for
    /// [`SoulGlow::Recolor`]).
    pub glow_hue: f64,
    /// Number of entries packed into the output VPK.
    pub entry_count: usize,
}

/// Build a soul-container override VPK from an in-memory GLB, fitting it to the
/// stock orb's bounds and writing it (plus material, atlas texture, and recolored
/// particles) to `out`. `pak` is a base `pak01_dir.vpk` the stock model, donor
/// textures, and particles are read from.
pub fn import_soul_container_clone(
    pak: impl AsRef<Path>,
    glb: &[u8],
    out: impl AsRef<Path>,
    opts: &SoulImportCloneOptions,
) -> Result<SoulImportReport> {
    import_clone(pak, glb, out, opts, &soul_target())
}

/// Generalized clone: build the GLB into `target.envelope_model` and pack the
/// result at `target.output_model` (plus material, atlas texture, and any
/// particles). See [`import_soul_container_clone`] for the soul-container default
/// and [`urn_target`] for the urn override.
#[allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::similar_names
)]
pub fn import_clone(
    pak: impl AsRef<Path>,
    glb: &[u8],
    out: impl AsRef<Path>,
    opts: &SoulImportCloneOptions,
    target: &CloneTarget,
) -> Result<SoulImportReport> {
    let name = &opts.name;
    let doc = glb_json(glb)?;
    let vpk = valve_pak::open(pak.as_ref())?;
    let read = |entry: &str| -> Result<Vec<u8>> {
        let mut f = vpk
            .get_file(entry)
            .with_context(|| format!("entry {entry} not found"))?;
        f.read_all()
    };

    // --- 1. read GLB prims, group by material, resolve each group's albedo ---
    let (prims, orient_label) = read_glb_primitives(glb, opts.orient, opts.rotate)?;
    let mat_index = material_index_by_name(&doc);
    let mut groups: Vec<Group> = Vec::new();
    for (pi, p) in prims.iter().enumerate() {
        if let Some(g) = groups
            .iter_mut()
            .find(|g| g.glb_material == p.material_name)
        {
            g.prims.push(pi);
        } else {
            let gi = p
                .material_name
                .as_ref()
                .and_then(|m| mat_index.get(m))
                .copied();
            groups.push(Group {
                glb_material: p.material_name.clone(),
                prims: vec![pi],
                albedo: gi
                    .map(|mi| material_albedo(glb, &doc, mi))
                    .transpose()?
                    .flatten(),
                color: gi.map_or([1.0; 4], |mi| base_color_factor(&doc, mi)),
                index_count: 0,
            });
        }
    }
    let n = groups.len();

    // --- 2. atlas layout: square grid on the 512 donor (big cells -> minimal
    //        mip bleed between flat colours at distance). Falls back to the small
    //        flat donor only if the 512 one is missing from this build's pak.
    //        NOTE: the atlas must be spliced into a same-size BCn donor; an inline
    //        PNG_RGBA8888 albedo minted from scratch decodes fine offline but the
    //        Deadlock engine REJECTS it on a model material and renders the
    //        missing-texture purple (in-game verified). Keep the BCn donor path. ---
    let (atlas, donor_entry) = if target.atlas_px >= 4096 && vpk.get_file(COLOR_DONOR_4K).is_ok() {
        (4096u32, COLOR_DONOR_4K)
    } else if target.atlas_px >= 2048 && vpk.get_file(COLOR_DONOR_2K).is_ok() {
        (2048u32, COLOR_DONOR_2K)
    } else if vpk.get_file(COLOR_DONOR).is_ok() {
        (512u32, COLOR_DONOR)
    } else {
        (64u32, FLAT_DONOR)
    };
    let cols = (n as f64).sqrt().ceil() as u32;
    let rows = n.div_ceil(cols as usize) as u32;
    let cw = atlas / cols;
    let ch = atlas / rows;
    let cell_rect = |i: usize| -> (u32, u32, u32, u32) {
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        (c * cw, r * ch, cw, ch)
    };

    // --- 3. merge geometry; remap each group's UVs into its atlas cell ---
    let mut merged = VertexBuffer {
        texcoords: vec![Vec::new()],
        ..VertexBuffer::default()
    };
    let mut indices: Vec<u32> = Vec::new();
    for (gi, g) in groups.iter_mut().enumerate() {
        let (x0, y0, w, h) = cell_rect(gi);
        let rect = AtlasCell::new(x0, y0, w, h);
        let start = indices.len();
        for &pi in &g.prims {
            let vb = &prims[pi].vertex_buffer;
            let base = u32::try_from(merged.positions.len())?;
            merged
                .positions
                .extend(vb.positions.iter().map(|v| [v[0], v[2], -v[1]]));
            merged
                .normals
                .extend(vb.normals.iter().map(|v| [v[0], v[2], -v[1]]));
            let src = vb.texcoords.first();
            for vi in 0..vb.positions.len() {
                let uv = src.and_then(|t| t.get(vi)).copied().unwrap_or([0.0, 0.0]);
                merged.texcoords[0].push(remap_atlas_uv(
                    uv,
                    rect,
                    atlas,
                    g.albedo
                        .as_ref()
                        .map_or(WrapMode::ClampToEdge, |a| a.wrap_s),
                    g.albedo
                        .as_ref()
                        .map_or(WrapMode::ClampToEdge, |a| a.wrap_t),
                    g.albedo.is_some(),
                ));
            }
            indices.extend(prims[pi].indices.iter().map(|&idx| base + idx));
        }
        g.index_count = indices.len() - start;
    }
    merged.element_count = merged.positions.len();

    // --- 4. fit to the envelope's bounds (or an explicit target span) ---
    let model_bytes = read(&target.envelope_model)?;
    let orb = morphic::model::decode(&model_bytes)
        .map_err(|e| anyhow!("decode envelope: {e}"))?
        .position_bounds()
        .ok_or_else(|| anyhow!("envelope has no positions"))?;
    let orb_center = [
        midpoint(orb.min[0], orb.max[0]),
        midpoint(orb.min[1], orb.max[1]),
        midpoint(orb.min[2], orb.max[2]),
    ];
    // The fit span defaults to the envelope's own largest axis (right for the soul
    // container, which is its own output). For a retargeted output (the urn) the
    // envelope is soul-sized, so an explicit target span sets the in-game scale.
    let orb_size = target.target_span.unwrap_or_else(|| {
        (0..3)
            .map(|k| orb.max[k] - orb.min[k])
            .fold(0.0_f32, f32::max)
    });
    let (mc, ms) = bounds_center_extent(&merged.positions);
    let scale = if ms > 0.0 { orb_size / ms } else { 1.0 };
    for p in &mut merged.positions {
        for k in 0..3 {
            p[k] = (p[k] - mc[k]) * scale + orb_center[k];
        }
    }
    // --- 4b. facing yaw: turn in place about Source-Z (up) through the orb
    // center. Baked into geometry so it is unambiguous and survives the
    // particle's yaw-only orientation remap (see `orient_particle_upright`).
    if opts.yaw != 0.0 {
        let rot = rotate_z(opts.yaw);
        let (cx, cy) = (orb_center[0], orb_center[1]);
        for p in &mut merged.positions {
            let r = transform3(&rot, [p[0] - cx, p[1] - cy, p[2]]);
            *p = [r[0] + cx, r[1] + cy, r[2]];
        }
        for n in &mut merged.normals {
            *n = transform3(&rot, *n);
        }
    }
    if opts.ground {
        let min_z = merged
            .positions
            .iter()
            .map(|p| p[2])
            .fold(f32::INFINITY, f32::min);
        if min_z.is_finite() {
            for p in &mut merged.positions {
                p[2] -= min_z;
            }
        }
    }
    let tri_count = indices.len() / 3;

    // --- 5. swap mesh in (UNCOMPRESSED) + repoint the single draw call ---
    let (mesh_swapped, _rep) =
        replace_mesh_part_uncompressed(&model_bytes, &target.mesh_name, &merged, &indices)
            .map_err(|e| anyhow!("replacing mesh: {e}"))?;
    let mat_dir = &target.mat_dir;
    let vmat_path = format!("{mat_dir}/{name}.vmat");
    let edited_model = set_model_material(&mesh_swapped, &vmat_path)
        .map_err(|e| anyhow!("repoint material: {e}"))?;

    // --- 6. atlas albedo: each group's cell = its image (resized) or flat colour ---
    let donor = read(donor_entry)?;
    let mut px = vec![0u8; (atlas * atlas * 4) as usize];
    for (gi, g) in groups.iter().enumerate() {
        let (x0, y0, w, h) = cell_rect(gi);
        paint_atlas_cell(
            &mut px,
            atlas,
            AtlasCell::new(x0, y0, w, h),
            g.albedo.as_ref().map(|a| &a.image),
            g.color,
        );
    }
    let color_vtex = format!("{mat_dir}/{name}_color.vtex");
    let color_entry = format!("{mat_dir}/{name}_color.vtex_c");
    let color_tex = replace_mip_chain(
        &donor,
        &Image {
            width: atlas,
            height: atlas,
            data: ImageData::Rgba8(px),
        },
    )
    .map_err(|e| anyhow!("encoding atlas: {e}"))?;

    // --- 6b. optional g_tNormalRoughness: synthesize relief + roughness so the prop
    //         doesn't render as a flat matte blob. Same atlas layout/resolution as the
    //         color atlas (UVs are shared), spliced into the same BCn donor. The shader
    //         samples this slot linearly, so the donor's sRGB-vs-linear is irrelevant. ---
    let mut normal_vtex_entry: Option<(String, String, Vec<u8>)> = None;
    if let Some(ns) = target.synth_normal {
        let mut npx = vec![0u8; (atlas * atlas * 4) as usize];
        for (gi, g) in groups.iter().enumerate() {
            let (x0, y0, w, h) = cell_rect(gi);
            paint_normal_cell(
                &mut npx,
                atlas,
                AtlasCell::new(x0, y0, w, h),
                g.albedo.as_ref().map(|a| &a.image),
                ns,
            );
        }
        let normal_vtex = format!("{mat_dir}/{name}_normal.vtex");
        let normal_entry = format!("{mat_dir}/{name}_normal.vtex_c");
        let normal_tex = replace_mip_chain(
            &donor,
            &Image {
                width: atlas,
                height: atlas,
                data: ImageData::Rgba8(npx),
            },
        )
        .map_err(|e| anyhow!("encoding normal atlas: {e}"))?;
        normal_vtex_entry = Some((normal_vtex, normal_entry, normal_tex));
    }
    let vmat = build_material(
        &color_vtex,
        normal_vtex_entry.as_ref().map(|(v, _, _)| v.as_str()),
    )?;

    // --- 7. recolor the soul-glow particles to the import's dominant hue ---
    let dom = groups.iter().max_by_key(|g| g.index_count);
    let dom_rgb = dom.map_or([255.0, 200.0, 60.0], |g| {
        g.albedo.as_ref().map_or(
            [
                to_srgb_u8(g.color[0]) as f64,
                to_srgb_u8(g.color[1]) as f64,
                to_srgb_u8(g.color[2]) as f64,
            ],
            |albedo| {
                let (mut r, mut gg, mut b, mut n) = (0f64, 0f64, 0f64, 0f64);
                for p in albedo.image.pixels() {
                    if p.0[3] > 8 {
                        r += f64::from(p.0[0]);
                        gg += f64::from(p.0[1]);
                        b += f64::from(p.0[2]);
                        n += 1.0;
                    }
                }
                if n > 0.0 {
                    [r / n, gg / n, b / n]
                } else {
                    [255.0, 200.0, 60.0]
                }
            },
        )
    });
    let hue = rgb_to_hue(dom_rgb[0], dom_rgb[1], dom_rgb[2]);

    // --- 8. pack: model + material + atlas + recolored particles ---
    let mut entries: Vec<(String, Vec<u8>)> = vec![
        (target.output_model.clone(), edited_model),
        (format!("{mat_dir}/{name}.vmat_c"), vmat),
        (color_entry, color_tex),
    ];
    if let Some((_, normal_entry, normal_tex)) = normal_vtex_entry {
        entries.push((normal_entry, normal_tex));
    }
    // SoulGlow::Off = don't ship glow particles (base game gold glow plays);
    // Base = ship unchanged (isolation); Recolor (default) = recolor to hue.
    // Iterate the target's own particle list, so targets with no particles
    // (e.g. the urn) skip this entirely. The first particle
    // (`holding_gold_neutral_model`) carries the orientation ops, so we still
    // ship it (alone) when only `orient_upright` is set, even with glow off.
    for (pi, p) in target.particles.iter().enumerate() {
        let is_model = pi == 0;
        if opts.glow == SoulGlow::Off && !(opts.orient_upright && is_model) {
            continue;
        }
        if let Ok(base) = read(p) {
            let mut bytes = if opts.glow == SoulGlow::Recolor {
                recolor_particle_bytes(&base, Recolor::hue(hue))?.unwrap_or(base)
            } else {
                base
            };
            if opts.orient_upright && is_model {
                bytes = orient_particle_upright(&bytes)?;
            }
            entries.push((p.clone(), bytes));
        }
    }
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    crate::pack(&refs, out.as_ref())?;

    let mut orient_label = orient_label;
    if opts.yaw != 0.0 {
        orient_label = format!("{orient_label} + yaw({})", opts.yaw);
    }
    if opts.orient_upright {
        orient_label = format!("{orient_label} + upright");
    }

    Ok(SoulImportReport {
        orient_label,
        prim_count: prims.len(),
        group_count: n,
        vert_count: merged.element_count,
        tri_count,
        atlas_px: atlas,
        atlas_cols: cols,
        atlas_rows: rows,
        fit_scale: scale,
        target_span: orb_size,
        source_span: ms,
        yaw: opts.yaw,
        upright: opts.orient_upright,
        glow_hue: hue,
        entry_count: refs.len(),
    })
}

/// Apply psyduck's upright-orientation recipe to the model soul-glow particle
/// (`holding_gold_neutral_model.vpcf_c`): clear `m_bLockRot` on
/// `C_OP_PositionLock` (so the orb no longer inherits the spinning control-point
/// rotation) and insert an empty `C_OP_RemapTransformOrientationToYaw` operator
/// right after it (collapses orientation to yaw-only, keeping the orb upright
/// instead of tumbling). Byte-faithful KV3 v5 edits, no re-encode. Idempotent;
/// a non-model particle (no `C_OP_PositionLock`) passes through unchanged.
fn orient_particle_upright(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let doc = morphic::decode_kv3_resource(bytes).map_err(|e| anyhow!("decode particle: {e}"))?;
    let Some(ops) = doc.get("m_Operators").and_then(Kv3::as_array) else {
        return Ok(bytes.to_vec());
    };
    let class_of = |op: &Kv3| op.get("_class").and_then(|c| c.as_str().map(str::to_owned));
    let Some(lock_idx) = ops
        .iter()
        .position(|op| class_of(op).as_deref() == Some("C_OP_PositionLock"))
    else {
        // Not the orientation-carrying particle; leave it alone.
        return Ok(bytes.to_vec());
    };
    let has_remap = ops
        .iter()
        .any(|op| class_of(op).as_deref() == Some("C_OP_RemapTransformOrientationToYaw"));
    let locks_rot = ops[lock_idx]
        .get("m_bLockRot")
        .and_then(Kv3::as_bool)
        .unwrap_or(false);

    let mut out = bytes.to_vec();
    // 1. Clear the rotation lock (flip the existing bool; indices unaffected).
    if locks_rot {
        let path = vec![
            Seg::Key("m_Operators".to_string()),
            Seg::Index(lock_idx),
            Seg::Key("m_bLockRot".to_string()),
        ];
        out = morphic::patch_kv3_resource_bools(&out, &[(path, false)])
            .map_err(|e| anyhow!("clear m_bLockRot: {e}"))?;
    }
    // 2. Insert the yaw-only orientation remap right after PositionLock.
    if !has_remap {
        let op = Kv3::Object(vec![(
            "_class".to_string(),
            Kv3::String("C_OP_RemapTransformOrientationToYaw".to_string()),
        )]);
        out = morphic::patch_kv3_resource_array_insert(
            &out,
            &[Seg::Key("m_Operators".to_string())],
            lock_idx + 1,
            &op,
        )
        .map_err(|e| anyhow!("insert RemapTransformOrientationToYaw: {e}"))?;
    }
    Ok(out)
}

fn to_srgb_u8(c: f64) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

fn midpoint(a: f32, b: f32) -> f32 {
    f32::midpoint(a, b)
}

/// sRGB 0-255 RGB -> hue in degrees [0,360).
#[allow(clippy::many_single_char_names)]
fn rgb_to_hue(r: f64, g: f64, b: f64) -> f64 {
    let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d <= f64::EPSILON {
        return 0.0;
    }
    let h = if (max - r).abs() < f64::EPSILON {
        ((g - b) / d).rem_euclid(6.0)
    } else if (max - g).abs() < f64::EPSILON {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0).rem_euclid(360.0)
}

fn bounds_center_extent(positions: &[[f32; 3]]) -> ([f32; 3], f32) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in positions {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let center = [
        midpoint(min[0], max[0]),
        midpoint(min[1], max[1]),
        midpoint(min[2], max[2]),
    ];
    let extent = (0..3).map(|k| max[k] - min[k]).fold(0.0_f32, f32::max);
    (center, extent)
}

// --- GLB helpers ---

type Mat3 = [[f32; 3]; 3];
type Mat4 = [[f32; 4]; 4];

pub struct ImportPrimitive {
    pub material_name: Option<String>,
    pub vertex_buffer: VertexBuffer,
    pub indices: Vec<u32>,
}

struct Orientation {
    label: String,
    matrix: Mat3,
}

pub fn glb_json(glb: &[u8]) -> Result<Json> {
    if glb.get(0..4) != Some(b"glTF") {
        return Err(anyhow!("not a binary glTF"));
    }
    let json_len = u32::from_le_bytes(glb[12..16].try_into()?) as usize;
    Ok(serde_json::from_slice(&glb[20..20 + json_len])?)
}

fn glb_bin(glb: &[u8]) -> Option<&[u8]> {
    let mut off = 12;
    while off + 8 <= glb.len() {
        let len = u32::from_le_bytes(glb.get(off..off + 4)?.try_into().ok()?) as usize;
        let ty = glb.get(off + 4..off + 8)?;
        let body = glb.get(off + 8..off + 8 + len)?;
        if ty == b"BIN\0" {
            return Some(body);
        }
        off += 8 + len;
    }
    None
}

pub fn material_index_by_name(doc: &Json) -> HashMap<String, usize> {
    let mut map = HashMap::new();
    if let Some(mats) = doc.get("materials").and_then(Json::as_array) {
        for (i, m) in mats.iter().enumerate() {
            if let Some(name) = m.get("name").and_then(Json::as_str) {
                map.insert(name.to_string(), i);
            }
        }
    }
    map
}

pub fn base_color_factor(doc: &Json, mat_idx: usize) -> [f64; 4] {
    let mut color = [1.0; 4];
    let mat = doc
        .get("materials")
        .and_then(Json::as_array)
        .and_then(|a| a.get(mat_idx));
    let factor = mat
        .and_then(|m| m.get("pbrMetallicRoughness"))
        .and_then(|p| p.get("baseColorFactor"))
        .or_else(|| {
            mat.and_then(|m| m.get("extensions"))
                .and_then(|e| e.get("KHR_materials_pbrSpecularGlossiness"))
                .and_then(|s| s.get("diffuseFactor"))
        })
        .and_then(Json::as_array);
    if let Some(f) = factor {
        for (i, v) in f.iter().take(4).enumerate() {
            color[i] = v.as_f64().unwrap_or(1.0);
        }
    }
    color
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WrapMode {
    Repeat,
    ClampToEdge,
    MirroredRepeat,
}

struct AlbedoTexture {
    image: image::RgbaImage,
    wrap_s: WrapMode,
    wrap_t: WrapMode,
}

fn material_albedo(glb: &[u8], doc: &Json, mat_idx: usize) -> Result<Option<AlbedoTexture>> {
    let mat = doc
        .get("materials")
        .and_then(Json::as_array)
        .and_then(|a| a.get(mat_idx));
    let Some(tex_i) = mat
        .and_then(|m| m.get("pbrMetallicRoughness"))
        .and_then(|p| p.get("baseColorTexture"))
        .and_then(|t| t.get("index"))
        .or_else(|| {
            mat.and_then(|m| m.get("extensions"))
                .and_then(|e| e.get("KHR_materials_pbrSpecularGlossiness"))
                .and_then(|s| s.get("diffuseTexture"))
                .and_then(|t| t.get("index"))
        })
        .and_then(Json::as_u64)
    else {
        return Ok(None);
    };
    let texture = doc
        .get("textures")
        .and_then(Json::as_array)
        .and_then(|a| a.get(tex_i as usize));
    let Some(src) = texture.and_then(|t| t.get("source")).and_then(Json::as_u64) else {
        return Ok(None);
    };
    let (wrap_s, wrap_t) = texture_sampler_wrap(doc, texture);
    let image_json = doc
        .get("images")
        .and_then(Json::as_array)
        .and_then(|a| a.get(src as usize))
        .ok_or_else(|| anyhow!("image {src} missing"))?;
    let Some(bv_i) = image_json.get("bufferView").and_then(Json::as_u64) else {
        return Ok(None);
    };
    let bin = glb_bin(glb).ok_or_else(|| anyhow!("GLB image in bufferView but no BIN chunk"))?;
    let bv = doc
        .get("bufferViews")
        .and_then(Json::as_array)
        .and_then(|a| a.get(bv_i as usize))
        .ok_or_else(|| anyhow!("bufferView {bv_i} missing"))?;
    let off = bv.get("byteOffset").and_then(Json::as_u64).unwrap_or(0) as usize;
    let len = bv
        .get("byteLength")
        .and_then(Json::as_u64)
        .ok_or_else(|| anyhow!("byteLength missing"))? as usize;
    let bytes = bin
        .get(off..off + len)
        .ok_or_else(|| anyhow!("image bufferView out of range"))?;
    let img = image::load_from_memory(bytes)
        .map_err(|e| anyhow!("decoding GLB albedo: {e}"))?
        .to_rgba8();
    Ok(Some(AlbedoTexture {
        image: img,
        wrap_s,
        wrap_t,
    }))
}

fn texture_sampler_wrap(doc: &Json, texture: Option<&Json>) -> (WrapMode, WrapMode) {
    let sampler = texture
        .and_then(|t| t.get("sampler"))
        .and_then(Json::as_u64)
        .and_then(|i| {
            doc.get("samplers")
                .and_then(Json::as_array)
                .and_then(|a| a.get(i as usize))
        });
    let wrap_s = sampler
        .and_then(|s| s.get("wrapS"))
        .and_then(Json::as_u64)
        .map_or(WrapMode::Repeat, gltf_wrap_mode);
    let wrap_t = sampler
        .and_then(|s| s.get("wrapT"))
        .and_then(Json::as_u64)
        .map_or(WrapMode::Repeat, gltf_wrap_mode);
    (wrap_s, wrap_t)
}

fn gltf_wrap_mode(value: u64) -> WrapMode {
    match value {
        33071 => WrapMode::ClampToEdge,
        33648 => WrapMode::MirroredRepeat,
        // 10497 is glTF REPEAT; unknown values fall back to the same default.
        _ => WrapMode::Repeat,
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AtlasCell {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    gutter: u32,
}

impl AtlasCell {
    pub(crate) fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        let gutter = ATLAS_GUTTER_PX.min((w.saturating_sub(1)) / 2);
        let gutter = gutter.min((h.saturating_sub(1)) / 2);
        Self { x, y, w, h, gutter }
    }

    pub(crate) fn inner(self) -> (u32, u32, u32, u32) {
        (
            self.x + self.gutter,
            self.y + self.gutter,
            (self.w - self.gutter * 2).max(1),
            (self.h - self.gutter * 2).max(1),
        )
    }
}

pub(crate) fn remap_atlas_uv(
    uv: [f32; 2],
    cell: AtlasCell,
    atlas: u32,
    wrap_s: WrapMode,
    wrap_t: WrapMode,
    textured: bool,
) -> [f32; 2] {
    if !textured {
        return [
            (cell.x as f32 + cell.w as f32 * 0.5) / atlas as f32,
            (cell.y as f32 + cell.h as f32 * 0.5) / atlas as f32,
        ];
    }
    let (ix, iy, iw, ih) = cell.inner();
    [
        remap_axis(uv[0], ix, iw, atlas, wrap_s),
        remap_axis(uv[1], iy, ih, atlas, wrap_t),
    ]
}

fn remap_axis(coord: f32, start: u32, len: u32, atlas: u32, wrap: WrapMode) -> f32 {
    let t = wrap_coord(coord, wrap);
    let span = len.saturating_sub(1) as f32;
    (start as f32 + 0.5 + t * span) / atlas as f32
}

fn wrap_coord(coord: f32, wrap: WrapMode) -> f32 {
    match wrap {
        WrapMode::ClampToEdge => coord.clamp(0.0, 1.0),
        WrapMode::Repeat => coord.rem_euclid(1.0),
        WrapMode::MirroredRepeat => {
            let t = coord.rem_euclid(2.0);
            if t <= 1.0 {
                t
            } else {
                2.0 - t
            }
        }
    }
}

pub(crate) fn paint_atlas_cell(
    px: &mut [u8],
    atlas: u32,
    cell: AtlasCell,
    image: Option<&image::RgbaImage>,
    color: [f64; 4],
) {
    let (ix, iy, iw, ih) = cell.inner();
    if let Some(img) = image {
        let resized = image::imageops::resize(img, iw, ih, image::imageops::FilterType::Lanczos3);
        for yy in 0..cell.h {
            let sy = yy.saturating_sub(cell.gutter).min(ih.saturating_sub(1));
            for xx in 0..cell.w {
                let sx = xx.saturating_sub(cell.gutter).min(iw.saturating_sub(1));
                let mut s = resized.get_pixel(sx, sy).0;
                // GLB base-color alpha is opacity, but this importer emits one
                // opaque Source material where albedo alpha may be read as a mask.
                s[3] = 255;
                write_atlas_pixel(px, atlas, cell.x + xx, cell.y + yy, s);
            }
        }
    } else {
        let c = [
            to_srgb_u8(color[0]),
            to_srgb_u8(color[1]),
            to_srgb_u8(color[2]),
            255,
        ];
        for yy in 0..cell.h {
            for xx in 0..cell.w {
                write_atlas_pixel(px, atlas, cell.x + xx, cell.y + yy, c);
            }
        }
    }
    debug_assert!(ix + iw <= cell.x + cell.w);
    debug_assert!(iy + ih <= cell.y + cell.h);
}

/// Paint one atlas cell of a `g_tNormalRoughness` map. The normal is a tangent-space
/// bump derived from the (resized) albedo's luminance via a Sobel height gradient;
/// roughness is the uniform `ns.roughness` packed into B. Deadlock `pbr.vfx` packs
/// R,G = normal.xy (0.5 = flat), B = roughness, A = 1. Cells without an albedo image
/// (flat-color groups) get a flat normal at the target roughness.
///
/// Mirrors [`paint_atlas_cell`]'s resize + gutter footprint so the normal aligns
/// texel-for-texel with the color atlas under the shared UVs.
pub(crate) fn paint_normal_cell(
    px: &mut [u8],
    atlas: u32,
    cell: AtlasCell,
    image: Option<&image::RgbaImage>,
    ns: NormalSynthesis,
) {
    let rough = (ns.roughness.clamp(0.0, 1.0) * 255.0).round() as u8;
    let flat = [128u8, 128, rough, 255];
    let (_, _, iw, ih) = cell.inner();
    let Some(img) = image else {
        for yy in 0..cell.h {
            for xx in 0..cell.w {
                write_atlas_pixel(px, atlas, cell.x + xx, cell.y + yy, flat);
            }
        }
        return;
    };
    let resized = image::imageops::resize(img, iw, ih, image::imageops::FilterType::Lanczos3);
    // Luminance height field over the resized cell image.
    let lum = |x: u32, y: u32| -> f32 {
        let p = resized.get_pixel(x.min(iw - 1), y.min(ih - 1)).0;
        (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])) / 255.0
    };
    for yy in 0..cell.h {
        let sy = yy.saturating_sub(cell.gutter).min(ih - 1);
        for xx in 0..cell.w {
            let sx = xx.saturating_sub(cell.gutter).min(iw - 1);
            // Central differences (clamped at edges) -> tangent-space normal.
            let dx = lum(sx + 1, sy) - lum(sx.saturating_sub(1), sy);
            let dy = lum(sx, sy + 1) - lum(sx, sy.saturating_sub(1));
            let nx = -dx * ns.strength;
            let ny = -dy * ns.strength;
            let inv = 1.0 / (nx * nx + ny * ny + 1.0).sqrt();
            let r = ((nx * inv * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
            let g = ((ny * inv * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
            write_atlas_pixel(px, atlas, cell.x + xx, cell.y + yy, [r, g, rough, 255]);
        }
    }
}

fn write_atlas_pixel(px: &mut [u8], atlas: u32, x: u32, y: u32, rgba: [u8; 4]) {
    let o = ((y * atlas + x) * 4) as usize;
    px[o..o + 4].copy_from_slice(&rgba);
}

pub fn read_glb_primitives(
    glb: &[u8],
    orient: SoulOrient,
    rotate: Option<[f32; 3]>,
) -> Result<(Vec<ImportPrimitive>, String)> {
    let parsed = gltf::Gltf::from_slice_without_validation(glb)
        .map_err(|e| anyhow!("failed to parse GLB: {e}"))?;
    let buffers = gltf::import_buffers(&parsed.document, None, parsed.blob)
        .map_err(|e| anyhow!("failed to read GLB buffers: {e}"))?;
    let doc = parsed.document;
    let world = node_world_transforms(&doc);
    let mut prims = Vec::new();
    for node in doc.nodes() {
        let Some(mesh) = node.mesh() else { continue };
        let node_world = world[node.index()];
        for prim in mesh.primitives() {
            if prim.mode() != Mode::Triangles {
                return Err(anyhow!(
                    "unsupported GLB primitive mode {:?}; only triangles are supported",
                    prim.mode()
                ));
            }
            prims.push(read_glb_primitive(&buffers, &node_world, &prim)?);
        }
    }
    if prims.is_empty() {
        return Err(anyhow!("glb has no mesh parts"));
    }

    let orientation = resolve_orientation(orient, rotate, &prims);
    for prim in &mut prims {
        for p in &mut prim.vertex_buffer.positions {
            *p = transform3(&orientation.matrix, *p);
        }
        for n in &mut prim.vertex_buffer.normals {
            *n = normalize3(transform3(&orientation.matrix, *n));
        }
    }

    Ok((prims, orientation.label))
}

fn read_glb_primitive(
    buffers: &[gltf::buffer::Data],
    node_world: &Mat4,
    prim: &gltf::Primitive<'_>,
) -> Result<ImportPrimitive> {
    let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| anyhow!("mesh primitive has no POSITION"))?
        .map(|p| gltf_to_source_point(transform_point4(node_world, p)))
        .collect();
    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(|ns| {
            ns.map(|n| normalize3(gltf_to_source_vector(transform_vector4(node_world, n))))
                .collect()
        })
        .unwrap_or_default();
    let texcoords: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|t| t.into_f32().collect())
        .unwrap_or_default();
    let indices: Vec<u32> = reader
        .read_indices()
        .ok_or_else(|| anyhow!("mesh primitive has no indices"))?
        .into_u32()
        .collect();
    if !indices.len().is_multiple_of(3) {
        return Err(anyhow!(
            "triangle primitive has {} indices, not a multiple of 3",
            indices.len()
        ));
    }

    let texcoords = if texcoords.is_empty() {
        Vec::new()
    } else {
        vec![texcoords]
    };
    let vb = VertexBuffer {
        element_count: positions.len(),
        positions,
        normals,
        texcoords,
        ..VertexBuffer::default()
    };
    Ok(ImportPrimitive {
        material_name: prim.material().name().map(str::to_owned),
        vertex_buffer: vb,
        indices,
    })
}

fn resolve_orientation(
    orient: SoulOrient,
    rotate: Option<[f32; 3]>,
    prims: &[ImportPrimitive],
) -> Orientation {
    let mut base = match orient {
        SoulOrient::YUp => Orientation {
            label: "y-up".to_string(),
            matrix: IDENTITY3,
        },
        SoulOrient::ZUp => Orientation {
            label: "z-up".to_string(),
            matrix: rotate_x(90.0),
        },
        SoulOrient::FlipY => Orientation {
            label: "flip-y".to_string(),
            matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]],
        },
        SoulOrient::Auto => auto_orientation(prims),
    };

    if let Some([x, y, z]) = rotate {
        if x != 0.0 || y != 0.0 || z != 0.0 {
            base.matrix = mat3_mul(euler_xyz(x, y, z), base.matrix);
            base.label = format!("{} + rotate({x},{y},{z})", base.label);
        }
    }
    base
}

fn auto_orientation(prims: &[ImportPrimitive]) -> Orientation {
    // Score candidate rotations about X (the common up-axis cases, including the
    // -90 the Sketchfab Z-up root wrap needs) in the FINAL mesh space, i.e. after
    // the merge-loop swizzle the assembler applies. The old picker only tried
    // identity and +90, measured the pre-swizzle span, and never checked the
    // up-sign, so it both missed the right rotation and could land a model
    // upside-down. We prefer the tallest vertical axis, then the most
    // bottom-heavy result (right-side-up), since props sit on their base.
    let candidates = [
        ("auto:y-up", IDENTITY3),
        ("auto:z-up", rotate_x(90.0)),
        ("auto:z-up-inv", rotate_x(-90.0)),
        ("auto:flip", rotate_x(180.0)),
    ];
    let mut best: Option<(f32, f32, &str, Mat3)> = None;
    for (label, matrix) in candidates {
        let (z_span, lift) = vertical_metrics(prims, &matrix);
        let better = match best {
            None => true,
            // "Clearly taller" wins outright; within ~2% it's a tie on height, so
            // pick the more bottom-heavy (smaller centroid-vs-center) orientation.
            Some((best_span, best_lift, _, _)) => {
                if (z_span - best_span).abs() > best_span.max(1.0) * 0.02 {
                    z_span > best_span
                } else {
                    lift < best_lift
                }
            }
        };
        if better {
            best = Some((z_span, lift, label, matrix));
        }
    }
    let (_, _, label, matrix) = best.expect("candidate list is non-empty");
    Orientation {
        label: label.to_string(),
        matrix,
    }
}

/// Vertical span and "lift" of a candidate orientation, measured in the FINAL
/// mesh space (after the assembler's `[x, z, -y]` swizzle, so Source-up = `-c[1]`).
/// `lift` is `centroid_z - center_z`: negative means bottom-heavy (right-side-up).
fn vertical_metrics(prims: &[ImportPrimitive], matrix: &Mat3) -> (f32, f32) {
    let mut zmin = f32::INFINITY;
    let mut zmax = f32::NEG_INFINITY;
    let mut zsum = 0.0_f64;
    let mut count = 0_u64;
    for prim in prims {
        for &p in &prim.vertex_buffer.positions {
            let z = -transform3(matrix, p)[1];
            zmin = zmin.min(z);
            zmax = zmax.max(z);
            zsum += f64::from(z);
            count += 1;
        }
    }
    if count == 0 {
        return (0.0, 0.0);
    }
    let span = zmax - zmin;
    let center = f32::midpoint(zmin, zmax);
    let centroid = (zsum / count as f64) as f32;
    (span, centroid - center)
}

fn node_world_transforms(doc: &gltf::Document) -> Vec<Mat4> {
    let mut out = vec![IDENTITY4; doc.nodes().count()];
    for scene in doc.scenes() {
        for node in scene.nodes() {
            accumulate_node(&node, IDENTITY4, &mut out);
        }
    }
    out
}

fn accumulate_node(node: &gltf::Node<'_>, parent: Mat4, out: &mut [Mat4]) {
    let world = mat4_mul(parent, node.transform().matrix());
    out[node.index()] = world;
    for child in node.children() {
        accumulate_node(&child, world, out);
    }
}

#[allow(clippy::many_single_char_names)]
fn mat4_mul(a: Mat4, b: Mat4) -> Mat4 {
    let mut r = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            for k in 0..4 {
                r[col][row] += a[k][row] * b[col][k];
            }
        }
    }
    r
}

#[allow(clippy::many_single_char_names)]
fn transform_point4(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let v = [p[0], p[1], p[2], 1.0];
    let mut o = [0.0f32; 3];
    for (row, oo) in o.iter_mut().enumerate() {
        for (col, &vc) in v.iter().enumerate() {
            *oo += m[col][row] * vc;
        }
    }
    o
}

#[allow(clippy::many_single_char_names)]
fn transform_vector4(m: &Mat4, v: [f32; 3]) -> [f32; 3] {
    let v = [v[0], v[1], v[2], 0.0];
    let mut o = [0.0f32; 3];
    for (row, oo) in o.iter_mut().enumerate() {
        for (col, &vc) in v.iter().enumerate() {
            *oo += m[col][row] * vc;
        }
    }
    o
}

fn gltf_to_source_point(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[2], -p[1]]
}

fn gltf_to_source_vector(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[2], -v[1]]
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len <= f32::EPSILON {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

fn transform3(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn mat3_mul(a: Mat3, b: Mat3) -> Mat3 {
    let mut r = [[0.0f32; 3]; 3];
    for row in 0..3 {
        for col in 0..3 {
            for k in 0..3 {
                r[row][col] += a[row][k] * b[k][col];
            }
        }
    }
    r
}

fn euler_xyz(x: f32, y: f32, z: f32) -> Mat3 {
    mat3_mul(rotate_z(z), mat3_mul(rotate_y(y), rotate_x(x)))
}

fn rotate_x(deg: f32) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn rotate_y(deg: f32) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

fn rotate_z(deg: f32) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

// --- material helpers ---

fn texture_param_index(v: &Kv3, name: &str) -> Option<usize> {
    v.get("m_textureParams")?
        .as_array()?
        .iter()
        .position(|p| p.get("m_name").and_then(Kv3::as_str) == Some(name))
}

fn texture_param(v: &Kv3, name: &str) -> Option<String> {
    let i = texture_param_index(v, name)?;
    v.get("m_textureParams")?
        .as_array()?
        .get(i)?
        .get("m_pValue")?
        .as_str()
        .map(str::to_string)
}

fn texture_pvalue_path(mat: &Kv3, slot: &str) -> Option<Vec<Seg>> {
    let i = texture_param_index(mat, slot)?;
    Some(vec![
        Seg::Key("m_textureParams".to_string()),
        Seg::Index(i),
        Seg::Key("m_pValue".to_string()),
    ])
}

/// Clean material = donor copy, `g_tColor` -> our atlas. `g_tNormalRoughness` is
/// repointed to `normal_vtex` when given (a synthesized relief+roughness map), else
/// reset to the flat default. Byte-faithful blob-aware string add (no re-encode).
pub(crate) fn build_material(color_vtex: &str, normal_vtex: Option<&str>) -> Result<Vec<u8>> {
    let vmat =
        morphic::decode_kv3_resource(DONOR_VMAT).map_err(|e| anyhow!("decoding donor: {e}"))?;
    let mut edits: Vec<(Vec<Seg>, String)> = Vec::new();
    let color_path =
        texture_pvalue_path(&vmat, "g_tColor").ok_or_else(|| anyhow!("donor has no g_tColor"))?;
    edits.push((color_path, color_vtex.to_string()));
    if let Some(p) = texture_pvalue_path(&vmat, "g_tNormalRoughness") {
        // With a synthesized normal, always repoint to it. Otherwise only overwrite a
        // prop-local donor normal with the flat default (a default path is left as-is).
        if let Some(nv) = normal_vtex {
            edits.push((p, nv.to_string()));
        } else if texture_param(&vmat, "g_tNormalRoughness")
            .is_some_and(|p| !p.starts_with("materials/default/"))
        {
            edits.push((p, DEFAULT_NORMAL.to_string()));
        }
    }
    let patched = morphic::patch_kv3_resource_strings_adding(DONOR_VMAT, &edits)
        .map_err(|e| anyhow!("repoint: {e}"))?;
    let check = morphic::decode_kv3_resource(&patched).map_err(|e| anyhow!("re-decode: {e}"))?;
    if texture_param(&check, "g_tColor").as_deref() != Some(color_vtex) {
        return Err(anyhow!("g_tColor repoint did not take"));
    }
    if let Some(nv) = normal_vtex {
        if texture_param(&check, "g_tNormalRoughness").as_deref() != Some(nv) {
            return Err(anyhow!("g_tNormalRoughness repoint did not take"));
        }
    }
    Ok(patched)
}

/// One material group: its source primitives, atlas cell, and color/image.
struct Group {
    glb_material: Option<String>,
    prims: Vec<usize>,
    albedo: Option<AlbedoTexture>,
    color: [f64; 4], // linear baseColorFactor (flat fallback)
    index_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_cell_packs_roughness_in_blue_and_flat_normal_without_image() {
        // Deadlock pbr.vfx g_tNormalRoughness: R,G = normal.xy (128 = flat),
        // B = roughness, A = 255. A flat-color group (no image) must emit a flat
        // normal at the target roughness across the whole cell.
        let atlas = 8u32;
        let mut px = vec![0u8; (atlas * atlas * 4) as usize];
        let ns = NormalSynthesis {
            strength: 1.0,
            roughness: 0.5,
        };
        paint_normal_cell(&mut px, atlas, AtlasCell::new(0, 0, 8, 8), None, ns);
        for p in px.chunks_exact(4) {
            assert_eq!(p, [128, 128, 128, 255]); // 0.5 roughness -> B = 128
        }
    }

    #[test]
    fn normal_cell_emits_relief_from_image_contrast() {
        // A cell with a hard light/dark luminance edge must produce normals that
        // deviate from flat (128) at the edge, while roughness stays pinned in B.
        let atlas = 16u32;
        let mut img = image::RgbaImage::new(16, 16);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            let v = if x < 8 { 0 } else { 255 };
            *p = image::Rgba([v, v, v, 255]);
        }
        let mut px = vec![0u8; (atlas * atlas * 4) as usize];
        let ns = NormalSynthesis {
            strength: 4.0,
            roughness: 0.25,
        };
        paint_normal_cell(&mut px, atlas, AtlasCell::new(0, 0, 16, 16), Some(&img), ns);
        let rough = (0.25 * 255.0_f32).round() as u8;
        let mut saw_relief = false;
        for p in px.chunks_exact(4) {
            assert_eq!(p[2], rough, "roughness must be pinned in B");
            assert_eq!(p[3], 255);
            if p[0] != 128 || p[1] != 128 {
                saw_relief = true;
            }
        }
        assert!(
            saw_relief,
            "a luminance edge should bend the normal off flat"
        );
    }

    #[test]
    fn atlas_uv_repeat_and_clamp_follow_gltf_sampler_modes() {
        let cell = AtlasCell::new(128, 64, 128, 128);
        let repeat = remap_atlas_uv(
            [1.25, -0.25],
            cell,
            512,
            WrapMode::Repeat,
            WrapMode::Repeat,
            true,
        );
        let clamped = remap_atlas_uv(
            [1.25, -0.25],
            cell,
            512,
            WrapMode::ClampToEdge,
            WrapMode::ClampToEdge,
            true,
        );

        assert!((repeat[0] - ((132.5 + 0.25 * 119.0) / 512.0)).abs() < 0.0001);
        assert!((repeat[1] - ((68.5 + 0.75 * 119.0) / 512.0)).abs() < 0.0001);
        assert!((clamped[0] - ((132.5 + 119.0) / 512.0)).abs() < 0.0001);
        assert!((clamped[1] - (68.5 / 512.0)).abs() < 0.0001);
    }

    #[test]
    fn textured_atlas_cell_forces_opaque_alpha_and_duplicates_edge_gutter() {
        let mut img = image::RgbaImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgba([10, 20, 30, 0]));
        img.put_pixel(1, 0, image::Rgba([40, 50, 60, 64]));
        img.put_pixel(0, 1, image::Rgba([70, 80, 90, 128]));
        img.put_pixel(1, 1, image::Rgba([100, 110, 120, 192]));

        let mut px = vec![0u8; 16 * 16 * 4];
        paint_atlas_cell(
            &mut px,
            16,
            AtlasCell::new(4, 4, 8, 8),
            Some(&img),
            [1.0; 4],
        );

        let top_left = ((4 * 16 + 4) * 4) as usize;
        let inner_top_left = ((7 * 16 + 7) * 4) as usize;
        let bottom_right = ((11 * 16 + 11) * 4) as usize;
        assert_eq!(
            &px[top_left..top_left + 4],
            &px[inner_top_left..inner_top_left + 4]
        );
        assert_eq!(px[top_left + 3], 255);
        assert_eq!(px[bottom_right + 3], 255);
    }
}
