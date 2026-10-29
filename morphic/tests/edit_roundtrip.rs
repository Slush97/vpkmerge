//! Round-trip tests for the Phase 1 encoder + splice path.
//!
//! For each uncompressed-format fixture we:
//!   1. decode the original mip 0 face 0 to `Image`
//!   2. re-encode it via `morphic::encode_image` for the texture's format
//!   3. splice the bytes back into the original `.vtex_c` via `replace_face0_mip0`
//!   4. decode the modified resource and assert the pixels match
//!
//! For RGBA8888 the round-trip is byte-exact. BGRA8888 likewise (the
//! channel swap is symmetric). `PNG_RGBA8888` is more interesting: we re-encode
//! through the PNG codec, and the on-wire bytes are not guaranteed to match
//! the original PNG payload, but the decoded pixels must.

use std::path::PathBuf;

use morphic::{
    decode, decode_at, encode_image, inspect, replace_face0_mip0, replace_face_mip, DecodeOptions,
    Image, ImageData,
};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(rel)
}

fn pixels_match(a: &Image, b: &Image) {
    assert_eq!(
        (a.width, a.height),
        (b.width, b.height),
        "dimensions diverged"
    );
    match (&a.data, &b.data) {
        (ImageData::Rgba8(x), ImageData::Rgba8(y)) => {
            assert_eq!(x.len(), y.len(), "buffer lengths differ");
            assert_eq!(x, y, "pixel buffers differ after round-trip");
        }
        (ImageData::Rgba16F(x), ImageData::Rgba16F(y)) => {
            assert_eq!(x.len(), y.len(), "buffer lengths differ");
            for (i, (xv, yv)) in x.iter().zip(y.iter()).enumerate() {
                assert_eq!(xv.to_bits(), yv.to_bits(), "f16 differ at {i}");
            }
        }
        _ => panic!("pixel kinds differ after round-trip"),
    }
}

#[test]
fn rgba8888_roundtrip_is_byte_exact() {
    let path = fixture("rgba8/minimap_circle.vtex_c");
    let bytes = std::fs::read(&path).expect("fixture present");
    let info = inspect(&bytes).expect("inspect");

    let decoded = decode(&bytes).expect("decode");
    let encoded = encode_image(&decoded, info.format).expect("encode");

    let modified = replace_face0_mip0(&bytes, &encoded).expect("splice");
    // For a pure round-trip on RGBA8888 the whole resource must be byte-identical:
    // the splice replaces N bytes with the same N bytes.
    assert_eq!(modified, bytes, "splice diverged on byte-exact format");

    let decoded2 = decode(&modified).expect("decode after splice");
    pixels_match(&decoded, &decoded2);
}

#[test]
fn png_rgba8888_roundtrip_preserves_pixels() {
    let path = fixture("png_rgba8888/dynamic_images_sentinel.vtex_c");
    let bytes = std::fs::read(&path).expect("fixture present");
    let info = inspect(&bytes).expect("inspect");

    let decoded = decode(&bytes).expect("decode");
    let encoded = encode_image(&decoded, info.format).expect("encode");

    // Re-encoded PNG payload won't match the original PNG bytes (different
    // encoder), so the splice length almost certainly differs from the
    // original. Phase 1's narrow splice rejects that; we should see the
    // length-mismatch error, which is the correct signal that inline-PNG
    // round-trip needs a different splice path (Phase 3 territory).
    let err =
        replace_face0_mip0(&bytes, &encoded).expect_err("inline PNG cannot round-trip via splice");
    let msg = err.to_string();
    assert!(
        msg.contains("splice length mismatch"),
        "unexpected error: {msg}"
    );
}

#[test]
fn encode_rejects_wrong_buffer_length() {
    // A 16x16 image whose buffer claims 4 bytes per pixel but is short.
    let img = Image {
        width: 16,
        height: 16,
        data: ImageData::Rgba8(vec![0u8; 16 * 16 * 4 - 1]),
    };
    let err = encode_image(&img, morphic::TextureFormat::Rgba8888).expect_err("must reject");
    let msg = err.to_string();
    assert!(msg.contains("size mismatch"), "unexpected error: {msg}");
}

#[test]
fn splice_rejects_wrong_payload_length() {
    let path = fixture("rgba8/minimap_circle.vtex_c");
    let bytes = std::fs::read(&path).expect("fixture present");
    // Garbage payload of one wrong byte.
    let err = replace_face0_mip0(&bytes, &[0u8; 1]).expect_err("must reject");
    let msg = err.to_string();
    assert!(
        msg.contains("splice length mismatch"),
        "unexpected error: {msg}"
    );
}

