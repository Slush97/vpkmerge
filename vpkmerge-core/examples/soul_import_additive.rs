// Build a custom soul container as an ADDITIVE mod (the way the shipped community
// mods are coded: nuggie / money_bag / tardis / hello_kitty), NOT a graft over
// vanilla. The goal: the in-game prop looks EXACTLY like the imported GLB.
//
// Every soul-container mod overrides models/props_gameplay/soul_container/
// soul_container.vmdl_c (there is one soul orb in-game, so a reskin must), but the
// good ones do it by shipping a FRESH model that references a NEW, uniquely-named
// material (`nuggie.vmat`, `tardis.vmat`, ...) plus that material's textures. They
// do NOT hijack the stock `soul_container.vmat` slots. The stock soul_container is
// trivial (one mesh, one bone, a radius-7 sphere collider; see cinnamoroll's
// shipped `.vmdl`), so we can build the fresh model headless on Linux by reusing
// the stock vmdl as a structural template (PHYS/skeleton/CTRL/DATA scaffolding),
// swapping in the GLB mesh UNCOMPRESSED (the engine reads uncompressed natively;
// no meshopt encoder, no Valve resourcecompiler), and repointing the material to a
// new name.
//
// MATERIAL STRATEGY (the rework): the old build copied the stock NPR toon material
// and then played whack-a-mole stripping/overriding each vanilla feature, dragging
// in a pile of soul-container internals we never wanted (the 2K self-illum "ao_png"
// splotch mask, the real normal/roughness map, the teal outline wash). The gold
// standard `hello_kitty.vmat` shows the right shape: it binds ONE custom color
// texture and points every other slot at a flat `materials/default/*` engine
// default, which samples fine through ANY UVs. So we copy the stock vmat (a valid
// compiled material; a vmat does not embed its own path, so a copy at a new entry
// is valid) and:
//   - neutralize the NPR features (outline / NPR-lighting / self-illum off, teal
//     additive zeroed) for a smooth, GLB-faithful look,
//   - repoint every PROP-LOCAL texture slot (normal, self-illum, AO) to the flat
//     engine default it should have been, so nothing high-res misprojects, and
//   - repoint g_tColor to THIS mod's own custom path.
// All edits are byte-faithful in-place KV3 patches (no re-encode: a morphic
// re-emit of a prop/hero .vmat_c renders the red error shader in game). We ship
// exactly ONE texture (our color) and depend on ZERO stock hash-stamped paths.
//
// TEXTURE FIDELITY: if the GLB carries a real albedo image, we ship it (resized
// onto a stock 512^2 BC7 donor) and keep the GLB's REAL per-vertex UVs, so the
// prop is textured exactly like the GLB. If the GLB is flat-shaded (per-material
// baseColorFactor, no image -- e.g. a Maya export like piplup), a per-part palette
// IS the faithful representation, so we bake one color band per part.
//
// Output (3 entries):
//   models/props_gameplay/soul_container/soul_container.vmdl_c   (fresh mesh -> NAME.vmat)
//   .../materials/NAME.vmat_c                                    (copy of stock, cleaned)
//   .../materials/NAME_color.vtex_c                              (the GLB's albedo)
//
// usage: cargo run --release --example soul_import_additive -- <pak01_dir.vpk> <model.glb> <out_dir.vpk> [skin_name]
use anyhow::{anyhow, Context, Result};
use morphic::kv3::{Seg, Value as Kv3};
use morphic::model::{
    read_edited_primitives, replace_mesh_part_uncompressed, set_model_material, VertexBuffer,
};
use morphic::{replace_mip_chain, Image, ImageData};
use serde_json::Value as Json;
use std::collections::HashMap;

const MODEL: &str = "models/props_gameplay/soul_container/soul_container.vmdl_c";
const VMAT: &str = "models/props_gameplay/soul_container/materials/soul_container.vmat_c";
const MAT_DIR: &str = "models/props_gameplay/soul_container/materials";

