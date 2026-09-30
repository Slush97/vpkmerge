//! Throwaway: for each named bone, print its world-bind translation and the
//! centroid of LOD0 verts carrying weight on it, plus the offset between them.
//! Usage: ear_geometry_probe <vpk> <entry> <bone> [bone...]
use std::path::Path;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(Path::new(&a[1])).expect("open");
    let bytes = vpk
        .get_file(&a[2])
        .expect("entry")
        .read_all()
        .expect("read");
    let sk = morphic::model::decode_skeleton(&bytes).expect("skel");
    let names: Vec<&str> = sk.bones.iter().map(|b| b.name.as_str()).collect();
    let m = morphic::model::decode(&bytes).expect("decode");
    for want in &a[3..] {
        let Some(bi) = names.iter().position(|x| x == want) else {
            println!("{want}: NOT IN SKELETON");
            continue;
        };
        let g = &sk.bones[bi].global_bind.m;
        let bone_w = [g[12], g[13], g[14]];
        let target = u16::try_from(bi).unwrap();
        let (mut sum, mut n) = ([0f64; 3], 0usize);
        for vb in &m.meshes[0].vertex_buffers {
            for i in 0..vb.positions.len() {
                let (j4, w4) = (vb.joints[i], vb.weights[i]);
                if (0..4).any(|k| w4[k] > 0.0 && j4[k] == target) {
                    for c in 0..3 {
                        sum[c] += f64::from(vb.positions[i][c]);
                    }
                    n += 1;
                }
            }
        }
        if n == 0 {
            println!(
                "{want}: bone_world=[{:.2},{:.2},{:.2}] verts=0",
                bone_w[0], bone_w[1], bone_w[2]
            );
            continue;
        }
        let c = [sum[0] / n as f64, sum[1] / n as f64, sum[2] / n as f64];
        let d: f64 = (0..3)
            .map(|k| (c[k] - f64::from(bone_w[k])).powi(2))
            .sum::<f64>()
            .sqrt();
        println!(
            "{want}: bone_world=[{:.2},{:.2},{:.2}]  vert_centroid=[{:.2},{:.2},{:.2}]  n={n}  |offset|={d:.2}",
            bone_w[0], bone_w[1], bone_w[2], c[0], c[1], c[2]
        );
    }
}
