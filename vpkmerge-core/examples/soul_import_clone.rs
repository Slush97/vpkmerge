// Build a custom soul container that is a faithful CLONE of an imported GLB.
//
// The pipeline now lives in `vpkmerge_core::soul_import_clone` (and is exposed as
// the `vpkmerge soul-container import` subcommand). This example is a thin
// env-var front-end kept for quick local iteration; it just maps the SOUL_*
// env vars onto the library options and prints the build report.
//
// usage: cargo run --release --example soul_import_clone -- \
//          <pak01_dir.vpk> <model.glb> <out_dir.vpk> [skin_name]
//   SOUL_ORIENT=y-up|z-up|flip-y|auto   (default y-up)
//   SOUL_ROTATE=X,Y,Z                    (extra Euler degrees, applied after orient)
//   SOUL_GLOW=recolor|base|off           (default recolor)
use anyhow::{anyhow, Context, Result};
use vpkmerge_core::{import_soul_container_clone, SoulGlow, SoulImportCloneOptions, SoulOrient};

fn env_orient() -> Result<SoulOrient> {
    let raw = std::env::var("SOUL_ORIENT").unwrap_or_else(|_| "y-up".to_string());
    Ok(
        match raw.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "" | "y-up" | "yup" | "default" => SoulOrient::YUp,
            "z-up" | "zup" => SoulOrient::ZUp,
            "flip-y" | "flipy" => SoulOrient::FlipY,
            "auto" => SoulOrient::Auto,
            other => {
                return Err(anyhow!(
                    "SOUL_ORIENT must be auto, y-up, z-up, or flip-y (got {other:?})"
                ))
            }
        },
    )
}

fn env_rotate() -> Result<Option<[f32; 3]>> {
    let Ok(spec) = std::env::var("SOUL_ROTATE") else {
        return Ok(None);
    };
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parts: Vec<f32> = trimmed
        .split(',')
        .map(|p| p.trim().parse::<f32>())
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("SOUL_ROTATE must be degrees as X,Y,Z, got {trimmed:?}"))?;
    if parts.len() != 3 {
        return Err(anyhow!(
            "SOUL_ROTATE must be degrees as X,Y,Z, got {trimmed:?}"
        ));
    }
    Ok(Some([parts[0], parts[1], parts[2]]))
}

fn env_glow() -> SoulGlow {
    match std::env::var("SOUL_GLOW")
        .unwrap_or_else(|_| "recolor".into())
        .as_str()
    {
        "off" => SoulGlow::Off,
        "base" => SoulGlow::Base,
        _ => SoulGlow::Recolor,
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let pak = args.next().context("arg1: pak01_dir.vpk")?;
    let glb_path = args.next().context("arg2: model glb")?;
    let out = args.next().context("arg3: out_dir.vpk")?;
    let name = args.next().unwrap_or_else(|| "custom_soul".to_string());

    let glb = std::fs::read(&glb_path)?;
    let opts = SoulImportCloneOptions {
        name,
        orient: env_orient()?,
        rotate: env_rotate()?,
        glow: env_glow(),
    };
    let report = import_soul_container_clone(&pak, &glb, &out, &opts)?;

    eprintln!("orient: {}", report.orient_label);
    eprintln!(
        "mesh:   {} prims -> {} group(s), {} verts, {} tris; atlas {}x{} ({}px); fit x{:.3}",
        report.prim_count,
        report.group_count,
        report.vert_count,
        report.tri_count,
        report.atlas_cols,
        report.atlas_rows,
        report.atlas_px,
        report.fit_scale,
    );
    eprintln!(
        "glow:   hue {:.0} deg (from dominant group)",
        report.glow_hue
    );
    eprintln!("wrote {out} ({} entries)", report.entry_count);
    Ok(())
}
