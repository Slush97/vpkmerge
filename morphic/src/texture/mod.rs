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
    /// Stored (on-disk) width: the pixel data is laid out at this size, which for
    /// non-power-of-two textures is padded up to the next power of two.
    pub width: u16,
    /// Stored (on-disk) height. See [`TextureInfo::width`].
    pub height: u16,
    /// Real image width at mip 0. Equals [`width`](Self::width) unless the texture
    /// carries a `FILL_TO_POWER_OF_TWO` extra-data block, in which case the stored
    /// canvas is padded and the real content occupies the top-left
    /// `actual_width x actual_height`. Display callers must crop to this; the
    /// padding region holds undefined pixels the engine never samples.
    pub actual_width: u16,
    /// Real image height at mip 0. See [`TextureInfo::actual_width`].
    pub actual_height: u16,
    pub depth: u16,
    pub mip_count: u8,
    pub flags: TextureFlags,
    /// The pixels are YCoCg-encoded (DXT5 carries Co/Cg in RGB, Y in alpha) and
    /// need an inverse transform after block decode to read as RGB. Signalled by
    /// the `Texture Compiler Version Image YCoCg Conversion` special-dependency in
    /// the resource's `RED2` edit-info block, not by anything in the DATA header,
    /// so it is populated by [`inspect`](crate::inspect) / `decode_at`, not by
    /// [`parse_texture_header`] (which sees only the DATA block and leaves it
    /// `false`). Skipping it is the classic "muddy / wrong-hue DXT5" decode.
    pub ycocg: bool,
}

impl TextureInfo {
    /// Whether the stored canvas is padded past the real image (non-power-of-two).
    #[must_use]
    pub fn is_non_pow2(&self) -> bool {
        self.actual_width != self.width || self.actual_height != self.height
    }

    /// Real (cropped) dimensions of mip level `mip`, mirroring VRF: each axis is
    /// `max(1, actual >> mip)`.
    #[must_use]
    pub fn actual_mip_dims(&self, mip: u8) -> (u32, u32) {
        (
            (u32::from(self.actual_width) >> mip).max(1),
            (u32::from(self.actual_height) >> mip).max(1),
        )
    }
}

/// VTEX extra-data block type carrying the unpadded dimensions of a
/// non-power-of-two texture (VRF `VTexExtraData.FILL_TO_POWER_OF_TWO`).
const EXTRA_DATA_FILL_TO_POWER_OF_TWO: u32 = 3;

#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub data: ImageData,
}

/// Crop a decoded mip down to its real (non-power-of-two) dimensions, keeping the
/// top-left `target_w x target_h` region. This is the display-side counterpart to
/// [`TextureInfo::actual_mip_dims`]: a decoder yields the full padded canvas, and
/// anything that *shows* the image (thumbnails, previews, portraits) must crop off
/// the undefined padding the engine never samples. A no-op when the target already
/// matches the image (the common power-of-two case) or exceeds it.
///
/// Re-encoders (recolor, icon replacement) deliberately do **not** call this: they
/// rebuild the full mip chain on the stored canvas and need the padded pixels.
#[must_use]
pub fn crop_to_actual(image: &Image, target_w: u32, target_h: u32) -> Image {
    let tw = target_w.min(image.width).max(1);
    let th = target_h.min(image.height).max(1);
    if tw == image.width && th == image.height {
        return image.clone();
    }
    let src_stride = image.width as usize;
    let out_cols = tw as usize;
    let out_rows = th as usize;
    let data = match &image.data {
        ImageData::Rgba8(px) => {
            let mut out = Vec::with_capacity(out_cols * out_rows * 4);
            for row in 0..out_rows {
                let start = row * src_stride * 4;
                out.extend_from_slice(&px[start..start + out_cols * 4]);
            }
            ImageData::Rgba8(out)
        }
        ImageData::Rgba16F(px) => {
            let mut out = Vec::with_capacity(out_cols * out_rows * 4);
            for row in 0..out_rows {
                let start = row * src_stride * 4;
                out.extend_from_slice(&px[start..start + out_cols * 4]);
            }
            ImageData::Rgba16F(out)
        }
    };
    Image {
        width: tw,
        height: th,
        data,
    }
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
    // Non-power-of-two textures pad the stored canvas up to a power of two and
    // record the real size in a FILL_TO_POWER_OF_TWO extra-data block. Default to
    // the stored dims; override if that block is present and sane.
    let (actual_width, actual_height) =
        parse_nonpow2_dims(data).map_or((width, height), |(aw, ah)| {
            // Guard against malformed blocks: the real image can only be smaller.
            if aw > 0 && ah > 0 && aw <= width && ah <= height {
                (aw, ah)
            } else {
                (width, height)
            }
        });
    Ok(TextureInfo {
        format,
        width,
        height,
        actual_width,
        actual_height,
        depth,
        mip_count,
        flags,
        // Set from the RED2 edit-info by inspect()/decode_at(); the DATA header
        // alone can't know. Default off so DATA-only callers stay correct for the
        // common non-YCoCg case.
        ycocg: false,
    })
}

