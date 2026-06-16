// Import a custom multi-material GLB as a clean soul-container addon.
//
// The soul container is a static Deadlock prop whose stock material (`pbr.vfx`
// with NPR lighting + a self-illum mask) is authored for the orb's own UVs. When
// you graft a foreign mesh (Piplup) into it, the orb's full-res 2048^2 stock
// textures (color, self-illum mask, ambient occlusion) get sampled through the
// imported mesh's unrelated UVs: the color map smears the orb's surface over the
// import and the masks scatter its glow/shadow into splotches.
//
// The exact slot each map fills has changed between game builds (an older build
// misnamed the self-illum mask `..._ao_png_...`; the current one gives it its own
// name and binds a real AO map), and the filenames carry content hashes that change
// too. So this importer reads the bound paths from the material at runtime
// (`bound_vtex_c`) rather than hardcoding them -- hardcoding stale, hash-stamped
// paths is exactly what let the stock orb texture render over the import.
//
// This importer:
//   1. reads every GLB part, bakes the glTF Y-up -> Source 2 Z-up axis swap, and
//      points each part's UVs at its own column of a palette so one merged draw
//      call still shows per-part colors,
//   2. authors that palette `g_tColor` from the GLB's per-material base colors so
//      the prop is actually colored (Piplup: blue body, white belly, orange beak),
//   3. overrides `g_tSelfIllumMask` with a flat black texture, which gates the
//      splotchy self-illum off so the prop reads as a solid toy,
//   4. patches the orb's material to drop the `pbr.vfx` NPR effects that a texture
//      override cannot reach, so it renders as a plain smooth prop:
//        a. `F_SOLID_COLOR_OUTLINE` -> 0 and `F_USE_NPR_LIGHTING` -> 0. The first is
//           the toon "solid color outline" that inks every silhouette and crease edge
//           of the grafted mesh (eyes, beak, flipper seams); the second is the cel
//           lighting whose hard bands read as "not smooth" on a rounded, light prop.
//           With both off, pbr.vfx uses ordinary smooth lighting -- the common combo
//           thousands of non-NPR props use. (`F_SELF_ILLUM` stays on but mask-gated
//           black at step 3.)
//        b. `g_vSolidOutlineAdditive` -> 0. A teal the outline *adds* across the
//           surface (its mask is the white engine default = full coverage), which on
//           its own lifts every albedo toward teal: orange beak/feet wash to cream,
//           blue body to pale cyan. Redundant once the outline feature is off, but
//           zeroed defensively.
//        c. `g_tTintMaskRimLightMask` -> the black mask already interned in the table
//           (the bound `g_tNprTransmissiveColor`). NPR rim light is mask-gated; the
//           stock white-ish rim reads as an unwanted halo on a smooth, light prop, so
//           a black mask removes it. A string redirect to an existing table entry --
//           no new texture, no table growth.
// then packs the model + the two textures + the patched material at their base entry
// paths. Steps 1-3 need no `.vmat_c` edit (textures win on path); step 4 reaches the
// feature flag, constant, and texture binding a texture override cannot, patched in
// place on the blobbed material (byte-faithful KV3 scalar + double + string patches,
// no fragile full re-encode).
//
// usage: cargo run --release --example piplup_soul -- <pak01_dir.vpk> <model.glb> <out_dir.vpk>
use anyhow::{anyhow, Context, Result};
use morphic::kv3::{Seg, Value as Kv3};
use morphic::model::{read_edited_primitives, replace_mesh_part, VertexBuffer};
use morphic::{replace_mip_chain, Image, ImageData};
use serde_json::Value as Json;
use std::collections::HashMap;

