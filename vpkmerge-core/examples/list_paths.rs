//! List VPK entry paths matching a substring (case-insensitive).
//! Usage: cargo run -p vpkmerge-core --example list_paths -- <vpk> <substr> [substr2...]
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(&args[1])?;
    let needles: Vec<String> = args[2..].iter().map(|s| s.to_lowercase()).collect();
    let mut hits: Vec<&String> = vpk
        .file_paths()
        .filter(|p| {
            let lp = p.to_lowercase();
            needles.iter().any(|n| lp.contains(n))
        })
        .collect();
    hits.sort();
    for h in &hits {
        println!("{h}");
    }
    eprintln!("{} matches", hits.len());
    Ok(())
}
