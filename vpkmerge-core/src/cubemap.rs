//! Export a Source 2 cubemap texture (`.vtex_c`) to six Radiance `.hdr` faces.
//!
//! Built for the grimoire mod manager's three.js model viewer: its
//! image-based lighting wants a real Deadlock probe (the BC6H cube textures
//! under `materials/skybox/`, e.g. `sky_dl_dusk_ibl_exr_3dabb6cd.vtex_c`)
//! instead of a synthetic environment. This is a decode-only path: nothing is
//! re-encoded or packed, the six faces just land as loose `.hdr` files the
//! viewer can load with `CubeTextureLoader` / `HDRCubeTextureLoader`.
//!
//! Face order follows morphic's cubemap storage, `[+X, -X, +Y, -Y, +Z, -Z]`
//! (see `morphic::texture::face_mip_byte_range`), written as
//! `px/nx/py/ny/pz/nz.hdr`, which is also the order three.js expects.
//!
//! The `.hdr` files are Radiance RGBE with flat (non-RLE) scanlines: every
//! reader accepts the flat encoding, and a 256x256 probe face is small enough
//! that the RLE saving is not worth the writer complexity.

use anyhow::{bail, Context, Result};
use morphic::{DecodeOptions, Image, ImageData, TextureFlags};
use std::path::Path;

/// Output file stems in morphic's cubemap face order `[+X, -X, +Y, -Y, +Z, -Z]`.
pub const CUBEMAP_FACE_NAMES: [&str; 6] = ["px", "nx", "py", "ny", "pz", "nz"];

/// Per-face summary returned by [`export_cubemap_hdr`], so a caller can print
/// an orientation sanity table (the `+Y`/`py` face should be the sky).
#[derive(Debug, Clone)]
pub struct CubemapFaceReport {
    /// Face file stem (`px`, `nx`, `py`, `ny`, `pz`, `nz`).
    pub face: &'static str,
    pub width: u32,
    pub height: u32,
    /// Mean Rec. 709 luminance of the face's linear pixels.
    pub mean_luminance: f64,
}

/// Decode a cubemap `.vtex_c` at mip 0 and write its six faces as Radiance
/// `.hdr` files (`px/nx/py/ny/pz/nz.hdr`) into `out_dir` (created if missing).
///
/// `input` is a loose file path, or (when `from_vpk` is given) an entry path
/// inside that VPK, same convention as the `texture` command. The texture must
/// carry the `CUBE_TEXTURE` flag; a 2D texture errors out rather than writing
/// a single mislabeled face.
///
/// Pixels are written as linear light: an f16 source (BC6H HDR) is already
/// linear and passes through, while an 8-bit source is treated as sRGB and
/// linearized first (Radiance HDR stores linear values).
pub fn export_cubemap_hdr(
    input: &Path,
    from_vpk: Option<&Path>,
    out_dir: &Path,
) -> Result<Vec<CubemapFaceReport>> {
    let bytes = match from_vpk {
        Some(vpk) => crate::read_vpk_entry(vpk, &input.to_string_lossy())?,
        None => std::fs::read(input).with_context(|| format!("reading {}", input.display()))?,
    };

    let info = morphic::inspect(&bytes)
        .with_context(|| format!("{} is not a readable .vtex_c", input.display()))?;
    if !info.flags.contains(TextureFlags::CUBE_TEXTURE) {
        bail!(
            "{} is not a cubemap: {:?} {}x{} lacks the CUBE_TEXTURE flag (flags {:?}); \
             expected a cube texture like the materials/skybox/ IBL probes",
            input.display(),
            info.format,
            info.width,
            info.height,
            info.flags,
        );
    }

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("creating output directory {}", out_dir.display()))?;

    let mut reports = Vec::with_capacity(CUBEMAP_FACE_NAMES.len());
    for (face, name) in (0u8..).zip(CUBEMAP_FACE_NAMES) {
        let image = morphic::decode_at(
            &bytes,
            &DecodeOptions {
                mip: 0,
                slice: 0,
                face,
            },
        )
        .with_context(|| format!("decoding cubemap face {face} ({name})"))?;

        let pixels = linear_rgb_pixels(&image);
        let hdr = encode_radiance_hdr(image.width, image.height, &pixels);
        let out_path = out_dir.join(format!("{name}.hdr"));
        std::fs::write(&out_path, &hdr)
            .with_context(|| format!("writing {}", out_path.display()))?;

        reports.push(CubemapFaceReport {
            face: name,
            width: image.width,
            height: image.height,
            mean_luminance: mean_luminance(&pixels),
        });
    }
    Ok(reports)
}

