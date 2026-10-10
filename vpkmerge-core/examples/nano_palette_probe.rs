// Throwaway probe: dump Calico's (nano) hero albedos and measure their palette
// so a band-split reskin spec can be designed against real numbers instead of
// guesses. Same question the bebop porcelain builder had to answer: which hue
// windows hold skin, hair and outfit, and what value range do they sit at.
//
// usage: cargo run --release --example nano_palette_probe -- <pak01_dir.vpk> <out_prefix>
use anyhow::{Context, Result};
use morphic::{ImageData, TextureFormat};

const DIR: &str = "models/heroes_staging/nano/nano_v2/materials/";

const MAPS: &[(&str, &str)] = &[
    ("body_color", "nanov2_body_color_png_95dda15b.vtex_c"),
    ("head_color", "nanov2_head_color_png_4a754211.vtex_c"),
    ("body_ao", "nanov2_body_ao_png_a424a23c.vtex_c"),
    ("head_ao", "nanov2_head_ao_png_1307669b.vtex_c"),
    ("body_rough", "nanov2_body_rough_png_bdfac937.vtex_c"),
    ("body_emissive", "nanov2_body_emissive_png_abf868a2.vtex_c"),
    ("head_normal", "nanov2_head_normal_png_1e3a68b1.vtex_c"),
    (
        "head_transmissive",
        "nanov2_head_vmat_g_tnprtransmissivecolor_e09201de.vtex_c",
    ),
    (
        "body_transmissive",
        "nanov2_body_vmat_g_tnprtransmissivecolor_e9ede78f.vtex_c",
    ),
];

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let h = if d < 1e-6 {
        0.0
    } else if mx == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if mx == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if mx < 1e-6 { 0.0 } else { d / mx };
    (h, s, mx)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pak = args.first().context("need pak01_dir.vpk")?;
    let prefix = args.get(1).context("need out prefix")?;

    for (name, file) in MAPS {
        let entry = format!("{DIR}{file}");
        let bytes = match vpkmerge_core::read_vpk_entry(pak, &entry) {
            Ok(b) => b,
            Err(e) => {
                println!("{name}: MISSING ({e})");
                continue;
            }
        };
        let info = morphic::inspect(&bytes)?;
        let img = morphic::decode(&bytes)?;
        println!(
            "\n=== {name}  {:?} {}x{} ===",
            info.format, info.width, info.height
        );

        let ImageData::Rgba8(px) = &img.data else {
            println!("  HDR, skipping stats");
            continue;
        };

        // 24 hue buckets of 15 deg, plus neutral / near-black counters.
        let mut buckets = [0u64; 24];
        let mut sat_sum = [0f64; 24];
        let mut val_sum = [0f64; 24];
        let mut neutral = 0u64;
        let mut dark = 0u64;
        let mut neutral_val_sum = 0f64;
        let total = (px.len() / 4) as f64;
        let mut alpha_min = 255u8;

        for p in px.chunks_exact(4) {
            alpha_min = alpha_min.min(p[3]);
            let (h, s, v) = rgb_to_hsv(
                f32::from(p[0]) / 255.0,
                f32::from(p[1]) / 255.0,
                f32::from(p[2]) / 255.0,
            );
            if v <= 0.05 {
                dark += 1;
                continue;
            }
            if s <= 0.13 {
                neutral += 1;
                neutral_val_sum += f64::from(v);
                continue;
            }
            let i = ((h / 15.0) as usize).min(23);
            buckets[i] += 1;
            sat_sum[i] += f64::from(s);
            val_sum[i] += f64::from(v);
        }

        println!("  alpha min {alpha_min}");
        println!(
            "  near-black {:>5.1}%   neutral(S<=.13) {:>5.1}% (mean V {:.2})",
            dark as f64 / total * 100.0,
            neutral as f64 / total * 100.0,
            if neutral > 0 {
                neutral_val_sum / neutral as f64
            } else {
                0.0
            }
        );
        let mut ranked: Vec<usize> = (0..24).collect();
        ranked.sort_by_key(|&i| std::cmp::Reverse(buckets[i]));
        for &i in ranked.iter().take(8) {
            if buckets[i] == 0 {
                break;
            }
            let n = buckets[i] as f64;
            println!(
                "  hue {:>3}-{:>3}  {:>5.1}%  meanS {:.2}  meanV {:.2}",
                i * 15,
                (i + 1) * 15,
                n / total * 100.0,
                sat_sum[i] / n,
                val_sum[i] / n
            );
        }

        let png = morphic::encode_image(&img, TextureFormat::PngRgba8888)?;
        let path = format!("{prefix}_{name}.png");
        std::fs::write(&path, png)?;
        println!("  wrote {path}");
    }
    Ok(())
}
