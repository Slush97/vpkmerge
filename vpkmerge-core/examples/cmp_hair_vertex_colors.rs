//! Compare the hair draw call's per-vertex COLOR lane between two paks.
//! usage: cmp_hair_vertex_colors <source.vpk> <ours.vpk>
const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    for p in &a[1..=2] {
        let bytes = vpkmerge_core::read_vpk_entry(p, ENTRY)?;
        let m = morphic::model::decode(&bytes).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        println!("== {p}");
        for (mi, mesh) in m.meshes.iter().enumerate() {
            for prim in &mesh.primitives {
                if !prim.material.contains("mina_hair") {
                    continue;
                }
                let vb = &mesh.vertex_buffers[prim.vertex_buffer];
                let mut verts: Vec<usize> = prim.indices.iter().map(|&i| i as usize).collect();
                verts.sort_unstable();
                verts.dedup();
                let Some(colors) = vb.colors.first() else {
                    println!(
                        "  mesh{mi} '{}' prim mat={} verts={} -> NO COLOR LANE",
                        mesh.name,
                        prim.material,
                        verts.len()
                    );
                    continue;
                };
                // stats over the hair verts
                let mut sum = [0f64; 4];
                let mut lo = [f32::MAX; 4];
                let mut hi = [f32::MIN; 4];
                for &v in &verts {
                    let c = colors[v];
                    for k in 0..4 {
                        sum[k] += f64::from(c[k]);
                        lo[k] = lo[k].min(c[k]);
                        hi[k] = hi[k].max(c[k]);
                    }
                }
                let n = verts.len() as f64;
                let mean: Vec<f64> = sum.iter().map(|s| (s / n * 10.0).round() / 10.0).collect();
                println!(
                    "  mesh{mi} '{}' prim mat={} verts={} COLOR mean={mean:?} min={lo:?} max={hi:?}",
                    mesh.name, prim.material, verts.len()
                );
            }
        }
    }
    Ok(())
}