/// Flatten an image to linear-light RGB triples, dropping alpha.
///
/// f16 pixels (BC6H and friends) are already linear light. 8-bit pixels are
/// treated as sRGB-encoded and converted to linear, since Radiance HDR stores
/// linear values.
fn linear_rgb_pixels(image: &Image) -> Vec<[f32; 3]> {
    match &image.data {
        ImageData::Rgba16F(px) => px
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [p[0].to_f32(), p[1].to_f32(), p[2].to_f32()])
            .collect(),
        ImageData::Rgba8(px) => px
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                [
                    srgb_to_linear(p[0]),
                    srgb_to_linear(p[1]),
                    srgb_to_linear(p[2]),
                ]
            })
            .collect(),
    }
}

/// One sRGB-encoded byte to linear light (IEC 61966-2-1).
fn srgb_to_linear(byte: u8) -> f32 {
    let c = f32::from(byte) / 255.0;
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Mean Rec. 709 luminance over linear pixels (accumulated in f64).
fn mean_luminance(pixels: &[[f32; 3]]) -> f64 {
    if pixels.is_empty() {
        return 0.0;
    }
    let sum: f64 = pixels
        .iter()
        .map(|p| f64::from(0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]))
        .sum();
    #[allow(clippy::cast_precision_loss)]
    let count = pixels.len() as f64;
    sum / count
}

