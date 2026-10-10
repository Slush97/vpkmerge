// Throwaway: which bones do a part's vertices actually resolve to?
// usage: cargo run -p morphic --example scratch_joint_check -- <file.vmdl_c> <part>
use std::collections::BTreeMap;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let model = morphic::model::decode(&bytes).expect("decode");
    for mesh in &model.meshes {
        if !mesh.name.starts_with(&a[2]) {
            continue;
        }
        for vb in &mesh.vertex_buffers {
            let mut counts: BTreeMap<u16, (usize, f32)> = BTreeMap::new();
            for (j, w) in vb.joints.iter().zip(&vb.weights) {
                for k in 0..4 {
                    if w[k] > 0.0 {
                        let e = counts.entry(j[k]).or_insert((0, 0.0));
                        e.0 += 1;
                        e.1 += w[k];
                    }
                }
            }
            println!("{} ({} verts):", mesh.name, vb.element_count);
            for (bone, (n, wsum)) in counts {
                let name = model
                    .skeleton
                    .bones
                    .get(bone as usize)
                    .map_or("<OUT OF RANGE>", |b| b.name.as_str());
                println!("   bone {bone:<4} {name:<22} {n:>7} verts, weight sum {wsum:.1}");
            }
        }
    }
}
