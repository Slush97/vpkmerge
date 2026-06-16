//! Write a standalone inline-PNG `.vtex_c` from a source PNG.
//!
//! Usage:
//!   cargo run -p morphic --example write_vtex_png -- <input.png> <out.vtex_c> [--no-lod]

use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let input = PathBuf::from(
        args.next()
            .ok_or("usage: write_vtex_png <input.png> <out.vtex_c>")?,
    );
    let output = PathBuf::from(args.next().ok_or("out.vtex_c")?);
    let mut flags = morphic::TextureFlags::empty();
    for arg in args {
        if arg == "--no-lod" {
            flags |= morphic::TextureFlags::NO_LOD;
        } else {
            return Err(format!("unknown option: {}", arg.to_string_lossy()).into());
        }
    }

    let png = std::fs::read(&input)?;
    let vtex = morphic::encode_vtex_png_rgba8888_from_png(&png, flags)?;
    std::fs::write(&output, &vtex)?;

    let info = morphic::inspect(&vtex)?;
    println!(
        "wrote {}: {}x{} {:?} mips={} flags=0x{:04x}",
        output.display(),
        info.width,
        info.height,
        info.format,
        info.mip_count,
        info.flags.bits()
    );
    Ok(())
}
