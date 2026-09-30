//! Post-compile, in-place bone-ORDER fix for an RC-built hero model.
//!
//! RC orders compiled bones deform-first (pelvis@0) and shoves `root_motion` /
//! IK helpers to the back; every CS2WT-built shipping hero has `root_motion@0`.
//! The Deadlock third-person camera applies its side/back offset in the local
//! frame of bone index 0, so a model with the wrong bone[0] loads dead-center
//! (height still works: that offset is world-vertical). The NM animgraph binds
//! bones by NAME, so animation is unaffected either way.
//!
//! This permutes the compiled model's `m_modelSkeleton` per-bone arrays into the
//! donor's (vanilla) bone order and remaps every bone-index reference
//! (`m_nParent` values + `m_remappingTable` values) through the permutation.
//! Vertex blend indices are per-mesh-LOCAL and reach the global skeleton only via
//! `m_remappingTable`, so the mesh buffers (MVTX/MIDX/MDAT) are never touched.
//!
//! It does this purely with morphic's byte-faithful in-place patch primitives
//! (`patch_kv3_resource_{strings,scalars,doubles}`) -- the same path
//! `fix_bone_flags` uses (in-game proven) -- NOT a full `encode_kv3_resource`
//! (which is engine-invalid for model DATA).
//!
//! Usage: reorder_skeleton <in.vmdl_c> <donor.vmdl_c> <out.vmdl_c>

use morphic::kv3::{Seg, Value};
use std::collections::HashMap;

