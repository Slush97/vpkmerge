// Scan a VPK for square BC7/BCn LDR textures at a target size (donor candidates).
// usage: cargo run --release --example find_donor -- <vpk> <size>
use anyhow::{Context, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let vpk_path = args.next().context("arg1: vpk")?;
    let want: u32 = args.next().unwrap_or_else(|| "4096".into()).parse()?;
    let info0 = vpkmerge_core::inspect(&vpk_path)?;
    let vpk = valve_pak::open(&vpk_path)?;
    let mut hits = 0;
    for entry in &info0.file_paths {
        if !entry.ends_with(".vtex_c") {
            continue;
        }
        let Ok(mut f) = vpk.get_file(entry) else {
            continue;
        };
        let Ok(bytes) = f.read_all() else { continue };
        if let Ok(info) = vpkmerge_core::inspect_texture(&bytes) {
            if info.width == want && info.height == want {
                println!("{} {}x{} {}", info.format, info.width, info.height, entry);
                hits += 1;
            }
        }
    }
    eprintln!("{hits} square {want} textures",);
    Ok(())
}
