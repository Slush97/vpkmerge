use anyhow::{Context, Result};
fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let vpk = a.next().context("vpk")?;
    let entry = a.next().context("entry")?;
    let bytes = vpkmerge_core::read_vpk_entry(&vpk, &entry)?;
    let info = morphic::inspect(&bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "{entry}\n  {:?} {}x{} depth{} mips{} flags={:?} (raw {:#06x}) bytes={}",
        info.format,
        info.width,
        info.height,
        info.depth,
        info.mip_count,
        info.flags,
        info.flags.bits(),
        bytes.len()
    );
    Ok(())
}
