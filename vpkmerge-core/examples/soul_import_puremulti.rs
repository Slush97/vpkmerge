// Pure multi-material soul container, built toward the pak06 resourcecompiler
// oracle (piplup: 6 materials initialshadinggroup + lambert4-8sg, 6 draw calls).
//
// Unlike soul_import_clone.rs (single atlas, single draw call -- the known-good
// fallback), this reproduces pak06's STRUCTURE: one merged uncompressed VBIB with
// N per-material index slices, N draw calls via morphic::model::set_draw_call_groups,
// N clean materials via morphic::compile_pbr_vmat (byte-faithful donor patch, the
// engine-accepted path), and N PNG_RGBA8888 textures via encode_vtex_png_rgba8888.
//
// This is the probe that isolates the model-side pure path after the VMAT/VTEX
// probes: the draw calls use resourcecompiler-style local index slices plus
// cumulative m_nAppliedIndexOffset / m_nVertexCount vertex ranges. If this still
// errors in game, the next gap is outside the obvious material, texture, and
// draw-range contract.
//
// The model envelope donor is the stock soul_container.vmdl_c read from pak01 --
// the SAME envelope pak06 uses (PHYS is byte-identical across all soul mods); a
// committed neutral envelope fixture is a later refactor once the draw-call
// encoding is cracked.
//
// usage: cargo run --release -p vpkmerge-core --example soul_import_puremulti -- \
//          <pak01_dir.vpk> <model.glb> <out_dir.vpk> [skin_name]
use anyhow::{anyhow, Context, Result};
use morphic::model::{
    read_edited_primitives, replace_mesh_part_uncompressed, set_draw_call_groups, DrawCallGroup,
    VertexBuffer,
};
use morphic::{Image, ImageData, TextureFlags};
use serde_json::Value as Json;
use std::collections::HashMap;

const MODEL: &str = "models/props_gameplay/soul_container/soul_container.vmdl_c";
const MAT_DIR: &str = "models/props_gameplay/soul_container/materials";
const DONOR_VMAT: &[u8] = include_bytes!("../../morphic/fixtures/soul/soul_material_donor.vmat_c");
const DEFAULT_NORMAL: &str = "materials/default/default_normal_tga_7be61377.vtex";

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
    (a + b) / 2.0
}

