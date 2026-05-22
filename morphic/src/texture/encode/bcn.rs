//! Block-compressed encoders backed by `intel_tex_2` (Intel's ISPC texture
//! compressor). Inverse of [`crate::texture::decode::bcn`].
//!
//! All `BCn` formats operate on 4x4 pixel blocks. When the input image's
//! dimensions are below a 4x4 multiple (which happens for the small tail
//! of a regenerated mip chain: 2x2, 1x1, 6x4, etc.) we pad up to the next
//! multiple of 4 by replicating the last row/column, encode the padded
//! image, and return all the encoded block bytes. The slot the decoder
//! reads from has size `div_ceil(4) * div_ceil(4) * block_bytes`, which
//! is exactly what the padded encoder emits, so the result drops in.
//!
//! Channel layout:
//! - BC1 / BC3 / BC7: full RGBA input
//! - BC4 (Ati1n): extract red channel only
//! - BC5 (Ati2n): extract red + green channels
//! - BC6H: HDR; expects RGBA f16 input, four channels packed as 8 bytes per pixel

use intel_tex_2::{bc1, bc3, bc4, bc5, bc6h, bc7, RSurface, RgSurface, RgbaSurface};

use crate::error::EncodeError;
use crate::texture::format::TextureFormat;
use crate::texture::{Image, ImageData};

pub fn encode_bc1(image: &Image) -> Result<Vec<u8>, EncodeError> {
    let buf = require_rgba8(image, TextureFormat::Dxt1)?;
    check_rgba8_len(buf, image, TextureFormat::Dxt1)?;
    let (data, pw, ph) = pad_rgba8(buf, image.width, image.height);
    let surface = RgbaSurface {
        width: pw,
        height: ph,
        stride: pw * 4,
        data: &data,
    };
    let mut out = vec![0u8; bc1::calc_output_size(pw, ph)];
    bc1::compress_blocks_into(&surface, &mut out);
    Ok(out)
}

pub fn encode_bc3(image: &Image) -> Result<Vec<u8>, EncodeError> {
    let buf = require_rgba8(image, TextureFormat::Dxt5)?;
    check_rgba8_len(buf, image, TextureFormat::Dxt5)?;
    let (data, pw, ph) = pad_rgba8(buf, image.width, image.height);
    let surface = RgbaSurface {
        width: pw,
        height: ph,
        stride: pw * 4,
        data: &data,
    };
    let mut out = vec![0u8; bc3::calc_output_size(pw, ph)];
    bc3::compress_blocks_into(&surface, &mut out);
    Ok(out)
}

pub fn encode_bc4(image: &Image) -> Result<Vec<u8>, EncodeError> {
    let buf = require_rgba8(image, TextureFormat::Ati1n)?;
    check_rgba8_len(buf, image, TextureFormat::Ati1n)?;
    let (data, pw, ph) = pad_rgba8(buf, image.width, image.height);
    // BC4 takes one channel. Extract red.
    let pixel_count = (pw as usize) * (ph as usize);
    let mut r = Vec::with_capacity(pixel_count);
    for px in data.chunks_exact(4) {
        r.push(px[0]);
    }
    let surface = RSurface {
        width: pw,
        height: ph,
        stride: pw,
        data: &r,
    };
    let mut out = vec![0u8; bc4::calc_output_size(pw, ph)];
    bc4::compress_blocks_into(&surface, &mut out);
    Ok(out)
}

pub fn encode_bc5(image: &Image) -> Result<Vec<u8>, EncodeError> {
    let buf = require_rgba8(image, TextureFormat::Ati2n)?;
    check_rgba8_len(buf, image, TextureFormat::Ati2n)?;
    let (data, pw, ph) = pad_rgba8(buf, image.width, image.height);
    // BC5 takes two channels. Extract red + green into interleaved RG.
    let pixel_count = (pw as usize) * (ph as usize);
    let mut rg = Vec::with_capacity(pixel_count * 2);
    for px in data.chunks_exact(4) {
        rg.push(px[0]);
        rg.push(px[1]);
    }
    let surface = RgSurface {
        width: pw,
        height: ph,
        stride: pw * 2,
        data: &rg,
    };
    let mut out = vec![0u8; bc5::calc_output_size(pw, ph)];
    bc5::compress_blocks_into(&surface, &mut out);
    Ok(out)
}

pub fn encode_bc7(image: &Image) -> Result<Vec<u8>, EncodeError> {
    let buf = require_rgba8(image, TextureFormat::Bc7)?;
    check_rgba8_len(buf, image, TextureFormat::Bc7)?;
    let (data, pw, ph) = pad_rgba8(buf, image.width, image.height);
    let surface = RgbaSurface {
        width: pw,
        height: ph,
        stride: pw * 4,
        data: &data,
    };
    // alpha_basic_settings is a reasonable default: handles alpha correctly
    // for textures that have it and isn't catastrophically slow. Quality
    // knobs can be exposed later if needed.
    let settings = bc7::alpha_basic_settings();
    let mut out = vec![0u8; bc7::calc_output_size(pw, ph)];
    bc7::compress_blocks_into(&settings, &surface, &mut out);
    Ok(out)
}

