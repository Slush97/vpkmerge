//! Repack a dir VPK with one extra (or replaced) entry from a loose file.
//! usage: vpk_add_entry <in_dir.vpk> <entry_path> <loose_file> <out_dir.vpk>
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 5 {
        eprintln!("usage: vpk_add_entry <in_dir.vpk> <entry_path> <loose_file> <out_dir.vpk>");
        std::process::exit(2);
    }
    let vpk = valve_pak::open(Path::new(&a[1])).expect("open vpk");
    let add = std::fs::read(&a[3]).expect("loose file");
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut paths: Vec<String> = vpk.file_paths().cloned().collect();
    paths.sort();
    let mut replaced = false;
    for p in &paths {
        let data = if *p == a[2] {
            replaced = true;
            add.clone()
        } else {
            vpk.get_file(p).expect("entry").read_all().expect("read")
        };
        files.push((p.clone(), data));
    }
    if !replaced {
        files.push((a[2].clone(), add));
    }
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(p, d)| (p.as_str(), d.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &a[4]).expect("pack");
    println!(
        "wrote {} ({} entries, {} {})",
        a[4],
        refs.len(),
        if replaced { "replaced" } else { "added" },
        a[2]
    );
}
