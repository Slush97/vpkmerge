//! Validate packed Celeste recolors against the original texture envelopes.
use std::path::Path;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let original = valve_pak::open(Path::new(&a[1])).unwrap();
    let packed = valve_pak::open(Path::new(&a[2])).unwrap();
    for stem in [
        "clownfit_diff_png_54039b26",
        "clownfit_accessory_diff_png_dcdbfa02",
    ] {
        let entry = format!("models/heroes_wip/unicorn/materials/{stem}.vtex_c");
        let src = original.get_file(&entry).unwrap().read_all().unwrap();
        let dst = packed.get_file(&entry).unwrap().read_all().unwrap();
        let si = morphic::inspect(&src).unwrap();
        let di = morphic::inspect(&dst).unwrap();
        assert_eq!(format!("{si:?}"), format!("{di:?}"));
        let morphic::ImageData::Rgba8(sp) = morphic::decode(&src).unwrap().data else {
            panic!()
        };
        let morphic::ImageData::Rgba8(dp) = morphic::decode(&dst).unwrap().data else {
            panic!()
        };
        let expected = image::open(Path::new(&a[3]).join(format!("{stem}.png")))
            .unwrap()
            .to_rgba8();
        let mut total = 0u64;
        let mut alpha_max = 0;
        let mut alpha_total = 0u64;
        for ((s, d), e) in sp
            .chunks_exact(4)
            .zip(dp.chunks_exact(4))
            .zip(expected.pixels())
        {
            for c in 0..3 {
                total += d[c].abs_diff(e[c]) as u64;
            }
            alpha_max = alpha_max.max(s[3].abs_diff(d[3]));
            alpha_total += s[3].abs_diff(d[3]) as u64;
            assert_eq!(s[3], e[3], "Authoring changed alpha");
        }
        let count = (sp.len() / 4) as f64;
        let mae = total as f64 / (count * 3.0);
        assert!(mae < 2.0, "Unexpected compression error {mae}");
        assert!(alpha_total as f64 / count < 0.5, "Unexpected alpha error");
        println!(
            "{stem}: {:?} {}x{} {} mips; RGB MAE={mae:.4}/255; alpha MAE={:.4}, max={alpha_max}",
            di.format,
            di.width,
            di.height,
            di.mip_count,
            alpha_total as f64 / count
        );
        std::fs::create_dir_all(Path::new(&a[3]).join("../verified")).unwrap();
        image::RgbaImage::from_raw(di.width.into(), di.height.into(), dp)
            .unwrap()
            .save(Path::new(&a[3]).join(format!("../verified/{stem}.png")))
            .unwrap();
    }
}
