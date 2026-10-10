//! Build the Abrams "Rem is the book" addon.
//!
//! Replaces Abrams's four held-book LOD mesh parts with static Rem meshes, rigidly
//! skins them to `book_0`, atlases Rem's five material sets into the existing
//! one-draw-call book material, and packs a self-contained addon VPK.
//!
//! ## Why one draw call and not five
//!
//! Rem has five materials, and binding each to its own draw call is the obvious
//! fix for the fidelity the atlas costs. It does not work: a build that grew
//! `m_drawCalls` (via `set_part_draw_call_groups`) and bound each call to Rem's
//! own `familiar_*.vmat` made Source 2 reject the whole `.vmdl_c` -- Abrams
//! rendered as the red ERROR model (2026-08-04). Both morphic and VRF parse that
//! model happily, so nothing offline catches it. The exact trigger was not
//! isolated; the untested pieces were the KV3 array growth itself and the
//! cross-model material references, which the model's `RERL` never precaches.
//!
//! So the material stays single -- the shape the previous working build had --
//! and the fidelity the atlas used to lose is bought back instead by:
//!
//! - taking the **donor from Rem's own `familiar_body.vmat_c`**, so `F_SELF_ILLUM`,
//!   `F_DETAIL` and the NPR outline come from a shipped material rather than from
//!   flipping static feature flags (which renders the error shader on hero
//!   materials);
//! - atlasing a real **self-illum** channel alongside colour, so the eyes read at
//!   all and the banner rim and potion glow again;
//! - a **4096** atlas rather than 2048, which takes each source from about a sixth
//!   of its texel density to about two thirds.
//!
//! Usage:
//!   cargo run --release -p vpkmerge-core --example abrams_rem_book -- \
//!     <pak01_dir.vpk> <lod0.glb> <lod1.glb> <lod2.glb> <lod3.glb> <out_dir.vpk>
//!     [--motion]

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::doc_markdown
)]

use std::collections::BTreeMap;

use anyhow::{anyhow, Context, Result};
use morphic::kv3::{Seg, Value};
use morphic::model::{read_edited_primitives, replace_mesh_part_uncompressed, VertexBuffer};
use morphic::{Image, ImageData};
use serde_json::Value as Json;

const MODEL_ENTRY: &str = "models/heroes_wip/abrams/abrams.vmdl_c";
const BOOK_MATERIAL: &str = "models/heroes_wip/abrams/abrams_book/materials/abrams_book.vmat";
const BOOK_MATERIAL_ENTRY: &str =
    "models/heroes_wip/abrams/abrams_book/materials/abrams_book.vmat_c";
const MAT_DIR: &str = "models/heroes_wip/abrams/abrams_book/materials";
const AMBIENT_BOOK_PARTICLE: &str = "particles/abilities/abrams/abrams_ambient_book.vpcf";

/// Rem's own body material, used as the `compile_pbr_vmat` donor so the merged
/// book material inherits his shader feature set instead of a generic one.
const VMAT_DONOR_ENTRY: &str = "models/heroes_wip/familiar/materials/familiar_body.vmat_c";

const NM_SKELETON_ENTRY: &str = "models/heroes_wip/abrams/abrams.vnmskel_c";
const CLIP_DIR: &str = "models/heroes_wip/abrams/clips";

const ATLAS_SIZE: u32 = 4096;
const GUTTER: u32 = 8;

const BOOK_PARTS: [&str; 4] = [
    "book_model",
    "book_model_lod1",
    "book_model_lod2",
    "book_model_lod3",
];

const MATERIAL_ORDER: [&str; 5] = [
    "familiar_clothes",
    "familiar_accessories",
    "familiar_body",
    "familiar_head",
    "familiar_eyes",
];

/// Experimental idle motion. Rem is bound across this ladder instead of rigidly
/// to `book_0`, and each bone gains a sway rotation in [`IDLE_CLIPS`].
///
/// Off by default: Valve animates these same bones in about six clips
/// (`dash_ground` opens the book fully), and a prop bound to them deforms with
/// the book opening, so the chain binding is only safe once those clips are
/// handled too. The clip-edit half has also never been confirmed in game.
const SPINE_CHAIN: [&str; 4] = [
    "flip_a_page_0",
    "flip_a_page_1",
    "flip_a_page_2",
    "flip_a_page_3",
];

