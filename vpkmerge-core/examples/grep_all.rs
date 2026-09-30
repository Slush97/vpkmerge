fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(&a[1])?;
    let needle = a[2].as_bytes().to_vec();
    let paths: Vec<String> = vpk.file_paths().cloned().collect();
    let mut n = 0;
    for p in paths {
        if p.ends_with(".vtex_c") || p.ends_with(".vsnd_c") || p.ends_with(".vmdl_c") {
            continue;
        }
        let Ok(mut f) = vpk.get_file(&p) else {
            continue;
        };
        let Ok(b) = f.read_all() else { continue };
        if b.windows(needle.len()).any(|w| w == needle.as_slice()) {
            println!("{p}");
            n += 1;
        }
    }
    eprintln!("{n} files contain {:?}", a[2]);
    Ok(())
}