// Flat engine defaults the cleaned material points its support slots at (the exact
// paths hello_kitty.vmat binds). default_black_mask is already interned in the stock
// soul_container.vmat string table (it is the bound g_tNprTransmissiveColor).
const DEFAULT_NORMAL: &str = "materials/default/default_normal_tga_7be61377.vtex";
const DEFAULT_AO: &str = "materials/default/default_ao_tga_559f1ac6.vtex";
const BLACK_MASK: &str = "materials/default/default_black_mask_tga_e7be3cc.vtex";

// A 64x64 BGRA8888 single-mip donor for the flat per-part palette (flat bands need
// no resolution); replace_mip_chain keeps its dims/format/flags, rewrites pixels.
const PALETTE_DONOR: &str = "panorama/images/hud/zipline_icon_psd.vtex_c";
const PALETTE_TEX: u32 = 64;
// A stock 512x512 BC7 color donor for shipping a GLB's real albedo at usable
// resolution (the stock soul_container color map is only 4x4, useless as a donor).
// Resolved at runtime with a fallback scan in case this exact entry moves.
const COLOR_DONOR: &str = "dev/helper/testgrid_color_tga_2d6cc34.vtex_c";
const COLOR_TEX: u32 = 512;

/// Linear (glTF base color) -> 8-bit sRGB (what an sRGB color texture stores).
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

// --- GLB JSON helpers (hand-parsed; the example already parses the GLB header) ---

/// Parse the GLB's JSON chunk (chunk 0).
fn glb_json(glb: &[u8]) -> Result<Json> {
    if glb.get(0..4) != Some(b"glTF") {
        return Err(anyhow!("not a binary glTF"));
    }
    let json_len = u32::from_le_bytes(glb[12..16].try_into()?) as usize;
    Ok(serde_json::from_slice(&glb[20..20 + json_len])?)
}

/// The GLB's BIN chunk payload (the buffer 0 bytes), scanning chunks past JSON.
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

/// Map each GLB material *name* to its `baseColorFactor` (linear RGBA).
fn glb_base_colors(doc: &Json) -> HashMap<String, [f64; 4]> {
    let mut map = HashMap::new();
    if let Some(mats) = doc.get("materials").and_then(Json::as_array) {
        for m in mats {
            let name = m
                .get("name")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string();
            let mut color = [1.0; 4];
            if let Some(f) = m
                .get("pbrMetallicRoughness")
                .and_then(|p| p.get("baseColorFactor"))
                .and_then(Json::as_array)
            {
                for (i, v) in f.iter().take(4).enumerate() {
                    color[i] = v.as_f64().unwrap_or(1.0);
                }
            }
            map.insert(name, color);
        }
    }
    map
}

/// Extract the GLB's single albedo image (decoded RGBA), if there is exactly one
/// usable base-color texture. Multi-image GLBs would need UV atlasing, so they fall
/// through to the flat-palette path. Returns `None` for a flat-shaded GLB.
fn glb_albedo_image(glb: &[u8], doc: &Json) -> Result<Option<image::RgbaImage>> {
    let images = doc.get("images").and_then(Json::as_array);
    let Some(images) = images.filter(|a| !a.is_empty()) else {
        return Ok(None);
    };
    // Which image index do the materials' baseColorTextures resolve to?
    let textures = doc.get("textures").and_then(Json::as_array);
    let used: std::collections::BTreeSet<usize> = doc
        .get("materials")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|m| {
            m.get("pbrMetallicRoughness")?
                .get("baseColorTexture")?
                .get("index")?
                .as_u64()
        })
        .filter_map(|tex_i| {
            textures?
                .get(tex_i as usize)?
                .get("source")?
                .as_u64()
                .map(|s| s as usize)
        })
        .collect();
    let img_idx = match used.len() {
        1 => *used.iter().next().unwrap(),
        // No baseColorTexture wired, but a lone image exists: use it.
        0 if images.len() == 1 => 0,
        _ => return Ok(None), // 0 with many images, or several distinct images
    };
    let image_json = &images[img_idx];
    let bytes: Vec<u8> = if let Some(bv_i) = image_json.get("bufferView").and_then(Json::as_u64) {
        let bin =
            glb_bin(glb).ok_or_else(|| anyhow!("GLB image in bufferView but no BIN chunk"))?;
        let bv = doc
            .get("bufferViews")
            .and_then(Json::as_array)
            .and_then(|a| a.get(bv_i as usize))
            .ok_or_else(|| anyhow!("bufferView {bv_i} missing"))?;
        let off = bv.get("byteOffset").and_then(Json::as_u64).unwrap_or(0) as usize;
        let len = bv
            .get("byteLength")
            .and_then(Json::as_u64)
            .ok_or_else(|| anyhow!("bufferView byteLength missing"))? as usize;
        bin.get(off..off + len)
            .ok_or_else(|| anyhow!("image bufferView out of range"))?
            .to_vec()
    } else {
        // External / data-URI images are not supported here.
        return Ok(None);
    };
    let img = image::load_from_memory(&bytes)
        .map_err(|e| anyhow!("decoding GLB albedo image: {e}"))?
        .to_rgba8();
    Ok(Some(img))
}

