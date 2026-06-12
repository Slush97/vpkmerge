//! Map every VPK entry to its chunk archive index and CRC32.
//!
//! Steam updates rewrite only the chunk files they touch, so filtering this
//! map to chunks with a fresh mtime isolates the entries an update changed
//! (a superset: untouched entries co-resident in a patched chunk appear too).
//!
//! Usage: cargo run --release --example chunkmap -- <pak_dir.vpk>
//! Output: one line per entry: `<archive_index>\t<crc32 hex>\t<path>`

use valve_pak::VPK;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("usage: chunkmap <pak_dir.vpk>");
    let vpk = VPK::open(path).expect("open vpk");
    let paths: Vec<String> = vpk.file_paths().cloned().collect();
    for p in paths {
        let f = vpk.get_file(&p).expect("entry");
        let m = f.metadata();
        println!("{}\t{:08x}\t{}", m.archive_index, m.crc32, p);
    }
}