const IDLE_CLIPS: [&str; 6] = [
    "weapon_stand_idle",
    "out_of_combat_stand_idle",
    "item_stand_idle",
    "syphon_stand_idle",
    "weapon_crouch_idle",
    "aim_weapon_idle",
];

/// Per-material self-illum gain, applied to that material's cell of the
/// self-illum atlas.
///
/// The merged material carries **one** `F_SELF_ILLUM` and **one**
/// `g_flSelfIllumScale1` (0.85, inherited from the `familiar_body` donor), but
/// Rem's five originals do not agree: `familiar_clothes` has no `F_SELF_ILLUM` at
/// all yet still binds the white `default_mask`, and `familiar_accessories`
/// leaves its scale at 0 for an entity expression to drive. Copying their masks
/// in raw therefore makes his whole coat glow at full strength and washes him
/// out. Each gain is `stock_scale / 0.85`, or 0 where the material does not
/// actually self-illuminate, so the merged material reproduces the stock look.
const SELF_ILLUM_GAIN: [(&str, f32); 5] = [
    ("familiar_clothes", 0.0), // no F_SELF_ILLUM
    // Static scale is 0, but `g_flSelfIllumScale1 = $SELFILLUM` drives it from
    // the entity, and the stock render clearly glows: this is the banner star,
    // its rim, and the potion. Approximate the driven value with the donor's own
    // scale rather than dropping the glow entirely.
    ("familiar_accessories", 1.0),
    ("familiar_body", 1.0),   // scale 0.85
    ("familiar_head", 1.331), // scale 1.131
    ("familiar_eyes", 3.059), // scale 2.6, clamps at white
];

const SWAY_DEGREES: [f32; 4] = [1.1, 1.3, 1.5, 1.7];
const SWAY_LAG: f32 = 0.38;
const SWAY_CYCLES: f32 = 1.0;
const ROLL_RATIO: f32 = 0.45;

// Blender meters to Source inches. The source Rem and Abrams exports both use
// this wrapper scale, so baking it back restores Abrams model space exactly.
const SOURCE_UNITS_PER_METER: f32 = 39.370_08;

#[derive(Clone, Copy)]
struct Cell {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Clone, Copy)]
enum AtlasKind {
    Color,
    SelfIllum,
    NormalRoughness,
    AmbientOcclusion,
}

impl AtlasKind {
    fn slot(self) -> (&'static str, &'static str) {
        match self {
            AtlasKind::Color => ("g_tColor", "rem_book_color"),
            AtlasKind::SelfIllum => ("g_tSelfIllumMask", "rem_book_selfillum"),
            AtlasKind::NormalRoughness => ("g_tNormalRoughness", "rem_book_normal_roughness"),
            AtlasKind::AmbientOcclusion => ("g_tAmbientOcclusion", "rem_book_ao"),
        }
    }

    /// A shipped texture whose format and flags suit this channel, spliced by
    /// [`morphic::replace_mip_chain`]. An inline-PNG albedo is rejected in game,
    /// so every atlas has to ride a real compiled donor.
    /// Each donor must already be [`ATLAS_SIZE`] square: `replace_mip_chain`
    /// splices pixels into an existing mip chain and cannot resize.
    fn donor(self) -> &'static str {
        match self {
            AtlasKind::Color => {
                "models/heroes_staging/chrono/materials/chrono_v2_color_png_d1d22ba7.vtex_c"
            }
            AtlasKind::SelfIllum => {
                "models/heroes_staging/chrono/materials/chrono_v2_ao_png_9ef0831f.vtex_c"
            }
            AtlasKind::NormalRoughness => {
                "models/heroes_staging/engineer/materials/engineer_normal_png_b3019aa5.vtex_c"
            }
            AtlasKind::AmbientOcclusion => {
                "models/heroes_staging/engineer/materials/engineer_ao_png_7fc2d722.vtex_c"
            }
        }
    }
}