/// Build a vtex on a donor whose mip0 pixels come from `fill(x, y)` (RGBA bytes).
fn build_texture(donor: &[u8], size: u32, fill: impl Fn(u32, u32) -> [u8; 4]) -> Result<Vec<u8>> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let o = ((y * size + x) * 4) as usize;
            px[o..o + 4].copy_from_slice(&fill(x, y));
        }
    }
    replace_mip_chain(
        donor,
        &Image {
            width: size,
            height: size,
            data: ImageData::Rgba8(px),
        },
    )
    .map_err(|e| anyhow!("encoding texture on donor: {e}"))
}

// --- material cleanup helpers ---

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

/// True if a material texture slot binds a PROP-LOCAL texture (one under the
/// soul_container materials dir). A `materials/default/*` path is a shared engine
/// texture, already flat and harmless; only prop-local maps need repointing.
fn slot_is_prop_local(tree: &Kv3, name: &str) -> bool {
    texture_param(tree, name)
        .is_some_and(|p| p.starts_with("models/props_gameplay/soul_container/"))
}

/// Path to a texture slot's `m_pValue` string field, for the string patcher.
fn texture_pvalue_path(mat: &Kv3, slot: &str) -> Option<Vec<Seg>> {
    let i = texture_param_index(mat, slot)?;
    Some(vec![
        Seg::Key("m_textureParams".to_string()),
        Seg::Index(i),
        Seg::Key("m_pValue".to_string()),
    ])
}

/// Index of a named entry in a material's `m_vectorParams` array.
fn vector_param_index(v: &Kv3, name: &str) -> Option<usize> {
    v.get("m_vectorParams")?
        .as_array()?
        .iter()
        .position(|p| p.get("m_name").and_then(Kv3::as_str) == Some(name))
}

/// Edits that zero the first three components (RGB) of a named vector param.
fn zero_vec3_edits(v: &Kv3, name: &str) -> Vec<(Vec<Seg>, f64)> {
    let Some(i) = vector_param_index(v, name) else {
        return Vec::new();
    };
    (0..3)
        .map(|k| {
            (
                vec![
                    Seg::Key("m_vectorParams".to_string()),
                    Seg::Index(i),
                    Seg::Key("m_value".to_string()),
                    Seg::Index(k),
                ],
                0.0,
            )
        })
        .collect()
}

