// Minimal, proven-safe denim: encode the deepened denim albedo into a BC7 sRGB
// .vtex_c (2048 via the skin's upper-color container) and pack it ALONE at the
// jeans g_tColor path. No normal, no .vmat change -> identical mechanism to the
// stock skin, just a higher-res denim albedo.
//   usage: paige_albedo_only <skin_pak> <albedo.png> <out_dir.vpk>
use morphic::{Image, ImageData};
const DIR: &str = "models/heroes_wip/bookworm/materials/";
const COLOR: &str = "clothes_color_png_e472671.vtex_c";
const DONOR_2K: &str = "bookworm_upper_color_tga_fc1e5171.vtex_c";
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (pak, png, out) = (&a[1], &a[2], &a[3]);
    let donor = vpkmerge_core::read_vpk_entry(pak, &format!("{DIR}{DONOR_2K}"))?;
    let d = image::load_from_memory(&std::fs::read(png)?)?.to_rgba8();
    let (w, h) = d.dimensions();
    let img = Image {
        width: w,
        height: h,
        data: ImageData::Rgba8(d.into_raw()),
    };
    let vtex = morphic::replace_mip_chain(&donor, &img)?;
    let chk = morphic::decode(&vtex)?;
    println!(
        "albedo {}x{} -> {} bytes, re-decode {}x{}",
        w,
        h,
        vtex.len(),
        chk.width,
        chk.height
    );
    let e = format!("{DIR}{COLOR}");
    vpkmerge_core::pack(&[(e.as_str(), vtex.as_slice())], out)?;
    println!("wrote albedo-only override: {out}");
    Ok(())
}