const ATLASES: [AtlasKind; 4] = [
    AtlasKind::Color,
    AtlasKind::SelfIllum,
    AtlasKind::NormalRoughness,
    AtlasKind::AmbientOcclusion,
];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let motion = args.iter().any(|a| a == "--motion");
    let positional: Vec<&String> = args[1..].iter().filter(|a| !a.starts_with("--")).collect();
    if positional.len() != 6 {
        anyhow::bail!(
            "usage: abrams_rem_book <pak01_dir.vpk> <lod0.glb> <lod1.glb> \
             <lod2.glb> <lod3.glb> <out_dir.vpk> [--motion]"
        );
    }
    let pak = positional[0];
    let lod_paths = [positional[1], positional[2], positional[3], positional[4]];
    let out = positional[5];

    let mut model_bytes = vpkmerge_core::read_vpk_entry(pak, MODEL_ENTRY)
        .with_context(|| format!("reading {MODEL_ENTRY}"))?;
    let model = morphic::model::decode(&model_bytes).context("decoding Abrams model")?;
    let bone_index = |name: &str| -> Result<u16> {
        let index = model
            .skeleton
            .bones
            .iter()
            .position(|bone| bone.name == name)
            .ok_or_else(|| anyhow!("Abrams skeleton has no {name} bone"))?;
        u16::try_from(index).context("bone index exceeds u16")
    };

    let binding = if motion {
        let mut chain = [0u16; 4];
        for (slot, name) in chain.iter_mut().zip(SPINE_CHAIN) {
            *slot = bone_index(name)?;
        }
        Binding::Chain(chain)
    } else {
        Binding::Rigid(bone_index("book_0")?)
    };

    let cell_map = atlas_cells();
    let mut lod_reports = Vec::new();
    for (part, path) in BOOK_PARTS.into_iter().zip(lod_paths) {
        let glb = std::fs::read(path).with_context(|| format!("reading {path}"))?;
        let (mesh, indices) = build_prop_mesh(&glb, binding, &cell_map)
            .with_context(|| format!("assembling {part} from {path}"))?;
        let (next, report) = replace_mesh_part_uncompressed(&model_bytes, part, &mesh, &indices)
            .map_err(|e| anyhow!("replacing {part}: {e}"))?;
        model_bytes = next;
        lod_reports.push((part, report.new_vertex_count, report.new_index_count / 3));
    }

    // The stock model spawns a glowing rectangular outline around the book.
    // It no longer matches the replacement prop, so remove that one-item model
    // particle list while preserving all other model metadata.
    model_bytes = disable_ambient_book_particle(&model_bytes)?;

    // Every atlas rides a compiled donor of a suitable format.
    let lod0 = std::fs::read(lod_paths[0])?;
    let mut textures: Vec<(String, Vec<u8>)> = Vec::new();
    let mut slot_paths: Vec<(String, String)> = Vec::new();
    for kind in ATLASES {
        let (slot, stem) = kind.slot();
        let pixels = build_atlas(&lod0, &cell_map, kind)
            .with_context(|| format!("building {stem} atlas"))?;
        let donor = vpkmerge_core::read_vpk_entry(pak, kind.donor())
            .with_context(|| format!("reading atlas donor {}", kind.donor()))?;
        let encoded = morphic::replace_mip_chain(
            &donor,
            &Image {
                width: ATLAS_SIZE,
                height: ATLAS_SIZE,
                data: ImageData::Rgba8(pixels),
            },
        )
        .map_err(|e| anyhow!("encoding {stem} atlas: {e}"))?;
        textures.push((format!("{MAT_DIR}/{stem}.vtex_c"), encoded));
        slot_paths.push((slot.to_string(), format!("{MAT_DIR}/{stem}.vtex")));
    }

    // Donor is Rem's own body material, so the merged material keeps his shader
    // feature set (self-illum, scrolling detail, NPR outline) instead of needing
    // a static feature-flag flip.
    let donor_vmat = vpkmerge_core::read_vpk_entry(pak, VMAT_DONOR_ENTRY)
        .with_context(|| format!("reading vmat donor {VMAT_DONOR_ENTRY}"))?;
    let slot_refs: Vec<(&str, &str)> = slot_paths
        .iter()
        .map(|(slot, path)| (slot.as_str(), path.as_str()))
        .collect();
    let vmat = morphic::compile_pbr_vmat(&donor_vmat, BOOK_MATERIAL, &slot_refs)
        .map_err(|e| anyhow!("building Rem book material: {e}"))?;

    let edited_clips = if motion {
        build_idle_motion(pak).context("authoring Rem's idle motion")?
    } else {
        Vec::new()
    };

    // Structural gate before packaging.
    morphic::model::decode(&model_bytes).context("modified Abrams model failed to re-decode")?;
    let final_targets = morphic::model::vertex_targets(&model_bytes)
        .context("modified Abrams vertex registry failed to decode")?;
    for (part, expected, _) in &lod_reports {
        let got = final_targets
            .iter()
            .find(|target| &target.mesh_name == part)
            .map(|target| target.vertex_count);
        anyhow::ensure!(
            got == Some(*expected),
            "post-build {part} vertex count did not match: expected {expected}, got {got:?}"
        );
    }
    let mut entries: Vec<(&str, &[u8])> = vec![
        (MODEL_ENTRY, model_bytes.as_slice()),
        (BOOK_MATERIAL_ENTRY, vmat.as_slice()),
    ];
    for (entry, bytes) in &textures {
        entries.push((entry.as_str(), bytes.as_slice()));
    }
    for (entry, bytes) in &edited_clips {
        entries.push((entry.as_str(), bytes.as_slice()));
    }
    vpkmerge_core::pack(&entries, out)?;

    println!("wrote {out}");
    println!("  model: {MODEL_ENTRY}");
    println!(
        "  skinned to: {}",
        match binding {
            Binding::Rigid(_) => "book_0 (rigid)".to_string(),
            Binding::Chain(_) => SPINE_CHAIN.join(" -> "),
        }
    );
    for (part, vertices, triangles) in lod_reports {
        println!("  {part}: {vertices} vertices, {triangles} triangles");
    }
    println!("  material: {BOOK_MATERIAL_ENTRY} (donor {VMAT_DONOR_ENTRY})");
    for (entry, bytes) in &textures {
        println!(
            "  atlas: {entry} ({ATLAS_SIZE}x{ATLAS_SIZE}, {} bytes)",
            bytes.len()
        );
    }
    for (entry, _) in &edited_clips {
        println!("  idle motion: {entry}");
    }
    Ok(())
}

