//! Compare model-space vertex bboxes of the same entry in two paks.
//! usage: cmp_model_bbox <vpk_a> <vpk_b> <entry>
use vpkmerge_core::read_vpk_entry;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    for p in &a[1..=2] {
        let bytes = read_vpk_entry(p, &a[3])?;
        let m = morphic::model::decode(&bytes)?;
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        let mut n = 0usize;
        for mesh in &m.meshes {
            for vb in &mesh.vertex_buffers {
                for v in &vb.positions {
                    for k in 0..3 {
                        lo[k] = lo[k].min(v[k]);
                        hi[k] = hi[k].max(v[k]);
                    }
                    n += 1;
                }
            }
        }
        println!(
            "{p}: {n} verts, bbox z {:.2}..{:.2} (h {:.2}), x {:.2}..{:.2}, y {:.2}..{:.2}",
            lo[2],
            hi[2],
            hi[2] - lo[2],
            lo[0],
            hi[0],
            lo[1],
            hi[1]
        );
    }
    Ok(())
}