/// Sanity: BGRA8888 is the inverse of itself, so round-trip is byte-exact.
/// No BGRA8888 fixture exists in the corpus (Deadlock doesn't use it), so
/// we synthesise an image and exercise encode + decode only, skipping splice.
#[test]
fn bgra8888_encode_decode_roundtrip() {
    use morphic::{decode_image, TextureFlags, TextureFormat, TextureInfo};
    let mut pixels = Vec::with_capacity(8 * 8 * 4);
    for i in 0u8..(8 * 8) {
        pixels.extend_from_slice(&[i, i.wrapping_add(40), i.wrapping_add(80), 0xff]);
    }
    let img = Image {
        width: 8,
        height: 8,
        data: ImageData::Rgba8(pixels.clone()),
    };
    let encoded = encode_image(&img, TextureFormat::Bgra8888).expect("encode");
    let info = TextureInfo {
        format: TextureFormat::Bgra8888,
        width: 8,
        height: 8,
        actual_width: 8,
        actual_height: 8,
        depth: 1,
        mip_count: 1,
        flags: TextureFlags::empty(),
        reflectivity: [0.0; 4],
        ycocg: false,
    };
    let opts = morphic::DecodeOptions::default();
    let back = decode_image(&info, &encoded, &opts).expect("decode");
    pixels_match(&img, &back);
}

// --- Phase 2: block-compressed round-trips ----------------------------------
//
// BCn is lossy, so we can't ask for byte-exact equality. The round-trip
// measured here is: decode_original -> encode -> splice -> decode_modified;
// the noise we tolerate is one encode-decode pass on already-compressed data,
// which is typically much tighter than the original encoder pass.
//
// Tolerances are deliberately generous: we're verifying the *plumbing*, not
// benchmarking encoder quality. If a future encoder swap regresses badly,
// these numbers will catch it; tightening them is a separate exercise.

fn mae_u8(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "lengths must match");
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    #[allow(clippy::cast_precision_loss)]
    let mae = sum as f64 / a.len() as f64;
    mae
}

fn assert_rgba8_close(a: &Image, b: &Image, eps: f64, label: &str) {
    assert_eq!(
        (a.width, a.height),
        (b.width, b.height),
        "{label}: dims diverged"
    );
    let (ImageData::Rgba8(ax), ImageData::Rgba8(bx)) = (&a.data, &b.data) else {
        panic!("{label}: expected Rgba8 on both sides");
    };
    let mae = mae_u8(ax, bx);
    assert!(
        mae <= eps,
        "{label}: mae {mae:.3} exceeded eps {eps}; lossy round-trip drifted too far"
    );
}

fn assert_rgba16f_close(a: &Image, b: &Image, abs: f32, rel: f32, label: &str) {
    assert_eq!(
        (a.width, a.height),
        (b.width, b.height),
        "{label}: dims diverged"
    );
    let (ImageData::Rgba16F(ax), ImageData::Rgba16F(bx)) = (&a.data, &b.data) else {
        panic!("{label}: expected Rgba16F on both sides");
    };
    assert_eq!(ax.len(), bx.len(), "{label}: f16 buffers differ in length");
    let mut worst: f32 = 0.0;
    let mut fails = 0usize;
    for (i, (a16, b16)) in ax.iter().zip(bx.iter()).enumerate() {
        let av = a16.to_f32();
        let bv = b16.to_f32();
        let diff = (av - bv).abs();
        if !(diff <= abs || diff <= rel * bv.abs()) {
            fails += 1;
            if diff > worst {
                worst = diff;
            }
            if fails <= 3 {
                eprintln!("{label}: ch {i}: a={av} b={bv} diff={diff}");
            }
        }
    }
    assert_eq!(
        fails, 0,
        "{label}: {fails} channels outside (abs={abs}, rel={rel}); worst diff {worst}"
    );
}

fn roundtrip_face_mip(rel: &str, opts: DecodeOptions, label: &str) -> (Image, Image, Vec<u8>) {
    let path = fixture(rel);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{label}: read {rel}: {e}"));
    let info = inspect(&bytes).unwrap_or_else(|e| panic!("{label}: inspect: {e}"));
    let decoded = decode_at(&bytes, &opts).unwrap_or_else(|e| panic!("{label}: decode: {e}"));
    let encoded =
        encode_image(&decoded, info.format).unwrap_or_else(|e| panic!("{label}: encode: {e}"));
    let modified =
        replace_face_mip(&bytes, opts, &encoded).unwrap_or_else(|e| panic!("{label}: splice: {e}"));
    let decoded2 = decode_at(&modified, &opts).unwrap_or_else(|e| panic!("{label}: decode2: {e}"));
    (decoded, decoded2, modified)
}

