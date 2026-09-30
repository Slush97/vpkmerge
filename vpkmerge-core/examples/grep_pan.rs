fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(&a[1])?;
    let needle = a[2].to_lowercase();
    let paths: Vec<String> = vpk.file_paths().cloned().collect();
    for p in paths {
        if !(p.ends_with(".vcss_c") || p.ends_with(".vxml_c") || p.ends_with(".vjs_c")) {
            continue;
        }
        let Ok(mut f) = vpk.get_file(&p) else {
            continue;
        };
        let Ok(b) = f.read_all() else { continue };
        let s = String::from_utf8_lossy(&b).to_lowercase();
        if s.contains(&needle) {
            println!("{p}");
        }
    }
    Ok(())
}