fn skel<'a>(tree: &'a Value) -> &'a Value {
    tree.get("m_modelSkeleton").expect("m_modelSkeleton")
}
fn arr<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{key} missing or not array"))
}
fn names(tree: &Value) -> Vec<String> {
    arr(skel(tree), "m_boneName")
        .iter()
        .map(|x| x.as_str().expect("bone name str").to_string())
        .collect()
}
fn key(parts: &[&str]) -> Vec<Seg> {
    parts.iter().map(|s| Seg::Key((*s).to_string())).collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (in_path, donor_path, out_path) = (&a[1], &a[2], &a[3]);

    let bytes = std::fs::read(in_path).expect("read in");
    let donor_bytes = std::fs::read(donor_path).expect("read donor");
    let tree = morphic::decode_kv3_resource(&bytes).expect("decode in");
    let donor = morphic::decode_kv3_resource(&donor_bytes).expect("decode donor");

    let our_names = names(&tree);
    let van_names = names(&donor);
    let n = our_names.len();
    assert_eq!(
        n,
        van_names.len(),
        "bone count mismatch {n} vs {}",
        van_names.len()
    );

    let our_idx: HashMap<&str, usize> = our_names
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    let van_idx: HashMap<&str, usize> = van_names
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    // Identical bone SETS (only order differs).
    for s in &our_names {
        assert!(
            van_idx.contains_key(s.as_str()),
            "our bone {s} absent from donor"
        );
    }
    for s in &van_names {
        assert!(
            our_idx.contains_key(s.as_str()),
            "donor bone {s} absent from ours"
        );
    }

    // perm[i] = new position of our bone i  (= its index in the donor order).
    // inv[j]  = our index of the bone that lands at new position j.
    let perm: Vec<usize> = our_names.iter().map(|s| van_idx[s.as_str()]).collect();
    let inv: Vec<usize> = van_names.iter().map(|s| our_idx[s.as_str()]).collect();
    let remap_val = |v: i64| -> i64 {
        if v < 0 {
            -1
        } else {
            perm[v as usize] as i64
        }
    };

    let s = skel(&tree);
    let our_parent: Vec<i64> = arr(s, "m_nParent")
        .iter()
        .map(|x| x.as_int().unwrap())
        .collect();
    let our_flag: Vec<u64> = arr(s, "m_nFlag")
        .iter()
        .map(|x| x.as_uint().unwrap())
        .collect();
    let our_scale: Vec<f64> = arr(s, "m_boneScaleParent")
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    let our_sphere: Vec<f64> = arr(s, "m_boneSphere")
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    let our_pos: Vec<Vec<f64>> = arr(s, "m_bonePosParent")
        .iter()
        .map(|e| {
            e.as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_f64().unwrap())
                .collect()
        })
        .collect();
    let our_rot: Vec<Vec<f64>> = arr(s, "m_boneRotParent")
        .iter()
        .map(|e| {
            e.as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_f64().unwrap())
                .collect()
        })
        .collect();
    let our_remap: Vec<i64> = arr(&tree, "m_remappingTable")
        .iter()
        .map(|x| x.as_int().unwrap())
        .collect();

    // ---- build edit batches (new position j holds our bone inv[j]) ----
    let bn = key(&["m_modelSkeleton", "m_boneName"]);
    let np = key(&["m_modelSkeleton", "m_nParent"]);
    let nf = key(&["m_modelSkeleton", "m_nFlag"]);
    let sc = key(&["m_modelSkeleton", "m_boneScaleParent"]);
    let sp = key(&["m_modelSkeleton", "m_boneSphere"]);
    let pp = key(&["m_modelSkeleton", "m_bonePosParent"]);
    let rp = key(&["m_modelSkeleton", "m_boneRotParent"]);

    let mut string_edits: Vec<(Vec<Seg>, String)> = Vec::new();
    let mut scalar_edits: Vec<(Vec<Seg>, i64)> = Vec::new();
    let mut double_edits: Vec<(Vec<Seg>, f64)> = Vec::new();
    let at = |base: &[Seg], idx: &[usize]| -> Vec<Seg> {
        let mut p = base.to_vec();
        for &i in idx {
            p.push(Seg::Index(i));
        }
        p
    };

    for j in 0..n {
        let src = inv[j];
        string_edits.push((at(&bn, &[j]), our_names[src].clone())); // == van_names[j]
        scalar_edits.push((at(&np, &[j]), remap_val(our_parent[src])));
        scalar_edits.push((at(&nf, &[j]), our_flag[src] as i64));
        double_edits.push((at(&sc, &[j]), our_scale[src]));
        double_edits.push((at(&sp, &[j]), our_sphere[src]));
        for c in 0..our_pos[src].len() {
            double_edits.push((at(&pp, &[j, c]), our_pos[src][c]));
        }
        for c in 0..our_rot[src].len() {
            double_edits.push((at(&rp, &[j, c]), our_rot[src][c]));
        }
    }
    // remapping table: remap VALUES (point each entry at its bone's new position).
    let rt = key(&["m_remappingTable"]);
    for (k, &v) in our_remap.iter().enumerate() {
        scalar_edits.push((at(&rt, &[k]), remap_val(v)));
    }

    println!(
        "edits: strings={} scalars={} doubles={}",
        string_edits.len(),
        scalar_edits.len(),
        double_edits.len()
    );

    // ---- apply in place (no full re-encode) ----
    let out = morphic::patch_kv3_resource_strings(&bytes, &string_edits).expect("string patch");
    let out = morphic::patch_kv3_resource_scalars(&out, &scalar_edits).expect("scalar patch");
    let out = morphic::patch_kv3_resource_doubles(&out, &double_edits).expect("double patch");

    // ---- verify ----
    let check = morphic::decode_kv3_resource(&out).expect("re-decode out");
    let new_names = names(&check);
    assert_eq!(new_names, van_names, "reordered skeleton != donor order");
    println!(
        "bone[0]={} (was {})  bone[1]={}",
        new_names[0], our_names[0], new_names[1]
    );

    // every bone's parent (by index) resolves to the same parent NAME as before.
    let cs = skel(&check);
    let new_parent: Vec<i64> = arr(cs, "m_nParent")
        .iter()
        .map(|x| x.as_int().unwrap())
        .collect();
    for j in 0..n {
        let src = inv[j];
        let old_pn = our_parent[src];
        let new_pn = new_parent[j];
        let old_name = if old_pn < 0 {
            "<root>"
        } else {
            our_names[old_pn as usize].as_str()
        };
        let new_name = if new_pn < 0 {
            "<root>"
        } else {
            new_names[new_pn as usize].as_str()
        };
        assert_eq!(
            old_name, new_name,
            "bone {} parent name changed",
            new_names[j]
        );
    }
    // remap entries resolve to the same bone NAMES as before.
    let new_remap: Vec<i64> = arr(&check, "m_remappingTable")
        .iter()
        .map(|x| x.as_int().unwrap())
        .collect();
    assert_eq!(new_remap.len(), our_remap.len(), "remap length changed");
    for k in 0..our_remap.len() {
        let before = our_names[our_remap[k] as usize].as_str();
        let after = new_names[new_remap[k] as usize].as_str();
        assert_eq!(before, after, "remap[{k}] resolves to wrong bone");
    }
    // pos/rot/flag/scale/sphere round-trip: new[j] == our[inv[j]] within tol.
    let nflag: Vec<u64> = arr(cs, "m_nFlag")
        .iter()
        .map(|x| x.as_uint().unwrap())
        .collect();
    let npos: Vec<Vec<f64>> = arr(cs, "m_bonePosParent")
        .iter()
        .map(|e| {
            e.as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_f64().unwrap())
                .collect()
        })
        .collect();
    let mut maxd = 0.0_f64;
    for j in 0..n {
        let src = inv[j];
        assert_eq!(nflag[j], our_flag[src], "flag mismatch at {j}");
        for c in 0..3 {
            maxd = maxd.max((npos[j][c] - our_pos[src][c]).abs());
        }
    }
    println!("verify OK: parents+remap resolve by name, max pos delta {maxd:.2e}");

    std::fs::write(out_path, &out).expect("write out");
    println!(
        "wrote {out_path} ({} bytes, in was {})",
        out.len(),
        bytes.len()
    );
}
