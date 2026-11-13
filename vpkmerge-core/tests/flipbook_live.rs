//! Live test for `workshop::flipbook` against the real game pak.
//!
//! Gated on `DEADLOCK_PAK` pointing at `citadel/pak01_dir.vpk`; CI skips.
//!
//! ```sh
//! DEADLOCK_PAK=~/.../Deadlock/game/citadel/pak01_dir.vpk \
//!   cargo test -p vpkmerge-core --test flipbook_live -- --nocapture
//! ```

// The head's UV bounds are compared exactly on purpose: its bytes must not move.
#![allow(clippy::float_cmp)]

use std::path::PathBuf;

use vpkmerge_core::read_vpk_entry;
use vpkmerge_core::workshop::flipbook::{flipbook_material, FlipbookOptions, FlipbookRows};

const MODEL: &str = "models/heroes_wip/abrams/abrams.vmdl_c";
const COAT: &str = "models/heroes_wip/abrams/materials/abrams_coat.vmat_c";
const HEAD: &str = "models/heroes_wip/abrams/materials/abrams_head.vmat";

/// `count` small opaque frames, each a different flat colour.
fn frames(count: u8) -> Vec<Vec<u8>> {
    (0..count)
        .map(|k| {
            let frame =
                image::RgbaImage::from_pixel(8, 8, image::Rgba([k * 40, 255 - k * 40, 128, 255]));
            let mut png = std::io::Cursor::new(Vec::new());
            frame.write_to(&mut png, image::ImageFormat::Png).unwrap();
            png.into_inner()
        })
        .collect()
}

fn options(rows_follow: FlipbookRows, keep_detail_maps: bool) -> FlipbookOptions {
    FlipbookOptions {
        columns: 2,
        rows: 2,
        fps: 8.0,
        rows_follow,
        max_texture: 512,
        keep_detail_maps,
    }
}

fn uv_bounds(model: &[u8], material: &str) -> morphic::model::UvRemapReport {
    morphic::model::remap_material_uvs(model, material, false, |uv| uv)
        .unwrap()
        .1
}

#[test]
fn a_time_flipbook_moves_the_part_onto_one_cell() {
    let Ok(pak) = std::env::var("DEADLOCK_PAK") else {
        eprintln!("DEADLOCK_PAK not set; skipping live flipbook test");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("flipbook_dir.vpk");
    let report = flipbook_material(
        &[PathBuf::from(&pak)],
        MODEL,
        COAT,
        &frames(4),
        &options(FlipbookRows::Time, false),
        &out,
    )
    .unwrap();

    // The coat's UVs sit in the first of 2 x 2 cells; the head's did not move.
    let original = read_vpk_entry(&pak, MODEL).unwrap();
    let patched = read_vpk_entry(&out, MODEL).unwrap();
    let coat = uv_bounds(&patched, COAT.trim_end_matches("_c"));
    assert_eq!(coat.vertices, report.vertices);
    assert!(coat.vertices > 0);
    for axis in 0..2 {
        assert!(coat.uv_min[axis] >= -1e-3, "{coat:?}");
        assert!(coat.uv_max[axis] <= 0.5 + 1e-3, "{coat:?}");
    }
    let (before, after) = (uv_bounds(&original, HEAD), uv_bounds(&patched, HEAD));
    assert!(before.vertices > 0);
    assert_eq!(before.uv_min, after.uv_min);
    assert_eq!(before.uv_max, after.uv_max);

    // The material steps one cell per frame and points at the grid textures.
    let material = morphic::material::parse(&read_vpk_entry(&out, COAT).unwrap()).unwrap();
    assert!(
        material
            .dynamic_params
            .contains_key("g_vAlbedoScrollSpeed1")
            || material.vector_params["g_vAlbedoScrollSpeed1"][0] > 0.0,
        "the scroll speed is set"
    );
    let color = report
        .textures
        .iter()
        .find(|t| t.slots.iter().any(|s| s == "g_tColor"))
        .expect("a colour grid");
    assert_eq!(material.texture("g_tColor"), Some(color.path.as_str()));
    assert_eq!((color.width, color.height), (512, 512));
    let texture = read_vpk_entry(&out, &format!("{}_c", color.path)).unwrap();
    let info = morphic::inspect(&texture).unwrap();
    assert_eq!((info.width, info.height), (512, 512));
}

#[test]
fn a_health_flipbook_keeps_the_detail_maps_and_refuses_a_second_pass() {
    let Ok(pak) = std::env::var("DEADLOCK_PAK") else {
        eprintln!("DEADLOCK_PAK not set; skipping live flipbook test");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("flipbook_dir.vpk");
    let report = flipbook_material(
        &[PathBuf::from(&pak)],
        MODEL,
        COAT,
        &frames(4),
        &options(FlipbookRows::Health, true),
        &out,
    )
    .unwrap();

    let original = morphic::material::parse(&read_vpk_entry(&pak, COAT).unwrap()).unwrap();
    let material = morphic::material::parse(&read_vpk_entry(&out, COAT).unwrap()).unwrap();
    assert!(material
        .dynamic_params
        .contains_key("g_vAlbedoScrollSpeed1"));
    assert_eq!(
        material.int_params.get("F_ENABLE_TEXTURE_TRANSFORMS"),
        Some(&1)
    );
    assert_eq!(
        material.texture("g_tNormalRoughness"),
        original.texture("g_tNormalRoughness")
    );
    assert!(report
        .textures
        .iter()
        .all(|t| t.slots.iter().all(|s| s != "g_tNormalRoughness")));

    // Its own output already scrolls: a second flipbook on the part is refused
    // rather than shrinking the UVs twice.
    let again = flipbook_material(
        &[out.clone(), PathBuf::from(&pak)],
        MODEL,
        COAT,
        &frames(4),
        &options(FlipbookRows::Time, false),
        &dir.path().join("again_dir.vpk"),
    );
    assert!(again.is_err());
}
