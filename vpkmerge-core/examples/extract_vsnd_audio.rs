//! Extract the appended streaming-audio tail (MP3/WAV) from a `.vsnd_c` entry
//! in a VPK, writing the raw audio bytes to an output file. Dev helper for
//! auditioning / reprocessing shipped Deadlock sounds.
//!
//! Usage: cargo run --release --example extract_vsnd_audio -- <vpk> <entry> <out_audio>

use anyhow::{Context, Result};

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let vpk = a
        .next()
        .context("usage: extract_vsnd_audio <vpk> <entry> <out>")?;
    let entry = a.next().context("entry")?;
    let out = a.next().context("out")?;
    let bytes = vpkmerge_core::read_vpk_entry(&vpk, &entry)
        .with_context(|| format!("reading {entry} from {vpk}"))?;
    // The resource size lives in the first u32; everything after it is the
    // appended audio stream (see examples/vsnd_probe.rs).
    let file_size = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    anyhow::ensure!(
        file_size < bytes.len(),
        "no audio tail ({file_size} >= {})",
        bytes.len()
    );
    let tail = &bytes[file_size..];
    std::fs::write(&out, tail)?;
    println!("{entry}: wrote {} audio bytes -> {out}", tail.len());
    Ok(())
}
