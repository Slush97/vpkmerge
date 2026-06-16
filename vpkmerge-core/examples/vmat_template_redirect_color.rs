//! Patch a donor `.vmat_c` g_tColor path byte-faithfully.
//!
//! This preserves the donor's KV3 DATA layout and all non-DATA blocks, adding
//! the target string to the KV3 string table if needed.
//!
//! Usage:
//!   cargo run -p vpkmerge-core --example vmat_template_redirect_color -- \
//!     <donor.vpk> <donor-entry.vmat_c> <color-texture.vtex> <out.vmat_c>

use anyhow::{Context, Result};
use morphic::kv3::Seg;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let donor_vpk = args.next().context("donor.vpk")?;
    let donor_entry = args.next().context("donor-entry.vmat_c")?;
    let color_texture = args.next().context("color-texture.vtex")?;
    let out = PathBuf::from(args.next().context("out.vmat_c")?);

    let donor = vpkmerge_core::read_vpk_entry(&donor_vpk, &donor_entry)
        .with_context(|| format!("reading {donor_entry} from {donor_vpk}"))?;
    let patched = morphic::patch_kv3_resource_strings_adding(
        &donor,
        &[(
            vec![
                Seg::Key("m_textureParams".to_string()),
                Seg::Index(1),
                Seg::Key("m_pValue".to_string()),
            ],
            color_texture,
        )],
    )?;

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&out, &patched).with_context(|| format!("writing {}", out.display()))?;

    let mat = morphic::material::parse(&patched)?;
    println!(
        "wrote {}: material={} g_tColor={:?}",
        out.display(),
        mat.name,
        mat.texture("g_tColor")
    );
    Ok(())
}
