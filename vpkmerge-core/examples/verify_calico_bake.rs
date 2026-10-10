// Verify a baked Calico reskin addon: re-read each entry out of the addon VPK,
// check its format/dimensions still match the source it overrides, and dump the
// decoded result to PNG so it can be diffed against the intended recolour.
// Catches a bad mip-chain re-encode or a mispacked entry before install.
//
// usage: cargo run --release --example verify_calico_bake -- <pak01_dir.vpk> <addon_dir.vpk> <out_prefix>
use anyhow::{Context, Result};
use morphic::TextureFormat;

const DIR: &str = "models/heroes_staging/nano/nano_v2/materials/";
const ENTRIES: &[(&str, &str)] = &[
    ("head", "nanov2_head_color_png_4a754211.vtex_c"),
    ("body", "nanov2_body_color_png_95dda15b.vtex_c"),
];

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        a.len() >= 3,
        "usage: verify_calico_bake <pak01_dir.vpk> <addon_dir.vpk> <out_prefix>"
    );
    let (base, addon, prefix) = (&a[0], &a[1], &a[2]);

    for (name, file) in ENTRIES {
        let entry = format!("{DIR}{file}");
        let baked = vpkmerge_core::read_vpk_entry(addon, &entry)
            .with_context(|| format!("{entry} missing from {addon}"))?;
        let orig = vpkmerge_core::read_vpk_entry(base, &entry)?;
        let bi = morphic::inspect(&baked)?;
        let oi = morphic::inspect(&orig)?;
        // An override has to keep the format and dimensions of the texture it
        // replaces, or the engine samples garbage.
        anyhow::ensure!(
            bi.format == oi.format && bi.width == oi.width && bi.height == oi.height,
            "{name}: baked {:?} {}x{} != source {:?} {}x{}",
            bi.format,
            bi.width,
            bi.height,
            oi.format,
            oi.width,
            oi.height
        );

        let img = morphic::decode(&baked).with_context(|| format!("decoding baked {name}"))?;
        let png = morphic::encode_image(&img, TextureFormat::PngRgba8888)?;
        let path = format!("{prefix}_{name}.png");
        std::fs::write(&path, png)?;
        println!(
            "{name:5} {:?} {}x{}  {} bytes in addon  -> {path}",
            bi.format,
            bi.width,
            bi.height,
            baked.len()
        );
    }
    println!("\nOK: every entry decodes and matches the source format/dimensions");
    Ok(())
}