/// How the prop's vertices attach to Abrams's skeleton.
#[derive(Clone, Copy)]
enum Binding {
    /// Every vertex 100% on one bone. Safe in every clip.
    Rigid(u16),
    /// Blended by height across a parent chain, for authored motion.
    Chain([u16; 4]),
}

fn atlas_cells() -> BTreeMap<String, Cell> {
    let columns = 3u32;
    let rows = 2u32;
    let cell_w = ATLAS_SIZE / columns;
    let cell_h = ATLAS_SIZE / rows;
    MATERIAL_ORDER
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let col = i as u32 % columns;
            let row = i as u32 / columns;
            (
                (*name).to_string(),
                Cell {
                    x: col * cell_w,
                    y: row * cell_h,
                    w: if col + 1 == columns {
                        ATLAS_SIZE - col * cell_w
                    } else {
                        cell_w
                    },
                    h: if row + 1 == rows {
                        ATLAS_SIZE - row * cell_h
                    } else {
                        cell_h
                    },
                },
            )
        })
        .collect()
}

fn canonical_material(name: Option<&str>) -> Result<&'static str> {
    let name = name.ok_or_else(|| anyhow!("Rem primitive has no material name"))?;
    MATERIAL_ORDER
        .iter()
        .copied()
        .find(|candidate| name.to_ascii_lowercase().contains(candidate))
        .ok_or_else(|| anyhow!("unexpected Rem material {name:?}"))
}

