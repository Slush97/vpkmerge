// Throwaway: extract one VPK entry to a file.
// usage: cargo run -p morphic --example scratch_extract_entry -- <vpk> <entry> <out>
use std::path::Path;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(Path::new(&a[1])).unwrap();
    let bytes = vpk.get_file(&a[2]).unwrap().read_all().unwrap();
    std::fs::write(&a[3], bytes).unwrap();
    println!("wrote {}", a[3]);
}
