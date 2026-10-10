//! Make a single-clip sound-swap addon louder, losslessly. Reads one `.vsnd_c`
//! entry from an addon VPK, applies an mp3gain (`global_gain`) shift to the
//! appended MP3 audio in place (no re-encode, byte length preserved so the
//! container's `m_nStreamingSize` stays valid), and repacks it at the same entry.
//!
//! Usage: boost_vsnd_vpk <in.vpk> <entry> <gain_db> <out_dir.vpk>
use anyhow::{ensure, Context, Result};

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (in_vpk, entry, gain_db, out_vpk) = (
        &a[1],
        a[2].as_str(),
        a[3].parse::<f64>().context("gain_db")?,
        &a[4],
    );

    let bytes = vpkmerge_core::read_vpk_entry(in_vpk, entry)
        .with_context(|| format!("reading {entry} from {in_vpk}"))?;

    // Compiled sound layout: [resource structure ... file_size][appended MP3].
    // The first u32 of the header is the resource's own size; the audio stream
    // is everything after it.
    let file_size = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    ensure!(file_size < bytes.len(), "no appended audio tail");
    let (head, mp3) = bytes.split_at(file_size);

    let louder = vpkmerge_core::apply_mp3_gain(mp3, gain_db)?;
    ensure!(
        louder.len() == mp3.len(),
        "gain changed byte length ({} -> {}); m_nStreamingSize would be stale",
        mp3.len(),
        louder.len()
    );

    let mut out = Vec::with_capacity(bytes.len());
    out.extend_from_slice(head);
    out.extend_from_slice(&louder);

    vpkmerge_core::pack(&[(entry, out.as_slice())], out_vpk)?;
    eprintln!(
        "boosted {entry} by {gain_db:+} dB ({} audio bytes, unchanged length) -> {out_vpk}",
        mp3.len()
    );
    Ok(())
}