const MODEL: &str = "models/props_gameplay/soul_container/soul_container.vmdl_c";
// The override entry paths for g_tColor / g_tSelfIllumMask / g_tAmbientOcclusion are
// resolved from the material at runtime (`bound_vtex_c`), NOT hardcoded: their
// filenames carry content hashes that change between game builds and the material has
// been re-authored more than once. A stale hardcoded path silently misses, leaving the
// stock texture to render over the import.
// The orb's stock material. We patch a couple of NPR knobs on it (see step 6.5).
const VMAT: &str = "models/props_gameplay/soul_container/materials/soul_container.vmat_c";
// A flat-black mask already interned in the stock material's string table (it is the
// bound `g_tNprTransmissiveColor`), so a texture slot can be repointed to it in place
// with no string-table growth. `.vtex` (not `_c`): that is how vmat paths are stored.
const BLACK_MASK: &str = "materials/default/default_black_mask_tga_e7be3cc.vtex";
// A 64x64 BGRA8888 single-mip donor: `replace_mip_chain` keeps its dims/format/
// flags and only rewrites the pixels, yielding an engine-valid texture in a format
// morphic can encode (the real maps are BC7 / RG11_EAC, which the encoder cannot
// emit, so they can't be used as donors).
const DONOR: &str = "panorama/images/hud/zipline_icon_psd.vtex_c";
const TEX: u32 = 64;

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

/// Map each GLB material *name* to its `baseColorFactor` (linear RGBA), parsed
/// straight from the JSON chunk. Missing factor defaults to opaque white.
fn glb_base_colors(glb: &[u8]) -> Result<HashMap<String, [f64; 4]>> {
    let json_len = u32::from_le_bytes(glb[12..16].try_into()?) as usize;
    let doc: Json = serde_json::from_slice(&glb[20..20 + json_len])?;
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
    Ok(map)
}

fn midpoint(a: f32, b: f32) -> f32 {
    (a + b) / 2.0
}

/// Bounding-box center and largest-axis extent over a set of positions.
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

/// Build a vtex on the donor whose pixels come from `fill(x, y)` (RGBA bytes).
fn build_texture(donor: &[u8], fill: impl Fn(u32, u32) -> [u8; 4]) -> Result<Vec<u8>> {
    let mut px = vec![0u8; (TEX * TEX * 4) as usize];
    for y in 0..TEX {
        for x in 0..TEX {
            let o = ((y * TEX + x) * 4) as usize;
            px[o..o + 4].copy_from_slice(&fill(x, y));
        }
    }
    replace_mip_chain(
        donor,
        &Image {
            width: TEX,
            height: TEX,
            data: ImageData::Rgba8(px),
        },
    )
    .map_err(|e| anyhow!("encoding texture on donor: {e}"))
}

/// Index of a named entry in a material's `m_vectorParams` array.
fn vector_param_index(v: &Kv3, name: &str) -> Option<usize> {
    v.get("m_vectorParams")?
        .as_array()?
        .iter()
        .position(|p| p.get("m_name").and_then(Kv3::as_str) == Some(name))
}

/// Path-edits that set the first three components (RGB) of a named vector param
/// to zero, for `morphic::patch_kv3_resource_doubles`.
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

/// Path-edit that sets a named `m_intParams` feature flag, for
/// `morphic::patch_kv3_resource_scalars`. Empty if the feature is absent.
fn int_param_edit(v: &Kv3, name: &str, val: i64) -> Vec<(Vec<Seg>, i64)> {
    let Some(i) = v.get("m_intParams").and_then(Kv3::as_array).and_then(|a| {
        a.iter()
            .position(|p| p.get("m_name").and_then(Kv3::as_str) == Some(name))
    }) else {
        return Vec::new();
    };
    vec![(
        vec![
            Seg::Key("m_intParams".to_string()),
            Seg::Index(i),
            Seg::Key("m_nValue".to_string()),
        ],
        val,
    )]
}

/// Read a named `m_intParams` feature flag (for the post-patch round-trip gate).
fn int_param(v: &Kv3, name: &str) -> Option<i64> {
    v.get("m_intParams")?
        .as_array()?
        .iter()
        .find(|p| p.get("m_name").and_then(Kv3::as_str) == Some(name))?
        .get("m_nValue")?
        .as_int()
}

/// Read a named `m_textureParams` bound path (gate) / its index (edit path).
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

