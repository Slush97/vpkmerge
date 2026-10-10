// Throwaway: report the book part's buffer layout + draw calls.
// usage: cargo run -p morphic --example scratch_book_part_layout -- <vpk> <model entry>
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(Path::new(&a[1])).unwrap();
    let bytes = vpk.get_file(&a[2]).unwrap().read_all().unwrap();

    for t in morphic::model::vertex_targets(&bytes).unwrap() {
        if t.mesh_name.starts_with("book_model") {
            println!(
                "part {:<18} verts={:<7} has_color={}",
                t.mesh_name, t.vertex_count, t.has_color
            );
        }
    }
    println!("--- draw calls ---");
    for dc in morphic::model::draw_call_targets(&bytes).unwrap() {
        println!("{dc:?}");
    }
}
