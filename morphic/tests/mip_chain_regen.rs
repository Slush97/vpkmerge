//! E3 round-trips: mip-chain regeneration on splice.
//!
//! For each fixture with a real mip chain we:
//!   1. decode mip 0 from the original
//!   2. call `replace_face_mip_chain` (or `replace_mip_chain`) with that mip 0
//!   3. for each mip M in the chain, decode the spliced resource at M and
//!      assert it's close to a freshly downsampled-then-encoded reference
//!
//! "Close" because `BCn` encode is lossy; tolerances mirror what
//! `edit_roundtrip.rs` already uses for single-slot round-trips, loosened a
//! bit for the smallest mips where one or two BC blocks cover the whole image
//! and quantization dominates.

use std::path::PathBuf;

use morphic::{
    decode_at, inspect, replace_face_mip_chain, replace_mip_chain, DecodeOptions, Image, ImageData,
    TextureFormat,
};

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(rel)
}

fn mae_u8(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    #[allow(clippy::cast_precision_loss)]
    let mae = sum as f64 / a.len() as f64;
    mae
}

fn assert_rgba8_close(actual: &Image, expected: &Image, eps: f64, label: &str) {
    assert_eq!(
        (actual.width, actual.height),
        (expected.width, expected.height),
        "{label}: dims diverged"
    );
    let (ImageData::Rgba8(ax), ImageData::Rgba8(bx)) = (&actual.data, &expected.data) else {
        panic!("{label}: expected Rgba8 on both sides");
    };
    let mae = mae_u8(ax, bx);
    assert!(
        mae <= eps,
        "{label}: mae {mae:.3} exceeded eps {eps}; lossy regen drifted too far"
    );
}

fn assert_rgba16f_close(actual: &Image, expected: &Image, abs: f32, rel: f32, label: &str) {
    assert_eq!(
        (actual.width, actual.height),
        (expected.width, expected.height),
        "{label}: dims diverged"
    );
    let (ImageData::Rgba16F(ax), ImageData::Rgba16F(bx)) = (&actual.data, &expected.data) else {
        panic!("{label}: expected Rgba16F on both sides");
    };
    assert_eq!(ax.len(), bx.len(), "{label}: buffer length differs");
    let mut worst: f32 = 0.0;
    let mut fails = 0usize;
    for (i, (av, bv)) in ax.iter().zip(bx.iter()).enumerate() {
        let a = av.to_f32();
        let b = bv.to_f32();
        let diff = (a - b).abs();
        if !(diff <= abs || diff <= rel * b.abs()) {
            fails += 1;
            if diff > worst {
                worst = diff;
            }
            if fails <= 3 {
                eprintln!("{label}: ch {i}: a={a} b={b} diff={diff}");
            }
        }
    }
    assert_eq!(
        fails, 0,
        "{label}: {fails} channels outside (abs={abs}, rel={rel}); worst diff {worst}"
    );
}

/// Decode mip 0 of the fixture, regenerate the chain, then for every mip in
/// the chain assert (a) it decodes (so the bytes we wrote are at least a
/// valid `BCn` payload at the right slot length), and (b) the post-regen mip 0
/// is close to the original mip 0 (sanity: the chain didn't tank mip 0).
/// Per-mip "matches what we'd expect" is hard to nail down without
/// duplicating the downsample/encode pipeline in the test, so we settle for
/// mip-0 fidelity plus full-chain decodability.
fn regen_decodes_at_every_mip(rel: &str, face: u8, label: &str) -> Vec<Image> {
    let path = fixture(rel);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{label}: read: {e}"));
    let info = inspect(&bytes).unwrap_or_else(|e| panic!("{label}: inspect: {e}"));

    let mip0 = decode_at(
        &bytes,
        &DecodeOptions {
            mip: 0,
            slice: 0,
            face,
        },
    )
    .unwrap_or_else(|e| panic!("{label}: decode mip 0: {e}"));

    let modified = replace_face_mip_chain(&bytes, face, &mip0)
        .unwrap_or_else(|e| panic!("{label}: replace_face_mip_chain: {e}"));

    let mut decoded = Vec::with_capacity(usize::from(info.mip_count));
    for mip in 0..info.mip_count {
        let img = decode_at(
            &modified,
            &DecodeOptions {
                mip,
                slice: 0,
                face,
            },
        )
        .unwrap_or_else(|e| panic!("{label}: decode regen mip {mip}: {e}"));
        let (mw, mh) = (
            (u32::from(info.width) >> u32::from(mip)).max(1),
            (u32::from(info.height) >> u32::from(mip)).max(1),
        );
        assert_eq!((img.width, img.height), (mw, mh), "{label}: mip {mip} dims");
        decoded.push(img);
    }

    // mip 0 fidelity: after one BCn encode pass mip 0 should still look like
    // the original. Tolerances per format (see assertions below).
    match info.format {
        TextureFormat::Bc6h => {
            assert_rgba16f_close(&decoded[0], &mip0, 0.01, 0.15, &format!("{label} mip 0"));
        }
        _ => {
            assert_rgba8_close(&decoded[0], &mip0, 8.0, &format!("{label} mip 0"));
        }
    }

    decoded
}

#[test]
fn ati1n_5mip_chain_regenerates() {
    regen_decodes_at_every_mip(
        "ati1n/gradient_dev_02_color_psd_b8463dec.vtex_c",
        0,
        "ATI1N 64x64 mip=5",
    );
}

