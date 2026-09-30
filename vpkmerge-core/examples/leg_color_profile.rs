// Exact per-vertex COLOR profile of the buffer that paints Vindicta's skin, so
// the foot fix can be keyed on real values instead of a guess.
//
// Prints, for the vertex buffer carrying COLOR: every distinct color with its
// vertex count, z range, and dominant bones; then the same restricted to the
// foot bones. The point is to establish (a) what the dark foot value is, (b)
// whether that exact value is used anywhere except the foot, and (c) what the
// adjacent leg tone is, i.e. the colour the foot should become.
//
// usage: cargo run --example leg_color_profile -- <vpk> <hero-codename>

use std::collections::BTreeMap;

fn luma(c: [u8; 3]) -> f32 {
    (0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2])) / 255.0
}

const FOOT_BONES: &[&str] = &["ankle", "ball_", "toe", "heel", "foot"];

fn is_foot(b: &str) -> bool {
    let l = b.to_ascii_lowercase();
    FOOT_BONES.iter().any(|t| l.contains(t))
}

fn main() -> anyhow::Result<()> {
    let vpk_path = std::env::args().nth(1).expect("vpk");
    let codename = std::env::args().nth(2).expect("codename");
    let entry = vpkmerge_core::hero_model_entry(&vpk_path, None, &codename)?;
    let bytes = vpkmerge_core::read_vpk_entry(&vpk_path, &entry)?;
    let model = morphic::model::decode(&bytes)?;
    println!("model: {entry}\n");

    for mesh in &model.meshes {
        if mesh.name.contains("_lod") {
            continue;
        }
        for (bi, vb) in mesh.vertex_buffers.iter().enumerate() {
            let Some(colors) = vb.colors.first() else {
                continue;
            };
            if colors.len() != vb.element_count {
                continue;
            }
            println!(
                "mesh {} buffer {bi}: {} verts with COLOR",
                mesh.name, vb.element_count
            );

            // Per-vertex dominant bone name.
            let bone_of = |i: usize| -> String {
                let Some(j) = vb.joints.get(i) else {
                    return "?".into();
                };
                let Some(w) = vb.weights.get(i) else {
                    return "?".into();
                };
                let mut best = 0usize;
                let mut bw = -1.0f32;
                for k in 0..4 {
                    if w[k] > bw {
                        bw = w[k];
                        best = j[k] as usize;
                    }
                }
                model
                    .skeleton
                    .bones
                    .get(best)
                    .map_or_else(|| "?".into(), |b| b.name.clone())
            };

            // Tally distinct quantized colors.
            let mut tally: BTreeMap<[u8; 3], (usize, f32, f32, usize, BTreeMap<String, usize>)> =
                BTreeMap::new();
            for i in 0..vb.element_count {
                let c = colors[i];
                let k = [
                    (c[0] * 255.0).round() as u8,
                    (c[1] * 255.0).round() as u8,
                    (c[2] * 255.0).round() as u8,
                ];
                let z = vb.positions[i][2];
                let bone = bone_of(i);
                let e = tally.entry(k).or_insert((
                    0,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    0,
                    BTreeMap::new(),
                ));
                e.0 += 1;
                e.1 = e.1.min(z);
                e.2 = e.2.max(z);
                if is_foot(&bone) {
                    e.3 += 1;
                }
                *e.4.entry(bone).or_default() += 1;
            }

            // The dark plateau: colors carried by foot vertices.
            println!("\n  colors used by FOOT-bone vertices:");
            println!(
                "  {:>4} {:>4} {:>4}  {:>6}  {:>7}  {:>7}  {:>7}  {:>6}  bones",
                "R", "G", "B", "verts", "L", "z_min", "z_max", "foot"
            );
            let mut foot_rows: Vec<_> = tally.iter().filter(|(_, v)| v.3 > 0).collect();
            foot_rows.sort_by_key(|(_, v)| std::cmp::Reverse(v.0));
            for (k, (n, zlo, zhi, nfoot, bones)) in foot_rows.iter().take(12) {
                let mut bl: Vec<_> = bones.iter().collect();
                bl.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
                let names: Vec<String> = bl
                    .iter()
                    .take(5)
                    .map(|(n2, c)| format!("{n2}({c})"))
                    .collect();
                println!(
                    "  {:>4} {:>4} {:>4}  {n:>6}  {:>7.4}  {zlo:>7.2}  {zhi:>7.2}  {nfoot:>6}  {}",
                    k[0],
                    k[1],
                    k[2],
                    luma(**k),
                    names.join(" ")
                );
            }
            println!("  ({} distinct colors on foot verts)", foot_rows.len());

            // Non-foot use of those exact colors: tells us whether a
            // colour-keyed edit would spill outside the foot.
            let foot_keys: Vec<[u8; 3]> = foot_rows.iter().map(|(k, _)| **k).collect();
            let mut spill = 0usize;
            for k in &foot_keys {
                let v = &tally[k];
                spill += v.0 - v.3;
            }
            println!(
                "  vertices with a foot colour but a NON-foot bone: {spill}  \
                 (0 means a colour-keyed edit is exactly the foot)"
            );

            // The leg ramp: brightest colours and their z, to pick a target tone.
            println!("\n  z profile of leg/skin tone (mean colour per 2-unit z band, z < 44):");
            let mut bands: BTreeMap<i32, (f64, f64, f64, usize)> = BTreeMap::new();
            for i in 0..vb.element_count {
                let z = vb.positions[i][2];
                if z >= 44.0 {
                    continue;
                }
                let b = (z / 2.0).floor() as i32;
                let c = colors[i];
                let e = bands.entry(b).or_insert((0.0, 0.0, 0.0, 0));
                e.0 += f64::from(c[0]) * 255.0;
                e.1 += f64::from(c[1]) * 255.0;
                e.2 += f64::from(c[2]) * 255.0;
                e.3 += 1;
            }
            for (b, (r, g, bb, n)) in &bands {
                let n = *n as f64;
                let (r, g, bb) = (r / n, g / n, bb / n);
                let l = (0.2126 * r + 0.7152 * g + 0.0722 * bb) / 255.0;
                let bar = "#".repeat((l * 40.0).round() as usize);
                println!(
                    "    z {:>5.1}..{:<5.1} n={:<5} rgb=({:>5.1},{:>5.1},{:>5.1})  L={l:.4} {bar}",
                    f64::from(*b) * 2.0,
                    f64::from(*b) * 2.0 + 2.0,
                    n as usize,
                    r,
                    g,
                    bb
                );
            }
            println!();
        }
    }
    Ok(())
}
