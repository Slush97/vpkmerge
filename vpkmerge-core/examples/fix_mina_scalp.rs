//! Fix the Baroness Mina scalp poke-through: the mina_head mesh carries a
//! complete bald skull whose clearance to the hair shell bottoms out at
//! ~0.5 mm on the upper back of the head, so runtime animation lets the scalp
//! punch through the hair and read as a pale bald spot in-game. The scalp is
//! never legitimately visible there (the hair is opaque), so the fix is to
//! pull the tight-clearance scalp vertices inward along their outward
//! normals, tapered so the dome stays smooth. Positions are spliced back via
//! `morphic::replace_vertex_positions` (native meshopt re-encode); every other
//! attribute, block, and VPK entry is byte-preserved.
//!
//! Usage: fix_mina_scalp <in_dir.vpk> <out_dir.vpk>

use std::path::Path;

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";
/// One millimetre in source units (inches).
const MM: f32 = 1.0 / 25.4;
/// Verts closer to the hair than this get pushed (taper reaches zero here).
const SELECT_CLEARANCE_MM: f32 = 6.0;
/// Push so the resulting clearance approaches this target.
const TARGET_CLEARANCE_MM: f32 = 6.0;

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn len(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = len(a).max(1e-9);
    [a[0] / l, a[1] / l, a[2] / l]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
/// Moller-Trumbore; returns the ray parameter t of the nearest forward hit.
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
    if t > -0.5 {
        Some(t.max(0.0))
    } else {
        None
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: fix_mina_scalp <in_dir.vpk> <out_dir.vpk>");
        std::process::exit(2);
    }
    let (in_vpk, out_vpk) = (&args[1], &args[2]);

    let vpk = valve_pak::open(Path::new(in_vpk)).expect("open vpk");
    let bytes = vpk
        .get_file(ENTRY)
        .expect("model entry")
        .read_all()
        .expect("read model");

    let model = morphic::model::decode(&bytes).expect("decode model");
    let mesh = &model.meshes[0];
    let vb = &mesh.vertex_buffers[0];
    println!(
        "mesh '{}': {} verts, {} normals",
        mesh.name,
        vb.positions.len(),
        vb.normals.len()
    );

    let prim_indices = |needle: &str| -> Vec<u32> {
        mesh.primitives
            .iter()
            .filter(|p| p.material.contains(needle) && p.vertex_buffer == 0)
            .flat_map(|p| p.indices.iter().copied())
            .collect()
    };
    let hair_idx = prim_indices("mina_hair");
    let head_idx = prim_indices("mina_head");
    let eye_idx = prim_indices("mina_eyes");
    println!(
        "indices: hair {} head {} eyes {}",
        hair_idx.len(),
        head_idx.len(),
        eye_idx.len()
    );
    assert!(!hair_idx.is_empty() && !head_idx.is_empty() && !eye_idx.is_empty());

    // Hair triangles (vertex triples): the coverage surface for ray tests.
    let hair_tris: Vec<[[f32; 3]; 3]> = hair_idx
        .chunks_exact(3)
        .map(|t| {
            [
                vb.positions[t[0] as usize],
                vb.positions[t[1] as usize],
                vb.positions[t[2] as usize],
            ]
        })
        .collect();

    // Region anchors, derived from the data so no axis convention is assumed:
    // the head-vert centroid and the eye centroid give "up" (z) and "front".
    let centroid = |idx: &[u32]| -> [f32; 3] {
        let mut c = [0.0f64; 3];
        for &i in idx {
            let p = vb.positions[i as usize];
            c[0] += f64::from(p[0]);
            c[1] += f64::from(p[1]);
            c[2] += f64::from(p[2]);
        }
        let n = idx.len() as f64;
        [(c[0] / n) as f32, (c[1] / n) as f32, (c[2] / n) as f32]
    };
    let eye_c = centroid(&eye_idx);
    let head_c = centroid(&head_idx);
    let mut front = sub(eye_c, head_c);
    front[2] = 0.0; // horizontal only; source models are Z-up
    let front = norm(front);
    let eye_frontness = dot(sub(eye_c, head_c), front);
    println!(
        "head centroid {head_c:?}  eye centroid {eye_c:?}  front {front:?}  eye frontness {eye_frontness:.2}"
    );

    // Upper-skull scalp region: above ear level, behind the face.
    let z_cut = eye_c[2] - 3.2; // ~8 cm below eye level
    let front_cut = eye_frontness * 0.65;

    let mut scalp_verts: Vec<u32> = head_idx.clone();
    scalp_verts.sort_unstable();
    scalp_verts.dedup();

    // Per-vertex outward normal accumulated from the head primitive's winding
    // (independent of the stored normals, which we leave untouched).
    let mut face_normal = vec![[0.0f32; 3]; vb.positions.len()];
    for t in head_idx.chunks_exact(3) {
        let (a, b, c) = (
            vb.positions[t[0] as usize],
            vb.positions[t[1] as usize],
            vb.positions[t[2] as usize],
        );
        let e1 = sub(b, a);
        let e2 = sub(c, a);
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        for &i in t {
            let f = &mut face_normal[i as usize];
            f[0] += n[0];
            f[1] += n[1];
            f[2] += n[2];
        }
    }

    let mut positions = vb.positions.clone();
    let select = SELECT_CLEARANCE_MM * MM;
    let target = TARGET_CLEARANCE_MM * MM;
    // Selection metric: the hair gap measured along the vertex's OUTWARD
    // normal ray (the quantity that governs poke-through). Several scalp
    // verts sit exactly ON the hair surface (gap 0.00 mm, interpenetrating),
    // which is what z-fights into the visible bald patch in-game.
    let mut moved_any: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut moved_bbox = ([f32::MAX; 3], [f32::MIN; 3]);
    let mut max_push = 0.0f32;
    let mut min_gap_seen = f32::MAX;
    for &vi in &scalp_verts {
        let p = vb.positions[vi as usize];
        if p[2] < z_cut || dot(sub(p, head_c), front) > front_cut {
            continue;
        }
        let n = norm(face_normal[vi as usize]);
        let outward = if dot(n, sub(p, head_c)) >= 0.0 {
            n
        } else {
            [-n[0], -n[1], -n[2]]
        };
        let mut gap = f32::MAX;
        for tv in &hair_tris {
            if let Some(t) = ray_tri(p, outward, tv[0], tv[1], tv[2]) {
                if t < gap {
                    gap = t;
                }
            }
        }
        if gap == f32::MAX {
            continue; // no hair above (hat-covered crown); leave it
        }
        min_gap_seen = min_gap_seen.min(gap);
        if gap >= select {
            continue;
        }
        let push = (target - gap).max(0.0);
        positions[vi as usize] = [
            p[0] - outward[0] * push,
            p[1] - outward[1] * push,
            p[2] - outward[2] * push,
        ];
        moved_any.insert(vi);
        max_push = max_push.max(push);
        for k in 0..3 {
            moved_bbox.0[k] = moved_bbox.0[k].min(p[k]);
            moved_bbox.1[k] = moved_bbox.1[k].max(p[k]);
        }
    }
    println!(
        "region min hair gap was {:.2} mm; pushed {} verts (max {:.2} mm)",
        min_gap_seen / MM,
        moved_any.len(),
        max_push / MM
    );
    let mut total_disp = 0.0f32;
    for &vi in &moved_any {
        total_disp = total_disp.max(len(sub(positions[vi as usize], vb.positions[vi as usize])));
    }
    println!(
        "moved {} scalp verts total, max total displacement {:.2} mm, bbox {:?} .. {:?} (source units)",
        moved_any.len(),
        total_disp / MM,
        moved_bbox.0,
        moved_bbox.1
    );
    assert!(
        moved_any.len() > 50,
        "selection collapsed; refusing to write"
    );
    assert!(
        total_disp < 25.0 * MM,
        "displacement ran away; refusing to write"
    );

    let new_model =
        morphic::model::replace_vertex_positions(&bytes, 0, &positions).expect("splice positions");
    println!(
        "model re-encoded: {} -> {} bytes",
        bytes.len(),
        new_model.len()
    );

    // Repack: every entry of the input VPK verbatim, with the model swapped.
    let mut paths: Vec<String> = vpk.file_paths().cloned().collect();
    paths.sort();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for p in &paths {
        let data = if p == ENTRY {
            new_model.clone()
        } else {
            vpk.get_file(p).expect("entry").read_all().expect("read")
        };
        files.push((p.clone(), data));
    }
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(p, d)| (p.as_str(), d.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, out_vpk).expect("pack vpk");
    println!("wrote {out_vpk} ({} entries)", refs.len());
}