/// Turn the stock soul_container.vmat (a valid compiled material) into a clean,
/// GLB-faithful material at a new name. CRUCIAL: we do NOT flip any `F_*` static
/// combo. Flipping a static combo in a compiled `.vmat_c` is unreliable on
/// `pbr.vfx` (the engine does not reliably honor a post-compile flip; on hero
/// materials it renders the error shader), so the previous build's
/// `F_SOLID_COLOR_OUTLINE/F_USE_NPR_LIGHTING -> 0` left the engine still drawing
/// the solid-color outline (white ink on every crease) = "lines all over".
///
/// Instead we neutralize each NPR effect with SAFE edits only -- point its MASK at
/// the interned black mask (a feature masked to black contributes nothing, the same
/// trick that fixed the rim light) and zero the outline additive. The static combo
/// stays valid; the look matches the clean shipped mods. Plus: prop-local support
/// maps -> flat engine defaults (so nothing high-res misprojects through the foreign
/// UVs) and g_tColor -> this mod's own path. All byte-faithful in-place (no re-encode).
fn build_clean_material(stock_vmat: &[u8], own_color_vtex: &str) -> Result<Vec<u8>> {
    let vmat = morphic::decode_kv3_resource(stock_vmat)
        .map_err(|e| anyhow!("decoding stock material: {e}"))?;
    let mut patched = stock_vmat.to_vec();

    // (a) zero g_vSolidOutlineAdditive (the teal the outline adds across the surface).
    let dbl_edits = zero_vec3_edits(&vmat, "g_vSolidOutlineAdditive");
    if !dbl_edits.is_empty() {
        patched = morphic::patch_kv3_resource_doubles(&patched, &dbl_edits)
            .map_err(|e| anyhow!("zeroing g_vSolidOutlineAdditive: {e}"))?;
    }

    // (b) one blob-aware string pass:
    //   - g_tColor -> our own path,
    //   - every NPR feature MASK -> black (outline / self-illum / rim): masked to
    //     black the effect draws nothing, without touching the static combo,
    //   - prop-local normal / AO -> their flat engine defaults.
    let mut str_edits: Vec<(Vec<Seg>, String)> = Vec::new();
    if let Some(p) = texture_pvalue_path(&vmat, "g_tColor") {
        str_edits.push((p, own_color_vtex.to_string()));
    } else {
        return Err(anyhow!("stock material has no g_tColor slot"));
    }
    for slot in [
        "g_tNprOutlineMask",
        "g_tSelfIllumMask",
        "g_tTintMaskRimLightMask",
    ] {
        if texture_param_index(&vmat, slot).is_some() {
            if let Some(p) = texture_pvalue_path(&vmat, slot) {
                str_edits.push((p, BLACK_MASK.to_string()));
            }
        }
    }
    for (slot, default) in [
        ("g_tNormalRoughness", DEFAULT_NORMAL),
        ("g_tAmbientOcclusion", DEFAULT_AO),
    ] {
        if slot_is_prop_local(&vmat, slot) {
            if let Some(p) = texture_pvalue_path(&vmat, slot) {
                str_edits.push((p, default.to_string()));
            }
        }
    }
    patched = morphic::patch_kv3_resource_strings_adding(&patched, &str_edits)
        .map_err(|e| anyhow!("repointing material textures: {e}"))?;

    // round-trip gate: the patched bytes still decode and every edit took.
    let check = morphic::decode_kv3_resource(&patched)
        .map_err(|e| anyhow!("patched vmat does not re-decode: {e}"))?;
    if texture_param(&check, "g_tColor").as_deref() != Some(own_color_vtex) {
        return Err(anyhow!("vmat patch did not take: g_tColor not repointed"));
    }
    for slot in [
        "g_tNprOutlineMask",
        "g_tSelfIllumMask",
        "g_tTintMaskRimLightMask",
    ] {
        if texture_param_index(&check, slot).is_some()
            && texture_param(&check, slot).as_deref() != Some(BLACK_MASK)
        {
            return Err(anyhow!("vmat patch did not take: {slot} mask not blacked"));
        }
    }
    for slot in ["g_tNormalRoughness", "g_tAmbientOcclusion"] {
        if slot_is_prop_local(&check, slot) {
            return Err(anyhow!("vmat patch did not take: {slot} still prop-local"));
        }
    }
    Ok(patched)
}

