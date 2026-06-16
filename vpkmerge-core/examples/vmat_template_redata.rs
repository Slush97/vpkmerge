//! Replace a donor `.vmat_c` DATA block with generated PBR material DATA.
//!
//! The donor's non-DATA blocks (`RERL`, `RED2`, `INSG`, etc.) are preserved
//! byte-for-byte. This is a diagnostic bridge for determining which compiled
//! material blocks the engine requires.
//!
//! Usage:
//!   cargo run -p vpkmerge-core --example vmat_template_redata -- \
//!     <donor.vpk> <donor-entry.vmat_c> <material-name.vmat> \
//!     <color-texture.vtex> <width> <height> <out.vmat_c>

use anyhow::{Context, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let donor_vpk = args.next().context("donor.vpk")?;
    let donor_entry = args.next().context("donor-entry.vmat_c")?;
    let material_name = args.next().context("material-name.vmat")?;
    let color_texture = args.next().context("color-texture.vtex")?;
    let width: u16 = args.next().context("width")?.parse()?;
    let height: u16 = args.next().context("height")?.parse()?;
    let out = PathBuf::from(args.next().context("out.vmat_c")?);

    let donor = vpkmerge_core::read_vpk_entry(&donor_vpk, &donor_entry)
        .with_context(|| format!("reading {donor_entry} from {donor_vpk}"))?;
    let generated = morphic::encode_pbr_vmat_c(&morphic::PbrVmatParams {
        material_name,
        color_texture,
        representative_width: width,
        representative_height: height,
    })?;
    let value = morphic::decode_kv3_resource(&generated)?;
    let rebuilt = morphic::encode_kv3_resource(&donor, &value)?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&out, &rebuilt).with_context(|| format!("writing {}", out.display()))?;

    let mat = morphic::material::parse(&rebuilt)?;
    println!(
        "wrote {}: material={} shader={} textures={}",
        out.display(),
        mat.name,
        mat.shader_name,
        mat.texture_params.len()
    );
    Ok(())
}
