// Entry-level diff of two VPKs: which paths were added, dropped, or changed.
//
// The check for a "repack with one file patched": every other entry must come
// through byte-identical, or the rebuild silently dropped or mangled part of the
// mod. Compares entry bytes directly (one pair at a time, so a large pak does not
// have to fit in memory), which also means a chunked vs single-file layout does
// not register as a difference.
//
// usage: cargo run --example vpk_diff -- <a_dir.vpk> <b_dir.vpk>

use std::collections::BTreeSet;

fn main() -> anyhow::Result<()> {
    let a_path = std::env::args().nth(1).expect("a_dir.vpk");
    let b_path = std::env::args().nth(2).expect("b_dir.vpk");

    let a: BTreeSet<String> = valve_pak::open(&a_path)?.file_paths().cloned().collect();
    let b: BTreeSet<String> = valve_pak::open(&b_path)?.file_paths().cloned().collect();
    println!("A: {} entries  ({a_path})", a.len());
    println!("B: {} entries  ({b_path})\n", b.len());

    let dropped: Vec<_> = a.difference(&b).collect();
    let added: Vec<_> = b.difference(&a).collect();

    let mut changed = Vec::new();
    let mut same = 0usize;
    for k in a.intersection(&b) {
        let ba = vpkmerge_core::read_vpk_entry(&a_path, k)?;
        let bb = vpkmerge_core::read_vpk_entry(&b_path, k)?;
        if ba == bb {
            same += 1;
        } else {
            changed.push((k.clone(), ba.len(), bb.len()));
        }
    }

    if !dropped.is_empty() {
        println!("DROPPED ({}):", dropped.len());
        for k in &dropped {
            println!("  {k}");
        }
    }
    if !added.is_empty() {
        println!("ADDED ({}):", added.len());
        for k in &added {
            println!("  {k}");
        }
    }
    println!("CHANGED ({}):", changed.len());
    for (k, sa, sb) in &changed {
        println!("  {k}  ({sa} -> {sb} bytes)");
    }
    println!("IDENTICAL: {same}");

    if dropped.is_empty() && added.is_empty() && changed.len() == 1 {
        println!("\nclean: exactly one entry differs, everything else byte-identical");
    }
    Ok(())
}