pub fn encode_bc6h(image: &Image) -> Result<Vec<u8>, EncodeError> {
    let ImageData::Rgba16F(pixels) = &image.data else {
        return Err(EncodeError::WrongPixelKind {
            format: TextureFormat::Bc6h,
            reason: "expected Rgba16F pixels, got Rgba8",
        });
    };
    let expected_pixels = (image.width as usize) * (image.height as usize) * 4;
    if pixels.len() != expected_pixels {
        return Err(EncodeError::SizeMismatch {
            format: TextureFormat::Bc6h,
            width: image.width,
            height: image.height,
            expected: expected_pixels * 2,
            got: pixels.len() * 2,
        });
    }
    let (padded, pw, ph) = pad_rgba16f(pixels, image.width, image.height);
    let bytes: &[u8] = bytemuck::cast_slice(&padded);
    let surface = RgbaSurface {
        width: pw,
        height: ph,
        stride: pw * 8, // 4 channels * f16 (2 bytes)
        data: bytes,
    };
    // M7's decoder uses bcdec_rs::bc6h_half (unsigned). intel_tex_2's BC6H
    // encoder is the matching unsigned UF16 variant, so decode->encode round
    // trips with the same interpretation.
    let settings = bc6h::basic_settings();
    let mut out = vec![0u8; bc6h::calc_output_size(pw, ph)];
    bc6h::compress_blocks_into(&settings, &surface, &mut out);
    Ok(out)
}

fn require_rgba8(image: &Image, format: TextureFormat) -> Result<&[u8], EncodeError> {
    match &image.data {
        ImageData::Rgba8(buf) => Ok(buf),
        ImageData::Rgba16F(_) => Err(EncodeError::WrongPixelKind {
            format,
            reason: "expected Rgba8 pixels, got Rgba16F",
        }),
    }
}

fn check_rgba8_len(buf: &[u8], image: &Image, format: TextureFormat) -> Result<(), EncodeError> {
    let expected = (image.width as usize) * (image.height as usize) * 4;
    if buf.len() != expected {
        return Err(EncodeError::SizeMismatch {
            format,
            width: image.width,
            height: image.height,
            expected,
            got: buf.len(),
        });
    }
    Ok(())
}

/// Round up to the next multiple of 4 (`BCn` block size).
fn block_pad(n: u32) -> u32 {
    n.div_ceil(4) * 4
}

/// Replicate edge pixels to grow an RGBA8 buffer up to `BCn` block-multiple
/// dims. Returns the padded buffer and its dims. Pass-through when the
/// input is already aligned.
fn pad_rgba8(buf: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let padded_width = block_pad(width);
    let padded_height = block_pad(height);
    if padded_width == width && padded_height == height {
        return (buf.to_vec(), width, height);
    }
    let src_width = width as usize;
    let src_height = height as usize;
    let dest_width = padded_width as usize;
    let dest_height = padded_height as usize;
    let mut out = vec![0u8; dest_width * dest_height * 4];
    for y in 0..dest_height {
        let sy = y.min(src_height - 1);
        for x in 0..dest_width {
            let sx = x.min(src_width - 1);
            let src = (sy * src_width + sx) * 4;
            let dst = (y * dest_width + x) * 4;
            out[dst..dst + 4].copy_from_slice(&buf[src..src + 4]);
        }
    }
    (out, padded_width, padded_height)
}

/// Same as [`pad_rgba8`] but for RGBA f16 buffers (BC6H input).
fn pad_rgba16f(buf: &[half::f16], width: u32, height: u32) -> (Vec<half::f16>, u32, u32) {
    let padded_width = block_pad(width);
    let padded_height = block_pad(height);
    if padded_width == width && padded_height == height {
        return (buf.to_vec(), width, height);
    }
    let src_width = width as usize;
    let src_height = height as usize;
    let dest_width = padded_width as usize;
    let dest_height = padded_height as usize;
    let mut out = vec![half::f16::ZERO; dest_width * dest_height * 4];
    for y in 0..dest_height {
        let sy = y.min(src_height - 1);
        for x in 0..dest_width {
            let sx = x.min(src_width - 1);
            let src = (sy * src_width + sx) * 4;
            let dst = (y * dest_width + x) * 4;
            out[dst..dst + 4].copy_from_slice(&buf[src..src + 4]);
        }
    }
    (out, padded_width, padded_height)
}
