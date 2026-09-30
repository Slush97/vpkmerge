// Diff every decoded vertex attribute of every mesh part between two .vmdl_c
// files; reports which parts/attributes changed and the max position delta.
// usage: mesh_attr_diff <a.vmdl_c> <b.vmdl_c>
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let ma = morphic::model::decode(&std::fs::read(&a[1])?)?;
    let mb = morphic::model::decode(&std::fs::read(&a[2])?)?;
    anyhow::ensure!(ma.meshes.len() == mb.meshes.len(), "mesh count differs");
    for (x, y) in ma.meshes.iter().zip(&mb.meshes) {
        let mut changed = Vec::new();
        for (va, vb) in x.vertex_buffers.iter().zip(&y.vertex_buffers) {
            if va.positions != vb.positions {
                let d = va
                    .positions
                    .iter()
                    .zip(&vb.positions)
                    .map(|(p, q)| (0..3).map(|k| (p[k] - q[k]).abs()).fold(0.0, f32::max))
                    .fold(0.0, f32::max);
                changed.push(format!("positions(max {d:.3})"));
            }
            for (name, same) in [
                ("normals", va.normals == vb.normals),
                ("tangents", va.tangents == vb.tangents),
                ("texcoords", va.texcoords == vb.texcoords),
                ("colors", va.colors == vb.colors),
                ("joints", va.joints == vb.joints),
                ("weights", va.weights == vb.weights),
            ] {
                if !same {
                    changed.push(name.to_string());
                }
            }
        }
        let idx_same = x.primitives.iter().zip(&y.primitives).all(|(p, q)| p.indices == q.indices);
        if !idx_same {
            changed.push("indices".into());
        }
        println!(
            "{:34} {}",
            x.name,
            if changed.is_empty() { "identical".to_string() } else { changed.join(", ") }
        );
    }
    Ok(())
}
