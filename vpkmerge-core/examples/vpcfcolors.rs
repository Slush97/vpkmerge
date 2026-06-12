//! Dump color-like constants (3/4-component 0..=255 int arrays) from `.vpcf_c`
//! entries in a VPK, with the KV3 field path that holds each one.
//!
//! Usage: cargo run --release --example vpcfcolors -- <vpk> <entry> [<entry>...]

use morphic::kv3::Value;

fn is_color(v: &Value) -> Option<[i64; 3]> {
    let Value::Array(items) = v else { return None };
    if items.len() != 3 && items.len() != 4 {
        return None;
    }
    let mut rgb = [0i64; 3];
    for (i, it) in items.iter().enumerate() {
        let n = match it {
            Value::Int(n) if (0..=255).contains(n) => *n,
            Value::UInt(u) if *u <= 255 => *u as i64,
            _ => return None,
        };
        if i < 3 {
            rgb[i] = n;
        }
    }
    Some(rgb)
}

fn hue(rgb: [i64; 3]) -> f64 {
    let (r, g, b) = (rgb[0] as f64, rgb[1] as f64, rgb[2] as f64);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max == min {
        return -1.0;
    }
    let d = max - min;
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h * 60.0
}

fn walk(v: &Value, path: &str, out: &mut Vec<(String, [i64; 3])>) {
    if let Some(rgb) = is_color(v) {
        // skip pure neutrals (white/black/grey) to cut noise
        if !(rgb[0] == rgb[1] && rgb[1] == rgb[2]) {
            out.push((path.to_string(), rgb));
        }
    }
    match v {
        Value::Object(fields) => {
            for (k, val) in fields {
                walk(val, &format!("{path}/{k}"), out);
            }
        }
        Value::Array(items) => {
            for (i, val) in items.iter().enumerate() {
                walk(val, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let vpk = a.next().expect("vpk");
    for entry in a {
        let bytes = vpkmerge_core::read_vpk_entry(&vpk, &entry)?;
        let doc = morphic::decode_kv3_resource(&bytes)?;
        let mut colors = Vec::new();
        walk(&doc, "", &mut colors);
        println!("{entry}: {} chromatic color constants", colors.len());
        for (path, rgb) in colors {
            println!(
                "  {:>3},{:>3},{:>3}  hue {:>5.1}  {}",
                rgb[0],
                rgb[1],
                rgb[2],
                hue(rgb),
                path
            );
        }
    }
    Ok(())
}
