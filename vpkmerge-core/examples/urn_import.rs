// Replace the Deadlock Idol/urn objective (`idol_urn.vmdl_c`) with an imported GLB.
//
// The urn is a legacy monolithic-VBIB model morphic cannot edit in place, so this
// reuses the proven soul-container CLONE pipeline (modern, editable envelope) and
// packs the result at the urn's model + material paths instead. The engine renders
// whatever `.vmdl_c` sits at that slot, so the urn becomes the imported mesh.
//
// Trade-off vs editing the real urn: it inherits the soul envelope's collision /
// skeleton and drops the urn's idle animation. Carried positioning is the thing to
// verify in-game.
//
// usage: cargo run --release --example urn_import -- \
//          <pak01_dir.vpk> <model.glb> <out_dir.vpk> [skin_name]
//   URN_SPAN=<units>                   largest-axis size in Source units (default 28)
//   URN_ORIENT=y-up|z-up|flip-y|auto   (default auto)
//   URN_ROTATE=X,Y,Z                   (extra Euler degrees, applied after orient)
//   URN_GROUND=0|1                     lift so the base sits at the origin (default 0)
use anyhow::{anyhow, Context, Result};
use vpkmerge_core::{import_clone, urn_target, SoulGlow, SoulImportCloneOptions, SoulOrient};

fn env_orient() -> Result<SoulOrient> {
    let raw = std::env::var("URN_ORIENT").unwrap_or_else(|_| "auto".to_string());
    Ok(
        match raw.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "" | "auto" => SoulOrient::Auto,
            "y-up" | "yup" | "default" => SoulOrient::YUp,
            "z-up" | "zup" => SoulOrient::ZUp,
            "flip-y" | "flipy" => SoulOrient::FlipY,
            other => {
                return Err(anyhow!(
                    "URN_ORIENT must be auto, y-up, z-up, or flip-y (got {other:?})"
                ))
            }
        },
    )
}

fn env_rotate() -> Result<Option<[f32; 3]>> {
    let Ok(spec) = std::env::var("URN_ROTATE") else {
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
        .with_context(|| format!("URN_ROTATE must be degrees as X,Y,Z, got {trimmed:?}"))?;
    if parts.len() != 3 {
        return Err(anyhow!(
            "URN_ROTATE must be degrees as X,Y,Z, got {trimmed:?}"
        ));
    }
    Ok(Some([parts[0], parts[1], parts[2]]))
}

fn env_span() -> Result<f32> {
    let raw = std::env::var("URN_SPAN").unwrap_or_else(|_| "28".to_string());
    raw.trim()
        .parse::<f32>()
        .with_context(|| format!("URN_SPAN must be a number, got {raw:?}"))
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let pak = args.next().context("arg1: pak01_dir.vpk")?;
    let glb_path = args.next().context("arg2: model glb")?;
    let out = args.next().context("arg3: out_dir.vpk")?;
    let name = args.next().unwrap_or_else(|| "soda_can".to_string());

    let span = env_span()?;
    let glb = std::fs::read(&glb_path)?;
    let opts = SoulImportCloneOptions {
        name,
        orient: env_orient()?,
        rotate: env_rotate()?,
        yaw: 0.0,
        orient_upright: false,
        ground: std::env::var("URN_GROUND")
            .map(|v| v == "1")
            .unwrap_or(false),
        // The urn ships no soul-glow particles; Off is also enforced by an empty
        // particle list in urn_target, so this is belt-and-braces.
        glow: SoulGlow::Off,
    };
    let report = import_clone(&pak, &glb, &out, &opts, &urn_target(span))?;

    eprintln!("orient: {}", report.orient_label);
    eprintln!(
        "mesh:   {} prims -> {} group(s), {} verts, {} tris; atlas {}x{} ({}px)",
        report.prim_count,
        report.group_count,
        report.vert_count,
        report.tri_count,
        report.atlas_cols,
        report.atlas_rows,
        report.atlas_px,
    );
    eprintln!(
        "fit:    source span {:.3} -> target {:.1} (x{:.3})",
        report.source_span, report.target_span, report.fit_scale,
    );
    eprintln!("normal: synthesized g_tNormalRoughness (relief from albedo + glossy roughness)");
    eprintln!(
        "wrote {out} ({} entries) -> overrides idol_urn.vmdl_c",
        report.entry_count
    );
    Ok(())
}
