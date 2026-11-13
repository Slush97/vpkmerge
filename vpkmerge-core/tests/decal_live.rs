//! Live test for `workshop::decal` against the real game pak.
//!
//! Gated on `DEADLOCK_PAK` pointing at `citadel/pak01_dir.vpk`; CI skips.
//!
//! ```sh
//! DEADLOCK_PAK=~/.../Deadlock/game/citadel/pak01_dir.vpk \
//!   cargo test -p vpkmerge-core --test decal_live -- --nocapture
//! ```

use std::path::PathBuf;

use vpkmerge_core::read_vpk_entry;
use vpkmerge_core::workshop::decal::{
    decal_picture, decal_preview, DecalOptions, DecalReport, DecalView,
};
use vpkmerge_core::workshop::flipbook::FlipbookRows;

const MODEL: &str = "models/heroes_wip/abrams/abrams.vmdl_c";
const COAT: &str = "models/heroes_wip/abrams/materials/abrams_coat.vmat";
const FOLDER: &str = "materials/grimoire/decal/models/heroes_wip/abrams/abrams";

/// `count` small frames, each a different flat colour. A cut-out frame has a
/// transparent corner.
fn frames(count: u8, cutout: bool) -> Vec<Vec<u8>> {
    (0..count)
        .map(|k| {
            let mut frame =
                image::RgbaImage::from_pixel(16, 16, image::Rgba([k * 40, 255 - k * 40, 128, 255]));
            if cutout {
                frame.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
            }
            let mut png = std::io::Cursor::new(Vec::new());
            frame.write_to(&mut png, image::ImageFormat::Png).unwrap();
            png.into_inner()
        })
        .collect()
}

fn options(name: &str, view: DecalView, columns: u32, rows: u32) -> DecalOptions {
    DecalOptions {
        name: name.into(),
        view,
        // The middle of the back, or of the chest.
        rect: [0.36, 0.18, 0.64, 0.42],
        columns,
        rows,
        fps: 8.0,
        rows_follow: FlipbookRows::Time,
        max_texture: 256,
        offset: 0.1,
    }
}

/// Draw calls of the model at its highest level of detail, as
/// `(material, vertex count)`.
fn draws(model: &[u8]) -> Vec<(String, usize)> {
    morphic::model::decode(model)
        .unwrap()
        .meshes
        .iter()
        .flat_map(|mesh| &mesh.primitives)
        .map(|primitive| (primitive.material.clone(), primitive.indices.len()))
        .collect()
}

fn build(
    pak: &str,
    earlier: Option<&PathBuf>,
    options: &DecalOptions,
    frames: &[Vec<u8>],
) -> (tempfile::TempDir, PathBuf, DecalReport) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("decal_dir.vpk");
    let mut paths: Vec<PathBuf> = earlier.into_iter().cloned().collect();
    paths.push(PathBuf::from(pak));
    let report = decal_picture(&paths, MODEL, frames, options, &out).unwrap();
    (dir, out, report)
}

