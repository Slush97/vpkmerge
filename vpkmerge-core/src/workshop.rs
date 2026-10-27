//! Targeted workshop edits that preserve compiled resource layouts and shared assets.
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use morphic::kv3::{Seg, Value};
use std::collections::{BTreeMap, BTreeSet};

use crate::vmat_style::{patch_vmat_params, VmatEdit};
const DASH_STAMP_PATH: &str = "materials/grimoire/movement/dash_stamp.vtex";
const DASH_STAMP_TEMPLATE: &str =
    "materials/particle/abilities/priest/priest_potent_vapors_cross.vtex_c";

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

/// Apply uniform roughness and metalness to one material. Clone the packed masks,
/// including opaque PBR color alpha (metalness), without changing shared textures.
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
    let material = morphic::material::parse(&original)?;
    let packed_metalness = color_alpha_is_metalness(&material);
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
            "g_tColor" if packed_metalness => (3, metalness),
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
        let mut image = morphic::decode(&original_texture)?;
        flatten_channel(&mut image, channel, value)?;
        // Full material path scopes cloned masks to this part, even when several parts
        // originally sampled the same normal map.
        let path = format!(
            "materials/grimoire/finish/{}/{slot}.vtex",
            entry.trim_end_matches(".vmat_c")
        )
        .to_ascii_lowercase();
        files.push((
            compiled(&path),
            crate::trippy::encode_part_texture(&original_texture, &image)?,
        ));
        bindings.push((slot.to_string(), path));
    }
    if !bindings.is_empty() {
        let name = tree
            .get("m_materialName")
            .and_then(Value::as_str)
            .context("Missing material name")?;
        bytes = bind_finish_textures(&bytes, name, &bindings)?;
    }
    files.push((entry, bytes));
    let refs: Vec<_> = files
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    crate::pack(&refs, output)
}

/// How a glowing part animates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlowAnimation {
    Steady,
    /// Brightness rises and falls about every two seconds.
    Pulse,
    /// The glow color cycles through the spectrum.
    Rainbow,
}

/// Turn on a part's self-illumination, limited to the white of `mask` (whole part without one).
///
/// Works on materials that never shipped `F_SELF_ILLUM`: it switches the feature on, binds a mask
/// cloned at a part-scoped path and drives the glow through dynamic expressions for animation.
///
/// # Errors
/// Rejects unsupported shaders, animated self-illumination, invalid values and missing textures.
pub fn glow_material(
    paths: &[PathBuf],
    entry: &str,
    mask: Option<&[u8]>,
    color: [f64; 3],
    strength: f64,
    animation: GlowAnimation,
    output: &Path,
) -> Result<()> {
    ensure!(
        strength.is_finite() && (0.0..=50.0).contains(&strength),
        "Glow strength must be between 0 and 50"
    );
    ensure!(
        color
            .iter()
            .all(|c| c.is_finite() && (0.0..=1.0).contains(c)),
        "Glow color channels must be between 0 and 1"
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
        "Only pbr.vfx supports workshop glow"
    );
    let dynamic = tree
        .get("m_dynamicParams")
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    ensure!(
        !dynamic.iter().any(|v| v
            .get("m_name")
            .and_then(Value::as_str)
            .is_some_and(|n| n.contains("SelfIllum"))),
        "Animated self-illumination cannot be overridden"
    );
    let material = morphic::material::parse(&original)?;
    let template_path = ["g_tColor", "g_tColorA", "g_tBaseColor", "g_tAlbedo"]
        .into_iter()
        .find_map(|slot| material.texture(slot))
        .context("Material has no color texture to size the glow mask from")?;
    let template = read(&paks, &compiled(template_path))?;
    let info = morphic::inspect(&template)?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    let luma = match mask {
        Some(png) => {
            let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png)
                .context("Decoding glow mask PNG")?
                .to_luma8();
            image::imageops::resize(
                &decoded,
                width,
                height,
                image::imageops::FilterType::Triangle,
            )
            .into_raw()
        }
        None => vec![255; (width * height) as usize],
    };
    // Every channel carries the mask, so it reads the same whichever one the shader samples.
    let pixels = luma.iter().flat_map(|&v| [v, v, v, 255]).collect();
    let glow_mask = morphic::Image {
        width,
        height,
        data: morphic::ImageData::Rgba8(pixels),
    };
    let mut edits = vec![
        VmatEdit::Int {
            name: "F_SELF_ILLUM".into(),
            value: 1,
        },
        VmatEdit::Float {
            name: "g_flSelfIllumAlbedoFactor1".into(),
            value: 0.0,
        },
        VmatEdit::Vector {
            name: "g_vSelfIllumScrollSpeed1".into(),
            value: [0.0, 0.0, 0.0, 0.0],
        },
    ];
    edits.push(if animation == GlowAnimation::Pulse {
        VmatEdit::expr(
            "g_flSelfIllumScale1",
            &format!("{strength} * (0.5 * sin(3 * time()) + 0.5)"),
        )?
    } else {
        VmatEdit::Float {
            name: "g_flSelfIllumScale1".into(),
            value: strength,
        }
    });
    edits.push(if animation == GlowAnimation::Rainbow {
        VmatEdit::expr(
            "g_vSelfIllumTint1",
            "float3(0.5 + 0.5 * sin(2 * time()), 0.5 + 0.5 * sin(2 * time() + 2.094), 0.5 + 0.5 * sin(2 * time() + 4.189))",
        )?
    } else {
        VmatEdit::Vector { name: "g_vSelfIllumTint1".into(), value: [color[0], color[1], color[2], 0.0] }
    });
    let (mut bytes, stats) = patch_vmat_params(&original, &edits)?;
    ensure!(
        stats.failed.is_empty(),
        "Glow parameters could not be patched: {:?}",
        stats.failed
    );
    let path = format!(
        "materials/grimoire/glow/{}/selfillum.vtex",
        entry.trim_end_matches(".vmat_c")
    )
    .to_ascii_lowercase();
    let name = tree
        .get("m_materialName")
        .and_then(Value::as_str)
        .context("Missing material name")?;
    bytes = bind_finish_textures(
        &bytes,
        name,
        &[("g_tSelfIllumMask".to_string(), path.clone())],
    )?;
    let texture = crate::trippy::encode_part_texture(&template, &glow_mask)?;
    crate::pack(
        &[
            (entry.as_str(), bytes.as_slice()),
            (compiled(&path).as_str(), texture.as_slice()),
        ],
        output,
    )
}

