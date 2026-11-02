//! Stored mip export, without display conversions or generated levels.

use std::borrow::Cow;

use super::{crop_to_actual, decode, face_size_bytes, is_inline_format, mip_dims};
use super::{Image, ImageData, TextureFlags, TextureFormat};
use crate::{resource::Resource, DecodeError, DecodeOptions};

fn read_u32(data: &[u8], offset: usize) -> Result<u32, DecodeError> {
    let bytes = data
        .get(offset..offset.saturating_add(4))
        .ok_or(DecodeError::BadResource("truncated texture extra data"))?;
    Ok(u32::from_le_bytes(bytes.try_into().expect("four bytes")))
}

fn compressed_mips(data: &[u8], count: u8) -> Result<Option<(u32, Vec<usize>)>, DecodeError> {
    let table = 32usize
        .checked_add(read_u32(data, 32)? as usize)
        .ok_or(DecodeError::BadResource("texture extra offset overflow"))?;
    let entries = read_u32(data, 36)? as usize;
    if entries > data.len() / 12 {
        return Err(DecodeError::BadResource("invalid texture extra count"));
    }
    for i in 0..entries {
        let entry = table
            .checked_add(i * 12)
            .ok_or(DecodeError::BadResource("texture extra offset overflow"))?;
        if read_u32(data, entry)? != 4 {
            continue;
        }
        let payload = entry
            .checked_add(4)
            .and_then(|v| v.checked_add(read_u32(data, entry + 4).ok()? as usize))
            .ok_or(DecodeError::BadResource("invalid compressed mip offset"))?;
        let end = payload
            .checked_add(read_u32(data, entry + 8)? as usize)
            .ok_or(DecodeError::BadResource("compressed mip size overflow"))?;
        let block = data
            .get(payload..end)
            .ok_or(DecodeError::BadResource("truncated compressed mip block"))?;
        let method = read_u32(block, 0)?;
        if method > 2 {
            return Err(DecodeError::BadResource("unknown mip compression method"));
        }
        if read_u32(block, 8)? != u32::from(count) {
            return Err(DecodeError::BadResource(
                "compressed mip count differs from header",
            ));
        }
        let sizes = 4usize
            .checked_add(read_u32(block, 4)? as usize)
            .ok_or(DecodeError::BadResource("mip size table offset overflow"))?;
        let sizes = (0..usize::from(count))
            .map(|mip| {
                let size = read_u32(block, sizes + mip * 4)?;
                if size == 0 || size > i32::MAX as u32 {
                    return Err(DecodeError::BadResource("invalid compressed mip size"));
                }
                Ok(size as usize)
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Some((method, sizes)));
    }
    Ok(None)
}

/// Decode exactly the stored mip count, largest first, as unpremultiplied RGBA8.
/// Reads face 0 / slice 0, like [`crate::decode`]. No gamma, `YCoCg`, normal-map,
/// or other display conversions are applied. Crops each level to VRF's display
/// rectangle. HDR formats are rejected rather than tone-mapped to RGBA8.
#[allow(clippy::too_many_lines)]
pub fn decode_stored_mips(bytes: &[u8]) -> Result<Vec<Image>, DecodeError> {
    let resource = Resource::parse(bytes)?;
    let data = resource.data_block()?;
    let mut info = super::parse_texture_header(data)?;
    if info.width == 0
        || info.height == 0
        || info.depth == 0
        || info.mip_count == 0
        || info.mip_count > 16
    {
        return Err(DecodeError::BadResource(
            "invalid texture dimensions or mip count",
        ));
    }
    // VRF ignores placeholder 1x1 display rectangles except on a 4x4 canvas.
    if info.actual_width == 1 && info.width != 4 {
        info.actual_width = info.width;
    }
    if info.actual_height == 1 && info.height != 4 {
        info.actual_height = info.height;
    }
    if matches!(
        info.format,
        TextureFormat::Bc6h | TextureFormat::Rgba16161616F
    ) {
        return Err(DecodeError::Unimplemented(info.format));
    }
    let block = resource.data_block_meta()?;
    let mut offset = block.offset as usize + block.size as usize;
    if is_inline_format(info.format) {
        if info.mip_count != 1 {
            return Err(DecodeError::BadResource(
                "inline image has multiple mip levels",
            ));
        }
        let pixels = bytes
            .get(offset..)
            .ok_or(DecodeError::BadResource("missing inline image"))?;
        let image = image::load_from_memory(pixels)
            .map_err(|e| DecodeError::InlineImage(e.to_string()))?
            .to_rgba8();
        return Ok(vec![Image {
            width: image.width(),
            height: image.height(),
            data: ImageData::Rgba8(image.into_raw()),
        }]);
    }
    let compressed = compressed_mips(data, info.mip_count)?;
    let mut levels = Vec::with_capacity(usize::from(info.mip_count));
    for mip in (0..info.mip_count).rev() {
        let (width, height) = mip_dims(info.width, info.height, mip);
        let face_size = face_size_bytes(info.format, width, height)?;
        let depth = if info.flags.contains(TextureFlags::VOLUME_TEXTURE) {
            (usize::from(info.depth) >> mip).max(1)
        } else {
            usize::from(info.depth)
        };
        let faces = if info.flags.contains(TextureFlags::CUBE_TEXTURE) {
            6
        } else {
            1
        };
        let mut layers = depth * faces;
        if !matches!(
            info.format,
            TextureFormat::Rgba8888 | TextureFormat::Bgra8888
        ) && (2..4).contains(&layers)
        {
            layers = 4;
        }
        let size = face_size
            .checked_mul(layers)
            .ok_or(DecodeError::BadResource("mip size overflow"))?;
        let stored_size = compressed
            .as_ref()
            .map_or(size, |(_, sizes)| sizes[usize::from(mip)].min(size));
        let end = offset
            .checked_add(stored_size)
            .ok_or(DecodeError::BadResource("mip offset overflow"))?;
        let stored = bytes.get(offset..end).ok_or(DecodeError::Truncated {
            offset: offset as u64,
            needed: stored_size,
            had: bytes.len().saturating_sub(offset),
        })?;
        let pixels = if let Some((method, _)) = &compressed {
            if *method != 0 && stored_size < size {
                if *method == 2 {
                    return Err(DecodeError::BadResource(
                        "Zstd texture mip compression is not supported",
                    ));
                }
                let decoded = lz4_flex::block::decompress(stored, size)
                    .map_err(|_| DecodeError::BadResource("invalid LZ4 texture mip"))?;
                if decoded.len() != size {
                    return Err(DecodeError::BadResource("LZ4 texture mip size mismatch"));
                }
                Cow::Owned(decoded)
            } else {
                Cow::Borrowed(stored)
            }
        } else {
            Cow::Borrowed(stored)
        };
        let mut image = if info.format == TextureFormat::Dxt5 {
            let mip_info = super::TextureInfo {
                width,
                height,
                ..info
            };
            decode::decode_bc3_stored(&mip_info, &pixels)?
        } else {
            decode::decode_image(
                &info,
                &pixels,
                &DecodeOptions {
                    mip,
                    ..DecodeOptions::default()
                },
            )?
        };
        if info.format == TextureFormat::Dxt1 {
            if let ImageData::Rgba8(rgba) = &mut image.data {
                // VRF decodes BC1 without punch-through alpha.
                for pixel in rgba.as_chunks_mut::<4>().0 {
                    pixel[3] = 255;
                }
            }
        }
        let (aw, ah) = info.actual_mip_dims(mip);
        levels.push(crop_to_actual(&image, aw, ah));
        offset = end;
    }
    levels.reverse();
    Ok(levels)
}
