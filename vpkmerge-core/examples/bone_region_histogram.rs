//! Throwaway: histogram which bones carry the verts inside a model-space
//! sphere, across ALL meshes/vertex buffers. Finds what a visible-but-
//! mispositioned part (e.g. the Baroness Mina purse keys) is skinned to.
//! Usage: bone_region_histogram <vpk> <entry> <x> <y> <z> <radius>
use std::collections::HashMap;
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 7 {
        eprintln!("usage: bone_region_histogram <vpk> <entry> <x> <y> <z> <radius>");
        std::process::exit(2);
    }
    let vpk = valve_pak::open(Path::new(&a[1])).expect("open");
    let bytes = vpk
        .get_file(&a[2])
        .expect("entry")
        .read_all()
        .expect("read");
    let c: Vec<f32> = a[3..6].iter().map(|s| s.parse().unwrap()).collect();
    let r: f32 = a[6].parse().unwrap();
    let sk = morphic::model::decode_skeleton(&bytes).expect("skel");
    let m = morphic::model::decode(&bytes).expect("decode");
    let mut hist: HashMap<String, usize> = HashMap::new();
    let mut inside = 0usize;
    for (mi, mesh) in m.meshes.iter().enumerate() {
        for (vi, vb) in mesh.vertex_buffers.iter().enumerate() {
            let mut hit = 0usize;
            for i in 0..vb.positions.len() {
                let p = vb.positions[i];
                let d2 = (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2);
                if d2 > r * r {
                    continue;
                }
                inside += 1;
                hit += 1;
                let (j4, w4) = (vb.joints[i], vb.weights[i]);
                for k in 0..4 {
                    if w4[k] > 0.0 {
                        // decode() already remaps joints onto the model skeleton.
                        let bone = j4[k] as usize;
                        let name = sk
                            .bones
                            .get(bone)
                            .map_or("<oob>".to_string(), |b| b.name.clone());
                        *hist.entry(name).or_default() += 1;
                    }
                }
            }
            if hit > 0 {
                eprintln!("mesh {mi} buffer {vi}: {hit} verts in region");
            }
        }
    }
    let mut v: Vec<_> = hist.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    println!(
        "{inside} verts within {r} of ({}, {}, {})",
        c[0], c[1], c[2]
    );
    for (name, n) in v.iter().take(20) {
        println!("  {name:<24} {n}");
    }
}
