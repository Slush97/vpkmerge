//! Measure the scalp->hair gap along each scalp vertex's OUTWARD normal ray
//! (the quantity that governs poke-through), before and after the scalp fix.
//! Usage: probe_mina_clearance <old_dir.vpk> <new_dir.vpk>

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-9);
    [a[0] / l, a[1] / l, a[2] / l]
}

fn ray_tri(o: [f32; 3], d: [f32; 3], v0: [f32; 3], v1: [f32; 3], v2: [f32; 3]) -> Option<f32> {
    let e1 = sub(v1, v0);
    let e2 = sub(v2, v0);
    let h = cross(d, e2);
    let a = dot(e1, h);
    if a.abs() < 1e-9 {
        return None;
    }
    let f = 1.0 / a;
    let s = sub(o, v0);
    let u = f * dot(s, h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = cross(s, e1);
    let v = f * dot(d, q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * dot(e2, q);
    if t > 1e-6 {
        Some(t)
    } else {
        None
    }
}

fn analyze(label: &str, vpk_path: &str) {
    let bytes = valve_pak::open(std::path::Path::new(vpk_path))
        .unwrap()
        .get_file("models/heroes_wip/vampirebat/vampirebat.vmdl_c")
        .unwrap()
        .read_all()
        .unwrap();
    let m = morphic::model::decode(&bytes).unwrap();
    let mesh = &m.meshes[0];
    let vb = &mesh.vertex_buffers[0];
    let idx = |needle: &str| -> Vec<u32> {
        mesh.primitives
            .iter()
            .filter(|p| p.material.contains(needle) && p.vertex_buffer == 0)
            .flat_map(|p| p.indices.iter().copied())
            .collect()
    };
    let hair = idx("mina_hair");
    let head = idx("mina_head");
    let eyes = idx("mina_eyes");
    let cen = |v: &[u32]| {
        let mut c = [0.0f64; 3];
        for &i in v {
            for k in 0..3 {
                c[k] += f64::from(vb.positions[i as usize][k]);
            }
        }
        let n = v.len() as f64;
        [(c[0] / n) as f32, (c[1] / n) as f32, (c[2] / n) as f32]
    };
    let eye_c = cen(&eyes);
    let head_c = cen(&head);
    let mut fr = [eye_c[0] - head_c[0], eye_c[1] - head_c[1], 0.0];
    let l = (fr[0] * fr[0] + fr[1] * fr[1]).sqrt();
    fr[0] /= l;
    fr[1] /= l;
    let ef = (eye_c[0] - head_c[0]) * fr[0] + (eye_c[1] - head_c[1]) * fr[1];
    // per-vert outward normal from head winding
    let mut fnorm = vec![[0.0f32; 3]; vb.positions.len()];
    for t in head.chunks_exact(3) {
        let (a, b, c) = (
            vb.positions[t[0] as usize],
            vb.positions[t[1] as usize],
            vb.positions[t[2] as usize],
        );
        let n = cross(sub(b, a), sub(c, a));
        for &i in t {
            for k in 0..3 {
                fnorm[i as usize][k] += n[k];
            }
        }
    }
    let mut hv: Vec<u32> = head.clone();
    hv.sort_unstable();
    hv.dedup();
    let hair_v: Vec<[[f32; 3]; 3]> = hair
        .chunks_exact(3)
        .map(|t| {
            [
                vb.positions[t[0] as usize],
                vb.positions[t[1] as usize],
                vb.positions[t[2] as usize],
            ]
        })
        .collect();
    let mut gaps: Vec<(f32, u32)> = Vec::new();
    let mut no_cover = 0usize;
    for &vi in &hv {
        let p = vb.positions[vi as usize];
        if p[2] < eye_c[2] - 3.2 {
            continue;
        }
        if (p[0] - head_c[0]) * fr[0] + (p[1] - head_c[1]) * fr[1] > ef * 0.65 {
            continue;
        }
        let mut n = norm(fnorm[vi as usize]);
        if dot(n, sub(p, head_c)) < 0.0 {
            n = [-n[0], -n[1], -n[2]];
        }
        let mut best = f32::MAX;
        for tv in &hair_v {
            if let Some(t) = ray_tri(p, n, tv[0], tv[1], tv[2]) {
                if t < best {
                    best = t;
                }
            }
        }
        if best == f32::MAX {
            no_cover += 1;
        } else {
            gaps.push((best, vi));
        }
    }
    gaps.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mm = 25.4f32;
    println!("{label}: {} region verts with hair cover, {} without; outward-ray gap min {:.2}mm p5 {:.2}mm median {:.2}mm",
        gaps.len(), no_cover,
        gaps[0].0*mm, gaps[gaps.len()/20].0*mm, gaps[gaps.len()/2].0*mm);
    for (g, vi) in gaps.iter().take(5) {
        let p = vb.positions[*vi as usize];
        println!(
            "   gap {:.2}mm at ({:.1},{:.1},{:.1})",
            g * mm,
            p[0],
            p[1],
            p[2]
        );
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    analyze("BEFORE", &a[1]);
    analyze("AFTER ", &a[2]);
}
