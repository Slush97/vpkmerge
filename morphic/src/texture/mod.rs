//! Texture-specific decode: composes [`crate::resource`] + per-format pixel
//! decoders under [`decode`].
//!
//! The texture DATA block uses a *binary* header (not KV3); layout per VRF's
//! `Texture.Read()`:
//!
//! ```text
//! u16   version            // must be 1
//! u16   flags (VTexFlags)
//! f32   reflectivity.x
//! f32   reflectivity.y
//! f32   reflectivity.z
//! f32   reflectivity.w
//! u16   width
//! u16   height
//! u16   depth
//! u8    format (VTexFormat)
//! u8    num_mip_levels
//! u32   picmip0_res
//! u32   extra_data_offset
//! u32   extra_data_count
//! [optional extra-data blocks at extra_data_offset]
//! ```
//!
//! Pixel data is stored *outside* the DATA block, starting at
//! `data_block.offset + data_block.size`. Mips are written smallest to
//! largest, so mip0 sits at the *end* of the resource.

pub mod decode;
pub mod encode;
pub mod format;
mod vtex;

use byteorder::{ByteOrder, LittleEndian};

use crate::error::DecodeError;
use crate::resource::Resource;
use format::{TextureFlags, TextureFormat};
pub use vtex::{encode_vtex_png_rgba8888, encode_vtex_png_rgba8888_from_png};

const TEXTURE_VERSION: u16 = 1;
const TEXTURE_HEADER_SIZE: usize = 40;

#[derive(Debug, Clone, Copy)]
pub struct TextureInfo {
    pub format: TextureFormat,
    pub width: u16,
    pub height: u16,
    pub depth: u16,
    pub mip_count: u8,
    pub flags: TextureFlags,
}

#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: ImageData,
}

#[derive(Debug, Clone)]
pub enum ImageData {
    /// 4 bytes per pixel, row-major, top-left origin.
    Rgba8(Vec<u8>),
    /// 4 half-floats per pixel, row-major, top-left origin.
    Rgba16F(Vec<half::f16>),
}

/// Parse the texture binary header out of a DATA block.
pub fn parse_texture_header(data: &[u8]) -> Result<TextureInfo, DecodeError> {
    if data.len() < TEXTURE_HEADER_SIZE {
        return Err(DecodeError::Truncated {
            offset: 0,
            needed: TEXTURE_HEADER_SIZE,
            had: data.len(),
        });
    }
    let version = LittleEndian::read_u16(&data[0..2]);
    if version != TEXTURE_VERSION {
        return Err(DecodeError::BadResource("texture version != 1"));
    }
    let flags_raw = LittleEndian::read_u16(&data[2..4]);
    let flags = TextureFlags::from_bits_truncate(flags_raw);
    // 4 floats of reflectivity at offset 4..20 (16 bytes), ignored for header.
    let width = LittleEndian::read_u16(&data[20..22]);
    let height = LittleEndian::read_u16(&data[22..24]);
    let depth = LittleEndian::read_u16(&data[24..26]);
    let format_id = data[26];
    let mip_count = data[27];
    let format = format_from_id(format_id)?;
    Ok(TextureInfo {
        format,
        width,
        height,
        depth,
        mip_count,
        flags,
    })
}

/// Map VRF's `VTexFormat` numeric id to our enum.
/// Source: `ValveResourceFormat/Resource/Enums/VTexFormat.cs`.
fn format_from_id(id: u8) -> Result<TextureFormat, DecodeError> {
    Ok(match id {
        0 | 3 | 22 => TextureFormat::Unknown, // UNKNOWN, I8, IA88 (not seen)
        1 => TextureFormat::Dxt1,
        2 => TextureFormat::Dxt5,
        4 => TextureFormat::Rgba8888,
        // 5..9: R16, RG1616, RGBA16161616, R16F, RG1616F
        10 => TextureFormat::Rgba16161616F,
        // 11..14: R32F, RG3232F, RGB323232F, RGBA32323232F
        // 15: JPEG_RGBA8888 (not seen in Deadlock)
        16 => TextureFormat::PngRgba8888,
        17 => TextureFormat::JpegDxt5,
        18 => TextureFormat::PngDxt5,
        19 => TextureFormat::Bc6h,
        20 => TextureFormat::Bc7,
        21 => TextureFormat::Ati2n,
        // 23..26: ETC2, ETC2_EAC, R11_EAC, RG11_EAC (not seen)
        27 => TextureFormat::Ati1n,
        28 => TextureFormat::Bgra8888,
        // 29..30: WEBP_RGBA8888, WEBP_DXT5 (not seen)
        other => return Err(DecodeError::UnknownFormat(i32::from(other))),
    })
}

/// Locate the pixel bytes for the requested face of the requested mip.
///
/// Thin convenience wrapper over [`face_mip_byte_range`]: slices the
/// resource's raw bytes at the returned range.
pub fn pixel_data<'a>(
    resource: &Resource<'a>,
    info: &TextureInfo,
    opts: crate::DecodeOptions,
) -> Result<&'a [u8], DecodeError> {
    let range = face_mip_byte_range(resource, info, opts)?;
    Ok(&resource.raw()[range])
}

