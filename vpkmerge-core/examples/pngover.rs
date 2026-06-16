// Stamp a PNG's RGB over a .vtex_c entry and pack the result into an addon VPK.
// The PNG must match the texture's mip-0 dimensions. The base texture's alpha
// plane is kept untouched (head/skin alphas often feed masks), only RGB is
// replaced, then the full mip chain re-encodes in the texture's own format.
//
// usage: cargo run --release -p vpkmerge-core --example pngover -- \
//          <pak01_dir.vpk> <entry.vtex_c> <overlay.png> <out_dir.vpk>
use morphic::ImageData;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let pak = a.next().expect("pak01_dir.vpk");
    let entry = a.next().expect("entry path");
    let png_path = a.next().expect("overlay png");
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
    let ov = overlay.into_raw();
    for (dst, src) in px.chunks_exact_mut(4).zip(ov.chunks_exact(4)) {
        dst[..3].copy_from_slice(&src[..3]); // RGB from overlay, alpha stays
    }

    let new_bytes = morphic::replace_mip_chain(&base_bytes, &img)?;
    eprintln!(
        "re-encoded {} ({} bytes -> {} bytes)",
        entry,
        base_bytes.len(),
        new_bytes.len()
    );
    vpkmerge_core::pack(&[(entry.as_str(), new_bytes.as_slice())], &out)?;
    eprintln!("wrote {out}");
    Ok(())
}
