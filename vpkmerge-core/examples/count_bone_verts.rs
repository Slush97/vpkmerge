//! Throwaway: count LOD0 verts carrying weight on the named bones.
//! Usage: count_bone_verts <vpk> <entry> <bone> [bone...]
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
    let targets: Vec<u16> = a[3..]
        .iter()
        .filter_map(|n| names.iter().position(|x| x == n))
        .map(|i| u16::try_from(i).unwrap())
        .collect();
    assert_eq!(targets.len(), a.len() - 3, "some bones missing");
    let m = morphic::model::decode(&bytes).expect("decode");
    let mut count = 0usize;
    for vb in &m.meshes[0].vertex_buffers {
        for i in 0..vb.positions.len() {
            let (j4, w4) = (vb.joints[i], vb.weights[i]);
            if (0..4).any(|k| w4[k] > 0.0 && targets.contains(&j4[k])) {
                count += 1;
            }
        }
    }
    println!("{count} verts weighted to {:?}", &a[3..]);
}
