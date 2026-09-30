//! Deterministic native-atlas color grade; retains source resolution and alpha.
use std::path::Path;
fn hsv(p: &[u8]) -> (f32, f32, f32) {
    let (r, g, b) = (p[0] as f32 / 255., p[1] as f32 / 255., p[2] as f32 / 255.);
    let v = r.max(g).max(b);
    let d = v - r.min(g).min(b);
    let h = if d < 1e-6 {
        0.
    } else if v == r {
        60. * ((g - b) / d).rem_euclid(6.)
    } else if v == g {
        60. * ((b - r) / d + 2.)
    } else {
        60. * ((r - g) / d + 4.)
    };
    (h, if v > 0. { d / v } else { 0. }, v)
}
fn rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let c = v * s;
    let x = c * (1. - ((h / 60.).rem_euclid(2.) - 1.).abs());
    let m = v - c;
    let a = match (h / 60.) as u32 {
        0 => [c, x, 0.],
        1 => [x, c, 0.],
        2 => [0., c, x],
        3 => [0., x, c],
        4 => [x, 0., c],
        _ => [c, 0., x],
    };
    a.map(|z| z + m)
}
fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}
fn band(h: f32, a: f32, b: f32, c: f32, d: f32) -> f32 {
    smooth(a, b, h) * (1. - smooth(c, d, h))
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let src = Path::new(&a[1]);
    let dst = Path::new(&a[2]);
    let red_accents = a.iter().any(|s| s == "--red-accents");
    let direction = a.get(3).map(String::as_str).unwrap_or("");
    std::fs::create_dir_all(dst).unwrap();
    for stem in [
        "clownfit_diff_png_54039b26",
        "clownfit_accessory_diff_png_dcdbfa02",
    ] {
        let mut img = image::open(src.join(format!("{stem}.png")))
            .unwrap()
            .to_rgba8();
        let (width, height) = img.dimensions();
        for (x, y, p) in img.enumerate_pixels_mut() {
            let (h, s, v) = hsv(&p.0);
            if v < 0.015 {
                continue;
            }
            let chroma = smooth(0.08, 0.30, s);
            let red = band(h, 300., 325., 355., 365.) * chroma;
            let blue = band(h, 195., 213., 241., 255.) * chroma;
            let gold = band(h, 22., 32., 53., 65.) * chroma;
            let violet = band(h, 248., 266., 303., 323.) * chroma;
            // Monotonic value curves lift colored cloth while retaining fold ordering.
            // Whites and the original warm skin/head atlases are never selected.
            let accessory = stem.contains("accessory");
            // The bow islands occupy the bottom-right corner, separated by black
            // padding. Select only their original red dots, not the blue backing.
            let bow = x as f32 / width as f32 > 0.865 && y as f32 / height as f32 > 0.705;
            // Small pom-pom islands beside the boot panels; bounds are measured
            // on the 1600px inspection atlas and scaled to source resolution.
            let ax = x as f32 * 1600. / width as f32;
            let ay = y as f32 * 1600. / height as f32;
            let shoe_ornament = [
                (240., 1018., 281., 1136.),
                (321., 922., 364., 994.),
                (409., 920., 454., 988.),
                (436., 970., 484., 1045.),
                (530., 942., 573., 1015.),
                (624., 1064., 666., 1128.),
            ]
            .iter()
            .any(|&(l, t, r, b)| ax >= l && ax <= r && ay >= t && ay <= b);
            let accent_red = red_accents && (accessory || bow || shoe_ornament);
            let accent_blue = red_accents && accessory;
            let mut targets = [
                (
                    red,
                    if accent_red {
                        rgb(8., 0.60, (0.80 * (v / 0.60).powf(0.95)).min(0.92))
                    } else {
                        rgb(204., 0.29, (0.90 * (v / 0.60).powf(0.9)).min(0.97))
                    },
                ),
                (
                    blue,
                    if accent_blue {
                        rgb(8., 0.60, (0.75 * (v / 0.50).powf(0.95)).min(0.90))
                    } else {
                        rgb(211., 0.40, (0.70 * (v / 0.50).powf(0.95)).min(0.91))
                    },
                ),
                (
                    gold,
                    rgb(41., 0.26, (0.82 * (v / 0.95).powf(0.95)).min(0.92)),
                ),
                (violet, rgb(216., s * 0.45, v)),
            ];
            let cherry = rgb(356., 0.70, (0.68 * (v / 0.58).powf(0.95)).min(0.85));
            let ivory = rgb(40., 0.065, (0.87 * (v / 0.50).powf(0.95)).min(0.95));
            match direction {
                "pierrot" => {
                    // Deeper cool structure, pale central cloth, small cherry beats.
                    targets[1].1 = rgb(216., 0.43, (0.55 * (v / 0.50).powf(0.95)).min(0.82));
                    if accessory || bow || shoe_ornament {
                        targets[0].1 = cherry;
                    }
                    if accessory {
                        targets[1].1 = cherry;
                    }
                    targets[2].1 = rgb(40., 0.12, (0.82 * (v / 0.95).powf(0.95)).min(0.92));
                }
                "ribbon" => {
                    // Red suspenders/belt connect the torso and boots; pale buttons
                    // give the eye resting points within that warm framing.
                    targets[2].1 = rgb(358., 0.67, (0.66 * (v / 0.95).powf(0.95)).min(0.84));
                    if accessory || bow || shoe_ornament {
                        targets[0].1 = cherry;
                    }
                    if accessory {
                        targets[1].1 = ivory;
                    }
                    if bow {
                        targets[1].1 = rgb(210., 0.32, (0.62 * (v / 0.50).powf(0.95)).min(0.86));
                    }
                }
                "porcelain" => {
                    // A single red bow frames the face. Ivory buttons and pale
                    // champagne trim keep competing warm areas quieter.
                    targets[0].1 = rgb(205., 0.22, (0.89 * (v / 0.60).powf(0.9)).min(0.96));
                    targets[1].1 = rgb(214., 0.30, (0.64 * (v / 0.50).powf(0.95)).min(0.87));
                    if bow {
                        targets[0].1 = ivory;
                        targets[1].1 = cherry;
                    }
                    if accessory {
                        targets[0].1 = cherry;
                        targets[1].1 = ivory;
                    }
                    if shoe_ornament {
                        targets[0].1 = cherry;
                    }
                    targets[2].1 = rgb(40., 0.14, (0.83 * (v / 0.95).powf(0.95)).min(0.93));
                }
                _ => {}
            }
            for c in 0..3 {
                let old = p[c] as f32 / 255.;
                let value = old + targets.iter().map(|(w, t)| w * (t[c] - old)).sum::<f32>();
                p[c] = (value.clamp(0., 1.) * 255.).round() as u8;
            }
        }
        img.save(dst.join(format!("{stem}.png"))).unwrap();
        println!(
            "Recolored {stem}, {}x{}, original alpha retained",
            img.width(),
            img.height()
        );
    }
}
