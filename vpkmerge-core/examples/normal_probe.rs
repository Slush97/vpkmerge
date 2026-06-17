use anyhow::{Context, Result};
fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let vpk = a.next().context("vpk")?;
    let entry = a.next().context("entry")?;
    let bytes = vpkmerge_core::read_vpk_entry(&vpk, &entry)?;
    let info = morphic::inspect(&bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
    let img = morphic::decode(&bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
    let px = match img.data {
        morphic::ImageData::Rgba8(v) => v,
        _ => anyhow::bail!("hdr"),
    };
    let n = px.len() / 4;
    let mut s = [0f64; 4];
    let mut mn = [255u8; 4];
    let mut mx = [0u8; 4];
    for p in px.chunks_exact(4) {
        for c in 0..4 {
            s[c] += p[c] as f64;
            mn[c] = mn[c].min(p[c]);
            mx[c] = mx[c].max(p[c]);
        }
    }
    println!(
        "{entry}\n  {:?} {}x{} mips{}",
        info.format, info.width, info.height, info.mip_count
    );
    for c in 0..4 {
        println!(
            "  ch{} ({}) mean={:.1} min={} max={}",
            c,
            ["R", "G", "B", "A"][c],
            s[c] / n as f64,
            mn[c],
            mx[c]
        );
    }
    Ok(())
}
