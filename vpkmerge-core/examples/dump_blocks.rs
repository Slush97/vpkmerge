use vpkmerge_core::read_vpk_entry;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let b = read_vpk_entry(&a[1], &a[2])?;
    // print 4-char block tags from the resource header region
    let s = String::from_utf8_lossy(&b[..b.len().min(4096)]);
    for tag in [
        "DATA", "CTRL", "RERL", "REDI", "RED2", "MDAT", "MBUF", "VBIB", "PHYS", "ASEQ", "AGRP",
        "MRPH", "NTRO",
    ] {
        if b.windows(4).any(|w| w == tag.as_bytes()) {
            print!("{tag} ");
        }
    }
    println!();
    let _ = s;
    Ok(())
}
