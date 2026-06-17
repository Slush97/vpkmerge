// Attach a GLB "hat" to a hero's head and pack an addon VPK.
// usage: cargo run --release --example import_hero_hat -- \
//          <pak01_dir.vpk> <hat.glb> <hero_codename> <out_dir.vpk> [name]
//   HAT_WIDTH, HAT_RAISE, HAT_YAW, HAT_ROTATE=X,Y,Z env vars tune fit/orientation.
use anyhow::{Context, Result};
use vpkmerge_core::{import_hero_hat, HatImportOptions};

fn envf(k: &str, d: f32) -> f32 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(d)
}

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let pak = a.next().context("arg1: pak01_dir.vpk")?;
    let glb_path = a.next().context("arg2: hat.glb")?;
    let hero = a.next().context("arg3: hero codename")?;
    let out = a.next().context("arg4: out_dir.vpk")?;
    let name = a.next().unwrap_or_else(|| "custom_hat".to_string());

    let rotate = std::env::var("HAT_ROTATE").ok().and_then(|s| {
        let v: Vec<f32> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        (v.len() == 3).then(|| [v[0], v[1], v[2]])
    });
    let opts = HatImportOptions {
        name,
        rotate,
        width: envf("HAT_WIDTH", 16.0),
        raise: envf("HAT_RAISE", -2.0),
        yaw: envf("HAT_YAW", 0.0),
        ..HatImportOptions::default()
    };
    let glb = std::fs::read(&glb_path)?;
    let r = import_hero_hat(&pak, &glb, &hero, &out, &opts)?;
    eprintln!(
        "hat -> {} (bone {} #{}); {} group(s), {} verts, {} tris; fit x{:.3}; crown z={:.1}",
        r.hero_entry,
        r.bone,
        r.bone_index,
        r.group_count,
        r.vert_count,
        r.tri_count,
        r.fit_scale,
        r.crown_z
    );
    eprintln!("wrote {out} ({} entries)", r.entry_count);
    Ok(())
}
