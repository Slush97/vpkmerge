use anyhow::{Context, Result};
use image::ImageEncoder;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct MipLevel {
    level: usize,
    width: u32,
    height: u32,
    path: String,
}

#[derive(Serialize)]
struct MipManifest {
    format: &'static str,
    level_count: usize,
    one_level: bool,
    no_lod: bool,
    face: u8,
    slice: u16,
    levels: Vec<MipLevel>,
}

pub(super) fn write_mips(out_dir: &Path, entry: &str, bytes: &[u8]) -> Result<Vec<String>> {
    let info = morphic::inspect(bytes)?;
    let images = morphic::decode_stored_mips(bytes)?;
    let stem = entry
        .strip_suffix(".vtex_c")
        .context("expected VTEX entry")?;
    let mut levels = Vec::with_capacity(images.len());
    let mut paths = Vec::with_capacity(images.len() + 1);
    for (level, image) in images.into_iter().enumerate() {
        let morphic::ImageData::Rgba8(rgba) = image.data else {
            anyhow::bail!("stored mip export requires RGBA8");
        };
        let path = format!("{stem}.mip{level}.png");
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png).write_image(
            &rgba,
            image.width,
            image.height,
            image::ExtendedColorType::Rgba8,
        )?;
        super::write_file(&super::safe_entry_path(out_dir, &path)?, &png)?;
        levels.push(MipLevel {
            level,
            width: image.width,
            height: image.height,
            path: path.clone(),
        });
        paths.push(path);
    }
    let manifest = MipManifest {
        format: info.format.name(),
        level_count: levels.len(),
        one_level: levels.len() == 1,
        no_lod: info.flags.contains(morphic::TextureFlags::NO_LOD),
        face: 0,
        slice: 0,
        levels,
    };
    let path = format!("{stem}.mips.json");
    super::write_file(
        &super::safe_entry_path(out_dir, &path)?,
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    paths.push(path);
    Ok(paths)
}
