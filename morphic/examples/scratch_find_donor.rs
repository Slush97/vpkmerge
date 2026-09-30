// Throwaway: find a texture of a given size/format to use as a mip-chain donor.
// usage: cargo run -p morphic --example scratch_find_donor -- <vpk> <w> <entry>...
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(Path::new(&a[1])).unwrap();
    let want: u16 = a[2].parse().unwrap();
    let mut found = 0;
    for entry in &a[3..] {
        let Ok(bytes) = vpk.get_file(entry).and_then(|mut f| f.read_all()) else {
            continue;
        };
        let Ok(info) = morphic::inspect(&bytes) else {
            continue;
        };
        if info.width == want && info.height == want {
            println!(
                "{:?}  {}x{}  {}",
                info.format, info.width, info.height, entry
            );
            found += 1;
            if found >= 12 {
                return;
            }
        }
    }
    if found == 0 {
        println!("none found at {want}x{want}");
    }
}