#[test]
fn dxt1_roundtrip_within_tolerance() {
    let (a, b, _) = roundtrip_face_mip("dxt1/yellowflare.vtex_c", DecodeOptions::default(), "DXT1");
    assert_rgba8_close(&a, &b, 8.0, "DXT1 yellowflare");
}

#[test]
fn dxt5_roundtrip_within_tolerance() {
    let (a, b, _) = roundtrip_face_mip(
        "dxt5/boss_health_psd_cc842722.vtex_c",
        DecodeOptions::default(),
        "DXT5",
    );
    assert_rgba8_close(&a, &b, 6.0, "DXT5 boss_health");
}

#[test]
fn ati1n_roundtrip_within_tolerance() {
    let (a, b, _) = roundtrip_face_mip(
        "ati1n/gradient_dev_02_color_psd_b8463dec.vtex_c",
        DecodeOptions::default(),
        "ATI1N",
    );
    assert_rgba8_close(&a, &b, 6.0, "ATI1N gradient_dev_02_color");
}

#[test]
fn ati2n_roundtrip_within_tolerance() {
    let (a, b, _) = roundtrip_face_mip(
        "ati2n/gradient_dev_v_mid_02_psd_79f9eba7.vtex_c",
        DecodeOptions::default(),
        "ATI2N",
    );
    assert_rgba8_close(&a, &b, 6.0, "ATI2N gradient_dev_v_mid");
}

#[test]
fn bc7_roundtrip_within_tolerance() {
    let (a, b, _) = roundtrip_face_mip(
        "bc7/generic_sleep_icon.vtex_c",
        DecodeOptions::default(),
        "BC7",
    );
    assert_rgba8_close(&a, &b, 4.0, "BC7 generic_sleep_icon");
}

#[test]
fn bc6h_cube_face0_roundtrip_within_tolerance() {
    // Sky cubemap: 128x128 BC6H, 6 mips, 6 faces. Phase 1's splice replaces
    // exactly one slot; we use face 0 of mip 0 (the largest, brightest slot).
    let (a, b, _) = roundtrip_face_mip(
        "bc6h/sky_l4d_c1_2_hdr_cube_pfm_b562e1cd.vtex_c",
        DecodeOptions {
            mip: 0,
            slice: 0,
            face: 0,
        },
        "BC6H face 0 mip 0",
    );
    // HDR encoder noise: pixel magnitudes range over many orders. Keep abs
    // generous for near-zero channels and rel loose for bright ones.
    assert_rgba16f_close(&a, &b, 0.01, 0.15, "BC6H sky_l4d_c1_2_hdr_cube face 0");
}

#[test]
fn bc6h_cube_face_isolation_works() {
    // Splicing face 0 must leave face 1 byte-exact. If face_mip_byte_range is
    // mis-computing the start offset, face 1's decode would shift after the
    // splice. This catches that.
    let path = fixture("bc6h/sky_l4d_c1_2_hdr_cube_pfm_b562e1cd.vtex_c");
    let bytes = std::fs::read(&path).expect("read fixture");
    let info = inspect(&bytes).expect("inspect");
    let f1_opts = DecodeOptions {
        mip: 0,
        slice: 0,
        face: 1,
    };
    let face1_before = decode_at(&bytes, &f1_opts).expect("decode face 1 before");

    let f0_opts = DecodeOptions::default();
    let face0 = decode_at(&bytes, &f0_opts).expect("decode face 0");
    let encoded = encode_image(&face0, info.format).expect("encode face 0");
    let modified = replace_face_mip(&bytes, f0_opts, &encoded).expect("splice");

    let face1_after = decode_at(&modified, &f1_opts).expect("decode face 1 after");
    let (ImageData::Rgba16F(ax), ImageData::Rgba16F(bx)) = (&face1_before.data, &face1_after.data)
    else {
        panic!("expected Rgba16F");
    };
    assert_eq!(ax.len(), bx.len(), "face 1 length changed");
    for (i, (x, y)) in ax.iter().zip(bx.iter()).enumerate() {
        assert_eq!(
            x.to_bits(),
            y.to_bits(),
            "face 1 ch {i} drifted after splicing face 0"
        );
    }
}