fn color_alpha_is_metalness(material: &morphic::material::Material) -> bool {
    material.alpha_mode() == morphic::material::AlphaMode::Opaque
        && material
            .int_params
            .get("F_BLEND_MODE")
            .copied()
            .unwrap_or(0)
            == 0
}

/// Change a static PBR part's blend mode without interpreting its packed metalness as opacity.
///
/// # Errors
/// Rejects unsupported shaders, cutouts, dynamic alpha bindings, invalid values and missing textures.
pub fn surface_material(
    paths: &[PathBuf],
    entry: &str,
    mode: &str,
    opacity: f64,
    output: &Path,
) -> Result<()> {
    ensure!(
        matches!(mode, "translucent" | "ghost"),
        "Unknown surface mode"
    );
    ensure!(
        opacity.is_finite() && (0.0..=1.0).contains(&opacity),
        "Opacity must be between 0 and 1"
    );
    ensure!(
        (entry.starts_with("materials/") || entry.starts_with("models/"))
            && !entry.contains("..")
            && !entry.contains('\\')
            && !entry.contains(':'),
        "Invalid material path"
    );
    let paks = open(paths)?;
    let entry = compiled(entry);
    let original = read(&paks, &entry)?;
    let material = morphic::material::parse(&original)?;
    ensure!(
        material.shader_name == "pbr.vfx",
        "Only pbr.vfx supports workshop surfaces"
    );
    ensure!(
        material
            .int_params
            .get("F_ALPHA_TEST")
            .copied()
            .unwrap_or(0)
            == 0,
        "Cutout materials cannot change surface mode"
    );
    let tree = morphic::decode_kv3_resource(&original)?;
    for group in ["m_dynamicParams", "m_dynamicTextureParams"] {
        let dynamic = tree.get(group).and_then(Value::as_array).unwrap_or(&[]);
        ensure!(
            !dynamic
                .iter()
                .filter_map(|v| v.get("m_name").and_then(Value::as_str))
                .any(|name| {
                    let name = name.to_ascii_lowercase();
                    ["opacity", "transluc", "alpha", "glass"]
                        .iter()
                        .any(|part| name.contains(part))
                }),
            "Animated transparency parameters cannot be overridden"
        );
    }
    let SurfaceTextures {
        mut files,
        bindings,
    } = surface_textures(&paks, &entry, mode, &material)?;
    let edits = surface_edits(mode, opacity);
    let (mut bytes, stats) = patch_vmat_params(&original, &edits)?;
    ensure!(
        stats.failed.is_empty(),
        "Surface parameters could not be patched: {:?}",
        stats.failed
    );
    if !bindings.is_empty() {
        let name = tree
            .get("m_materialName")
            .and_then(Value::as_str)
            .context("Missing material name")?;
        bytes = bind_finish_textures(&bytes, name, &bindings)?;
    }
    files.push((entry, bytes));
    crate::pack(
        &files
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect::<Vec<_>>(),
        output,
    )
}

fn surface_edits(mode: &str, opacity: f64) -> [VmatEdit; 8] {
    [
        VmatEdit::Int {
            name: "F_TRANSLUCENT".into(),
            value: 1,
        },
        VmatEdit::Int {
            name: "F_ADVANCED_TRANSLUCENCY".into(),
            value: i64::from(mode == "ghost"),
        },
        VmatEdit::Int {
            name: "F_GLASS".into(),
            value: 0,
        },
        VmatEdit::Int {
            name: "F_ADDITIVE_BLEND".into(),
            value: 0,
        },
        VmatEdit::Int {
            name: "g_nTranslucencyBlendMode1".into(),
            value: 0,
        },
        VmatEdit::Float {
            name: "g_flOpacityScale1".into(),
            value: opacity,
        },
        VmatEdit::Float {
            name: "g_flAlphaAnglePower1".into(),
            value: if mode == "ghost" { 2.0 } else { 0.0 },
        },
        VmatEdit::Vector {
            name: "g_flAlphaAngleScaleBias1".into(),
            value: if mode == "ghost" {
                [-1.0, 1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0, 0.0]
            },
        },
    ]
}

struct SurfaceTextures {
    files: Vec<(String, Vec<u8>)>,
    bindings: Vec<(String, String)>,
}

