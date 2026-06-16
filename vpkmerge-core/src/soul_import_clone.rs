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
const DONOR_VMAT: &[u8] = include_bytes!("../../morphic/fixtures/soul/soul_material_donor.vmat_c");
const DEFAULT_NORMAL: &str = "materials/default/default_normal_tga_7be61377.vtex";
const FLAT_DONOR: &str = "panorama/images/hud/zipline_icon_psd.vtex_c";
const COLOR_DONOR: &str = "dev/helper/testgrid_color_tga_2d6cc34.vtex_c";
const IDENTITY3: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
const IDENTITY4: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

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
    pub glow: SoulGlow,
}

impl Default for SoulImportCloneOptions {
    fn default() -> Self {
        Self {
            name: "custom_soul".to_string(),
            orient: SoulOrient::YUp,
            rotate: None,
            glow: SoulGlow::Recolor,
        }
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
#[allow(
    clippy::too_many_lines,
    clippy::many_single_char_names,
    clippy::similar_names
)]
pub fn import_soul_container_clone(
    pak: impl AsRef<Path>,
    glb: &[u8],
    out: impl AsRef<Path>,
    opts: &SoulImportCloneOptions,
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
                image: gi
                    .map(|mi| material_albedo_image(glb, &doc, mi))
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
    let (atlas, donor_entry) = if vpk.get_file(COLOR_DONOR).is_ok() {
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
        let (u0, v0) = (
            f64::from(x0) / f64::from(atlas),
            f64::from(y0) / f64::from(atlas),
        );
        let (us, vs) = (
            f64::from(w) / f64::from(atlas),
            f64::from(h) / f64::from(atlas),
        );
        let textured = g.image.is_some();
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
                let (u, v) = if textured {
                    (
                        u0 + f64::from(uv[0].clamp(0.0, 1.0)) * us,
                        v0 + f64::from(uv[1].clamp(0.0, 1.0)) * vs,
                    )
                } else {
                    (u0 + us / 2.0, v0 + vs / 2.0) // flat: sample the cell centre
                };
                merged.texcoords[0].push([u as f32, v as f32]);
            }
            indices.extend(prims[pi].indices.iter().map(|&idx| base + idx));
        }
        g.index_count = indices.len() - start;
    }
    merged.element_count = merged.positions.len();

    // --- 4. fit to the orb's bounds ---
    let model_bytes = read(MODEL)?;
    let orb = morphic::model::decode(&model_bytes)
        .map_err(|e| anyhow!("decode orb: {e}"))?
        .position_bounds()
        .ok_or_else(|| anyhow!("orb has no positions"))?;
    let orb_center = [
        midpoint(orb.min[0], orb.max[0]),
        midpoint(orb.min[1], orb.max[1]),
        midpoint(orb.min[2], orb.max[2]),
    ];
    let orb_size = (0..3)
        .map(|k| orb.max[k] - orb.min[k])
        .fold(0.0_f32, f32::max);
    let (mc, ms) = bounds_center_extent(&merged.positions);
    let scale = if ms > 0.0 { orb_size / ms } else { 1.0 };
    for p in &mut merged.positions {
        for k in 0..3 {
            p[k] = (p[k] - mc[k]) * scale + orb_center[k];
        }
    }
    let tri_count = indices.len() / 3;

    // --- 5. swap mesh in (UNCOMPRESSED) + repoint the single draw call ---
    let (mesh_swapped, _rep) =
        replace_mesh_part_uncompressed(&model_bytes, "soul_container", &merged, &indices)
            .map_err(|e| anyhow!("replacing mesh: {e}"))?;
    let vmat_path = format!("{MAT_DIR}/{name}.vmat");
    let edited_model = set_model_material(&mesh_swapped, &vmat_path)
        .map_err(|e| anyhow!("repoint material: {e}"))?;

    // --- 6. atlas albedo: each group's cell = its image (resized) or flat colour ---
    let donor = read(donor_entry)?;
    let mut px = vec![0u8; (atlas * atlas * 4) as usize];
    for (gi, g) in groups.iter().enumerate() {
        let (x0, y0, w, h) = cell_rect(gi);
        if let Some(img) = &g.image {
            let r = image::imageops::resize(img, w, h, image::imageops::FilterType::Lanczos3);
            for yy in 0..h {
                for xx in 0..w {
                    let s = r.get_pixel(xx, yy).0;
                    let o = (((y0 + yy) * atlas + (x0 + xx)) * 4) as usize;
                    px[o..o + 4].copy_from_slice(&s);
                }
            }
        } else {
            let c = [
                to_srgb_u8(g.color[0]),
                to_srgb_u8(g.color[1]),
                to_srgb_u8(g.color[2]),
                255,
            ];
            for yy in 0..h {
                for xx in 0..w {
                    let o = (((y0 + yy) * atlas + (x0 + xx)) * 4) as usize;
                    px[o..o + 4].copy_from_slice(&c);
                }
            }
        }
    }
    let color_vtex = format!("{MAT_DIR}/{name}_color.vtex");
    let color_entry = format!("{MAT_DIR}/{name}_color.vtex_c");
    let color_tex = replace_mip_chain(
        &donor,
        &Image {
            width: atlas,
            height: atlas,
            data: ImageData::Rgba8(px),
        },
    )
    .map_err(|e| anyhow!("encoding atlas: {e}"))?;
    let vmat = build_material(&color_vtex)?;

    // --- 7. recolor the soul-glow particles to the import's dominant hue ---
    let dom = groups.iter().max_by_key(|g| g.index_count);
    let dom_rgb = dom.map_or([255.0, 200.0, 60.0], |g| {
        g.image.as_ref().map_or(
            [
                to_srgb_u8(g.color[0]) as f64,
                to_srgb_u8(g.color[1]) as f64,
                to_srgb_u8(g.color[2]) as f64,
            ],
            |img| {
                let (mut r, mut gg, mut b, mut n) = (0f64, 0f64, 0f64, 0f64);
                for p in img.pixels() {
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
        (MODEL.to_string(), edited_model),
        (format!("{MAT_DIR}/{name}.vmat_c"), vmat),
        (color_entry, color_tex),
    ];
    // SoulGlow::Off = don't ship particles (base game gold glow plays);
    // Base = ship unchanged (isolation); Recolor (default) = recolor to hue.
    if opts.glow != SoulGlow::Off {
        for p in PARTICLES {
            if let Ok(base) = read(p) {
                let bytes = if opts.glow == SoulGlow::Recolor {
                    recolor_particle_bytes(&base, Recolor::hue(hue))?.unwrap_or(base)
                } else {
                    base
                };
                entries.push((p.to_string(), bytes));
            }
        }
    }
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    crate::pack(&refs, out.as_ref())?;

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
        glow_hue: hue,
        entry_count: refs.len(),
    })
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

struct ImportPrimitive {
    material_name: Option<String>,
    vertex_buffer: VertexBuffer,
    indices: Vec<u32>,
}

struct Orientation {
    label: String,
    matrix: Mat3,
}

fn glb_json(glb: &[u8]) -> Result<Json> {
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

fn material_index_by_name(doc: &Json) -> HashMap<String, usize> {
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

fn base_color_factor(doc: &Json, mat_idx: usize) -> [f64; 4] {
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

fn material_albedo_image(
    glb: &[u8],
    doc: &Json,
    mat_idx: usize,
) -> Result<Option<image::RgbaImage>> {
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
    let Some(src) = doc
        .get("textures")
        .and_then(Json::as_array)
        .and_then(|a| a.get(tex_i as usize))
        .and_then(|t| t.get("source"))
        .and_then(Json::as_u64)
    else {
        return Ok(None);
    };
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
    Ok(Some(img))
}

fn read_glb_primitives(
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

/// Clean material = donor copy, `g_tColor` -> our atlas, prop-local normal -> flat
/// default. Byte-faithful blob-aware string add (no re-encode).
fn build_material(color_vtex: &str) -> Result<Vec<u8>> {
    let vmat =
        morphic::decode_kv3_resource(DONOR_VMAT).map_err(|e| anyhow!("decoding donor: {e}"))?;
    let mut edits: Vec<(Vec<Seg>, String)> = Vec::new();
    let color_path =
        texture_pvalue_path(&vmat, "g_tColor").ok_or_else(|| anyhow!("donor has no g_tColor"))?;
    edits.push((color_path, color_vtex.to_string()));
    if let Some(p) = texture_pvalue_path(&vmat, "g_tNormalRoughness") {
        if texture_param(&vmat, "g_tNormalRoughness")
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
    Ok(patched)
}

/// One material group: its source primitives, atlas cell, and color/image.
struct Group {
    glb_material: Option<String>,
    prims: Vec<usize>,
    image: Option<image::RgbaImage>,
    color: [f64; 4], // linear baseColorFactor (flat fallback)
    index_count: usize,
}
