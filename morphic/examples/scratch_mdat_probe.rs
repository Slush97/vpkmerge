// Throwaway: what does each edit step do to the book_model MDAT container?
// usage: cargo run -p morphic --example scratch_mdat_probe -- <vpk> <model entry>
use morphic::model::{
    replace_mesh_part_uncompressed, set_part_draw_call_groups, DrawCallGroup, VertexBuffer,
};
use morphic::resource::Resource;
use std::path::Path;

fn report(tag: &str, bytes: &[u8]) {
    let res = Resource::parse(bytes).unwrap();
    let b = res.get_block_by_index(24).unwrap();
    let method = u32::from_le_bytes(b[20..24].try_into().unwrap());
    println!(
        "{tag:<28} block24 len={:<8} compression_method={method}",
        b.len()
    );
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(Path::new(&a[1])).unwrap();
    let stock = vpk.get_file(&a[2]).unwrap().read_all().unwrap();
    report("stock", &stock);

    // A trivial same-shape mesh: reuse the part's own decoded geometry so the only
    // variable is which edit ran.
    let model = morphic::model::decode(&stock).unwrap();
    let part = model
        .meshes
        .iter()
        .find(|m| m.name == "book_model")
        .unwrap();
    let src = &part.vertex_buffers[0];
    let mesh = VertexBuffer {
        element_count: src.element_count,
        positions: src.positions.clone(),
        normals: src.normals.clone(),
        texcoords: src.texcoords.clone(),
        joints: src.joints.clone(),
        weights: src.weights.clone(),
        ..VertexBuffer::default()
    };
    let indices = part.primitives[0].indices.clone();

    let (replaced, _) =
        replace_mesh_part_uncompressed(&stock, "book_model", &mesh, &indices).expect("replace");
    report("replace (same counts)", &replaced);

    // Now with a DIFFERENT vertex/index count, which is what the real build does:
    // the draw call's m_nVertexCount / m_nIndexCount must be rewritten.
    let half_v = mesh.element_count / 2;
    let mut half_i: Vec<u32> = Vec::new();
    for tri in indices.chunks(3) {
        if tri.iter().all(|&i| (i as usize) < half_v) {
            half_i.extend_from_slice(tri);
        }
    }
    let trimmed = VertexBuffer {
        element_count: half_v,
        positions: mesh.positions[..half_v].to_vec(),
        normals: mesh.normals[..half_v].to_vec(),
        texcoords: vec![mesh.texcoords[0][..half_v].to_vec()],
        joints: mesh.joints[..half_v].to_vec(),
        weights: mesh.weights[..half_v].to_vec(),
        ..VertexBuffer::default()
    };
    let (shrunk, _) = replace_mesh_part_uncompressed(&stock, "book_model", &trimmed, &half_i)
        .expect("replace shrunk");
    report("replace (changed counts)", &shrunk);

    let total = mesh.element_count;
    let groups = vec![
        DrawCallGroup {
            material: "models/heroes_wip/familiar/materials/familiar_body.vmat".into(),
            start_index: 0,
            index_count: indices.len() / 2,
            vertex_start: 0,
            vertex_end: total / 2,
        },
        DrawCallGroup {
            material: "models/heroes_wip/familiar/materials/familiar_head.vmat".into(),
            start_index: indices.len() / 2,
            index_count: indices.len() - indices.len() / 2,
            vertex_start: total / 2,
            vertex_end: total,
        },
    ];
    let split = set_part_draw_call_groups(&replaced, "book_model", &groups, total).expect("split");
    report("after draw-call split", &split);
}