/// Detect whether a resource's texture pixels are YCoCg-encoded by inspecting the
/// `RED2` edit-info block's `m_SpecialDependencies` for the `YCoCg` compiler marker.
/// `RED2` is KV3; we decode it and scan, falling back to a raw substring match if
/// the KV3 parse fails or the block is the legacy `REDI` form. Absent block or no
/// marker -> `false` (the overwhelmingly common case).
pub(crate) fn detect_ycocg(resource: &crate::resource::Resource) -> bool {
    const MARKER: &[u8] = b"YCoCg Conversion";
    for kind in [*b"RED2", *b"REDI"] {
        let Some(block) = resource.find_block(kind) else {
            continue;
        };
        // Structured path: RED2 is KV3. Look for a SpecialDependencies entry whose
        // m_String carries the YCoCg marker.
        if let Ok(value) = crate::kv3::decode(block) {
            if special_dependency_has(&value, "YCoCg Conversion") {
                return true;
            }
        }
        // Fallback: the marker string is specific enough that a raw scan of the
        // edit-info block is a safe backstop (legacy REDI, or a KV3 parse miss).
        if block.windows(MARKER.len()).any(|w| w == MARKER) {
            return true;
        }
    }
    false
}

/// Whether any `m_SpecialDependencies[*].m_String` contains `needle`.
fn special_dependency_has(value: &crate::kv3::Value, needle: &str) -> bool {
    use crate::kv3::Value;
    let Value::Object(root) = value else {
        return false;
    };
    let Some(Value::Array(deps)) = root
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("m_SpecialDependencies"))
        .map(|(_, v)| v)
    else {
        return false;
    };
    deps.iter().any(|dep| {
        let Value::Object(fields) = dep else {
            return false;
        };
        fields.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("m_String")
                && matches!(v, Value::String(s) if s.contains(needle))
        })
    })
}

/// Apply the inverse scaled-YCoCg transform in place on an RGBA8 image, matching
/// VRF (the van Waveren "YCoCg-DXT5" packing the Source 2 texture compiler emits).
///
/// After block decode the channels hold `R=Co`, `G=Cg`, `B=scale`, `A=Y`, where
/// the per-block blue scale maximises chroma precision. Reconstruct:
/// `scale = (B>>3)+1`, `co = (R-128)/scale`, `cg = (G-128)/scale`, then
/// `R = Y+co-cg`, `G = Y+cg`, `B = Y-co-cg`, `A = 255`. Integer division
/// truncates toward zero, matching VRF's C#. A no-op for non-RGBA8 data.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn apply_ycocg(image: &mut Image) {
    let ImageData::Rgba8(px) = &mut image.data else {
        return;
    };
    for p in px.chunks_exact_mut(4) {
        let scale = (i32::from(p[2]) >> 3) + 1;
        let co = (i32::from(p[0]) - 128) / scale;
        let cg = (i32::from(p[1]) - 128) / scale;
        let y = i32::from(p[3]);
        p[0] = (y + co - cg).clamp(0, 255) as u8;
        p[1] = (y + cg).clamp(0, 255) as u8;
        p[2] = (y - co - cg).clamp(0, 255) as u8;
        p[3] = 255;
    }
}

/// Forward scaled-YCoCg transform: the inverse of [`apply_ycocg`], applied in
/// place to an RGBA8 image so its bytes can be DXT5-encoded and stored in a
/// texture that declares the `YCoCg Conversion` special-dependency. WITHOUT this
/// step, replacing a `YCoCg` texture (icon/item-art replace, or a recolor re-encode)
/// stores plain RGB under a `YCoCg` flag, so the engine's inverse transform garbles
/// the colors. This is the fix for that icon-replace bug.
///
/// We pack with per-block scale = 1 (stored blue = 0, since the decoder reads
/// `scale = (B>>3)+1`). Scale 1 makes this a near-exact integer inverse of the
/// decode: solving `R = Y+co-cg`, `G = Y+cg`, `B = Y-co-cg` for `(Y, co, cg)`
/// gives `Y = (R+2G+B)/4`, `co = (R-B)/2`, `cg = (2G-R-B)/4`, which round-trips
/// back to the original RGB within ~2 LSB of integer rounding, far below DXT5
/// quantization. The trade-off vs. an optimal per-4x4-block scale is only chroma
/// precision (we don't amplify Co/Cg into the full 0..255 range), never
/// correctness. A per-block-scale quality pass can come later. No-op for
/// non-RGBA8 data. Store order matches the decode: Co->R, Cg->G, scale->B, Y->A.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn encode_ycocg(image: &mut Image) {
    let ImageData::Rgba8(px) = &mut image.data else {
        return;
    };
    for p in px.chunks_exact_mut(4) {
        let r = i32::from(p[0]);
        let g = i32::from(p[1]);
        let b = i32::from(p[2]);
        let y = (r + 2 * g + b + 2) / 4; // luma, round to nearest
        let co = (r - b) / 2; // chroma orange (trunc toward zero, <=1 LSB)
        let cg = (2 * g - r - b + 2) / 4; // chroma green, round to nearest
        p[0] = (co + 128).clamp(0, 255) as u8; // Co    -> DXT5 color R
        p[1] = (cg + 128).clamp(0, 255) as u8; // Cg    -> DXT5 color G
        p[2] = 0; // scale byte 0 => decoder scale = 1
        p[3] = y.clamp(0, 255) as u8; // Y     -> DXT5 alpha
    }
}

