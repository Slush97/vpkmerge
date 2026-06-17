//! Import a custom GLB prop (a "hat") and attach it to a Deadlock hero's head by
//! welding it into the hero body `.vmdl_c`, rigid-skinned to the head bone, with
//! its own atlas material. The geometry/appearance pipeline is the soul-container
//! clone's proven recipe (GLB -> one atlased albedo + one draw call), redirected
//! from a standalone prop to an additive, head-skinned draw call on the hero via
//! [`morphic::model::append_skinned_draw_call`].
//!
//! In-game confirmed prerequisite: a box skinned 100% to the head bone, welded
//! into Mina's body model, loads and tracks the head (the probe that preceded
//! this module).

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::many_single_char_names,
    clippy::similar_names
)]

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use morphic::model::{append_skinned_draw_call, VertexBuffer};
use morphic::{replace_mip_chain, Image, ImageData};

use crate::soul_import_clone::{
    base_color_factor, build_material, glb_json, material_index_by_name, paint_atlas_cell,
    read_glb_primitives, remap_atlas_uv, AtlasCell, WrapMode, COLOR_DONOR, FLAT_DONOR,
};
use crate::SoulOrient;

/// Inputs for [`import_hero_hat`].
#[derive(Debug, Clone)]
pub struct HatImportOptions {
    /// Material/texture basename used inside the VPK (e.g. `clownhat`).
    pub name: String,
    /// Coordinate convention applied to the GLB before fitting (default `YUp`).
    pub orient: SoulOrient,
    /// Extra Euler degrees `[X, Y, Z]` applied in GLB space after `orient`.
    pub rotate: Option<[f32; 3]>,
    /// Skeleton bone the hat rides (default `head`).
    pub bone: String,
    /// Target largest horizontal span in Source units the hat is scaled to.
    pub width: f32,
    /// Vertical offset (Source units) of the hat's base above the crown. Negative
    /// sinks it onto the head.
    pub raise: f32,
    /// Facing yaw in degrees about Source-Z, baked into the geometry.
    pub yaw: f32,
}

impl Default for HatImportOptions {
    fn default() -> Self {
        Self {
            name: "custom_hat".to_string(),
            orient: SoulOrient::YUp,
            rotate: None,
            bone: "head".to_string(),
            width: 16.0,
            raise: -2.0,
            yaw: 0.0,
        }
    }
}

/// Diagnostics from a successful hat attach.
#[derive(Debug, Clone)]
pub struct HatImportReport {
    pub hero_entry: String,
    pub bone: String,
    pub bone_index: usize,
    pub group_count: usize,
    pub vert_count: usize,
    pub tri_count: usize,
    pub fit_scale: f32,
    pub crown_z: f32,
    pub entry_count: usize,
}

struct HatGroup {
    material: Option<String>,
    prims: Vec<usize>,
    color: [f64; 4],
}

