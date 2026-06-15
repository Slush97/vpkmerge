// Build a custom soul container that is a faithful CLONE of an imported GLB.
//
// APPROACH (revised after in-game testing): ONE clean material with an ATLASED
// albedo + a single draw call -- NOT N draw calls. Source 2 soul containers all
// render through one shader (pbr.vfx NPR toon); the shipped multi-material mods
// (psyduck body+eyes) differ only in their albedo TEXTURE, same shader/params. So
// packing every GLB material group into one atlas albedo (each group's UVs
// remapped into its atlas cell) is visually identical to N materials, and uses
// the single-draw-call path that is confirmed to load in-game. (The multi-draw-
// call path -- morphic::model::set_draw_call_groups -- decodes fine but the engine
// rejects the re-encoded draw-call array; that needs a tagless->lane KV3
// promotion to ever load, disproportionate for an identical-looking result.)
//
// The material is a committed clean DONOR (a shipped soul material: pbr.vfx, NPR
// toon on, NO solid outline, NO self-illum, support slots at materials/default/*)
// with only g_tColor repointed to the atlas. Vanilla soul_container.vmat is never
// touched (no re-encode: a morphic re-emit renders the red error shader).
//
// PARTICLES: the orb's gold "soul" look is 3 entity-attached particles (a tinted
// model-glow shell that re-renders the orb model, a glow halo, gold embers). We
// ship recolored copies (recolor_particle_bytes -> the import's dominant hue) so
// the glow matches the model instead of staying default gold.
//
// Output: fresh model + 1 material + 1 atlas texture + 3 recolored particles.
//
// usage: cargo run --release --example soul_import_clone -- \
//          <pak01_dir.vpk> <model.glb> <out_dir.vpk> [skin_name]
use anyhow::{anyhow, Context, Result};
use morphic::kv3::{Seg, Value as Kv3};
use morphic::model::{
    read_edited_primitives, replace_mesh_part_uncompressed, set_model_material, VertexBuffer,
};
use morphic::{replace_mip_chain, Image, ImageData};
use serde_json::Value as Json;
use std::collections::HashMap;
use vpkmerge_core::{recolor_particle_bytes, Recolor};

const MODEL: &str = "models/props_gameplay/soul_container/soul_container.vmdl_c";
const MAT_DIR: &str = "models/props_gameplay/soul_container/materials";
const DONOR_VMAT: &[u8] = include_bytes!("../../morphic/fixtures/soul/soul_material_donor.vmat_c");
const DEFAULT_NORMAL: &str = "materials/default/default_normal_tga_7be61377.vtex";
const FLAT_DONOR: &str = "panorama/images/hud/zipline_icon_psd.vtex_c";
const COLOR_DONOR: &str = "dev/helper/testgrid_color_tga_2d6cc34.vtex_c";

// The 3 soul-glow particles (base game) the orb entity attaches; we recolor them.
const PARTICLES: [&str; 3] = [
    "particles/generic/holding_gold_neutral_model.vpcf_c",
    "particles/generic/holding_gold_neutral_model_glow.vpcf_c",
    "particles/generic/holding_gold_neutral_embers.vpcf_c",
];

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

/// sRGB 0-255 RGB -> hue in degrees [0,360).
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

fn material_albedo_image(
    glb: &[u8],
    doc: &Json,
    mat_idx: usize,
) -> Result<Option<image::RgbaImage>> {
    let Some(tex_i) = doc
        .get("materials")
        .and_then(Json::as_array)
        .and_then(|a| a.get(mat_idx))
        .and_then(|m| m.get("pbrMetallicRoughness"))
        .and_then(|p| p.get("baseColorTexture"))
        .and_then(|t| t.get("index"))
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

/// Clean material = donor copy, g_tColor -> our atlas, prop-local normal -> flat
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

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let pak = args.next().context("arg1: pak01_dir.vpk")?;
    let glb_path = args.next().context("arg2: model glb")?;
    let out = args.next().context("arg3: out_dir.vpk")?;
    let name = args.next().unwrap_or_else(|| "custom_soul".to_string());

    let glb = std::fs::read(&glb_path)?;
    let doc = glb_json(&glb)?;
    let vpk = valve_pak::open(&pak)?;
    let read = |entry: &str| -> Result<Vec<u8>> {
        let mut f = vpk
            .get_file(entry)
            .with_context(|| format!("entry {entry} not found"))?;
        Ok(f.read_all()?)
    };

    // --- 1. read GLB prims, group by material, resolve each group's albedo ---
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
                image: gi
                    .map(|mi| material_albedo_image(&glb, &doc, mi))
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
    //        flat donor only if the 512 one is missing from this build's pak. ---
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
    eprintln!("mesh:   {} prims -> {n} group(s), {} verts, {} tris; atlas {cols}x{rows} ({atlas}px); fit x{scale:.3}",
        prims.len(), merged.element_count, indices.len() / 3);

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
    eprintln!("color:  {n}-cell atlas; material -> clean donor (g_tColor -> atlas)");

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
    eprintln!("glow:   recolor particles -> hue {hue:.0} deg (from dominant group)");

    // --- 8. pack: model + material + atlas + recolored particles ---
    let mut entries: Vec<(String, Vec<u8>)> = vec![
        (MODEL.to_string(), edited_model),
        (format!("{MAT_DIR}/{name}.vmat_c"), vmat),
        (color_entry, color_tex),
    ];
    // SOUL_GLOW: off = don't ship particles (base game gold glow plays);
    // base = ship unchanged (isolation); recolor (default) = recolor to hue.
    let glow_mode = std::env::var("SOUL_GLOW").unwrap_or_else(|_| "recolor".into());
    if glow_mode != "off" {
        for p in PARTICLES {
            match read(p) {
                Ok(base) => {
                    let bytes = if glow_mode == "recolor" {
                        recolor_particle_bytes(&base, Recolor::hue(hue))?.unwrap_or(base)
                    } else {
                        base
                    };
                    entries.push((p.to_string(), bytes));
                }
                Err(e) => eprintln!("glow:   skip {p} ({e})"),
            }
        }
    }
    let refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &out)?;
    eprintln!(
        "wrote {out} ({} entries: model + material + atlas + {} particles)",
        refs.len(),
        PARTICLES.len()
    );
    Ok(())
}
