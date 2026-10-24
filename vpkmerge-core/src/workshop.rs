//! Targeted workshop edits that preserve compiled resource layouts and shared assets.
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use morphic::kv3::{Seg, Value};
use std::collections::{BTreeMap, BTreeSet};

use crate::vmat_style::{patch_vmat_params, VmatEdit};

fn compiled(entry: &str) -> String {
    if entry.ends_with("_c") {
        entry.to_string()
    } else {
        format!("{entry}_c")
    }
}

fn read(paks: &[valve_pak::VPK], entry: &str) -> Result<Vec<u8>> {
    for pak in paks {
        if let Ok(mut file) = pak.get_file(entry) {
            return file.read_all().context("reading workshop asset");
        }
    }
    bail!("Missing workshop asset: {entry}")
}

fn open(paths: &[PathBuf]) -> Result<Vec<valve_pak::VPK>> {
    paths
        .iter()
        .map(|path| valve_pak::open(path).with_context(|| format!("opening {}", path.display())))
        .collect()
}

/// Apply uniform roughness and metalness to one material. Bound masks are cloned,
/// preserving the normal map's red/green channels and leaving shared textures alone.
///
/// # Errors
/// Rejects unsupported shaders, animated finish bindings, missing textures and invalid values.
pub fn finish_material(
    paths: &[PathBuf],
    entry: &str,
    roughness: f64,
    metalness: f64,
    output: &Path,
) -> Result<()> {
    ensure!(
        [roughness, metalness]
            .iter()
            .all(|n| n.is_finite() && (0.0..=1.0).contains(n)),
        "Finish values must be between 0 and 1"
    );
    ensure!(
        entry.starts_with("materials/") || entry.starts_with("models/"),
        "Expected a material resource path"
    );
    ensure!(
        !entry.contains("..") && !entry.contains('\\') && !entry.contains(':'),
        "Invalid material path"
    );
    let paks = open(paths)?;
    let entry = compiled(entry);
    let original = read(&paks, &entry)?;
    let tree = morphic::decode_kv3_resource(&original)?;
    ensure!(
        tree.get("m_shaderName").and_then(Value::as_str) == Some("pbr.vfx"),
        "Only pbr.vfx supports workshop finishes"
    );
    let dynamic = tree
        .get("m_dynamicParams")
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    ensure!(
        !dynamic.iter().any(|v| v
            .get("m_name")
            .and_then(Value::as_str)
            .is_some_and(|n| n.contains("Roughness") || n.contains("Metalness"))),
        "Animated finish parameters cannot be overridden"
    );
    let edits = [
        VmatEdit::Vector {
            name: "TextureRoughness1".into(),
            value: [roughness, roughness, roughness, 0.0],
        },
        VmatEdit::Vector {
            name: "TextureMetalness1".into(),
            value: [metalness, metalness, metalness, 0.0],
        },
    ];
    let (mut bytes, stats) = patch_vmat_params(&original, &edits)?;
    ensure!(
        stats.failed.is_empty(),
        "Material finish parameters could not be patched: {:?}",
        stats.failed
    );
    let mut files = Vec::<(String, Vec<u8>)>::new();
    let mut bindings = Vec::<(String, String)>::new();
    for param in tree
        .get("m_textureParams")
        .and_then(Value::as_array)
        .unwrap_or(&[])
    {
        let slot = param.get("m_name").and_then(Value::as_str).unwrap_or("");
        let (channel, value) = match slot {
            "g_tNormalRoughness" => (2, roughness),
            "g_tRoughness" => (0, roughness),
            "g_tMetalness" | "g_tMetalnessMask" => (0, metalness),
            _ => continue,
        };
        let texture = param
            .get("m_pValue")
            .and_then(Value::as_str)
            .context("Missing finish texture binding")?;
        let original_texture = read(&paks, &compiled(texture))?;
        let flags = morphic::inspect(&original_texture)?.flags;
        let mut image = morphic::decode(&original_texture)?;
        flatten_channel(&mut image, channel, value)?;
        // Full material path scopes cloned masks to this part, even when several parts
        // originally sampled the same normal map.
        let path = format!(
            "materials/grimoire/finish/{}/{slot}.vtex",
            entry.trim_end_matches(".vmat_c")
        );
        files.push((
            compiled(&path),
            morphic::encode_vtex_png_rgba8888(&image, flags)?,
        ));
        bindings.push((slot.to_string(), path));
    }
    if !bindings.is_empty() {
        let name = tree
            .get("m_materialName")
            .and_then(Value::as_str)
            .context("Missing material name")?;
        let refs: Vec<_> = bindings
            .iter()
            .map(|(slot, path)| (slot.as_str(), path.as_str()))
            .collect();
        bytes = morphic::compile_pbr_vmat(&bytes, name, &refs)?;
    }
    files.push((entry, bytes));
    let refs: Vec<_> = files
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    crate::pack(&refs, output)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn flatten_channel(image: &mut morphic::Image, channel: usize, value: f64) -> Result<()> {
    let morphic::ImageData::Rgba8(pixels) = &mut image.data else {
        bail!("HDR finish masks are unsupported")
    };
    let byte = (value * 255.0).round() as u8;
    for pixel in pixels.chunks_exact_mut(4) {
        pixel[channel] = byte;
    }
    Ok(())
}

/// Current-game movement definitions, recolored with optional image stamps on
/// dash trail spritecards. No third-party particle definitions are used.
///
/// # Errors
/// Rejects invalid recipes, colors, images and unbounded or missing particle graphs.
pub fn movement_fx(
    base: &Path,
    kind: &str,
    color: &str,
    image: Option<&Path>,
    output: &Path,
) -> Result<()> {
    ensure!(
        matches!(kind, "dash" | "heavy-melee"),
        "Unknown movement effect"
    );
    ensure!(
        image.is_none() || kind == "dash",
        "Image stamps are supported on dash trails"
    );
    let rgb = parse_color(color)?;
    let pak = valve_pak::open(base)?;
    let paks = [pak];
    let mut queue: Vec<String> = if kind == "dash" {
        ["air_dash", "down_dash", "roll_jump"]
            .iter()
            .map(|n| format!("particles/generic/{n}.vpcf_c"))
            .collect()
    } else {
        paks[0]
            .file_paths()
            .filter(|p| {
                p.starts_with("particles/abilities/melee/")
                    && p.ends_with(".vpcf_c")
                    && p.rsplit('/').next().is_some_and(|n| n.contains("heavy"))
            })
            .cloned()
            .collect()
    };
    ensure!(
        !queue.is_empty(),
        "No matching movement effects in this game build"
    );
    let image_path = "materials/grimoire/movement/dash_stamp.vtex";
    let mut files = BTreeMap::<String, Vec<u8>>::new();
    if let Some(path) = image {
        let png = std::fs::read(path)?;
        ensure!(png.len() <= 4 * 1024 * 1024, "Stamp image exceeds 4 MiB");
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png)?;
        ensure!(
            decoded.width() <= 512 && decoded.height() <= 512,
            "Stamp image exceeds 512 pixels"
        );
        let flags = morphic::TextureFlags::SUGGEST_CLAMP_S | morphic::TextureFlags::SUGGEST_CLAMP_T;
        files.insert(
            compiled(image_path),
            morphic::encode_vtex_png_rgba8888_from_png(&png, flags)?,
        );
    }
    let mut seen = BTreeSet::new();
    while let Some(entry) = queue.pop() {
        if !seen.insert(entry.clone()) {
            continue;
        }
        ensure!(
            seen.len() <= 256,
            "Movement particle graph exceeds 256 systems"
        );
        ensure!(
            entry.starts_with("particles/") && !entry.contains("..") && !entry.contains('\\'),
            "Invalid particle reference"
        );
        let original = read(&paks, &entry)?;
        let tree = morphic::decode_kv3_resource(&original)?;
        ensure!(
            tree.get("_class").and_then(Value::as_str) == Some("CParticleSystemDefinition"),
            "Not a particle definition: {entry}"
        );
        for child in tree
            .get("m_Children")
            .and_then(Value::as_array)
            .unwrap_or(&[])
        {
            if let Some(path) = child.get("m_ChildRef").and_then(Value::as_str) {
                queue.push(compiled(path));
            }
        }
        let mut colors = Vec::new();
        collect_colors(&tree, &mut Vec::new(), false, rgb, &mut colors);
        let mut bytes = morphic::patch_kv3_resource_scalars(&original, &colors)?;
        if image.is_some()
            && entry.starts_with("particles/generic/")
            && entry.ends_with("_trail.vpcf_c")
        {
            bytes = image_trail(&bytes, &tree, image_path)?;
        }
        files.insert(entry, bytes);
    }
    let refs: Vec<_> = files
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    crate::pack(&refs, output)
}