/// Attach `glb` to `hero_codename`'s head, writing an addon VPK to `out` that
/// overrides the hero body model and ships the hat's material + atlas texture.
/// `pak` is a base `pak01_dir.vpk` the hero model and donor texture are read from.
#[allow(clippy::too_many_lines)]
pub fn import_hero_hat(
    pak: impl AsRef<Path>,
    glb: &[u8],
    hero_codename: &str,
    out: impl AsRef<Path>,
    opts: &HatImportOptions,
) -> Result<HatImportReport> {
    let pak = pak.as_ref();
    let name = &opts.name;

    // --- hero model + head bone ---
    let hero_entry = crate::hero_model_entry(pak, None, hero_codename)?;
    let model_bytes = crate::read_vpk_entry(pak, &hero_entry)?;
    let model = morphic::model::decode(&model_bytes).map_err(|e| anyhow!("decode hero: {e}"))?;
    let bone_index = model
        .skeleton
        .bones
        .iter()
        .position(|b| b.name.eq_ignore_ascii_case(&opts.bone))
        .ok_or_else(|| anyhow!("hero has no bone named {:?}", opts.bone))?;
    let anchor = {
        let t = model.skeleton.bones[bone_index].global_bind.m;
        [t[12], t[13], t[14]]
    };
    // Crown height: the bone's `_end` child if present, else a head-ish lift.
    let crown_z = model
        .skeleton
        .bones
        .iter()
        .find(|b| b.name.eq_ignore_ascii_case(&format!("{}_end", opts.bone)))
        .map_or(anchor[2] + 7.0, |b| b.global_bind.m[14]);

    // --- 1. read GLB prims, group by material, resolve each group's flat color ---
    let doc = glb_json(glb)?;
    let (prims, _orient_label) = read_glb_primitives(glb, opts.orient, opts.rotate)?;
    let mat_index = material_index_by_name(&doc);
    let mut groups: Vec<HatGroup> = Vec::new();
    for (pi, p) in prims.iter().enumerate() {
        if let Some(g) = groups.iter_mut().find(|g| g.material == p.material_name) {
            g.prims.push(pi);
        } else {
            let color = p
                .material_name
                .as_ref()
                .and_then(|m| mat_index.get(m))
                .map_or([0.8, 0.8, 0.8, 1.0], |&mi| base_color_factor(&doc, mi));
            groups.push(HatGroup {
                material: p.material_name.clone(),
                prims: vec![pi],
                color,
            });
        }
    }
    let n = groups.len();

    // --- 2. atlas grid on the BCn donor (inline PNG is rejected in-game) ---
    let (atlas, donor_entry) = if crate::read_vpk_entry(pak, COLOR_DONOR).is_ok() {
        (512u32, COLOR_DONOR)
    } else {
        (64u32, FLAT_DONOR)
    };
    let cols = (n as f64).sqrt().ceil() as u32;
    let rows = (n as u32).div_ceil(cols);
    let cw = atlas / cols;
    let ch = atlas / rows;
    let cell_rect = |i: usize| -> (u32, u32, u32, u32) {
        let (c, r) = (i as u32 % cols, i as u32 / cols);
        (c * cw, r * ch, cw, ch)
    };

    // --- 3. merge geometry (Y-up GLB -> Source Z-up), atlas UV per group ---
    let mut merged = VertexBuffer {
        texcoords: vec![Vec::new()],
        ..VertexBuffer::default()
    };
    let mut indices: Vec<u32> = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        let (x0, y0, w, h) = cell_rect(gi);
        let cell = AtlasCell::new(x0, y0, w, h);
        let uv = remap_atlas_uv(
            [0.0, 0.0],
            cell,
            atlas,
            WrapMode::ClampToEdge,
            WrapMode::ClampToEdge,
            false,
        );
        for &pi in &g.prims {
            let vb = &prims[pi].vertex_buffer;
            let base = u32::try_from(merged.positions.len())?;
            merged
                .positions
                .extend(vb.positions.iter().map(|v| [v[0], v[2], -v[1]]));
            merged
                .normals
                .extend(vb.normals.iter().map(|v| [v[0], v[2], -v[1]]));
            for _ in 0..vb.positions.len() {
                merged.texcoords[0].push(uv);
            }
            indices.extend(prims[pi].indices.iter().map(|&idx| base + idx));
        }
    }
    merged.element_count = merged.positions.len();
    if merged.element_count == 0 {
        return Err(anyhow!("GLB had no geometry"));
    }
    let tri_count = indices.len() / 3;

    // --- 4. fit: scale to width, center XY on the bone, base at the crown ---
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in &merged.positions {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let span_xy = (max[0] - min[0]).max(max[1] - min[1]).max(1e-4);
    let scale = opts.width / span_xy;
    let center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
    for p in &mut merged.positions {
        p[0] = (p[0] - center[0]) * scale + anchor[0];
        p[1] = (p[1] - center[1]) * scale + anchor[1];
        p[2] *= scale;
    }
    // Lift so the lowest point sits at crown + raise.
    let lifted_min_z = merged
        .positions
        .iter()
        .map(|p| p[2])
        .fold(f32::INFINITY, f32::min);
    let dz = crown_z + opts.raise - lifted_min_z;
    for p in &mut merged.positions {
        p[2] += dz;
    }
    // Optional facing yaw about Source-Z through the bone.
    if opts.yaw != 0.0 {
        let (s, c) = opts.yaw.to_radians().sin_cos();
        let (cx, cy) = (anchor[0], anchor[1]);
        for p in &mut merged.positions {
            let (x, y) = (p[0] - cx, p[1] - cy);
            *p = [c * x - s * y + cx, s * x + c * y + cy, p[2]];
        }
        for nrm in &mut merged.normals {
            let (x, y) = (nrm[0], nrm[1]);
            *nrm = [c * x - s * y, s * x + c * y, nrm[2]];
        }
    }

    // --- 5. skin every hat vertex 100% to the bone (MODEL index) ---
    let vc = merged.element_count;
    merged.joints = vec![
        [
            bone_index as u16,
            bone_index as u16,
            bone_index as u16,
            bone_index as u16
        ];
        vc
    ];
    merged.weights = vec![[1.0, 0.0, 0.0, 0.0]; vc];

    // --- 6. atlas albedo: paint each group's cell with its flat color ---
    let donor = crate::read_vpk_entry(pak, donor_entry)?;
    let mut px = vec![0u8; (atlas * atlas * 4) as usize];
    for (gi, g) in groups.iter().enumerate() {
        let (x0, y0, w, h) = cell_rect(gi);
        paint_atlas_cell(&mut px, atlas, AtlasCell::new(x0, y0, w, h), None, g.color);
    }
    let mat_dir = hero_entry.rsplit_once('/').map_or_else(
        || "materials".to_string(),
        |(d, _)| format!("{d}/materials"),
    );
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
    let vmat_path = format!("{mat_dir}/{name}.vmat");
    let vmat_entry = format!("{mat_dir}/{name}.vmat_c");
    let vmat = build_material(&color_vtex, None)?;

    // --- 7. weld the hat onto the head part as its own skinned draw call ---
    let part = head_part_name(&model, bone_index)?;
    let edited = append_skinned_draw_call(&model_bytes, &part, &merged, &indices, &vmat_path)
        .map_err(|e| anyhow!("append skinned draw call: {e}"))?;
    // Structural sanity: re-decode.
    morphic::model::decode(&edited).context("edited hero model failed to re-decode")?;

    // --- 8. pack ---
    let entries: Vec<(String, Vec<u8>)> = vec![
        (hero_entry.clone(), edited),
        (vmat_entry, vmat),
        (color_entry, color_tex),
    ];
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    crate::pack(&refs, out.as_ref())?;

    Ok(HatImportReport {
        hero_entry,
        bone: opts.bone.clone(),
        bone_index,
        group_count: n,
        vert_count: vc,
        tri_count,
        fit_scale: scale,
        crown_z,
        entry_count: refs.len(),
    })
}

/// The mesh part whose bone palette contains `bone_index` and that is a single
/// vertex/index buffer (the append contract). Prefers a part literally named for
/// the bone (`head`).
fn head_part_name(model: &morphic::model::Model, bone_index: usize) -> Result<String> {
    let uses_bone = |p: &morphic::model::MeshPart| {
        p.vertex_buffers.iter().any(|vb| {
            vb.joints
                .iter()
                .zip(vb.weights.iter())
                .any(|(j, w)| (0..4).any(|k| w[k] > 0.0 && j[k] as usize == bone_index))
        })
    };
    if let Some(p) = model
        .meshes
        .iter()
        .find(|p| p.name == "head" && p.vertex_buffers.len() == 1 && uses_bone(p))
    {
        return Ok(p.name.clone());
    }
    model
        .meshes
        .iter()
        .find(|p| p.vertex_buffers.len() == 1 && uses_bone(p))
        .map(|p| p.name.clone())
        .ok_or_else(|| anyhow!("no single-buffer mesh part skins to the target bone"))
}
