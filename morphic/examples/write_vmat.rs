//! Write a constrained `pbr.vfx` `.vmat_c`.
//!
//! Usage:
//!   cargo run -p morphic --example write_vmat -- \
//!     <material-name.vmat> <color-texture.vtex> <width> <height> <out.vmat_c>

use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let material_name = args.next().ok_or(
        "usage: write_vmat <material-name.vmat> <color-texture.vtex> <width> <height> <out.vmat_c>",
    )?;
    let color_texture = args.next().ok_or("color-texture.vtex")?;
    let width: u16 = args.next().ok_or("width")?.parse()?;
    let height: u16 = args.next().ok_or("height")?.parse()?;
    let output = PathBuf::from(args.next().ok_or("out.vmat_c")?);

    let bytes = morphic::encode_pbr_vmat_c(&morphic::PbrVmatParams {
        material_name,
        color_texture,
        representative_width: width,
        representative_height: height,
    })?;
    std::fs::write(&output, &bytes)?;

    let mat = morphic::material::parse(&bytes)?;
    println!(
        "wrote {}: {} shader={} textures={}",
        output.display(),
        mat.name,
        mat.shader_name,
        mat.texture_params.len()
    );
    Ok(())
}
