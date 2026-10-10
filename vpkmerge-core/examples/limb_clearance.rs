// Measure a mesh part's surface radius around a bone's local X axis, in slices,
// to size a cloth collision capsule (FeModel m_TaperedCapsuleRigids) against the
// actual limb it should keep cloth off.
//
// usage: limb_clearance <vpk> <entry> <bone> <mesh-part> [x_min x_max slice]
use morphic::model::Vec3;
use vpkmerge_core::read_vpk_entry;

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let bytes = read_vpk_entry(&a[1], &a[2])?;
    let model = morphic::model::decode(&bytes)?;
    let bone = model
        .skeleton
        .bones
        .iter()
        .find(|b| b.name == a[3])
        .ok_or_else(|| anyhow::anyhow!("no bone {}", a[3]))?;
    let x_min: f32 = a.get(5).map_or(Ok(-4.0), |s| s.parse())?;
    let x_max: f32 = a.get(6).map_or(Ok(26.0), |s| s.parse())?;
    let slice: f32 = a.get(7).map_or(Ok(2.0), |s| s.parse())?;
    let bins = ((x_max - x_min) / slice).ceil() as usize;
    let mut radii: Vec<Vec<f32>> = vec![Vec::new(); bins];
    for mesh in model.meshes.iter().filter(|m| m.name == a[4]) {
        for vb in &mesh.vertex_buffers {
            for p in &vb.positions {
                let l = bone.inverse_bind.transform_point(Vec3 {
                    x: p[0],
                    y: p[1],
                    z: p[2],
                });
                if l.x < x_min || l.x >= x_max {
                    continue;
                }
                let r = (l.y * l.y + l.z * l.z).sqrt();
                if r > std::env::var("RMAX")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(15.0)
                {
                    continue;
                }
                radii[((l.x - x_min) / slice) as usize].push(r);
            }
        }
    }
    println!("{} around {} (local x slices of {slice})", a[4], a[3]);
    for (i, mut rs) in radii.into_iter().enumerate() {
        if rs.is_empty() {
            continue;
        }
        rs.sort_by(f32::total_cmp);
        let q = |f: f32| rs[((rs.len() - 1) as f32 * f) as usize];
        println!(
            "  x {:>5.1}..{:>5.1}  n={:>4}  r p50={:.2} p90={:.2} max={:.2}",
            x_min + i as f32 * slice,
            x_min + (i + 1) as f32 * slice,
            rs.len(),
            q(0.5),
            q(0.9),
            q(1.0)
        );
    }
    Ok(())
}
