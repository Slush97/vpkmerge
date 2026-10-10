// Print per-material UV bounds for a model's LOD0 primitives: the go/no-go
// check for whole-material flipbook animation (texcoord scale 1/G + stepped
// offset). UVs outside [0,1] wrap under normal sampling but bleed into the
// NEIGHBORING GRID CELL once the material's texcoords are scaled into a
// flipbook cell, so a material is only a clean flipbook target when its UVs
// stay inside the unit square.
//
// usage: cargo run --release -p vpkmerge-core --example uv_range -- \
//          <vpk> <entry.vmdl_c>
use std::collections::BTreeMap;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let vpk = a.next().expect("vpk path");
    let entry = a.next().expect("model entry");

    let bytes = vpkmerge_core::read_vpk_entry(&vpk, &entry)?;
    let model = morphic::model::decode(&bytes)?;

    // material -> (min_u, min_v, max_u, max_v, verts)
    let mut ranges: BTreeMap<String, (f32, f32, f32, f32, usize)> = BTreeMap::new();
    for mesh in &model.meshes {
        for prim in &mesh.primitives {
            let Some(vb) = mesh.vertex_buffers.get(prim.vertex_buffer) else {
                continue;
            };
            let Some(uvs) = vb.texcoords.first() else {
                continue;
            };
            let r = ranges.entry(prim.material.clone()).or_insert((
                f32::MAX,
                f32::MAX,
                f32::MIN,
                f32::MIN,
                0,
            ));
            for &i in &prim.indices {
                let Some(&[u, v]) = uvs.get(i as usize) else {
                    continue;
                };
                r.0 = r.0.min(u);
                r.1 = r.1.min(v);
                r.2 = r.2.max(u);
                r.3 = r.3.max(v);
                r.4 += 1;
            }
        }
    }

    println!("{entry}: UV layer 0 bounds per material (LOD0)");
    for (mat, (u0, v0, u1, v1, n)) in &ranges {
        let clean = *u0 >= 0.0 && *v0 >= 0.0 && *u1 <= 1.0 && *v1 <= 1.0;
        println!(
            "  {:<60} u [{u0:>7.3}, {u1:>7.3}]  v [{v0:>7.3}, {v1:>7.3}]  {} verts  {}",
            mat,
            n,
            if clean {
                "FLIPBOOK-CLEAN"
            } else {
                "OUT OF [0,1]"
            }
        );
    }
    Ok(())
}