/// Serialize a Radiance `.hdr` image: text header, then flat (non-RLE)
/// scanlines of 4-byte RGBE pixels, row-major from the top-left (the header's
/// `-Y h +X w` resolution string declares exactly that order).
fn encode_radiance_hdr(width: u32, height: u32, pixels: &[[f32; 3]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + pixels.len() * 4);
    out.extend_from_slice(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n");
    out.extend_from_slice(format!("-Y {height} +X {width}\n").as_bytes());
    for p in pixels {
        out.extend_from_slice(&float_to_rgbe(*p));
    }
    out
}

/// Encode one linear RGB pixel as Radiance RGBE (shared exponent).
///
/// Picks the exponent `e` so the max component `m` scaled by `256 / 2^e`
/// lands in `[128, 256)` (the classic `frexp` convention, fraction in
/// `[0.5, 1.0)`), then stores each channel as `round(c * 256 / 2^e)` with the
/// exponent byte biased by 128. std has no `frexp`, so `e` comes from
/// `m.log2().floor() + 1`; at an exact power of two that yields fraction 0.5
/// (byte 128), and a correction step guards against `log2` rounding pushing
/// the max channel out of `[128, 256)` either way. Non-finite or negative
/// components clamp to zero; a pixel whose max component is `<= 1e-32` is the
/// special all-zero RGBE.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::many_single_char_names
)]
fn float_to_rgbe(rgb: [f32; 3]) -> [u8; 4] {
    let sanitize = |v: f32| if v.is_finite() && v > 0.0 { v } else { 0.0 };
    let r = sanitize(rgb[0]);
    let g = sanitize(rgb[1]);
    let b = sanitize(rgb[2]);
    let m = r.max(g).max(b);
    if m <= 1e-32 {
        return [0, 0, 0, 0];
    }

    let mut e = (m.log2().floor() as i32) + 1;
    let mut scale = ((8 - e) as f32).exp2();
    // log2 rounding guard: keep the max channel in [128, 256).
    if m * scale >= 256.0 {
        e += 1;
        scale *= 0.5;
    } else if m * scale < 128.0 {
        e -= 1;
        scale *= 2.0;
    }
    // Saturate an exponent the biased byte cannot hold (m near f32::MAX);
    // the channel clamp below then pegs the pixel at the encodable maximum.
    if e > 127 {
        e = 127;
        scale = ((8 - e) as f32).exp2();
    }

    let channel = |v: f32| (v * scale).round().min(255.0) as u8;
    [channel(r), channel(g), channel(b), (e + 128) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Standard RGBE inverse: `v = byte * 2^(e_byte - 136)` (bias 128 plus the
    /// 8-bit mantissa shift). The all-zero exponent byte means a zero pixel.
    fn rgbe_to_float(rgbe: [u8; 4]) -> [f32; 3] {
        if rgbe[3] == 0 {
            return [0.0, 0.0, 0.0];
        }
        let scale = (f32::from(rgbe[3]) - 136.0).exp2();
        [
            f32::from(rgbe[0]) * scale,
            f32::from(rgbe[1]) * scale,
            f32::from(rgbe[2]) * scale,
        ]
    }

    fn assert_round_trips(pixel: [f32; 3]) {
        let decoded = rgbe_to_float(float_to_rgbe(pixel));
        let max = pixel[0].max(pixel[1]).max(pixel[2]);
        for c in 0..3 {
            let err = (decoded[c] - pixel[c]).abs();
            if max <= 1e-32 {
                assert!(err == 0.0, "zero pixel must decode to exact zero");
            } else {
                assert!(
                    err / max < 0.5 / 256.0,
                    "channel {c} of {pixel:?} decoded to {decoded:?} (err {err})"
                );
            }
        }
    }

    #[test]
    fn rgbe_round_trips_known_values() {
        // Zero is the special all-zero RGBE.
        assert_eq!(float_to_rgbe([0.0, 0.0, 0.0]), [0, 0, 0, 0]);
        assert_round_trips([0.0, 0.0, 0.0]);

        // Exact powers of two: the fraction sits right at 0.5 and must not
        // fall out of the byte range in either direction.
        assert_round_trips([1.0, 1.0, 1.0]);
        assert_round_trips([0.5, 0.25, 0.125]);
        assert_round_trips([2.0, 1.0, 0.5]);

        // Tiny and large magnitudes share one exponent across channels.
        assert_round_trips([1e-10, 5e-11, 2.5e-11]);
        assert_round_trips([100.0, 61.4, 7.0]);
    }

    #[test]
    fn rgbe_powers_of_two_encode_exactly() {
        // 1.0 must encode as fraction 0.5 (byte 128, exponent byte 129), not
        // saturate at 255 via a fraction of 1.0.
        assert_eq!(float_to_rgbe([1.0, 1.0, 1.0]), [128, 128, 128, 129]);
        let decoded = rgbe_to_float([128, 128, 128, 129]);
        assert!((decoded[0] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn writer_emits_parseable_header_and_flat_scanlines() {
        let pixels = vec![
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.5, 0.25, 0.125],
            [100.0, 61.4, 7.0],
        ];
        let bytes = encode_radiance_hdr(2, 2, &pixels);

        let header = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 2\n";
        assert!(
            bytes.starts_with(header),
            "header mismatch: {:?}",
            String::from_utf8_lossy(&bytes[..header.len().min(bytes.len())])
        );
        // Flat encoding: exactly 4 RGBE bytes per pixel after the header.
        assert_eq!(bytes.len(), header.len() + pixels.len() * 4);

        // First scanline starts with the zero pixel, then exactly 1.0.
        let body = &bytes[header.len()..];
        assert_eq!(&body[0..4], &[0, 0, 0, 0]);
        assert_eq!(&body[4..8], &[128, 128, 128, 129]);
    }

    #[test]
    fn srgb_endpoints_map_to_linear_endpoints() {
        assert!(srgb_to_linear(0).abs() < 1e-6);
        assert!((srgb_to_linear(255) - 1.0).abs() < 1e-6);
        // Mid-gray sRGB 128 is roughly linear 0.2158.
        assert!((srgb_to_linear(128) - 0.2158).abs() < 1e-3);
    }
}