fn glb_json(glb: &[u8]) -> Result<Json> {
    if glb.get(0..4) != Some(b"glTF") {
        return Err(anyhow!("not a binary glTF"));
    }
    let json_len = u32::from_le_bytes(glb[12..16].try_into()?) as usize;
    Ok(serde_json::from_slice(&glb[20..20 + json_len])?)
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
    if let Some(f) = doc
        .get("materials")
        .and_then(Json::as_array)
        .and_then(|a| a.get(mat_idx))
        .and_then(|m| m.get("pbrMetallicRoughness"))
        .and_then(|p| p.get("baseColorFactor"))
        .and_then(Json::as_array)
    {
        for (i, v) in f.iter().take(4).enumerate() {
            color[i] = v.as_f64().unwrap_or(1.0);
        }
    }
    color
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

/// One material group: its source primitives, its flat color, its index slice.
struct Group {
    glb_material: Option<String>,
    prims: Vec<usize>,
    color: [f64; 4],
    start_index: usize,
    index_count: usize,
    vertex_start: usize,
    vertex_end: usize,
}

/// Source-relative material stem for a group. Matches pak06's flavor of Maya
/// material names where possible; falls back to mat{N}.
fn material_stem(g: &Group, idx: usize) -> String {
    g.glb_material
        .as_deref()
        .map(|m| {
            m.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '_'
                    }
                })
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("mat{idx}"))
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let pak = args.next().context("arg1: pak01_dir.vpk")?;
    let glb_path = args.next().context("arg2: model glb")?;
    let out = args.next().context("arg3: out_dir.vpk")?;
    let _name = args.next().unwrap_or_else(|| "piplup".to_string());

    let glb = std::fs::read(&glb_path)?;
    let doc = glb_json(&glb)?;
    let vpk = valve_pak::open(&pak)?;
    let read = |entry: &str| -> Result<Vec<u8>> {
        let mut f = vpk
            .get_file(entry)
            .with_context(|| format!("entry {entry} not found"))?;
        Ok(f.read_all()?)
    };

    // --- 1. read GLB prims, group by material ---
    let prims = read_edited_primitives(&glb).map_err(|e| anyhow!("reading glb: {e}"))?;
    if prims.is_empty() {
        return Err(anyhow!("glb has no mesh parts"));
    }
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
                color: gi.map_or([0.8, 0.8, 0.8, 1.0], |mi| base_color_factor(&doc, mi)),
                start_index: 0,
                index_count: 0,
                vertex_start: 0,
                vertex_end: 0,
            });
        }
    }
    let n = groups.len();

    // --- 2. merge geometry; each group owns a contiguous slice of one index
    //        buffer (base_vertex=0, the multi-draw-call contract) ---
    let mut merged = VertexBuffer {
        texcoords: vec![Vec::new()],
        ..VertexBuffer::default()
    };
    let mut indices: Vec<u32> = Vec::new();
    for g in &mut groups {
        let start = indices.len();
        let vertex_start = merged.positions.len();
        for &pi in &g.prims {
            let vb = &prims[pi].vertex_buffer;
            let base = u32::try_from(merged.positions.len() - vertex_start)?;
            // glTF Y-up -> Source Z-up.
            merged
                .positions
                .extend(vb.positions.iter().map(|v| [v[0], v[2], -v[1]]));
            merged
                .normals
                .extend(vb.normals.iter().map(|v| [v[0], v[2], -v[1]]));
            let src = vb.texcoords.first();
            for vi in 0..vb.positions.len() {
                let uv = src.and_then(|t| t.get(vi)).copied().unwrap_or([0.0, 0.0]);
                merged.texcoords[0].push(uv);
            }
            indices.extend(prims[pi].indices.iter().map(|&idx| base + idx));
        }
        g.start_index = start;
        g.index_count = indices.len() - start;
        g.vertex_start = vertex_start;
        g.vertex_end = merged.positions.len();
    }
    merged.element_count = merged.positions.len();

    // --- 3. fit to the orb's bounds ---
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
    eprintln!(
        "mesh:   {} prims -> {n} material group(s), {} verts, {} tris; fit x{scale:.3}",
        prims.len(),
        merged.element_count,
        indices.len() / 3
    );

    // --- 4. swap mesh in (UNCOMPRESSED) ---
    let (mesh_swapped, _rep) =
        replace_mesh_part_uncompressed(&model_bytes, "soul_container", &merged, &indices)
            .map_err(|e| anyhow!("replacing mesh: {e}"))?;

    // --- 5. grow the single draw call to N, one slice per material group ---
    let stems: Vec<String> = groups
        .iter()
        .enumerate()
        .map(|(i, g)| material_stem(g, i))
        .collect();
    let dc_groups: Vec<DrawCallGroup> = groups
        .iter()
        .zip(&stems)
        .map(|(g, stem)| DrawCallGroup {
            material: format!("{MAT_DIR}/{stem}.vmat"),
            start_index: g.start_index,
            index_count: g.index_count,
            vertex_start: g.vertex_start,
            vertex_end: g.vertex_end,
        })
        .collect();
    let multi_model = set_draw_call_groups(&mesh_swapped, &dc_groups, merged.element_count)
        .map_err(|e| anyhow!("set draw-call groups: {e}"))?;
    eprintln!(
        "model:  {n} draw calls -> {:?}",
        dc_groups
            .iter()
            .map(|d| (
                &d.material,
                d.start_index,
                d.index_count,
                d.vertex_start,
                d.vertex_end
            ))
            .collect::<Vec<_>>()
    );

    // --- 6. one clean material + one flat-colour texture per group ---
    let mut entries: Vec<(String, Vec<u8>)> = vec![(MODEL.to_string(), multi_model)];
    for (g, stem) in groups.iter().zip(&stems) {
        let color_vtex = format!("{MAT_DIR}/{stem}_color.vtex");
        let vmat = morphic::compile_pbr_vmat(
            DONOR_VMAT,
            &format!("{MAT_DIR}/{stem}.vmat"),
            &[
                ("g_tColor", &color_vtex),
                ("g_tNormalRoughness", DEFAULT_NORMAL),
            ],
        )
        .map_err(|e| anyhow!("compile material {stem}: {e}"))?;

        // 16x16 flat swatch of the group's baseColorFactor.
        let (w, h) = (16u32, 16u32);
        let rgba = [
            to_srgb_u8(g.color[0]),
            to_srgb_u8(g.color[1]),
            to_srgb_u8(g.color[2]),
            255,
        ];
        let px: Vec<u8> = (0..(w * h)).flat_map(|_| rgba).collect();
        let tex = morphic::encode_vtex_png_rgba8888(
            &Image {
                width: w,
                height: h,
                data: ImageData::Rgba8(px),
            },
            TextureFlags::empty(),
        )
        .map_err(|e| anyhow!("encode texture {stem}: {e}"))?;

        entries.push((format!("{MAT_DIR}/{stem}.vmat_c"), vmat));
        entries.push((format!("{MAT_DIR}/{stem}_color.vtex_c"), tex));
    }
    eprintln!("color:  {n} materials (compile_pbr_vmat) + {n} flat PNG_RGBA8888 textures");

    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &out)?;
    eprintln!(
        "wrote {out} ({} entries: model + {n} materials + {n} textures)",
        refs.len()
    );
    Ok(())
}
