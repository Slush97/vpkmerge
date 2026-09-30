// Throwaway: dump a .vnmskel_c's bone names in track order (track i == bone i).
// usage: cargo run -p morphic --example dump_nm_skeleton -- <vpk> <entry>
use std::path::Path;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(Path::new(&a[1])).unwrap();
    let bytes = vpk.get_file(&a[2]).unwrap().read_all().unwrap();
    let nm = morphic::model::decode_nm_skeleton(&bytes).unwrap();
    let mut out = String::from("[");
    for (i, n) in nm.bone_names.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        out.push_str(n);
        out.push('"');
    }
    out.push(']');
    println!("{out}");
    eprintln!("--- {} nm bones ---", nm.bone_names.len());
}
