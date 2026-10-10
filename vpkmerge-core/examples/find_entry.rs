//! List VPK entries containing a substring.
//! Usage: cargo run -p vpkmerge-core --example find_entry -- <vpk> <substr>
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(&args[1])?;
    let needle = args.get(2).map(String::as_str).unwrap_or("");
    let mut n = 0;
    for p in vpk.file_paths() {
        if p.contains(needle) {
            println!("{p}");
            n += 1;
        }
    }
    eprintln!("{n} match(es) for {needle:?} in {}", args[1]);
    Ok(())
}