/// Resolve a stock 512x512 BC7 color donor from the pak (named first, then a scan).
fn find_color_donor(vpk: &valve_pak::VPK, size: u32) -> Result<Vec<u8>> {
    let is_match = |b: &[u8]| {
        morphic::inspect(b).is_ok_and(|i| {
            u32::from(i.width) == size
                && u32::from(i.height) == size
                && format!("{:?}", i.format) == "Bc7"
        })
    };
    if let Ok(mut f) = vpk.get_file(COLOR_DONOR) {
        if let Ok(b) = f.read_all() {
            if is_match(&b) {
                return Ok(b);
            }
        }
    }
    for p in vpk.file_paths().filter(|p| p.ends_with(".vtex_c")) {
        if let Ok(mut f) = vpk.get_file(p) {
            if let Ok(b) = f.read_all() {
                if is_match(&b) {
                    return Ok(b);
                }
            }
        }
    }
    Err(anyhow!("no {size}x{size} BC7 color donor found in pak"))
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
            .with_context(|| format!("entry {entry} not found in pak"))?;
        Ok(f.read_all()?)
    };

    // The new (additive) material + its own color texture, both uniquely named so
    // they never collide with stock soul_container assets.
    let vmat_entry = format!("{MAT_DIR}/{name}.vmat_c");
    let vmat_path = format!("{MAT_DIR}/{name}.vmat");
    let color_vtex = format!("{MAT_DIR}/{name}_color.vtex");
    let color_entry = format!("{MAT_DIR}/{name}_color.vtex_c");

    // --- 1. read GLB parts, flat per-material colors, and (maybe) the albedo image ---
    let prims = read_edited_primitives(&glb).map_err(|e| anyhow!("reading glb parts: {e}"))?;
    let colors = glb_base_colors(&doc);
    let albedo_img = glb_albedo_image(&glb, &doc)?;
    let n = prims.len();
    if n == 0 {
        return Err(anyhow!("glb has no mesh parts"));
    }
    let textured = albedo_img.is_some();

    // --- 2. merge parts: bake Y-up -> Z-up. Textured: keep the GLB's REAL UVs so
    //        it samples the shipped image exactly. Flat: point each part's UVs at
    //        its palette column (one band per part) -- the faithful flat look. ---
    let mut merged = VertexBuffer {
        texcoords: vec![Vec::new()],
        ..VertexBuffer::default()
    };
    let mut indices: Vec<u32> = Vec::new();
    let mut palette: Vec<[f64; 4]> = Vec::with_capacity(n);
    for (i, p) in prims.iter().enumerate() {
        let vb = &p.vertex_buffer;
        let base = u32::try_from(merged.positions.len())?;
        merged
            .positions
            .extend(vb.positions.iter().map(|v| [v[0], v[2], -v[1]]));
        merged
            .normals
            .extend(vb.normals.iter().map(|v| [v[0], v[2], -v[1]]));
        if textured {
            let src = vb.texcoords.first();
            for vi in 0..vb.positions.len() {
                let uv = src.and_then(|t| t.get(vi)).copied().unwrap_or([0.0, 0.0]);
                merged.texcoords[0].push(uv);
            }
        } else {
            let u = (i as f32 + 0.5) / n as f32;
            merged.texcoords[0].extend(std::iter::repeat([u, 0.5]).take(vb.positions.len()));
        }
        indices.extend(p.indices.iter().map(|&idx| base + idx));
        let color = p
            .material_name
            .as_deref()
            .and_then(|m| colors.get(m).copied())
            .unwrap_or([1.0, 1.0, 1.0, 1.0]);
        palette.push(color);
    }
    merged.element_count = merged.positions.len();

    // --- 3. fit the merged mesh to the soul-container's bounds ---
    let model_bytes = read(MODEL)?;
    let orb = morphic::model::decode(&model_bytes)
        .map_err(|e| anyhow!("decoding orb model: {e}"))?
        .position_bounds()
        .ok_or_else(|| anyhow!("orb model has no positions"))?;
    let orb_center = [
        midpoint(orb.min[0], orb.max[0]),
        midpoint(orb.min[1], orb.max[1]),
        midpoint(orb.min[2], orb.max[2]),
    ];
    let orb_size = (0..3)
        .map(|k| orb.max[k] - orb.min[k])
        .fold(0.0_f32, f32::max);
    let (mesh_center, mesh_size) = bounds_center_extent(&merged.positions);
    let scale = if mesh_size > 0.0 {
        orb_size / mesh_size
    } else {
        1.0
    };
    for p in &mut merged.positions {
        for k in 0..3 {
            p[k] = (p[k] - mesh_center[k]) * scale + orb_center[k];
        }
    }
    eprintln!(
        "mesh:   merged {n} parts -> {} verts, {} tris; fit x{scale:.3} to orb size {orb_size:.2}",
        merged.element_count,
        indices.len() / 3
    );

    // --- 4. swap the mesh into the stock vmdl (UNCOMPRESSED; meshopt re-encode
    //        garbles in game) and repoint its material to our new name ---
    let (mesh_swapped, rep) =
        replace_mesh_part_uncompressed(&model_bytes, "soul_container", &merged, &indices)
            .map_err(|e| anyhow!("replacing soul_container mesh: {e}"))?;
    let edited_model = set_model_material(&mesh_swapped, &vmat_path)
        .map_err(|e| anyhow!("repointing model material: {e}"))?;
    let dcs = morphic::model::draw_call_targets(&edited_model)
        .map_err(|e| anyhow!("re-reading draw calls: {e}"))?;
    if dcs.iter().any(|d| d.material != vmat_path) {
        return Err(anyhow!("material repoint did not take"));
    }
    eprintln!(
        "model:  replaced mesh -> {} verts, material -> {vmat_path}",
        rep.new_vertex_count
    );

    // --- 5. clean material: copy stock, NPR off, support maps -> defaults, color -> ours ---
    let new_vmat = build_clean_material(&read(VMAT)?, &color_vtex)?;
    eprintln!("vmat:   stock copy cleaned (NPR masks blacked, support maps -> engine defaults, g_tColor -> {color_entry})");

    // --- 6. the color texture: the GLB's real albedo, or a flat per-part palette ---
    let color_tex = if let Some(img) = albedo_img {
        let donor = find_color_donor(&vpk, COLOR_TEX)?;
        let resized = image::imageops::resize(
            &img,
            COLOR_TEX,
            COLOR_TEX,
            image::imageops::FilterType::Lanczos3,
        );
        let tex = replace_mip_chain(
            &donor,
            &Image {
                width: COLOR_TEX,
                height: COLOR_TEX,
                data: ImageData::Rgba8(resized.into_raw()),
            },
        )
        .map_err(|e| anyhow!("encoding GLB albedo onto donor: {e}"))?;
        eprintln!(
            "color:  GLB albedo {}x{} -> {COLOR_TEX}^2 BC7",
            img.width(),
            img.height()
        );
        tex
    } else {
        let donor = read(PALETTE_DONOR)?;
        let tex = build_texture(&donor, PALETTE_TEX, |x, _| {
            let part = ((x * n as u32) / PALETTE_TEX).min(n as u32 - 1) as usize;
            let c = palette[part];
            [to_srgb_u8(c[0]), to_srgb_u8(c[1]), to_srgb_u8(c[2]), 255]
        })?;
        eprintln!("color:  {n}-band flat palette (GLB has no albedo image)");
        tex
    };

    // --- 7. pack: fresh model + cleaned material + the one color texture ---
    let entries: Vec<(&str, &[u8])> = vec![
        (MODEL, edited_model.as_slice()),
        (vmat_entry.as_str(), new_vmat.as_slice()),
        (color_entry.as_str(), color_tex.as_slice()),
    ];
    vpkmerge_core::pack(&entries, &out)?;
    eprintln!(
        "wrote {out} ({} entries; 1 texture, 0 stock-path dependencies)",
        entries.len()
    );
    Ok(())
}