fn image_trail(bytes: &[u8], tree: &Value, image_path: &str) -> Result<Vec<u8>> {
    let renderers = tree
        .get("m_Renderers")
        .and_then(Value::as_array)
        .context("Dash trail has no renderer")?;
    let mut strings = Vec::new();
    for (ri, renderer) in renderers.iter().enumerate() {
        ensure!(
            renderer.get("_class").and_then(Value::as_str) == Some("C_OP_RenderRopes"),
            "Dash renderer changed; image stamps need a recipe update"
        );
        let root = vec![Seg::Key("m_Renderers".into()), Seg::Index(ri)];
        strings.push((
            [root.clone(), vec![Seg::Key("_class".into())]].concat(),
            "C_OP_RenderSprites".into(),
        ));
        let textures = renderer
            .get("m_vecTexturesInput")
            .and_then(Value::as_array)
            .context("Dash trail has no texture inputs")?;
        ensure!(!textures.is_empty(), "Dash trail has no primary texture");
        for (ti, texture) in textures.iter().enumerate() {
            ensure!(
                ti == 0 || texture.get("m_bEnabled") == Some(&Value::Bool(false)),
                "Dash trail has another enabled layer; image stamps need a recipe update"
            );
        }
        strings.push((
            [
                root.clone(),
                vec![
                    Seg::Key("m_vecTexturesInput".into()),
                    Seg::Index(0),
                    Seg::Key("m_hTexture".into()),
                ],
            ]
            .concat(),
            image_path.into(),
        ));
        if renderer.get("m_nOutputBlendMode").is_some() {
            strings.push((
                [root, vec![Seg::Key("m_nOutputBlendMode".into())]].concat(),
                "PARTICLE_BLEND_MODE_ALPHA".into(),
            ));
        }
    }
    morphic::patch_kv3_resource_strings_adding(bytes, &strings).map_err(Into::into)
}

