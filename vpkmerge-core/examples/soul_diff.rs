// Fidelity scorecard: diff a soul-container addon VPK against a golden reference
// VPK (e.g. the commissioned `hello_kitty_soul_container_commission_dir.vpk`).
//
// This answers the "is it byte-matched?" question concretely. It does NOT try to
// make two VPKs equal; it measures how far apart they are, entry by entry and --
// for the Source 2 resource files (`.vmdl_c` / `.vmat_c` / `.vtex_c`) -- block by
// block. The compiled `.vmdl_c` mesh blocks (MVTX/MIDX/...) come out of Valve's
// closed Windows resourcecompiler, so a Linux-built import cannot reproduce them
// bit-for-bit; this harness turns that gap into numbers instead of a guess.
//
// usage: cargo run --release --example soul_diff -- <ours_dir.vpk> <golden_dir.vpk>
use anyhow::{Context, Result};
use std::collections::BTreeSet;

/// One Source 2 block: 4-char tag + payload size.
struct Block {
    tag: String,
    size: u32,
}

/// Parse the Source 2 resource header block table (filesize, header/resource
/// version, then `blockCount` x {tag, offset, size}). Returns `None` if the bytes
/// are not a Source 2 resource (e.g. a plain `.txt`).
fn parse_blocks(d: &[u8]) -> Option<Vec<Block>> {
    if d.len() < 16 {
        return None;
    }
    let file_size = u32::from_le_bytes(d[0..4].try_into().ok()?) as usize;
    // The on-disk filesize field must match (or be within) the actual length for
    // this to be a resource file; guards against treating arbitrary bytes as one.
    if file_size != d.len() {
        return None;
    }
    let block_offset = u32::from_le_bytes(d[8..12].try_into().ok()?) as usize;
    let block_count = u32::from_le_bytes(d[12..16].try_into().ok()?) as usize;
    if block_count == 0 || block_count > 64 {
        return None;
    }
    let base = 8 + block_offset;
    let mut out = Vec::with_capacity(block_count);
    for i in 0..block_count {
        let off = base + i * 12;
        let tag = d.get(off..off + 4)?;
        let tag = std::str::from_utf8(tag).ok()?.to_string();
        let size = u32::from_le_bytes(d.get(off + 8..off + 12)?.try_into().ok()?);
        out.push(Block { tag, size });
    }
    Some(out)
}

fn read_all(vpk: &valve_pak::VPK, entry: &str) -> Result<Vec<u8>> {
    let mut f = vpk
        .get_file(entry)
        .with_context(|| format!("entry {entry} not found"))?;
    Ok(f.read_all()?)
}

/// Short fingerprint so two payloads of equal size can still be told apart in the
/// report without dumping bytes. FNV-1a 64-bit.
fn fnv1a(d: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in d {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let ours_path = args.next().context("arg1: ours_dir.vpk")?;
    let gold_path = args.next().context("arg2: golden_dir.vpk")?;

    let ours = valve_pak::open(&ours_path)?;
    let gold = valve_pak::open(&gold_path)?;

    let ours_entries: BTreeSet<String> = ours.file_paths().cloned().collect();
    let gold_entries: BTreeSet<String> = gold.file_paths().cloned().collect();

    println!("ours:   {ours_path}  ({} entries)", ours_entries.len());
    println!("golden: {gold_path}  ({} entries)", gold_entries.len());

    // --- entry-set diff ---
    let only_gold: Vec<_> = gold_entries.difference(&ours_entries).collect();
    let only_ours: Vec<_> = ours_entries.difference(&gold_entries).collect();
    let shared: Vec<_> = ours_entries.intersection(&gold_entries).cloned().collect();

    println!("\n== entry set ==");
    println!("  shared:      {}", shared.len());
    println!("  only golden: {}", only_gold.len());
    for e in &only_gold {
        println!("    - {e}");
    }
    println!("  only ours:   {}", only_ours.len());
    for e in &only_ours {
        println!("    + {e}");
    }

    // --- per-shared-entry diff ---
    println!("\n== shared entries ==");
    let mut identical = 0usize;
    for entry in &shared {
        let a = read_all(&ours, entry)?;
        let b = read_all(&gold, entry)?;
        if a == b {
            identical += 1;
            println!("  [IDENTICAL] {entry}  ({} bytes)", a.len());
            continue;
        }
        println!(
            "  [DIFF]      {entry}  ours={} golden={} bytes  (fnv {:016x} vs {:016x})",
            a.len(),
            b.len(),
            fnv1a(&a),
            fnv1a(&b)
        );
        // Source 2 resource? show the block-level breakdown.
        if let (Some(ba), Some(bb)) = (parse_blocks(&a), parse_blocks(&b)) {
            let tags: BTreeSet<&str> = ba.iter().chain(bb.iter()).map(|x| x.tag.as_str()).collect();
            for tag in tags {
                let sa = ba.iter().find(|x| x.tag == tag).map(|x| x.size);
                let sb = bb.iter().find(|x| x.tag == tag).map(|x| x.size);
                let mark = match (sa, sb) {
                    (Some(x), Some(y)) if x == y => "=",
                    (Some(_), Some(_)) => "~",
                    (Some(_), None) => "-only-ours",
                    (None, Some(_)) => "-only-gold",
                    _ => "?",
                };
                println!(
                    "                {tag}  ours={:>8}  golden={:>8}  {mark}",
                    sa.map_or("-".into(), |x| x.to_string()),
                    sb.map_or("-".into(), |x| x.to_string()),
                );
            }
        }
    }

    // --- verdict ---
    let total = shared.len() + only_gold.len() + only_ours.len();
    println!("\n== verdict ==");
    println!(
        "  byte-identical entries: {identical}/{} shared  ({total} entries across both)",
        shared.len()
    );
    let byte_matched = identical == shared.len() && only_gold.is_empty() && only_ours.is_empty();
    if byte_matched {
        println!("  RESULT: byte-matched (every entry identical, same entry set).");
    } else {
        println!(
            "  RESULT: NOT byte-matched. {} shared entries differ; {} entries only in golden; {} only in ours.",
            shared.len() - identical,
            only_gold.len(),
            only_ours.len()
        );
        println!("  (Compiled .vmdl_c/.vtex_c blocks are Valve-resourcecompiler / Valve-BC7");
        println!("   output; a Linux graft reproduces structure, not exact bytes.)");
    }
    if byte_matched {
        Ok(())
    } else {
        // Non-zero exit so CI/scripts can gate on it, but the report above is the point.
        std::process::exit(1);
    }
}