fn surface_textures(
    paks: &[valve_pak::VPK],
    entry: &str,
    mode: &str,
    material: &morphic::material::Material,
) -> Result<SurfaceTextures> {
    let color_path = material
        .texture("g_tColor")
        .context("A bound g_tColor is required for surface edits")?;
    let color_bytes = read(paks, &compiled(color_path))?;
    let stem = format!(
        "materials/grimoire/surface/{}",
        entry.trim_end_matches(".vmat_c")
    )
    .to_ascii_lowercase();
    let mut files = Vec::<(String, Vec<u8>)>::new();
    let mut bindings = Vec::<(String, String)>::new();
    let already_blended = material
        .int_params
        .get("F_TRANSLUCENT")
        .copied()
        .unwrap_or(0)
        != 0
        || material
            .int_params
            .get("F_ADVANCED_TRANSLUCENCY")
            .copied()
            .unwrap_or(0)
            != 0
        || material
            .int_params
            .get("F_BLEND_MODE")
            .copied()
            .unwrap_or(0)
            != 0;
    if !already_blended {
        // Color alpha becomes opacity. Packed metalness moves only into a g_tMetalness slot the
        // material already binds: adding a texture slot rewrites the whole material, which the
        // game renders as a red wireframe. Without one, the see-through part loses its metalness.
        let mut color = morphic::decode(&color_bytes)?;
        flatten_channel(&mut color, 3, 1.0)?;
        let path = format!("{stem}/color.vtex");
        files.push((
            compiled(&path),
            crate::trippy::encode_part_texture(&color_bytes, &color)?,
        ));
        bindings.push(("g_tColor".into(), path));
    }
    if mode == "ghost" && material.texture("g_tAltTranslucency").is_none() {
        let mask = morphic::Image {
            width: 1,
            height: 1,
            data: morphic::ImageData::Rgba8(vec![255; 4]),
        };
        let path = format!("{stem}/alttranslucency.vtex");
        files.push((
            compiled(&path),
            crate::trippy::encode_part_texture(&color_bytes, &mask)?,
        ));
        bindings.push(("g_tAltTranslucency".into(), path));
    }
    Ok(SurfaceTextures { files, bindings })
}

fn bind_finish_textures(
    bytes: &[u8],
    name: &str,
    bindings: &[(String, String)],
) -> Result<Vec<u8>> {
    let mut tree = morphic::decode_kv3_resource(bytes)?;
    let params = tree
        .get_mut("m_textureParams")
        .and_then(|value| {
            if let Value::Array(params) = value {
                Some(params)
            } else {
                None
            }
        })
        .context("Material has no texture parameter table")?;
    let mut inserted = false;
    for (slot, path) in bindings {
        if !params
            .iter()
            .any(|param| param.get("m_name").and_then(Value::as_str) == Some(slot))
        {
            params.push(Value::Object(vec![
                ("m_name".into(), Value::String(slot.clone())),
                ("m_pValue".into(), Value::String(path.clone())),
            ]));
            inserted = true;
        }
    }
    // Appended slots borrow the existing texture entry's native Resource flag.
    let donor = if inserted {
        morphic::encode_kv3_resource_like(bytes, &tree)?
    } else {
        bytes.to_vec()
    };
    let refs: Vec<_> = bindings
        .iter()
        .map(|(slot, path)| (slot.as_str(), path.as_str()))
        .collect();
    let changed = morphic::compile_pbr_vmat(&donor, name, &refs)?;
    let resource = morphic::resource::Resource::parse(&changed)?;
    let index = resource
        .blocks()
        .iter()
        .position(|block| block.kind == *b"RERL")
        .context("Material has no resource reference list")?;
    let mut references = morphic::resource::read_rerl(
        resource
            .get_block_by_index(index)
            .context("Material references")?,
    )?;
    for (_, path) in bindings {
        if !references.iter().any(|(_, name)| name == path) {
            references.push((morphic::resource::resource_id(path), path.clone()));
        }
    }
    references.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(resource.rebuild_with_block(index, &morphic::resource::write_rerl(&references))?)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn flatten_channel(image: &mut morphic::Image, channel: usize, value: f64) -> Result<()> {
    let morphic::ImageData::Rgba8(pixels) = &mut image.data else {
        bail!("HDR finish masks are unsupported")
    };
    let byte = (value * 255.0).round() as u8;
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel[channel] = byte;
    }
    Ok(())
}

#[derive(Default)]
pub struct MovementFxOptions<'a> {
    pub end_color: Option<&'a str>,
    pub rainbow: bool,
    pub image: Option<&'a Path>,
    pub brightness: Option<f64>,
    pub illuminated: bool,
    pub outline_color: Option<&'a str>,
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
    options: &MovementFxOptions<'_>,
    output: &Path,
) -> Result<()> {
    let MovementFxOptions {
        end_color,
        rainbow,
        image,
        brightness,
        illuminated,
        outline_color,
    } = *options;
    let brightness = brightness.unwrap_or(1.0);
    ensure!(
        brightness.is_finite() && (0.25..=4.0).contains(&brightness),
        "Brightness must be between 0.25 and 4"
    );
    ensure!(
        outline_color.is_none() || image.is_some(),
        "Outlines require an image trail"
    );
    ensure!(
        matches!(kind, "dash" | "heavy-melee"),
        "Unknown movement effect"
    );
    ensure!(
        image.is_none() || kind == "dash",
        "Image stamps are supported on dash trails"
    );
    let rgb = parse_color(color)?;
    ensure!(
        !rainbow || end_color.is_none(),
        "Choose a gradient or rainbow"
    );
    let end_rgb = end_color.map(parse_color).transpose()?;
    let paks = [valve_pak::open(base)?];
    let mut queue = movement_roots(&paks[0], kind)?;
    let mut files = BTreeMap::<String, Vec<u8>>::new();
    if let Some(path) = image {
        let template = read(&paks, DASH_STAMP_TEMPLATE)?;
        files.insert(compiled(DASH_STAMP_PATH), stamp_texture(path, &template)?);
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
        let colored = kind != "dash" || entry.ends_with("_trail.vpcf_c") || rainbow;
        if colored {
            collect_colors(&tree, &mut Vec::new(), false, rgb, &mut colors);
        }
        let mut bytes = morphic::patch_kv3_resource_scalars(&original, &colors)?;
        if image.is_some()
            && entry.starts_with("particles/generic/")
            && entry.ends_with("_trail.vpcf_c")
        {
            bytes = image_trail(&bytes, &tree, DASH_STAMP_PATH)?;
        }
        if kind == "dash" && entry.ends_with("_trail.vpcf_c") {
            bytes = trail_appearance(
                &bytes,
                image.is_some(),
                brightness,
                illuminated,
                outline_color,
            )?;
        }
        if colored && (rainbow || end_rgb.is_some()) {
            bytes = color_ramp(&bytes, rgb, end_rgb, rainbow)?;
        }
        files.insert(entry, bytes);
    }
    let refs: Vec<_> = files
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    crate::pack(&refs, output)
}

