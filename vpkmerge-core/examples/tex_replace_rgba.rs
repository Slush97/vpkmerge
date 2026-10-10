// Replace ALL of a .vtex_c's RGBA (mip 0) from a PNG, re-encode the full mip
// chain in the texture's own format, and pack into an addon VPK. Unlike
// `pngover` (RGB only, keeps alpha), this takes the alpha from the PNG too, for
// alpha-driven overlays like the ritual-circle glyph (pattern lives in alpha).
//
// usage: cargo run --release -p vpkmerge-core --example tex_replace_rgba -- \
//          <pak01_dir.vpk> <entry.vtex_c> <rgba.png> <out_dir.vpk>
use morphic::ImageData;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let pak = a.next().expect("pak01_dir.vpk");
    let entry = a.next().expect("entry path");
    let png_path = a.next().expect("rgba png");
    let out = a.next().expect("out_dir.vpk");

    let base_bytes = vpkmerge_core::read_vpk_entry(&pak, &entry)?;
    let mut img = morphic::decode(&base_bytes)?;
    let overlay = image::open(&png_path)?.to_rgba8();
    anyhow::ensure!(
        overlay.width() == img.width && overlay.height() == img.height,
        "overlay {}x{} != texture {}x{}",
        overlay.width(),
        overlay.height(),
        img.width,
        img.height
    );

    let ImageData::Rgba8(ref mut px) = img.data else {
        anyhow::bail!("expected Rgba8 decode (LDR texture)");
    };
    px.copy_from_slice(&overlay.into_raw());

    let new_bytes = morphic::replace_mip_chain(&base_bytes, &img)?;
    eprintln!(
        "re-encoded {entry} ({} -> {} bytes)",
        base_bytes.len(),
        new_bytes.len()
    );
    vpkmerge_core::pack(&[(entry.as_str(), new_bytes.as_slice())], &out)?;
    eprintln!("wrote {out}");
    Ok(())
}
