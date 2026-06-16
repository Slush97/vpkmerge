//! Pack a single loose file into an addon VPK at a given entry path.
//! Usage: cargo run --release --example pack_one -- <entry> <file> <out_dir.vpk>

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let entry = a
        .next()
        .expect("usage: pack_one <entry> <file> <out_dir.vpk>");
    let file = a.next().expect("file");
    let out = a.next().expect("out_dir.vpk");
    let bytes = std::fs::read(&file)?;
    vpkmerge_core::pack(&[(entry.as_str(), bytes.as_slice())], &out)?;
    println!("packed {} ({} bytes) -> {}", entry, bytes.len(), out);
    Ok(())
}
