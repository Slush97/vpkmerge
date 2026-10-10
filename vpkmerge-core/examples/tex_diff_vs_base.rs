//! Throwaway: pair every `.vtex_c` in a mod VPK with its base-pak counterpart
//! (mod re-imports append `_png_<hash>` / `_psd_<hash>` to the source name) and
//! report which ones the mod actually changed.
//! Usage: tex_diff_vs_base <mod_vpk> <base_vpk>
use std::path::Path;

fn base_candidates(entry: &str) -> Vec<String> {
    // "foo_psd_c9873321_png_31cdb2d4.vtex_c" -> try dropping trailing
    // "_<tag>_<hex>" groups one at a time.
    let stem = entry.trim_end_matches(".vtex_c");
    let (dir, file) = match stem.rfind('/') {
        Some(i) => (&stem[..=i], &stem[i + 1..]),
        None => ("", stem),
    };
    let parts: Vec<&str> = file.split('_').collect();
    let mut out = Vec::new();
    let mut end = parts.len();
    while end >= 2 {
        let tag = parts[end - 2];
        let hex = parts[end - 1];
        let is_hash = hex.len() >= 6 && hex.chars().all(|c| c.is_ascii_hexdigit());
        let is_tag = matches!(tag, "png" | "psd" | "tga" | "jpg");
        if is_hash && is_tag {
            end -= 2;
            out.push(format!("{dir}{}.vtex_c", parts[..end].join("_")));
        } else {
            break;
        }
    }
    out
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (modv, basev) = (&a[1], &a[2]);
    let mv = valve_pak::open(Path::new(modv))?;
    let bv = valve_pak::open(Path::new(basev))?;
    let base_paths: std::collections::HashSet<String> = bv.file_paths().cloned().collect();

    let mut mods: Vec<String> = mv
        .file_paths()
        .filter(|p| p.ends_with(".vtex_c"))
        .cloned()
        .collect();
    mods.sort();

    for entry in &mods {
        let mut matched = None;
        for cand in base_candidates(entry) {
            if base_paths.contains(&cand) {
                matched = Some(cand);
                break;
            }
        }
        let Some(base_entry) = matched else {
            println!("{entry}\n   NO BASE COUNTERPART (mod-only texture)");
            continue;
        };
        let mb = vpkmerge_core::read_vpk_entry(modv, entry)?;
        let bb = vpkmerge_core::read_vpk_entry(basev, &base_entry)?;
        let mi = morphic::decode(&mb)?;
        let bi = morphic::decode(&bb)?;
        let (mw, mh) = (mi.width, mi.height);
        let (bw, bh) = (bi.width, bi.height);
        if mw != bw || mh != bh {
            println!("{entry}\n   size differs {bw}x{bh} -> {mw}x{mh}");
            continue;
        }
        let rgba = |img: &morphic::Image| -> anyhow::Result<Vec<u8>> {
            match &img.data {
                morphic::ImageData::Rgba8(v) => Ok(v.clone()),
                morphic::ImageData::Rgba16F(v) => Ok(v
                    .iter()
                    .map(|h| (f32::from(*h).clamp(0.0, 1.0) * 255.0) as u8)
                    .collect()),
            }
        };
        let mp = rgba(&mi)?;
        let bp = rgba(&bi)?;
        let n = mp.len().min(bp.len());
        let mut diff = 0u64;
        let mut changed = 0u64;
        for i in (0..n).step_by(4) {
            let d = (0..3)
                .map(|k| i32::from(mp[i + k]).abs_diff(i32::from(bp[i + k])))
                .max()
                .unwrap_or(0);
            diff += u64::from(d);
            if d > 6 {
                changed += 1;
            }
        }
        let px = (n / 4) as u64;
        let pct = 100.0 * changed as f64 / px as f64;
        let tag = if pct > 1.0 { "CHANGED" } else { "same" };
        println!(
            "{tag:8} {pct:6.1}% texels  mean|d|={:.1}  {bw}x{bh}  {}",
            diff as f64 / px as f64,
            entry.rsplit('/').next().unwrap_or(entry)
        );
    }
    Ok(())
}
