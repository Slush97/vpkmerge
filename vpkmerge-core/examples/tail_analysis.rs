//! Throwaway: analyze which vertices/triangles a bone-name prefix owns, and whether
//! that geometry is a separate connected component (safe to collapse) or welded into
//! the surrounding mesh.
//! Usage: tail_analysis <vpk> <entry> <bone-prefix>
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(Path::new(&a[1])).expect("open");
    let bytes = vpk
        .get_file(&a[2])
        .expect("entry")
        .read_all()
        .expect("read");
    let prefix = &a[3];

    let sk = morphic::model::decode_skeleton(&bytes).expect("skel");
    let targets: Vec<u16> = sk
        .bones
        .iter()
        .enumerate()
        .filter(|(_, b)| b.name.starts_with(prefix.as_str()))
        .map(|(i, _)| u16::try_from(i).unwrap())
        .collect();
    println!("{} bones match prefix {prefix:?}", targets.len());

    let m = morphic::model::decode(&bytes).expect("decode");
    for (mi, mesh) in m.meshes.iter().enumerate() {
        for (bi, vb) in mesh.vertex_buffers.iter().enumerate() {
            let n = vb.positions.len();
            let has_skin = !vb.joints.is_empty() && !vb.weights.is_empty();
            let mut flagged = vec![false; n];
            let mut weight_sum = vec![0f32; n];
            if has_skin {
                for i in 0..n {
                    let (j4, w4) = (vb.joints[i], vb.weights[i]);
                    for k in 0..4 {
                        if w4[k] > 0.0 && targets.contains(&j4[k]) {
                            flagged[i] = true;
                            weight_sum[i] += w4[k];
                        }
                    }
                }
            }
            let count = flagged.iter().filter(|f| **f).count();
            println!(
                "mesh {mi} buffer {bi}: {n} verts, skinned={has_skin}, {count} touched by {prefix}"
            );
            if count == 0 {
                continue;
            }
            // partial-weight verts (blended between tail and body) are the tear risk
            let partial = (0..n)
                .filter(|&i| flagged[i] && weight_sum[i] < 0.999)
                .count();
            let full = count - partial;
            println!("   full-weight (>=0.999): {full}   partial: {partial}");
            let mut mn = [f32::MAX; 3];
            let mut mx = [f32::MIN; 3];
            for i in 0..n {
                if flagged[i] {
                    for k in 0..3 {
                        mn[k] = mn[k].min(vb.positions[i][k]);
                        mx[k] = mx[k].max(vb.positions[i][k]);
                    }
                }
            }
            println!("   bbox min {mn:?} max {mx:?}");
            // contiguous index range?
            let first = flagged.iter().position(|f| *f).unwrap();
            let last = n - 1 - flagged.iter().rev().position(|f| *f).unwrap();
            let contiguous = (first..=last).all(|i| flagged[i]);
            println!("   index range {first}..={last} contiguous={contiguous}");
        }

        // triangle-level: does any triangle mix flagged and unflagged verts?
        for (ii, prim) in mesh.primitives.iter().enumerate() {
            let vb = match mesh.vertex_buffers.get(prim.vertex_buffer) {
                Some(v) => v,
                None => continue,
            };
            if vb.joints.is_empty() || vb.weights.is_empty() {
                continue;
            }
            let n = vb.positions.len();
            let mut flagged = vec![false; n];
            for i in 0..n {
                let (j4, w4) = (vb.joints[i], vb.weights[i]);
                for k in 0..4 {
                    if w4[k] > 0.0 && targets.contains(&j4[k]) {
                        flagged[i] = true;
                    }
                }
            }
            let (mut pure, mut mixed, mut none) = (0usize, 0usize, 0usize);
            for tri in prim.indices.chunks_exact(3) {
                let c = tri.iter().filter(|&&v| flagged[v as usize]).count();
                match c {
                    3 => pure += 1,
                    0 => none += 1,
                    _ => mixed += 1,
                }
            }
            println!(
                "prim {ii} [{}] vb={}: {} tris  pure-{prefix}={pure}  mixed={mixed}  other={none}",
                prim.material,
                prim.vertex_buffer,
                prim.indices.len() / 3
            );
        }
    }
}
