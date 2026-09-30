// Throwaway: print a mesh part's vertex input layout.
// usage: cargo run -p morphic --example scratch_layout -- <file.vmdl_c> <part>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let model = morphic::model::decode(&bytes).expect("decode");
    for mesh in &model.meshes {
        if !mesh.name.starts_with(&a[2]) {
            continue;
        }
        for (i, vb) in mesh.vertex_buffers.iter().enumerate() {
            println!(
                "{} buffer {i}: {} verts, stride {}",
                mesh.name, vb.element_count, vb.stride
            );
            for f in &vb.layout {
                println!(
                    "    {:<16} idx={} fmt={:?} offset={}",
                    f.semantic_name, f.semantic_index, f.format, f.offset
                );
            }
        }
    }
}