fn parse_color(color: &str) -> Result<[i64; 3]> {
    let hex = color.strip_prefix('#').unwrap_or(color);
    ensure!(
        hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "Expected a six-digit RGB color"
    );
    Ok([
        i64::from_str_radix(&hex[0..2], 16)?,
        i64::from_str_radix(&hex[2..4], 16)?,
        i64::from_str_radix(&hex[4..6], 16)?,
    ])
}

fn collect_colors(
    value: &Value,
    path: &mut Vec<Seg>,
    colorish: bool,
    rgb: [i64; 3],
    edits: &mut Vec<(Vec<Seg>, i64)>,
) {
    if let Value::Array(items) = value {
        if colorish && matches!(items.len(), 3 | 4) {
            let channels: Option<Vec<i64>> = items
                .iter()
                .map(|v| match v {
                    Value::Int(n) if (0..=255).contains(n) => Some(*n),
                    Value::UInt(n) => i64::try_from(*n).ok().filter(|n| (0..=255).contains(n)),
                    _ => None,
                })
                .collect();
            if let Some(channels) = channels {
                let peak = channels[..3].iter().copied().max().unwrap_or(0);
                for (i, channel) in rgb.iter().enumerate() {
                    let mut p = path.clone();
                    p.push(Seg::Index(i));
                    edits.push((p, (channel * peak + 127) / 255));
                }
                return;
            }
        }
        for (i, child) in items.iter().enumerate() {
            path.push(Seg::Index(i));
            collect_colors(child, path, false, rgb, edits);
            path.pop();
        }
    } else if let Value::Object(pairs) = value {
        for (key, child) in pairs {
            let name = key.to_ascii_lowercase();
            path.push(Seg::Key(key.clone()));
            collect_colors(
                child,
                path,
                name.contains("color") || name.contains("tint"),
                rgb,
                edits,
            );
            path.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finish_clones_a_bound_mask_without_changing_shared_source() {
        let donor = std::fs::read(format!("{}/../morphic/fixtures/material/vindicta_headv2.vmat_c", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let tree = morphic::decode_kv3_resource(&donor).unwrap();
        let pixels = morphic::Image { width: 1, height: 1, data: morphic::ImageData::Rgba8(vec![127, 140, 200, 255]) };
        let texture = morphic::encode_vtex_png_rgba8888(&pixels, morphic::TextureFlags::empty()).unwrap();
        let mut files = vec![("materials/test/part.vmat_c".to_string(), donor.clone())];
        for param in tree.get("m_textureParams").and_then(Value::as_array).unwrap() {
            let path = param.get("m_pValue").and_then(Value::as_str).unwrap();
            files.push((compiled(path), texture.clone()));
        }
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source_dir.vpk");
        let output = dir.path().join("finish_dir.vpk");
        crate::pack(&files.iter().map(|(path, bytes)| (path.as_str(), bytes.as_slice())).collect::<Vec<_>>(), &source).unwrap();
        let source_bytes = std::fs::read(&source).unwrap();
        finish_material(std::slice::from_ref(&source), "materials/test/part.vmat", 0.5, 1.0, &output).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        let paks = open(&[output]).unwrap();
        let edited = morphic::decode_kv3_resource(&read(&paks, "materials/test/part.vmat_c").unwrap()).unwrap();
        let params = edited.get("m_textureParams").and_then(Value::as_array).unwrap();
        let normal = params.iter().find(|p| p.get("m_name").and_then(Value::as_str) == Some("g_tNormalRoughness")).unwrap();
        let path = normal.get("m_pValue").and_then(Value::as_str).unwrap();
        assert!(path.starts_with("materials/grimoire/finish/materials/test/part/"));
        let mask = morphic::decode(&read(&paks, &compiled(path)).unwrap()).unwrap();
        let morphic::ImageData::Rgba8(bytes) = mask.data else { panic!() };
        assert_eq!(bytes, [127, 140, 128, 255]);
        // Material shaders and all unrelated flags stay on the byte-faithful path.
        assert_eq!(edited.get("m_shaderName"), tree.get("m_shaderName"));
        assert_eq!(edited.get("m_intParams"), tree.get("m_intParams"));
    }
    #[test]
    fn roughness_preserves_normal_and_alpha() {
        let mut image = morphic::Image {
            width: 2,
            height: 1,
            data: morphic::ImageData::Rgba8(vec![127, 140, 0, 255, 160, 120, 255, 19]),
        };
        flatten_channel(&mut image, 2, 0.5).unwrap();
        let morphic::ImageData::Rgba8(pixels) = image.data else {
            panic!()
        };
        assert_eq!(pixels, [127, 140, 128, 255, 160, 120, 128, 19]);
    }

    #[test]
    fn color_picker_preserves_fades_alpha_and_positions() {
        let tree = Value::Object(vec![
            (
                "m_ConstantColor".into(),
                Value::Array(vec![
                    Value::Int(255),
                    Value::Int(255),
                    Value::Int(255),
                    Value::Int(120),
                ]),
            ),
            ("m_ColorFade".into(), Value::Array(vec![Value::Int(0); 3])),
            ("m_Position".into(), Value::Array(vec![Value::Int(12); 3])),
        ]);
        let mut edits = Vec::new();
        collect_colors(&tree, &mut Vec::new(), false, [255, 0, 128], &mut edits);
        assert_eq!(
            edits.iter().map(|(_, n)| *n).collect::<Vec<_>>(),
            [255, 0, 128, 0, 0, 0]
        );
        assert!(parse_color("#ff0080").is_ok());
        assert!(parse_color("#wat").is_err());
    }
}