/// Compute the absolute byte range (within the original resource) of one
/// face of one mip.
///
/// Source 2 stores mips smallest-first, so mip 0 is at the *end* of the
/// pixel-data region that lives just past the DATA block. Cubemaps split
/// each mip into 6 contiguous faces in `[+X, -X, +Y, -Y, +Z, -Z]` order;
/// non-cubemap textures have a single face. For inline PNG/JPEG/WebP
/// formats there is no mip chain at all; the payload is a literal
/// compressed image and the full remainder is returned.
///
/// The splice path in [`crate::encode`] uses the same arithmetic; sharing
/// this helper keeps decode and edit in lockstep on offsets.
pub fn face_mip_byte_range(
    resource: &Resource<'_>,
    info: &TextureInfo,
    opts: crate::DecodeOptions,
) -> Result<core::ops::Range<usize>, DecodeError> {
    let data_block = resource.data_block_meta()?;
    let start = data_block
        .offset
        .checked_add(data_block.size)
        .ok_or(DecodeError::BadResource("pixel offset overflow"))? as usize;
    let raw = resource.raw();
    if start > raw.len() {
        return Err(DecodeError::Truncated {
            offset: start as u64,
            needed: 0,
            had: raw.len(),
        });
    }
    let all_len = raw.len() - start;
    if is_inline_format(info.format) {
        // Inline payloads don't have faces / mips. Reject anything but the
        // default target so callers get a clear error.
        if opts.face != 0 || opts.slice != 0 || opts.mip != 0 {
            return Err(DecodeError::InvalidTarget {
                mip: opts.mip,
                slice: opts.slice,
                face: opts.face,
            });
        }
        return Ok(start..start + all_len);
    }
    let is_cube = info.flags.contains(TextureFlags::CUBE_TEXTURE);
    let face_count: usize = if is_cube { 6 } else { 1 };
    if usize::from(opts.face) >= face_count || opts.mip >= info.mip_count {
        return Err(DecodeError::InvalidTarget {
            mip: opts.mip,
            slice: opts.slice,
            face: opts.face,
        });
    }
    // Mips are stored smallest-first, so mip 0 sits at the very end and mips
    // with smaller index (larger dims) live after the target mip in the file.
    // To find mip M's start from the end of the pixel-data region, skip past
    // mips 0..M-1 (each contributing face_count faces).
    let mut after_target = 0usize;
    for i in 0..opts.mip {
        let (mw, mh) = mip_dims(info.width, info.height, i);
        let face_size_i = face_size_bytes(info.format, mw, mh)?;
        let mip_total = face_size_i
            .checked_mul(face_count)
            .ok_or(DecodeError::BadResource("mip total overflow"))?;
        after_target = after_target
            .checked_add(mip_total)
            .ok_or(DecodeError::BadResource("pixel offset overflow"))?;
    }
    let (tw, th) = mip_dims(info.width, info.height, opts.mip);
    let target_face_size = face_size_bytes(info.format, tw, th)?;
    let target_mip_total = target_face_size
        .checked_mul(face_count)
        .ok_or(DecodeError::BadResource("mip total overflow"))?;
    let needed = after_target
        .checked_add(target_mip_total)
        .ok_or(DecodeError::BadResource("pixel offset overflow"))?;
    if needed > all_len {
        return Err(DecodeError::Truncated {
            offset: start as u64,
            needed,
            had: all_len,
        });
    }
    let target_end = (start + all_len) - after_target;
    let target_mip_start = target_end - target_mip_total;
    let face_start = target_mip_start + usize::from(opts.face) * target_face_size;
    Ok(face_start..face_start + target_face_size)
}

/// Dimensions of a given mip level. Each successive mip halves both
/// dimensions, never dropping below 1.
#[must_use]
pub fn mip_dims(width: u16, height: u16, mip: u8) -> (u16, u16) {
    let shift = u32::from(mip);
    let w = (u32::from(width) >> shift).max(1);
    let h = (u32::from(height) >> shift).max(1);
    // shift up to 16 of a u16: result still fits in u16 (worst case 1).
    #[allow(clippy::cast_possible_truncation)]
    let (w, h) = (w as u16, h as u16);
    (w, h)
}

fn is_inline_format(fmt: TextureFormat) -> bool {
    matches!(
        fmt,
        TextureFormat::PngRgba8888 | TextureFormat::PngDxt5 | TextureFormat::JpegDxt5
    )
}

/// Bytes occupied by one face of one mip at the given dimensions.
pub(crate) fn face_size_bytes(
    fmt: TextureFormat,
    width: u16,
    height: u16,
) -> Result<usize, DecodeError> {
    let w = usize::from(width);
    let h = usize::from(height);
    if let Some(bytes_per_pixel) = uncompressed_bytes_per_pixel(fmt) {
        Ok(w * h * bytes_per_pixel)
    } else {
        let blocks = w.div_ceil(4) * h.div_ceil(4);
        Ok(blocks * block_bytes_per_format(fmt)?)
    }
}

fn block_bytes_per_format(fmt: TextureFormat) -> Result<usize, DecodeError> {
    Ok(match fmt {
        // Uncompressed: size computed via bytes-per-pixel instead; sentinel 0.
        TextureFormat::Rgba8888 | TextureFormat::Bgra8888 | TextureFormat::Rgba16161616F => 0,
        TextureFormat::Dxt1 | TextureFormat::Ati1n => 8,
        TextureFormat::Dxt5 | TextureFormat::Ati2n | TextureFormat::Bc6h | TextureFormat::Bc7 => 16,
        // Inline-encoded formats are handled in pixel_data via is_inline_format;
        // this branch should not be reached for them. Unknown means we couldn't
        // map the format id.
        TextureFormat::PngRgba8888
        | TextureFormat::PngDxt5
        | TextureFormat::JpegDxt5
        | TextureFormat::Unknown => return Err(DecodeError::Unimplemented(fmt)),
    })
}

fn uncompressed_bytes_per_pixel(fmt: TextureFormat) -> Option<usize> {
    match fmt {
        TextureFormat::Rgba8888 | TextureFormat::Bgra8888 => Some(4),
        TextureFormat::Rgba16161616F => Some(8),
        _ => None,
    }
}