fn build_prop_mesh(
    glb: &[u8],
    binding: Binding,
    cells: &BTreeMap<String, Cell>,
) -> Result<(VertexBuffer, Vec<u32>)> {
    let primitives =
        read_edited_primitives(glb).map_err(|e| anyhow!("reading edited GLB primitives: {e}"))?;
    let mut merged = VertexBuffer {
        texcoords: vec![Vec::new()],
        ..VertexBuffer::default()
    };
    let mut indices = Vec::new();

    for primitive in primitives {
        let material = canonical_material(primitive.material_name.as_deref())?;
        let cell = cells
            .get(material)
            .ok_or_else(|| anyhow!("no atlas cell for {material}"))?;
        let vb = primitive.vertex_buffer;
        anyhow::ensure!(
            vb.positions.len() == vb.element_count,
            "{material} position count mismatch"
        );
        let base = u32::try_from(merged.positions.len())?;

        // Blender world -> glTF accessor is [Bx, Bz, -By]. Recover the original
        // Source model axes as [-By, Bx, Bz] = [Gz, Gx, Gy], then restore inches.
        merged.positions.extend(
            vb.positions
                .iter()
                .map(|p| [p[2], p[0], p[1]].map(|value| value * SOURCE_UNITS_PER_METER)),
        );
        merged.normals.extend(vb.normals.iter().map(|n| {
            let mapped = [n[2], n[0], n[1]];
            normalize(mapped)
        }));
        let source_uv = vb.texcoords.first();
        for vertex in 0..vb.element_count {
            let uv = source_uv
                .and_then(|values| values.get(vertex))
                .copied()
                .unwrap_or([0.5, 0.5]);
            merged.texcoords[0].push(remap_uv(uv, *cell));
        }
        indices.extend(primitive.indices.into_iter().map(|index| base + index));
    }

    merged.element_count = merged.positions.len();
    anyhow::ensure!(
        u16::try_from(merged.element_count).is_ok(),
        "prop exceeds 16-bit vertex limit"
    );

    match binding {
        Binding::Rigid(bone) => {
            merged.joints = vec![[bone; 4]; merged.element_count];
            merged.weights = vec![[1.0, 0.0, 0.0, 0.0]; merged.element_count];
        }
        Binding::Chain(chain) => {
            let (low, high) = merged
                .positions
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                    (lo.min(p[2]), hi.max(p[2]))
                });
            anyhow::ensure!(high > low, "Rem prop has no vertical extent");
            merged.joints = Vec::with_capacity(merged.element_count);
            merged.weights = Vec::with_capacity(merged.element_count);
            for position in &merged.positions {
                let (joints, weights) = spine_binding(position[2], low, high, &chain);
                merged.joints.push(joints);
                merged.weights.push(weights);
            }
        }
    }
    Ok((merged, indices))
}

/// Blends one vertex between two neighbouring [`SPINE_CHAIN`] bones by height.
fn spine_binding(z: f32, low: f32, high: f32, chain: &[u16; 4]) -> ([u16; 4], [f32; 4]) {
    let last = chain.len() - 1;
    let t = ((z - low) / (high - low)).clamp(0.0, 1.0) * last as f32;
    let index = (t.floor() as usize).min(last - 1);
    let blend = t - index as f32;
    (
        [chain[index], chain[index + 1], chain[0], chain[0]],
        [1.0 - blend, blend, 0.0, 0.0],
    )
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length <= f32::EPSILON {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / length, v[1] / length, v[2] / length]
    }
}

fn wrap(value: f32) -> f32 {
    value.rem_euclid(1.0)
}

fn remap_uv(uv: [f32; 2], cell: Cell) -> [f32; 2] {
    let inner_w = cell.w.saturating_sub(GUTTER * 2).max(1);
    let inner_h = cell.h.saturating_sub(GUTTER * 2).max(1);
    [
        (cell.x as f32 + GUTTER as f32 + 0.5 + wrap(uv[0]) * (inner_w - 1) as f32)
            / ATLAS_SIZE as f32,
        (cell.y as f32 + GUTTER as f32 + 0.5 + wrap(uv[1]) * (inner_h - 1) as f32)
            / ATLAS_SIZE as f32,
    ]
}