#[test]
fn an_animated_decal_adds_draw_calls_and_leaves_the_part_alone() {
    let Ok(pak) = std::env::var("DEADLOCK_PAK") else {
        eprintln!("DEADLOCK_PAK not set; skipping live decal test");
        return;
    };
    let (_dir, out, report) = build(
        &pak,
        None,
        &options("cat", DecalView::Back, 2, 2),
        &frames(4, false),
    );

    // The picture lands on the coat, on every level of detail.
    assert_eq!(report.hosts[0].0, COAT);
    assert!(report.triangles > 100, "{report:?}");
    assert!(report.coverage > 0.8, "{report:?}");
    assert!(report.draw_calls >= 4, "{report:?}");
    assert!(!report.cutout);
    assert_eq!((report.width, report.height), (32, 32));

    // The model gained draw calls of the decal's material; every draw call it
    // had still has its material and its triangles.
    let original = draws(&read_vpk_entry(&pak, MODEL).unwrap());
    let patched = draws(&read_vpk_entry(&out, MODEL).unwrap());
    assert!(patched.len() > original.len());
    for draw in &original {
        assert!(patched.contains(draw), "{draw:?} changed");
    }
    let added: Vec<_> = patched
        .iter()
        .filter(|draw| !original.contains(draw))
        .collect();
    assert!(added
        .iter()
        .all(|(material, _)| *material == report.material));
    assert_eq!(
        added.iter().map(|(_, indices)| indices / 3).sum::<usize>(),
        report.triangles
    );

    // The coat's own material is not in the addon: the part keeps its look.
    assert!(read_vpk_entry(&out, &format!("{COAT}_c")).is_err());

    // The decal's material plays the frames from its own texture.
    assert_eq!(report.material, format!("{FOLDER}/cat.vmat"));
    let material =
        morphic::material::parse(&read_vpk_entry(&out, &format!("{}_c", report.material)).unwrap())
            .unwrap();
    assert_eq!(material.texture_params["g_tColor"], report.texture);
    let speed = material.vector_params["g_vAlbedoScrollSpeed1"];
    assert!(
        (speed[0] - 4.0).abs() < 1e-6 && (speed[1] - 2.0).abs() < 1e-6,
        "{speed:?}"
    );
    assert_eq!(
        material
            .int_params
            .get("F_ALPHA_TEST")
            .copied()
            .unwrap_or(0),
        0
    );
    let texture =
        morphic::inspect(&read_vpk_entry(&out, &format!("{}_c", report.texture)).unwrap()).unwrap();
    assert_eq!(
        (u32::from(texture.width), u32::from(texture.height)),
        (32, 32)
    );
}

#[test]
fn a_second_decal_keeps_off_the_first_and_a_cut_out_tests_alpha() {
    let Ok(pak) = std::env::var("DEADLOCK_PAK") else {
        eprintln!("DEADLOCK_PAK not set; skipping live decal test");
        return;
    };
    let (_first_dir, first, first_report) = build(
        &pak,
        None,
        &options("cat", DecalView::Back, 2, 2),
        &frames(4, false),
    );
    // The same box again: the second picture lands on the coat, not on the
    // first picture.
    let (_second_dir, second, report) = build(
        &pak,
        Some(&first),
        &options("dog", DecalView::Back, 1, 1),
        &frames(1, true),
    );
    assert_eq!(report.hosts[0].0, COAT);
    assert!(report
        .hosts
        .iter()
        .all(|(material, _)| !material.contains("/decal/")));
    assert_eq!(report.triangles, first_report.triangles);
    assert!(report.cutout);

    // The second addon's model carries both decals.
    let patched = draws(&read_vpk_entry(&second, MODEL).unwrap());
    for material in [&first_report.material, &report.material] {
        assert!(
            patched.iter().any(|(name, _)| name == material),
            "{material}"
        );
    }

    // A still picture does not scroll; its transparent parts are cut out.
    let material = morphic::material::parse(
        &read_vpk_entry(&second, &format!("{}_c", report.material)).unwrap(),
    )
    .unwrap();
    assert_eq!(material.int_params["F_ALPHA_TEST"], 1);
    assert!(material
        .vector_params
        .get("g_vAlbedoScrollSpeed1")
        .is_none_or(|speed| speed[0] == 0.0 && speed[1] == 0.0));
}

#[test]
fn a_picture_that_misses_the_model_is_refused_and_a_preview_is_a_png() {
    let Ok(pak) = std::env::var("DEADLOCK_PAK") else {
        eprintln!("DEADLOCK_PAK not set; skipping live decal test");
        return;
    };
    let paths = [PathBuf::from(&pak)];
    let dir = tempfile::tempdir().unwrap();
    let miss = DecalOptions {
        // Above the head.
        rect: [0.4, -0.5, 0.6, -0.3],
        ..options("cat", DecalView::Front, 1, 1)
    };
    let error = decal_picture(
        &paths,
        MODEL,
        &frames(1, false),
        &miss,
        &dir.path().join("x_dir.vpk"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("does not land"), "{error}");

    let png = decal_preview(
        &paths,
        MODEL,
        &frames(1, false)[0],
        DecalView::Front,
        [0.36, 0.18, 0.64, 0.42],
        256,
    )
    .unwrap();
    let preview = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(preview.height(), 256);
    // The frame's colour shows up on the chest.
    assert!(preview.pixels().any(|pixel| pixel.0 == [0, 255, 128, 255]));
}