/// True for a texture path the importer may safely override: one in THIS prop's own
/// material folder. A `materials/default/*` path is a SHARED engine texture; packing
/// over it would clobber every other prop in the game (e.g. the soul_container binds
/// the shared `default_ao_tga` for `g_tAmbientOcclusion` in some builds), so those are
/// never overridden -- a stock default is flat, so it reads harmlessly through the
/// foreign UVs anyway.
fn is_prop_local(path: &str) -> bool {
    path.starts_with("models/props_gameplay/soul_container/")
}

/// The overridable VPK entry (`.vtex_c`) for a material texture slot: `Some` only when
/// the material binds a PROP-LOCAL texture there (safe to override in place). `None` if
/// the slot is unbound or points at a shared `materials/default/*` texture. Bound paths
/// are stored as `.vtex`; the compiled entry is `.vtex_c`. Reading the path from the
/// material at runtime (instead of hardcoding a content-hash-stamped filename) keeps the
/// override aimed at the texture the material samples even as the hash changes between
/// game builds.
fn overridable_entry(v: &Kv3, slot: &str) -> Option<String> {
    let p = texture_param(v, slot)?;
    if !is_prop_local(&p) {
        return None;
    }
    Some(match p.strip_suffix(".vtex") {
        Some(stem) => format!("{stem}.vtex_c"),
        None => p,
    })
}