/// Scan the VTEX extra-data table for a `FILL_TO_POWER_OF_TWO` block and return
/// its `(width, height)`. Returns `None` when absent (the common, pow2 case) or
/// when the table runs past the DATA block (treated as not present rather than an
/// error: callers fall back to the stored dims).
///
/// Layout, all little-endian, relative to the texture header start:
/// `u32 extra_data_offset @32`, `u32 extra_data_count @36`. The entry table
/// begins at `32 + extra_data_offset`; each 12-byte entry is `u32 type`,
/// `u32 rel_offset`, `u32 size`, and its payload sits at
/// `entry_start + 4 + rel_offset` (matching VRF's `offset - 8` quirk). The
/// FILL payload is `u16 _unused, u16 width, u16 height`.
fn parse_nonpow2_dims(data: &[u8]) -> Option<(u16, u16)> {
    let read_u32 = |o: usize| -> Option<u32> {
        data.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let read_u16 =
        |o: usize| -> Option<u16> { data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])) };
    let extra_offset = read_u32(32)? as usize;
    let extra_count = read_u32(36)? as usize;
    let table_base = 32usize.checked_add(extra_offset)?;
    for i in 0..extra_count {
        let entry = table_base.checked_add(i.checked_mul(12)?)?;
        let ty = read_u32(entry)?;
        if ty != EXTRA_DATA_FILL_TO_POWER_OF_TWO {
            continue;
        }
        let rel = read_u32(entry + 4)? as usize;
        let payload = entry.checked_add(4)?.checked_add(rel)?;
        // payload: [u16 unused][u16 width][u16 height]
        let w = read_u16(payload + 2)?;
        let h = read_u16(payload + 4)?;
        return Some((w, h));
    }
    None
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

#[cfg(test)]
mod ycocg_tests {
    use super::{apply_ycocg, encode_ycocg, Image, ImageData};

    /// `encode_ycocg` then `apply_ycocg` must return the original RGB within a few
    /// LSB (integer rounding only; no DXT5 here). This is the correctness gate
    /// for the icon/recolor `YCoCg` fix: a `YCoCg`-flagged texture stores these
    /// packed bytes and the engine runs `apply_ycocg` on load.
    #[test]
    #[allow(clippy::cast_possible_truncation)] // tiny fixed test image; len fits u32
    fn ycocg_forward_inverse_roundtrips() {
        // A spread of colors: primaries, greys, saturated, dark, light.
        let mut px: Vec<u8> = Vec::new();
        for &(r, g, b) in &[
            (0u8, 0u8, 0u8),
            (255, 255, 255),
            (128, 128, 128),
            (255, 0, 0),
            (0, 255, 0),
            (0, 0, 255),
            (200, 50, 100),
            (17, 240, 33),
            (90, 90, 200),
            (240, 200, 10),
        ] {
            px.extend_from_slice(&[r, g, b, 255]);
        }
        let original = px.clone();

        let mut img = Image {
            width: (px.len() / 4) as u32,
            height: 1,
            data: ImageData::Rgba8(px),
        };
        encode_ycocg(&mut img); // RGB -> packed YCoCg (scale=1, blue=0)
                                // Scale byte must be 0 so the decoder recovers scale = (0>>3)+1 = 1.
        if let ImageData::Rgba8(packed) = &img.data {
            for p in packed.chunks_exact(4) {
                assert_eq!(p[2], 0, "scale byte must be 0 for scale=1 packing");
            }
        }
        apply_ycocg(&mut img); // packed YCoCg -> RGB (what the engine does)

        let ImageData::Rgba8(result) = &img.data else {
            panic!("expected rgba8");
        };
        for (i, (o, r)) in original
            .chunks_exact(4)
            .zip(result.chunks_exact(4))
            .enumerate()
        {
            for c in 0..3 {
                let diff = i32::from(o[c]) - i32::from(r[c]);
                assert!(
                    diff.abs() <= 3,
                    "channel {c} of pixel {i}: got {} want {} (diff {diff})",
                    r[c],
                    o[c],
                );
            }
        }
    }
}