fn movement_roots(pak: &valve_pak::VPK, kind: &str) -> Result<Vec<String>> {
    let roots: Vec<String> = if kind == "dash" {
        ["air_dash", "down_dash", "roll_jump"]
            .iter()
            .map(|name| format!("particles/generic/{name}.vpcf_c"))
            .collect()
    } else {
        pak.file_paths()
            .filter(|path| {
                (path.starts_with("particles/abilities/melee/")
                    || path.starts_with("particles/abilities/melee_"))
                    && path.ends_with(".vpcf_c")
                    && path
                        .rsplit('/')
                        .next()
                        .is_some_and(|name| name.contains("heavy"))
            })
            .cloned()
            .collect()
    };
    ensure!(
        !roots.is_empty(),
        "No matching movement effects in this game build"
    );
    Ok(roots)
}

fn stamp_texture(path: &Path, template: &[u8]) -> Result<Vec<u8>> {
    let png = std::fs::read(path)?;
    ensure!(png.len() <= 4 * 1024 * 1024, "Stamp image exceeds 4 MiB");
    let (width, height) =
        image::ImageReader::with_format(std::io::Cursor::new(&png), image::ImageFormat::Png)
            .into_dimensions()?;
    ensure!(
        width > 0 && height > 0 && width <= 512 && height <= 512,
        "Stamp image exceeds 512 pixels"
    );
    // Use the GPU format and mip layout of a game-authored sprite texture.
    crate::icon::build_icon_from_template(template, &png)
}

/// Append an RGB operator without re-encoding the compiled resource. Particle
/// age gives a gradient along the tail; collection age cycles the rainbow.
fn color_ramp(
    bytes: &[u8],
    start: [i64; 3],
    end: Option<[i64; 3]>,
    rainbow: bool,
) -> Result<Vec<u8>> {
    let tree = morphic::decode_kv3_resource(bytes)?;
    let Some(operators) = tree.get("m_Operators").and_then(Value::as_array) else {
        return Ok(bytes.to_vec());
    };
    let colors = if rainbow {
        vec![
            [255, 50, 50],
            [255, 230, 50],
            [50, 255, 90],
            [50, 220, 255],
            [100, 70, 255],
            [255, 60, 200],
            [255, 50, 50],
        ]
    } else {
        vec![start, end.unwrap_or(start)]
    };
    let intervals = u32::try_from(colors.len() - 1)?;
    let stops = colors
        .into_iter()
        .enumerate()
        .map(|(i, rgb)| {
            Ok(Value::Object(vec![
                (
                    "m_flPosition".into(),
                    Value::Double(f64::from(u32::try_from(i)?) / f64::from(intervals)),
                ),
                (
                    "m_Color".into(),
                    Value::Array(rgb.into_iter().map(Value::Int).collect()),
                ),
            ]))
        })
        .collect::<Result<Vec<_>>>()?;
    let input = Value::Object(vec![
        (
            "m_nType".into(),
            Value::String(
                if rainbow {
                    "PF_TYPE_COLLECTION_AGE"
                } else {
                    "PF_TYPE_PARTICLE_AGE_NORMALIZED"
                }
                .into(),
            ),
        ),
        (
            "m_nInputMode".into(),
            Value::String(
                if rainbow {
                    "PF_INPUT_MODE_LOOPED"
                } else {
                    "PF_INPUT_MODE_CLAMPED"
                }
                .into(),
            ),
        ),
        ("m_flInput0".into(), Value::Double(0.0)),
        (
            "m_flInput1".into(),
            Value::Double(if rainbow { 1.5 } else { 1.0 }),
        ),
        ("m_flOutput0".into(), Value::Double(0.0)),
        ("m_flOutput1".into(), Value::Double(1.0)),
    ]);
    let operator = Value::Object(vec![
        ("_class".into(), Value::String("C_OP_SetVec".into())),
        ("m_nOutputField".into(), Value::Int(6)),
        (
            "m_InputValue".into(),
            Value::Object(vec![
                (
                    "m_nType".into(),
                    Value::String("PVEC_TYPE_FLOAT_INTERP_GRADIENT".into()),
                ),
                ("m_FloatInterp".into(), input),
                (
                    "m_Gradient".into(),
                    Value::Object(vec![("m_Stops".into(), Value::Array(stops))]),
                ),
            ]),
        ),
    ]);
    Ok(morphic::patch_kv3_resource_array_insert(
        bytes,
        &[Seg::Key("m_Operators".into())],
        operators.len(),
        &operator,
    )?)
}

fn image_trail(bytes: &[u8], tree: &Value, image_path: &str) -> Result<Vec<u8>> {
    let renderers = tree
        .get("m_Renderers")
        .and_then(Value::as_array)
        .context("Dash trail has no renderer")?;
    let mut strings = Vec::new();
    let mut references = Vec::new();
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
        let original_texture = textures[0]
            .get("m_hTexture")
            .and_then(Value::as_str)
            .context("Dash trail has no primary texture resource")?;
        references.push((original_texture.into(), image_path.into()));
        if renderer.get("m_nOutputBlendMode").is_some() {
            strings.push((
                [root, vec![Seg::Key("m_nOutputBlendMode".into())]].concat(),
                "PARTICLE_OUTPUT_BLEND_MODE_ALPHA".into(),
            ));
        }
    }
    let bytes = morphic::patch_kv3_resource_strings_adding(bytes, &strings)?;
    // The engine precaches RERL, while the viewer follows the DATA string.
    // Redirect both so the game loads the same texture the preview displays.
    Ok(morphic::repoint_resource_refs(&bytes, &references)?.0)
}

