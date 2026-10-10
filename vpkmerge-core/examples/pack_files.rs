// Pack loose files into an addon VPK at given entry paths.
// Usage: pack_files <out_dir.vpk> <file1> <entry1> [<file2> <entry2> ...]
use std::fs;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    anyhow::ensure!(
        args.len() >= 3 && (args.len() - 1) % 2 == 0,
        "usage: <out.vpk> <file> <entry> [...]"
    );
    let out = &args[0];
    let mut bufs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut i = 1;
    while i + 1 < args.len() {
        bufs.push((args[i + 1].clone(), fs::read(&args[i])?));
        i += 2;
    }
    let refs: Vec<(&str, &[u8])> = bufs
        .iter()
        .map(|(e, b)| (e.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, out)?;
    println!("packed {} file(s) -> {out}", refs.len());
    for (e, b) in &bufs {
        println!("  {e} ({} bytes)", b.len());
    }
    Ok(())
}