fn glb_json(glb: &[u8]) -> Result<Json> {
    anyhow::ensure!(glb.get(0..4) == Some(b"glTF"), "not a GLB");
    let len = u32::from_le_bytes(glb[12..16].try_into()?) as usize;
    Ok(serde_json::from_slice(&glb[20..20 + len])?)
}

fn glb_bin(glb: &[u8]) -> Result<&[u8]> {
    let mut offset = 12usize;
    while offset + 8 <= glb.len() {
        let len = u32::from_le_bytes(glb[offset..offset + 4].try_into()?) as usize;
        let kind = &glb[offset + 4..offset + 8];
        let body = glb
            .get(offset + 8..offset + 8 + len)
            .ok_or_else(|| anyhow!("GLB chunk extends past EOF"))?;
        if kind == b"BIN\0" {
            return Ok(body);
        }
        offset += 8 + len;
    }
    anyhow::bail!("GLB has no BIN chunk")
}

fn material_image(
    glb: &[u8],
    material_name: &str,
    texture_path: &[&str],
) -> Result<image::RgbaImage> {
    let doc = glb_json(glb)?;
    let materials = doc["materials"]
        .as_array()
        .ok_or_else(|| anyhow!("GLB has no materials"))?;
    let material = materials
        .iter()
        .find(|material| {
            material["name"]
                .as_str()
                .is_some_and(|name| name.to_ascii_lowercase().contains(material_name))
        })
        .ok_or_else(|| anyhow!("GLB has no material {material_name}"))?;
    let mut texture_value = material;
    for key in texture_path {
        texture_value = texture_value.get(*key).ok_or_else(|| {
            anyhow!("material {material_name} has no texture path {texture_path:?}")
        })?;
    }
    let texture_index = texture_value.as_u64().ok_or_else(|| {
        anyhow!("material {material_name} texture path {texture_path:?} is not an index")
    })? as usize;
    let texture = &doc["textures"][texture_index];
    let image_index = texture["source"]
        .as_u64()
        .ok_or_else(|| anyhow!("texture {texture_index} has no image source"))?
        as usize;
    let image = &doc["images"][image_index];
    let view_index = image["bufferView"]
        .as_u64()
        .ok_or_else(|| anyhow!("image {image_index} is not embedded"))?
        as usize;
    let view = &doc["bufferViews"][view_index];
    let offset = view["byteOffset"].as_u64().unwrap_or(0) as usize;
    let length = view["byteLength"]
        .as_u64()
        .ok_or_else(|| anyhow!("image view has no byteLength"))? as usize;
    let bin = glb_bin(glb)?;
    let encoded = bin
        .get(offset..offset + length)
        .ok_or_else(|| anyhow!("embedded image extends past BIN chunk"))?;
    Ok(image::load_from_memory(encoded)?.to_rgba8())
}

