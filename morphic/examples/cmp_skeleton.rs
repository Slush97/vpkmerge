//! Compare two models' m_modelSkeleton bone-by-bone (by name): parent name,
//! bind pos, bind rot. Tells whether skeletons differ meaningfully or are just
//! reordered.
//! Usage: cmp_skeleton <a.vmdl_c> <b.vmdl_c>
use morphic::kv3::Value;
use std::collections::BTreeMap;

#[derive(PartialEq, Debug)]
struct Bone {
    parent: String,
    pos: [f64; 3],
    rot: [f64; 4],
}

fn arr_f(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap_or(&[])
        .iter()
        .map(|x| x.as_f64().unwrap_or(f64::NAN))
        .collect()
}

fn load(path: &str) -> BTreeMap<String, Bone> {
    let bytes = std::fs::read(path).expect("read");
    let tree = morphic::decode_kv3_resource(&bytes).expect("decode");
    let sk = tree.get("m_modelSkeleton").expect("skel");
    let names: Vec<String> = sk
        .get("m_boneName")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap_or("?").to_string())
        .collect();
    let parents: Vec<i64> = sk
        .get("m_nParent")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_int().unwrap_or(-1))
        .collect();
    let pos = sk.get("m_bonePosParent").unwrap().as_array().unwrap();
    let rot = sk.get("m_boneRotParent").unwrap().as_array().unwrap();
    let mut m = BTreeMap::new();
    for i in 0..names.len() {
        let p = parents[i];
        let pn = if p < 0 || p as usize >= names.len() {
            "<root>".to_string()
        } else {
            names[p as usize].clone()
        };
        let pv = arr_f(&pos[i]);
        let rv = arr_f(&rot[i]);
        m.insert(
            names[i].clone(),
            Bone {
                parent: pn,
                pos: [pv[0], pv[1], pv[2]],
                rot: [rv[0], rv[1], rv[2], rv[3]],
            },
        );
    }
    m
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-4
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let sa = load(&a[1]);
    let sb = load(&a[2]);
    let only_a: Vec<_> = sa.keys().filter(|k| !sb.contains_key(*k)).collect();
    let only_b: Vec<_> = sb.keys().filter(|k| !sa.contains_key(*k)).collect();
    println!("A bones={} B bones={}", sa.len(), sb.len());
    println!("only in A ({}): {:?}", only_a.len(), only_a);
    println!("only in B ({}): {:?}", only_b.len(), only_b);
    let mut diff_parent = 0;
    let mut diff_pos = 0;
    let mut diff_rot = 0;
    let mut examples = 0;
    for (name, ba) in &sa {
        if let Some(bb) = sb.get(name) {
            let dp = ba.parent != bb.parent;
            let dpos = !ba.pos.iter().zip(bb.pos).all(|(x, y)| close(*x, y));
            let drot = !ba.rot.iter().zip(bb.rot).all(|(x, y)| close(*x, y));
            if dp {
                diff_parent += 1;
            }
            if dpos {
                diff_pos += 1;
            }
            if drot {
                diff_rot += 1;
            }
            if (dp || dpos || drot) && examples < 12 {
                examples += 1;
                println!(
                    "DIFF {name}: parent A={:?} B={:?} | posA={:?} posB={:?} | rotDiff={drot}",
                    ba.parent, bb.parent, ba.pos, bb.pos
                );
            }
        }
    }
    println!("common bones differing: parent={diff_parent} pos={diff_pos} rot={diff_rot}");
}