/// Path-edit that repoints a named texture slot to another `.vtex` path, for
/// `morphic::patch_kv3_resource_strings` (the target must already be interned).
fn repoint_texture_edit(v: &Kv3, name: &str, new_path: &str) -> Vec<(Vec<Seg>, String)> {
    let Some(i) = texture_param_index(v, name) else {
        return Vec::new();
    };
    vec![(
        vec![
            Seg::Key("m_textureParams".to_string()),
            Seg::Index(i),
            Seg::Key("m_pValue".to_string()),
        ],
        new_path.to_string(),
    )]
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let pak = args.next().context("arg1: pak01_dir.vpk")?;
    let glb_path = args.next().context("arg2: model glb")?;
    let out = args.next().context("arg3: out_dir.vpk")?;

    let glb = std::fs::read(&glb_path)?;
    let vpk = valve_pak::open(&pak)?;
    let read = |entry: &str| -> Result<Vec<u8>> {
        let mut f = vpk
            .get_file(entry)
            .with_context(|| format!("entry {entry} not found in pak"))?;
        Ok(f.read_all()?)
    };

    // --- 1. read the GLB parts + their flat per-material colors ---
    let prims = read_edited_primitives(&glb).map_err(|e| anyhow!("reading glb parts: {e}"))?;
    let colors = glb_base_colors(&glb)?;
    let n = prims.len();
    if n == 0 {
        return Err(anyhow!("glb has no mesh parts"));
    }

    // --- 2. merge parts: bake Y-up -> Z-up, point each part's UVs at its column ---
    let mut merged = VertexBuffer {
        texcoords: vec![Vec::new()],
        ..VertexBuffer::default()
    };
    let mut indices: Vec<u32> = Vec::new();
    let mut palette: Vec<[f64; 4]> = Vec::with_capacity(n);
    for (i, p) in prims.iter().enumerate() {
        let vb = &p.vertex_buffer;
        let base = u32::try_from(merged.positions.len())?;
        // glTF Y-up -> Source 2 Z-up: (x, y, z) -> (x, z, -y). Pure rotation, so
        // the same swap applies to normals.
        merged
            .positions
            .extend(vb.positions.iter().map(|v| [v[0], v[2], -v[1]]));
        merged
            .normals
            .extend(vb.normals.iter().map(|v| [v[0], v[2], -v[1]]));
        // Every vertex of this part samples the center of palette column `i`.
        let u = (i as f32 + 0.5) / n as f32;
        merged.texcoords[0].extend(std::iter::repeat([u, 0.5]).take(vb.positions.len()));
        indices.extend(p.indices.iter().map(|&idx| base + idx));
        let color = p
            .material_name
            .as_deref()
            .and_then(|m| colors.get(m).copied())
            .unwrap_or([1.0, 1.0, 1.0, 1.0]);
        palette.push(color);
    }
    merged.element_count = merged.positions.len();

    // --- 3. fit the merged mesh to the soul-container's own bounds ---
    // The GLB is in its own arbitrary scale/origin; scale it uniformly so its
    // largest axis matches the orb's size and recenter it on the orb, so the
    // import drops into exactly the slot the original mesh occupied (uniform +
    // centered, so it is independent of any residual axis convention).
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

    // --- 4. graft the merged mesh into the soul_container part ---
    let (edited_model, rep) = replace_mesh_part(&model_bytes, "soul_container", &merged, &indices)
        .map_err(|e| anyhow!("replacing soul_container mesh: {e}"))?;
    eprintln!("model:  replaced part -> {rep:?}");

    // --- 5. palette albedo: one horizontal band per part ---
    let donor = read(DONOR)?;
    let albedo = build_texture(&donor, |x, _| {
        let part = ((x * n as u32) / TEX).min(n as u32 - 1) as usize;
        let c = palette[part];
        [to_srgb_u8(c[0]), to_srgb_u8(c[1]), to_srgb_u8(c[2]), 255]
    })?;
    eprintln!("albedo: {n}-band palette ({} bytes)", albedo.len());

    // --- 6. flat-black self-illum mask: kills the splotchy glow ---
    let mask = build_texture(&donor, |_, _| [0, 0, 0, 255])?;
    eprintln!("mask:   flat-black self-illum ({} bytes)", mask.len());

    // --- 6.5. resolve the real override paths + strip the orb's toon shading ---
    let vmat_bytes = read(VMAT)?;
    let vmat = morphic::decode_kv3_resource(&vmat_bytes)
        .map_err(|e| anyhow!("decoding soul_container vmat: {e}"))?;

    // Resolve where each texture override must land from the material's OWN bindings.
    // Hardcoding hash-stamped paths is what let the stock orb color texture overlap the
    // import: the override was written to a path the material does not sample.
    let gtcolor_entry = overridable_entry(&vmat, "g_tColor").ok_or_else(|| {
        anyhow!("g_tColor is unbound or a shared default; cannot place the palette albedo")
    })?;
    // Override the self-illum mask + AO only when they are PROP-LOCAL (see is_prop_local):
    // overriding a shared materials/default/* texture would clobber every other prop. A
    // shared default is flat, so it reads harmlessly through the foreign UVs anyway.
    let selfillum_entry = overridable_entry(&vmat, "g_tSelfIllumMask");
    let ao_entry = overridable_entry(&vmat, "g_tAmbientOcclusion");
    eprintln!("bind:   g_tColor -> {gtcolor_entry} (palette albedo)");
    match &selfillum_entry {
        Some(e) => eprintln!("bind:   g_tSelfIllumMask -> {e} (flat-black override)"),
        None => eprintln!("bind:   g_tSelfIllumMask shared/absent -> no override (harmless)"),
    }
    match &ao_entry {
        Some(e) => eprintln!("bind:   g_tAmbientOcclusion -> {e} (flat-white override)"),
        None => eprintln!("bind:   g_tAmbientOcclusion shared/absent -> no override (harmless)"),
    }

    // Patch only the NPR knobs this material actually has. Across builds the orb's
    // material has gained/lost features (F_SOLID_COLOR_OUTLINE + g_vSolidOutlineAdditive
    // are absent in newer builds), so a missing feature is skipped, not a hard error.
    let mut patched_vmat = vmat_bytes.clone();

    // (a) NPR static features -> off (each only if present). F_SOLID_COLOR_OUTLINE is the
    //     contour/ink line; F_USE_NPR_LIGHTING is the cel/toon banding that reads as "not
    //     smooth" on a rounded, light prop. With both off, pbr.vfx falls back to standard
    //     smooth lighting. F_SELF_ILLUM stays on but mask-gated black.
    let mut int_edits = int_param_edit(&vmat, "F_SOLID_COLOR_OUTLINE", 0);
    int_edits.extend(int_param_edit(&vmat, "F_USE_NPR_LIGHTING", 0));
    if !int_edits.is_empty() {
        patched_vmat = morphic::patch_kv3_resource_scalars(&patched_vmat, &int_edits)
            .map_err(|e| anyhow!("disabling NPR features: {e}"))?;
    }

    // (b) zero g_vSolidOutlineAdditive (the teal the outline adds across the whole
    //     surface), if the material has it.
    let dbl_edits = zero_vec3_edits(&vmat, "g_vSolidOutlineAdditive");
    if !dbl_edits.is_empty() {
        patched_vmat = morphic::patch_kv3_resource_doubles(&patched_vmat, &dbl_edits)
            .map_err(|e| anyhow!("zeroing g_vSolidOutlineAdditive: {e}"))?;
    }

    // (c) repoint g_tTintMaskRimLightMask to a flat-black mask already interned in the
    //     table. The repoint target must already be in the string table; the bound
    //     g_tNprTransmissiveColor is that black mask, so only repoint when it matches.
    let black_interned =
        texture_param(&vmat, "g_tNprTransmissiveColor").as_deref() == Some(BLACK_MASK);
    let str_edits = if black_interned {
        repoint_texture_edit(&vmat, "g_tTintMaskRimLightMask", BLACK_MASK)
    } else {
        Vec::new()
    };
    if !str_edits.is_empty() {
        patched_vmat = morphic::patch_kv3_resource_strings(&patched_vmat, &str_edits)
            .map_err(|e| anyhow!("repointing rim-light mask: {e}"))?;
    }

    // round-trip gate: confirm the patched bytes still decode and every edit that
    // applied took. Features absent from this material are not asserted.
    let check = morphic::decode_kv3_resource(&patched_vmat)
        .map_err(|e| anyhow!("patched vmat does not re-decode: {e}"))?;
    for flag in ["F_SOLID_COLOR_OUTLINE", "F_USE_NPR_LIGHTING"] {
        if int_param(&vmat, flag).is_some() && int_param(&check, flag) != Some(0) {
            return Err(anyhow!("vmat patch did not take: {flag} still on"));
        }
    }
    if !dbl_edits.is_empty() {
        let still_teal = vector_param_index(&check, "g_vSolidOutlineAdditive")
            .and_then(|i| {
                check
                    .get("m_vectorParams")?
                    .as_array()?
                    .get(i)?
                    .get("m_value")?
                    .as_array()
            })
            .map(|c| {
                c.iter()
                    .take(3)
                    .any(|x| x.as_f64().unwrap_or(0.0).abs() > 1e-6)
            })
            .unwrap_or(true);
        if still_teal {
            return Err(anyhow!("vmat patch did not take: additive still non-zero"));
        }
    }
    if !str_edits.is_empty()
        && texture_param(&check, "g_tTintMaskRimLightMask").as_deref() != Some(BLACK_MASK)
    {
        return Err(anyhow!(
            "vmat patch did not take: rim-light mask not repointed"
        ));
    }
    eprintln!(
        "vmat:   NPR features off where present, teal zeroed, rim mask -> black ({} bytes)",
        patched_vmat.len()
    );

    // --- 7. pack (textures win on path; the patched material overrides the base) ---
    // Flat-white AO neutralizes a full-res stock AO sampled through the foreign UVs.
    let ao_white = ao_entry
        .as_ref()
        .map(|_| build_texture(&donor, |_, _| [255, 255, 255, 255]))
        .transpose()?;

    let mut entries: Vec<(&str, &[u8])> = vec![
        (MODEL, edited_model.as_slice()),
        (gtcolor_entry.as_str(), albedo.as_slice()),
        (VMAT, patched_vmat.as_slice()),
    ];
    if let Some(e) = selfillum_entry.as_ref() {
        entries.push((e.as_str(), mask.as_slice()));
    }
    if let (Some(e), Some(white)) = (ao_entry.as_ref(), ao_white.as_ref()) {
        entries.push((e.as_str(), white.as_slice()));
    }
    let n_entries = entries.len();
    vpkmerge_core::pack(&entries, &out)?;
    eprintln!(
        "wrote {out} ({n_entries} entries: model + palette albedo + de-toon'd material{}{})",
        if selfillum_entry.is_some() {
            " + black self-illum mask"
        } else {
            ""
        },
        if ao_white.is_some() {
            " + flat-white AO"
        } else {
            ""
        }
    );
    Ok(())
}
