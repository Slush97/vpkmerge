fn main() {
    let a: Vec<String> = std::env::args().collect();
    let old = valve_pak::open(std::path::Path::new(&a[1]))
        .unwrap()
        .get_file("models/heroes_wip/vampirebat/vampirebat.vmdl_c")
        .unwrap()
        .read_all()
        .unwrap();
    let new = valve_pak::open(std::path::Path::new(&a[2]))
        .unwrap()
        .get_file("models/heroes_wip/vampirebat/vampirebat.vmdl_c")
        .unwrap()
        .read_all()
        .unwrap();
    let mo = morphic::model::decode(&old).unwrap();
    let mn = morphic::model::decode(&new).unwrap();
    let vo = &mo.meshes[0].vertex_buffers[0];
    let vn = &mn.meshes[0].vertex_buffers[0];
    assert_eq!(vo.positions.len(), vn.positions.len());
    let mut moved = 0;
    let mut maxd = 0.0f32;
    for (a, b) in vo.positions.iter().zip(&vn.positions) {
        let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        if d > 1e-6 {
            moved += 1;
            if d > maxd {
                maxd = d;
            }
        }
    }
    println!("positions: {} moved, max {:.2} mm", moved, maxd * 25.4);
    assert_eq!(vo.normals, vn.normals, "normals changed!");
    assert_eq!(vo.texcoords, vn.texcoords, "uvs changed!");
    assert_eq!(vo.joints, vn.joints, "joints changed!");
    assert_eq!(vo.weights, vn.weights, "weights changed!");
    let po: Vec<_> = mo.meshes[0]
        .primitives
        .iter()
        .map(|p| (p.material.clone(), p.indices.len()))
        .collect();
    let pn: Vec<_> = mn.meshes[0]
        .primitives
        .iter()
        .map(|p| (p.material.clone(), p.indices.len()))
        .collect();
    assert_eq!(po, pn, "primitives changed!");
    assert_eq!(mo.skeleton.bones.len(), mn.skeleton.bones.len());
    // recompute the min scalp->hair clearance on the NEW model
    let mesh = &mn.meshes[0];
    let vb = vn;
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
    let tris: Vec<[f32; 3]> = hair
        .chunks_exact(3)
        .map(|t| {
            let (a, b, c) = (
                vb.positions[t[0] as usize],
                vb.positions[t[1] as usize],
                vb.positions[t[2] as usize],
            );
            [
                (a[0] + b[0] + c[0]) / 3.0,
                (a[1] + b[1] + c[1]) / 3.0,
                (a[2] + b[2] + c[2]) / 3.0,
            ]
        })
        .collect();
    let mut hv: Vec<u32> = head.clone();
    hv.sort_unstable();
    hv.dedup();
    let mut fr = [eye_c[0] - head_c[0], eye_c[1] - head_c[1], 0.0];
    let l = (fr[0] * fr[0] + fr[1] * fr[1]).sqrt();
    fr[0] /= l;
    fr[1] /= l;
    let ef = (eye_c[0] - head_c[0]) * fr[0] + (eye_c[1] - head_c[1]) * fr[1];
    let mut minc = f32::MAX;
    for &vi in &hv {
        let p = vb.positions[vi as usize];
        if p[2] < eye_c[2] - 3.2 {
            continue;
        }
        if (p[0] - head_c[0]) * fr[0] + (p[1] - head_c[1]) * fr[1] > ef * 0.65 {
            continue;
        }
        let mut d = f32::MAX;
        for h in &tris {
            let dd = ((p[0] - h[0]).powi(2) + (p[1] - h[1]).powi(2) + (p[2] - h[2]).powi(2)).sqrt();
            if dd < d {
                d = dd;
            }
        }
        if d < minc {
            minc = d;
        }
    }
    println!(
        "NEW min scalp clearance in fixed region: {:.2} mm",
        minc * 25.4
    );
    println!("ALL CHECKS PASSED");
}
