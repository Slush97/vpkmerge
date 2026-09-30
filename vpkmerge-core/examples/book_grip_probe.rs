//! Report stock Abrams book geometry grouped by dominant skin bone.
//! Usage: book_grip_probe <pak01_dir.vpk>

use std::collections::BTreeMap;

use anyhow::{Context, Result};

const ENTRY: &str = "models/heroes_wip/abrams/abrams.vmdl_c";

#[derive(Default)]
struct Region {
    count: usize,
    sum: [f64; 3],
    lo: [f32; 3],
    hi: [f32; 3],
}

fn main() -> Result<()> {
    let pak = std::env::args()
        .nth(1)
        .context("usage: book_grip_probe <pak01_dir.vpk>")?;
    let bytes = vpkmerge_core::read_vpk_entry(pak, ENTRY)?;
    let model = morphic::model::decode(&bytes)?;
    let mesh = model
        .meshes
        .iter()
        .find(|mesh| mesh.name == "book_model")
        .context("book_model not found")?;
    let mut regions: BTreeMap<String, Region> = BTreeMap::new();
    for vb in &mesh.vertex_buffers {
        for (index, &position) in vb.positions.iter().enumerate() {
            let Some((lane, _)) = vb.weights[index]
                .iter()
                .copied()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(&b.1))
            else {
                continue;
            };
            let bone = usize::from(vb.joints[index][lane]);
            let name = model
                .skeleton
                .bones
                .get(bone)
                .map_or("<oob>", |bone| bone.name.as_str())
                .to_owned();
            let region = regions.entry(name).or_insert_with(|| Region {
                lo: [f32::INFINITY; 3],
                hi: [f32::NEG_INFINITY; 3],
                ..Region::default()
            });
            region.count += 1;
            for axis in 0..3 {
                region.sum[axis] += f64::from(position[axis]);
                region.lo[axis] = region.lo[axis].min(position[axis]);
                region.hi[axis] = region.hi[axis].max(position[axis]);
            }
        }
    }
    for (name, region) in regions {
        let center = region.sum.map(|value| value / region.count as f64);
        println!(
            "{name:<16} n={:<6} centroid={center:?} lo={:?} hi={:?}",
            region.count, region.lo, region.hi
        );
    }
    Ok(())
}