fn trail_appearance(
    bytes: &[u8],
    image: bool,
    brightness: f64,
    illuminated: bool,
    outline: Option<&str>,
) -> Result<Vec<u8>> {
    if !image && (brightness - 1.0).abs() < f64::EPSILON && !illuminated && outline.is_none() {
        return Ok(bytes.to_vec());
    }
    let resource = morphic::resource::Resource::parse(bytes)?;
    let template = morphic::kv3::decode_lossless(resource.data_block()?)?;
    let mut tree = template.to_value();
    let Some(Value::Array(renderers)) = tree.get_mut("m_Renderers") else {
        bail!("Trail has no renderers")
    };
    for renderer in renderers {
        let mut light = renderer
            .get("m_flSelfIllumAmount")
            .cloned()
            .unwrap_or_else(|| {
                Value::Object(vec![
                    ("m_nType".into(), Value::String("PF_TYPE_LITERAL".into())),
                    ("m_flLiteralValue".into(), Value::Double(1.0)),
                ])
            });
        let value = light
            .get_mut("m_flLiteralValue")
            .context("Trail illumination is not a literal")?;
        let base = if image {
            1.0
        } else {
            value
                .as_f64()
                .context("Trail illumination is not numeric")?
        };
        *value = Value::Double(base * brightness * if illuminated { 3.0 } else { 1.0 });
        let Value::Object(fields) = renderer else {
            bail!("Trail renderer is not an object")
        };
        if let Some((_, old)) = fields
            .iter_mut()
            .find(|(name, _)| name == "m_flSelfIllumAmount")
        {
            *old = light;
        } else {
            fields.push(("m_flSelfIllumAmount".into(), light));
        }
        if let Some(color) = outline {
            for (key, value) in [
                ("m_bOutline", Value::Bool(true)),
                (
                    "m_OutlineColor",
                    Value::Array(parse_color(color)?.into_iter().map(Value::Int).collect()),
                ),
                ("m_flOutlineStart0", Value::Double(0.1)),
                ("m_flOutlineStart1", Value::Double(0.25)),
                ("m_flOutlineEnd0", Value::Double(0.7)),
                ("m_flOutlineEnd1", Value::Double(0.9)),
            ] {
                if let Some((_, old)) = fields.iter_mut().find(|(name, _)| name == key) {
                    *old = value;
                } else {
                    fields.push((key.into(), value));
                }
            }
        }
    }
    let data = morphic::kv3::encode_v5_like(&tree, &template)?;
    Ok(resource.rebuild_with_data(&data)?)
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
    #[test]
    fn heavy_melee_includes_the_generic_swing_outside_the_melee_folder() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source_dir.vpk");
        let paths = [
            "particles/abilities/melee_swing_heavy.vpcf_c",
            "particles/abilities/melee_swing_heavy_ring.vpcf_c",
            "particles/abilities/melee/melee_heavy_activate.vpcf_c",
            "particles/abilities/melee_swing_light.vpcf_c",
            "particles/abilities/baba/baba_witching_hour_melee_swing_heavy.vpcf_c",
        ];
        crate::pack(
            &paths
                .iter()
                .map(|path| (*path, b"fixture".as_slice()))
                .collect::<Vec<_>>(),
            &source,
        )
        .unwrap();
        let pak = valve_pak::open(&source).unwrap();
        let roots = movement_roots(&pak, "heavy-melee").unwrap();
        assert!(roots.contains(&paths[0].to_string()));
        assert!(roots.contains(&paths[1].to_string()));
        assert!(roots.contains(&paths[2].to_string()));
        assert!(!roots.contains(&paths[3].to_string()));
        assert!(!roots.contains(&paths[4].to_string()));
    }
    use super::*;
    #[test]
    fn stamp_texture_keeps_gpu_format_mips_and_transparency() {
        let template = std::fs::read(format!(
            "{}/../morphic/fixtures/bc7/generic_sleep_icon.vtex_c",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("stamp.png");
        image::RgbaImage::from_fn(64, 64, |x, _| {
            image::Rgba([200, 30, 100, if x < 32 { 0 } else { 255 }])
        })
        .save(&path)
        .unwrap();
        let output = stamp_texture(&path, &template).unwrap();
        let before = morphic::inspect(&template).unwrap();
        let after = morphic::inspect(&output).unwrap();
        assert_eq!(after.format, morphic::TextureFormat::Bc7);
        assert_eq!(after.mip_count, before.mip_count);
        assert_eq!((after.width, after.height), (before.width, before.height));
        let morphic::ImageData::Rgba8(pixels) = morphic::decode(&output).unwrap().data else {
            panic!()
        };
        assert_eq!(pixels[3], 0);
        assert_eq!(pixels[pixels.len() - 1], 255);
    }

    #[test]
    fn image_trail_accepts_double_illumination_and_preserves_the_particle_tree() {
        let container = std::fs::read(format!(
            "{}/../morphic/fixtures/particle/grenade_frag_fx.vpcf_c",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let tree = Value::Object(vec![
            (
                "_class".into(),
                Value::String("CParticleSystemDefinition".into()),
            ),
            ("m_nMaxParticles".into(), Value::Int(20)),
            (
                "m_Renderers".into(),
                Value::Array(vec![Value::Object(vec![
                    ("_class".into(), Value::String("C_OP_RenderRopes".into())),
                    (
                        "m_flSelfIllumAmount".into(),
                        Value::Object(vec![
                            ("m_nType".into(), Value::String("PF_TYPE_LITERAL".into())),
                            ("m_flLiteralValue".into(), Value::Double(0.2)),
                        ]),
                    ),
                    (
                        "m_nOutputBlendMode".into(),
                        Value::String("PARTICLE_OUTPUT_BLEND_MODE_ADD".into()),
                    ),
                    (
                        "m_vecTexturesInput".into(),
                        Value::Array(vec![Value::Object(vec![(
                            "m_hTexture".into(),
                            Value::String("materials/particle/base_sprite.vtex".into()),
                        )])]),
                    ),
                ])]),
            ),
        ]);
        let resource = morphic::resource::Resource::parse(&container).unwrap();
        let format = morphic::kv3::decode_lossless(resource.data_block().unwrap())
            .unwrap()
            .format;
        let old_texture = "materials/particle/base_sprite.vtex";
        let other_texture = "materials/particle/noise.vtex";
        let references = morphic::resource::write_rerl(&[
            (
                morphic::resource::resource_id(old_texture),
                old_texture.into(),
            ),
            (
                morphic::resource::resource_id(other_texture),
                other_texture.into(),
            ),
        ]);
        let data = morphic::kv3::encode(&tree, &format);
        let index = |kind| {
            resource
                .blocks()
                .iter()
                .position(|block| block.kind == kind)
                .unwrap()
        };
        let source = resource
            .rebuild_with_blocks(&[(index(*b"DATA"), &data), (index(*b"RERL"), &references)])
            .unwrap();
        let image = image_trail(&source, &tree, DASH_STAMP_PATH).unwrap();
        let styled = trail_appearance(&image, true, 1.5, true, Some("#ff00ff")).unwrap();
        let after = morphic::decode_kv3_resource(&styled).unwrap();
        assert_eq!(after.get("m_nMaxParticles"), tree.get("m_nMaxParticles"));
        let renderer = &after.get("m_Renderers").unwrap().as_array().unwrap()[0];
        assert_eq!(
            renderer.get("_class").unwrap().as_str(),
            Some("C_OP_RenderSprites")
        );
        assert_eq!(
            renderer
                .get("m_flSelfIllumAmount")
                .unwrap()
                .get("m_flLiteralValue")
                .unwrap()
                .as_f64(),
            Some(4.5)
        );
        assert_eq!(renderer.get("m_bOutline"), Some(&Value::Bool(true)));
        let resource = morphic::resource::Resource::parse(&styled).unwrap();
        let references =
            morphic::resource::read_rerl(resource.find_block(*b"RERL").unwrap()).unwrap();
        assert!(references.contains(&(
            morphic::resource::resource_id(DASH_STAMP_PATH),
            DASH_STAMP_PATH.into()
        )));
        assert!(!references.iter().any(|(_, path)| path == old_texture));
        assert!(references.contains(&(
            morphic::resource::resource_id(other_texture),
            other_texture.into()
        )));
        assert_eq!(
            renderer.get("m_nOutputBlendMode").unwrap().as_str(),
            Some("PARTICLE_OUTPUT_BLEND_MODE_ALPHA")
        );
    }
    #[test]
    fn gradient_operator_roundtrips_compiled_particle_without_changing_other_fields() {
        let source = std::fs::read(format!(
            "{}/../morphic/fixtures/particle/grenade_frag_fx.vpcf_c",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let before = morphic::decode_kv3_resource(&source).unwrap();
        let count = before.get("m_Operators").unwrap().as_array().unwrap().len();
        let patched = color_ramp(&source, [255, 0, 0], Some([0, 0, 255]), false).unwrap();
        let after = morphic::decode_kv3_resource(&patched).unwrap();
        let operators = after.get("m_Operators").unwrap().as_array().unwrap();
        assert_eq!(operators.len(), count + 1);
        assert_eq!(after.get("m_Initializers"), before.get("m_Initializers"));
        assert_eq!(after.get("m_Renderers"), before.get("m_Renderers"));
        assert_eq!(operators[count].get("m_nOutputField"), Some(&Value::Int(6)));
        assert_eq!(
            operators[count]
                .get("m_InputValue")
                .unwrap()
                .get("m_FloatInterp")
                .unwrap()
                .get("m_nType")
                .unwrap()
                .as_str(),
            Some("PF_TYPE_PARTICLE_AGE_NORMALIZED")
        );
    }
    fn assert_texture_pixels(paks: &[valve_pak::VPK], path: &str, expected: [u8; 4]) {
        let image = morphic::decode(&read(paks, &compiled(path)).unwrap()).unwrap();
        let morphic::ImageData::Rgba8(pixels) = image.data else {
            panic!()
        };
        for pixel in pixels.as_chunks::<4>().0 {
            for (actual, expected) in pixel.iter().zip(expected) {
                assert!(
                    actual.abs_diff(expected) <= 3,
                    "Unexpected channel in {path}: {pixel:?}"
                );
            }
        }
    }

    #[test]
    fn finish_clones_a_bound_mask_without_changing_shared_source() {
        let donor = std::fs::read(format!(
            "{}/../morphic/fixtures/material/vindicta_headv2.vmat_c",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let tree = morphic::decode_kv3_resource(&donor).unwrap();
        let pixels = morphic::Image {
            width: 8,
            height: 8,
            data: morphic::ImageData::Rgba8([127, 140, 200, 0].repeat(64)),
        };
        let template = std::fs::read(format!(
            "{}/../morphic/fixtures/bc7/gradient_dev_02_color_psd_73660177.vtex_c",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let texture = morphic::resize_mip_chain(&template, &pixels).unwrap();
        let source_info = morphic::inspect(&texture).unwrap();
        let mut files = vec![("materials/test/part.vmat_c".to_string(), donor.clone())];
        for param in tree
            .get("m_textureParams")
            .and_then(Value::as_array)
            .unwrap()
        {
            let path = param.get("m_pValue").and_then(Value::as_str).unwrap();
            files.push((compiled(path), texture.clone()));
        }
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source_dir.vpk");
        let output = dir.path().join("finish_dir.vpk");
        crate::pack(
            &files
                .iter()
                .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
                .collect::<Vec<_>>(),
            &source,
        )
        .unwrap();
        let source_bytes = std::fs::read(&source).unwrap();
        finish_material(
            std::slice::from_ref(&source),
            "materials/test/part.vmat",
            0.5,
            1.0,
            &output,
        )
        .unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        let paks = open(&[output]).unwrap();
        let edited =
            morphic::decode_kv3_resource(&read(&paks, "materials/test/part.vmat_c").unwrap())
                .unwrap();
        let params = edited
            .get("m_textureParams")
            .and_then(Value::as_array)
            .unwrap();
        let normal = params
            .iter()
            .find(|p| p.get("m_name").and_then(Value::as_str) == Some("g_tNormalRoughness"))
            .unwrap();
        let path = normal.get("m_pValue").and_then(Value::as_str).unwrap();
        assert!(path.starts_with("materials/grimoire/finish/materials/test/part/"));
        assert_eq!(
            path,
            path.to_ascii_lowercase(),
            "Engine resource paths must be lowercase"
        );
        let material_bytes = read(&paks, "materials/test/part.vmat_c").unwrap();
        let resource = morphic::resource::Resource::parse(&material_bytes).unwrap();
        let refs = morphic::resource::read_rerl(resource.find_block(*b"RERL").unwrap()).unwrap();
        assert!(
            refs.iter()
                .any(|(id, name)| name == path && *id == morphic::resource::resource_id(path)),
            "Cloned finish textures must be precached by the material"
        );
        let encoded = read(&paks, &compiled(path)).unwrap();
        let mask_info = morphic::inspect(&encoded).unwrap();
        assert_eq!(mask_info.format, source_info.format);
        assert_eq!(mask_info.flags, source_info.flags);
        assert_eq!(mask_info.mip_count, source_info.mip_count);
        assert_texture_pixels(&paks, path, [127, 140, 128, 0]);
        // Material shaders and all unrelated flags stay on the byte-faithful path.
        assert_eq!(edited.get("m_shaderName"), tree.get("m_shaderName"));
        assert_eq!(edited.get("m_intParams"), tree.get("m_intParams"));
        let color_path = params
            .iter()
            .find(|param| param.get("m_name").and_then(Value::as_str) == Some("g_tColor"))
            .unwrap()
            .get("m_pValue")
            .and_then(Value::as_str)
            .unwrap();
        assert_texture_pixels(&paks, color_path, [127, 140, 200, 255]);
    }
    #[test]
    fn transparent_finishes_keep_the_authored_opacity_texture() {
        let donor = include_bytes!("../../morphic/fixtures/material/vindicta_headv2.vmat_c");
        let texture = morphic::encode_vtex_png_rgba8888(
            &morphic::Image {
                width: 1,
                height: 1,
                data: morphic::ImageData::Rgba8(vec![100, 120, 150, 37]),
            },
            morphic::TextureFlags::empty(),
        )
        .unwrap();
        let parsed = morphic::material::parse(donor).unwrap();
        for flag in ["F_TRANSLUCENT", "F_ALPHA_TEST", "F_BLEND_MODE"] {
            let (material, stats) = patch_vmat_params(
                donor,
                &[VmatEdit::Int {
                    name: flag.into(),
                    value: 1,
                }],
            )
            .unwrap();
            assert_eq!(stats.failed, Vec::<String>::new());
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source_dir.vpk");
            let output = dir.path().join("finish_dir.vpk");
            let mut files = vec![("models/test/part.vmat_c".to_string(), material)];
            for path in parsed.texture_params.values() {
                files.push((compiled(path), texture.clone()));
            }
            crate::pack(
                &files
                    .iter()
                    .map(|(entry, data)| (entry.as_str(), data.as_slice()))
                    .collect::<Vec<_>>(),
                &source,
            )
            .unwrap();
            finish_material(&[source], "models/test/part.vmat", 0.1, 1.0, &output).unwrap();
            let pak = open(&[output]).unwrap();
            let edited =
                morphic::material::parse(&read(&pak, "models/test/part.vmat_c").unwrap()).unwrap();
            assert_eq!(
                edited.texture("g_tColor"),
                parsed.texture("g_tColor"),
                "{flag} uses color alpha for opacity"
            );
        }
    }

    fn surface_source(dir: &Path, edits: &[VmatEdit]) -> PathBuf {
        let donor = include_bytes!("../../morphic/fixtures/material/vindicta_headv2.vmat_c");
        let (bytes, stats) = patch_vmat_params(donor, edits).unwrap();
        assert_eq!(stats.failed, Vec::<String>::new());
        let material = morphic::material::parse(&bytes).unwrap();
        let image = morphic::Image {
            width: 8,
            height: 8,
            data: morphic::ImageData::Rgba8([100, 120, 150, 37].repeat(64)),
        };
        let template =
            include_bytes!("../../morphic/fixtures/bc7/gradient_dev_02_color_psd_73660177.vtex_c");
        let texture = morphic::resize_mip_chain(template, &image).unwrap();
        let mut files = vec![("models/test/part.vmat_c".to_string(), bytes)];
        for path in material.texture_params.values() {
            files.push((compiled(path), texture.clone()));
        }
        let source = dir.join("source_dir.vpk");
        crate::pack(
            &files
                .iter()
                .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
                .collect::<Vec<_>>(),
            &source,
        )
        .unwrap();
        source
    }

    fn assert_surface_resource_flags(bytes: &[u8]) {
        let resource = morphic::resource::Resource::parse(bytes).unwrap();
        let (tree, flags) = morphic::kv3::decode_flagged(resource.data_block().unwrap()).unwrap();
        for (index, param) in tree
            .get("m_textureParams")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            if ["g_tMetalness", "g_tAltTranslucency"]
                .contains(&param.get("m_name").unwrap().as_str().unwrap())
            {
                assert_eq!(
                    flags.get(&vec![
                        Seg::Key("m_textureParams".into()),
                        Seg::Index(index),
                        Seg::Key("m_pValue".into())
                    ]),
                    Some(&1),
                    "Inserted texture paths must retain the native Resource flag"
                );
            }
        }
    }

    #[test]
    fn surface_turns_color_alpha_into_opacity_without_adding_texture_slots() {
        for mode in ["translucent", "ghost"] {
            let dir = tempfile::tempdir().unwrap();
            let source = surface_source(dir.path(), &[]);
            let before = std::fs::read(&source).unwrap();
            let output = dir.path().join("surface_dir.vpk");
            surface_material(
                std::slice::from_ref(&source),
                "models/test/part.vmat",
                mode,
                0.35,
                &output,
            )
            .unwrap();
            assert_eq!(std::fs::read(&source).unwrap(), before);
            let paks = open(&[output.clone(), source.clone()]).unwrap();
            let bytes = read(&paks, "models/test/part.vmat_c").unwrap();
            let mat = morphic::material::parse(&bytes).unwrap();
            assert_eq!(mat.int_params["F_TRANSLUCENT"], 1);
            assert_eq!(
                mat.int_params["F_ADVANCED_TRANSLUCENCY"],
                i64::from(mode == "ghost")
            );
            assert!((mat.float_params["g_flOpacityScale1"] - 0.35).abs() < 0.00001);
            assert!(
                (mat.float_params["g_flAlphaAnglePower1"]
                    - if mode == "ghost" { 2.0 } else { 0.0 })
                .abs()
                    < 0.00001
            );
            let resource = morphic::resource::Resource::parse(&bytes).unwrap();
            let refs =
                morphic::resource::read_rerl(resource.find_block(*b"RERL").unwrap()).unwrap();
            assert_surface_resource_flags(&bytes);
            // No new slot: adding one rewrites the material, which renders as a red wireframe.
            assert!(mat.texture("g_tMetalness").is_none());
            let path = mat.texture("g_tColor").unwrap();
            assert!(path.starts_with("materials/grimoire/surface/models/test/part/"));
            assert!(refs.contains(&(morphic::resource::resource_id(path), path.into())));
            let encoded = read(&paks, &compiled(path)).unwrap();
            assert_eq!(
                morphic::inspect(&encoded).unwrap().format,
                morphic::TextureFormat::Bc7
            );
            assert_texture_pixels(&paks, path, [100, 120, 150, 255]);
            if mode == "ghost" {
                let mask = mat.texture("g_tAltTranslucency").unwrap();
                assert!(refs.contains(&(morphic::resource::resource_id(mask), mask.into())));
                let image = morphic::decode(&read(&paks, &compiled(mask)).unwrap()).unwrap();
                assert_eq!((image.width, image.height), (1, 1));
            }
            // A finish afterwards still reads opacity from color alpha and adds no metalness slot.
            let finish = dir.path().join("finish_dir.vpk");
            finish_material(
                &[output, source],
                "models/test/part.vmat",
                0.2,
                1.0,
                &finish,
            )
            .unwrap();
            let finished = open(&[finish]).unwrap();
            let after =
                morphic::material::parse(&read(&finished, "models/test/part.vmat_c").unwrap())
                    .unwrap();
            assert_eq!(after.texture("g_tColor"), mat.texture("g_tColor"));
            assert!(after.texture("g_tMetalness").is_none());
        }
    }

    #[test]
    fn surface_preserves_existing_blended_opacity_and_rejects_cutouts_or_dynamic_alpha() {
        let dir = tempfile::tempdir().unwrap();
        let source = surface_source(
            dir.path(),
            &[VmatEdit::Int {
                name: "F_TRANSLUCENT".into(),
                value: 1,
            }],
        );
        let paks = open(std::slice::from_ref(&source)).unwrap();
        let original =
            morphic::material::parse(&read(&paks, "models/test/part.vmat_c").unwrap()).unwrap();
        let output = dir.path().join("surface_dir.vpk");
        surface_material(
            std::slice::from_ref(&source),
            "models/test/part.vmat",
            "ghost",
            0.35,
            &output,
        )
        .unwrap();
        let paks = open(&[output]).unwrap();
        let edited =
            morphic::material::parse(&read(&paks, "models/test/part.vmat_c").unwrap()).unwrap();
        assert_eq!(edited.texture("g_tColor"), original.texture("g_tColor"));
        let source = surface_source(
            dir.path(),
            &[VmatEdit::Int {
                name: "F_ALPHA_TEST".into(),
                value: 1,
            }],
        );
        assert!(surface_material(
            std::slice::from_ref(&source),
            "models/test/part.vmat",
            "ghost",
            0.35,
            &dir.path().join("cutout_dir.vpk")
        )
        .unwrap_err()
        .to_string()
        .contains("Cutout"));
        let source = surface_source(
            dir.path(),
            &[VmatEdit::expr("g_flOpacityScale1", "time()").unwrap()],
        );
        assert!(surface_material(
            &[source],
            "models/test/part.vmat",
            "ghost",
            0.35,
            &dir.path().join("dynamic_dir.vpk")
        )
        .unwrap_err()
        .to_string()
        .contains("Animated"));
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
