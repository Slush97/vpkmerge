use std::collections::BTreeSet;

use anyhow::Context;
use vpkmerge_core::read_vpk_entry;

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";

fn main() -> anyhow::Result<()> {
    let vpk = std::env::args().nth(1).context("arg1: vpk")?;
    let bytes = read_vpk_entry(&vpk, ENTRY)?;
    let model = morphic::model::decode(&bytes)?;
    for (mi, mesh) in model.meshes.iter().enumerate() {
        println!(
            "mesh[{mi}] '{}' index={} buffers={} prims={}",
            mesh.name,
            mesh.mesh_index,
            mesh.vertex_buffers.len(),
            mesh.primitives.len()
        );
        for (pi, prim) in mesh.primitives.iter().enumerate() {
            let vb = &mesh.vertex_buffers[prim.vertex_buffer];
            let mut verts = BTreeSet::new();
            for &idx in &prim.indices {
                verts.insert(idx as usize);
            }
            let (mut mn, mut mx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            let mut c = [0.0f64; 3];
            for &vi in &verts {
                let p = vb.positions[vi];
                for k in 0..3 {
                    mn[k] = mn[k].min(p[k]);
                    mx[k] = mx[k].max(p[k]);
                    c[k] += f64::from(p[k]);
                }
            }
            let n = verts.len().max(1) as f64;
            println!(
                "  prim[{pi:02}] vb={} idx={} verts={} mat={} bbox x[{:.2},{:.2}] y[{:.2},{:.2}] z[{:.2},{:.2}] centroid=({:.2},{:.2},{:.2})",
                prim.vertex_buffer,
                prim.indices.len(),
                verts.len(),
                prim.material,
                mn[0],
                mx[0],
                mn[1],
                mx[1],
                mn[2],
                mx[2],
                c[0] / n,
                c[1] / n,
                c[2] / n
            );
        }
    }
    Ok(())
}