fn build_atlas(glb: &[u8], cells: &BTreeMap<String, Cell>, kind: AtlasKind) -> Result<Vec<u8>> {
    let mut atlas = vec![0u8; (ATLAS_SIZE * ATLAS_SIZE * 4) as usize];
    for material in MATERIAL_ORDER {
        let source = match kind {
            AtlasKind::Color => material_image(
                glb,
                material,
                &["pbrMetallicRoughness", "baseColorTexture", "index"],
            )?,
            // glTF emissive is where the exporter puts `g_tSelfIllumMask`.
            AtlasKind::SelfIllum => {
                let gain = SELF_ILLUM_GAIN
                    .iter()
                    .find(|(name, _)| *name == material)
                    .map(|(_, gain)| *gain)
                    .ok_or_else(|| anyhow!("no self-illum gain for {material}"))?;
                let mut mask = material_image(glb, material, &["emissiveTexture", "index"])?;
                for pixel in mask.pixels_mut() {
                    for channel in 0..3 {
                        let scaled = f32::from(pixel.0[channel]) * gain;
                        pixel.0[channel] = scaled.clamp(0.0, 255.0) as u8;
                    }
                }
                mask
            }
            AtlasKind::NormalRoughness => {
                let normal = material_image(glb, material, &["normalTexture", "index"])?;
                let orm = material_image(
                    glb,
                    material,
                    &["pbrMetallicRoughness", "metallicRoughnessTexture", "index"],
                )?;
                let width = normal.width().max(orm.width());
                let height = normal.height().max(orm.height());
                let normal = image::imageops::resize(
                    &normal,
                    width,
                    height,
                    image::imageops::FilterType::Lanczos3,
                );
                let orm = image::imageops::resize(
                    &orm,
                    width,
                    height,
                    image::imageops::FilterType::Lanczos3,
                );
                image::RgbaImage::from_fn(width, height, |x, y| {
                    let n = normal.get_pixel(x, y).0;
                    let o = orm.get_pixel(x, y).0;
                    image::Rgba([n[0], n[1], o[1], 255])
                })
            }
            AtlasKind::AmbientOcclusion => {
                let mut orm = material_image(
                    glb,
                    material,
                    &["pbrMetallicRoughness", "metallicRoughnessTexture", "index"],
                )?;
                for pixel in orm.pixels_mut() {
                    let ao = pixel.0[0];
                    *pixel = image::Rgba([ao, ao, ao, 255]);
                }
                orm
            }
        };
        let cell = cells[material];
        let inner_w = cell.w.saturating_sub(GUTTER * 2).max(1);
        let inner_h = cell.h.saturating_sub(GUTTER * 2).max(1);
        let resized = image::imageops::resize(
            &source,
            inner_w,
            inner_h,
            image::imageops::FilterType::Lanczos3,
        );
        for y in 0..cell.h {
            let sy = y.saturating_sub(GUTTER).min(inner_h - 1);
            for x in 0..cell.w {
                let sx = x.saturating_sub(GUTTER).min(inner_w - 1);
                let mut pixel = resized.get_pixel(sx, sy).0;
                pixel[3] = 255;
                let target = (((cell.y + y) * ATLAS_SIZE + cell.x + x) * 4) as usize;
                atlas[target..target + 4].copy_from_slice(&pixel);
            }
        }
    }
    Ok(atlas)
}

fn build_idle_motion(pak: &str) -> Result<Vec<(String, Vec<u8>)>> {
    let skeleton_bytes = vpkmerge_core::read_vpk_entry(pak, NM_SKELETON_ENTRY)
        .with_context(|| format!("reading {NM_SKELETON_ENTRY}"))?;
    let skeleton = morphic::model::decode_nm_skeleton(&skeleton_bytes)
        .map_err(|e| anyhow!("decoding Abrams NM skeleton: {e}"))?;

    let mut tracks = [0usize; 4];
    for (slot, name) in tracks.iter_mut().zip(SPINE_CHAIN) {
        *slot = skeleton
            .bone_names
            .iter()
            .position(|bone| bone == name)
            .ok_or_else(|| anyhow!("Abrams NM skeleton has no {name} bone"))?;
    }

    let mut out = Vec::with_capacity(IDLE_CLIPS.len());
    for name in IDLE_CLIPS {
        let entry = format!("{CLIP_DIR}/{name}.vnmclip_c");
        let original = vpkmerge_core::read_vpk_entry(pak, &entry)
            .with_context(|| format!("reading clip {entry}"))?;
        let mut clip = morphic::model::decode_nm_clip(&original)
            .map_err(|e| anyhow!("decoding clip {name}: {e}"))?;

        let frames = clip.frame_count as usize;
        anyhow::ensure!(frames > 1, "clip {name} has {frames} frame(s), cannot sway");

        for (step, &track_index) in tracks.iter().enumerate() {
            let track = clip
                .tracks
                .get_mut(track_index)
                .ok_or_else(|| anyhow!("clip {name} has no track {track_index}"))?;
            anyhow::ensure!(
                track.rotations.is_none(),
                "clip {name} already animates {}; refusing to overwrite Valve's motion",
                SPINE_CHAIN[step]
            );

            let constant = track.settings.constant_rotation;
            let amplitude = SWAY_DEGREES[step].to_radians();
            let lag = SWAY_LAG * step as f32;
            let mut samples = Vec::with_capacity(frames);
            for frame in 0..frames {
                // Phase runs over frames-1 so the last frame lands exactly on the
                // first: these are looping idles and a seam would read as a hitch.
                let phase =
                    std::f32::consts::TAU * SWAY_CYCLES * frame as f32 / (frames - 1) as f32 - lag;
                let sway = quat_axis_angle([0.0, 1.0, 0.0], amplitude * phase.sin());
                let roll = quat_axis_angle(
                    [1.0, 0.0, 0.0],
                    amplitude * ROLL_RATIO * (phase * 2.0).sin(),
                );
                samples.push(quat_mul(quat_mul(constant, sway), roll));
            }
            track.rotations = Some(samples);
        }

        let encoded = morphic::model::reencode_nm_clip(&original, &clip)
            .map_err(|e| anyhow!("re-encoding clip {name}: {e}"))?;
        let check = morphic::model::decode_nm_clip(&encoded)
            .map_err(|e| anyhow!("edited clip {name} failed to re-decode: {e}"))?;
        anyhow::ensure!(
            check.frame_count == clip.frame_count,
            "clip {name} frame count changed"
        );
        out.push((entry, encoded));
    }
    Ok(out)
}