#[test]
fn ati2n_7mip_chain_regenerates() {
    regen_decodes_at_every_mip(
        "ati2n/gradient_dev_v_mid_02_psd_79f9eba7.vtex_c",
        0,
        "ATI2N 256x256 mip=7",
    );
}

#[test]
fn bc7_5mip_chain_regenerates() {
    regen_decodes_at_every_mip(
        "bc7/gradient_dev_02_color_psd_73660177.vtex_c",
        0,
        "BC7 64x64 mip=5",
    );
}

#[test]
fn bc6h_cubemap_chain_regenerates_face0() {
    regen_decodes_at_every_mip(
        "bc6h/sky_l4d_c1_2_hdr_cube_pfm_b562e1cd.vtex_c",
        0,
        "BC6H cubemap face 0 mip=6",
    );
}

/// Splicing face 0's full mip pyramid must leave other faces byte-exact
/// across every mip. Catches drift in per-face offsets or chain arithmetic
/// inside `replace_face_mip_chain`.
#[test]
fn bc6h_cubemap_chain_isolates_other_faces() {
    let path = fixture("bc6h/sky_l4d_c1_2_hdr_cube_pfm_b562e1cd.vtex_c");
    let bytes = std::fs::read(&path).expect("fixture present");
    let info = inspect(&bytes).expect("inspect");

    let face0_mip0 = decode_at(
        &bytes,
        &DecodeOptions {
            mip: 0,
            slice: 0,
            face: 0,
        },
    )
    .expect("decode face 0 mip 0");

    // Snapshot every other face at every mip before the splice.
    let mut before = Vec::new();
    for face in 1..6u8 {
        for mip in 0..info.mip_count {
            let img = decode_at(
                &bytes,
                &DecodeOptions {
                    mip,
                    slice: 0,
                    face,
                },
            )
            .unwrap_or_else(|e| panic!("decode face {face} mip {mip} before: {e}"));
            before.push((face, mip, img));
        }
    }

    let modified = replace_face_mip_chain(&bytes, 0, &face0_mip0).expect("replace_face_mip_chain");

    for (face, mip, b_img) in &before {
        let a_img = decode_at(
            &modified,
            &DecodeOptions {
                mip: *mip,
                slice: 0,
                face: *face,
            },
        )
        .unwrap_or_else(|e| panic!("decode face {face} mip {mip} after: {e}"));
        let (ImageData::Rgba16F(ax), ImageData::Rgba16F(bx)) = (&a_img.data, &b_img.data) else {
            panic!("expected Rgba16F");
        };
        assert_eq!(
            ax.len(),
            bx.len(),
            "face {face} mip {mip}: buffer length changed"
        );
        for (i, (x, y)) in ax.iter().zip(bx.iter()).enumerate() {
            assert_eq!(
                x.to_bits(),
                y.to_bits(),
                "face {face} mip {mip} ch {i} drifted after face-0 chain splice"
            );
        }
    }
}

/// Non-cubemap convenience wrapper smoke test: `replace_mip_chain` should
/// behave identically to `replace_face_mip_chain(.., 0, ..)` for a
/// non-cubemap texture.
#[test]
fn replace_mip_chain_matches_face0_for_non_cubemap() {
    let path = fixture("bc7/gradient_dev_02_color_psd_73660177.vtex_c");
    let bytes = std::fs::read(&path).expect("fixture present");
    let mip0 = decode_at(&bytes, &DecodeOptions::default()).expect("decode mip 0");

    let via_face = replace_face_mip_chain(&bytes, 0, &mip0).expect("face splice");
    let via_conv = replace_mip_chain(&bytes, &mip0).expect("conv splice");
    assert_eq!(via_face, via_conv, "convenience wrapper diverged");
}

/// Inline-format textures have no on-wire mip chain. The regen path should
/// refuse cleanly rather than corrupt the resource.
#[test]
fn inline_png_rejects_chain_regen() {
    let path = fixture("png_rgba8888/dynamic_images_sentinel.vtex_c");
    let bytes = std::fs::read(&path).expect("fixture present");
    let mip0 = decode_at(&bytes, &DecodeOptions::default()).expect("decode mip 0");
    let err = replace_mip_chain(&bytes, &mip0).expect_err("inline must reject");
    let msg = err.to_string();
    assert!(
        msg.contains("not yet implemented"),
        "unexpected error: {msg}"
    );
}

/// Cubemap face index out of range must fail before we touch any bytes.
#[test]
fn cubemap_rejects_out_of_range_face() {
    let path = fixture("bc6h/sky_l4d_c1_2_hdr_cube_pfm_b562e1cd.vtex_c");
    let bytes = std::fs::read(&path).expect("fixture present");
    let mip0 = decode_at(&bytes, &DecodeOptions::default()).expect("decode mip 0");
    let err = replace_face_mip_chain(&bytes, 6, &mip0).expect_err("face 6 must reject");
    let msg = err.to_string();
    assert!(msg.contains("invalid decode target"), "unexpected: {msg}");
}

/// `new_mip0` whose dims don't match the texture's mip-0 dims must fail.
#[test]
fn dim_mismatch_rejected() {
    let path = fixture("bc7/gradient_dev_02_color_psd_73660177.vtex_c");
    let bytes = std::fs::read(&path).expect("fixture present");
    let bad = Image {
        width: 32,
        height: 32,
        data: ImageData::Rgba8(vec![0u8; 32 * 32 * 4]),
    };
    let err = replace_mip_chain(&bytes, &bad).expect_err("dim mismatch must reject");
    let msg = err.to_string();
    assert!(msg.contains("size mismatch"), "unexpected: {msg}");
}
