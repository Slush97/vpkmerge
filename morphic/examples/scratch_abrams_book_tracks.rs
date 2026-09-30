// Throwaway: report which tracks a loose NM clip animates.
// usage: cargo run -p morphic --example scratch_abrams_book_tracks -- <vpk> <clip entry>...
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(Path::new(&a[1])).unwrap();
    for entry in &a[2..] {
        let bytes = match vpk.get_file(entry).and_then(|mut f| f.read_all()) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let Ok(clip) = morphic::model::decode_nm_clip(&bytes) else {
            continue;
        };
        let rot: Vec<usize> = clip
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.rotations.is_some())
            .map(|(i, _)| i)
            .collect();
        let trans: Vec<usize> = clip
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.translations.is_some())
            .map(|(i, _)| i)
            .collect();
        println!("{}\t{}\t{:?}\t{:?}", entry, clip.frame_count, rot, trans);
    }
}