fn quat_axis_angle(axis: [f32; 3], angle: f32) -> morphic::model::Quat {
    let half = angle * 0.5;
    let s = half.sin();
    morphic::model::Quat {
        x: axis[0] * s,
        y: axis[1] * s,
        z: axis[2] * s,
        w: half.cos(),
    }
}

fn quat_mul(a: morphic::model::Quat, b: morphic::model::Quat) -> morphic::model::Quat {
    morphic::model::Quat {
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    }
}

fn disable_ambient_book_particle(model: &[u8]) -> Result<Vec<u8>> {
    let tree = morphic::decode_kv3_resource(model).context("decoding Abrams model metadata")?;
    let key_value_text = tree
        .get("m_modelInfo")
        .and_then(|value| value.get("m_keyValueText"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Abrams model has no m_modelInfo.m_keyValueText"))?;
    let section = key_value_text
        .find("particle_cfg_list")
        .ok_or_else(|| anyhow!("Abrams model has no particle_cfg_list"))?;
    let open = key_value_text[section..]
        .find('[')
        .map(|offset| section + offset)
        .ok_or_else(|| anyhow!("Abrams particle_cfg_list has no opening bracket"))?;
    let close = matching_bracket(key_value_text, open)?;
    let body = &key_value_text[open + 1..close];
    anyhow::ensure!(
        body.contains(AMBIENT_BOOK_PARTICLE),
        "Abrams particle_cfg_list no longer contains the ambient book particle"
    );
    anyhow::ensure!(
        body.matches(".vpcf").count() == 1,
        "Abrams particle_cfg_list gained another particle; refusing to remove it with the book outline"
    );

    let mut updated = key_value_text.to_owned();
    updated.replace_range(open..=close, "[ ]");
    let patched = morphic::patch_kv3_resource_strings_adding(
        model,
        &[(
            vec![
                Seg::Key("m_modelInfo".into()),
                Seg::Key("m_keyValueText".into()),
            ],
            updated,
        )],
    )
    .context("removing Abrams ambient book particle")?;
    let check = morphic::decode_kv3_resource(&patched)
        .context("re-decoding model after removing ambient book particle")?;
    let check_text = check
        .get("m_modelInfo")
        .and_then(|value| value.get("m_keyValueText"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("patched Abrams model lost m_keyValueText"))?;
    anyhow::ensure!(
        !check_text.contains(AMBIENT_BOOK_PARTICLE),
        "ambient book particle remained after patch"
    );
    Ok(patched)
}

fn matching_bracket(text: &str, open: usize) -> Result<usize> {
    let mut depth = 0u32;
    let mut quoted = false;
    let mut escaped = false;
    for (offset, byte) in text.as_bytes()[open..].iter().copied().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
            continue;
        }
        match byte {
            b'"' => quoted = true,
            b'[' => depth += 1,
            b']' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| anyhow!("unbalanced particle_cfg_list brackets"))?;
                if depth == 0 {
                    return Ok(open + offset);
                }
            }
            _ => {}
        }
    }
    anyhow::bail!("Abrams particle_cfg_list has no closing bracket")
}
