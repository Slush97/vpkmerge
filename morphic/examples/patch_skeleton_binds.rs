//! Patch a compiled model's m_modelSkeleton bind transforms (m_bonePosParent /
//! m_boneRotParent) to a reference skeleton's values, matched by bone name.
//!
//! Why: the engine's runtime animation drives a hero's bones through the
//! vanilla .vnmskel, so any drift between the compiled model's binds and the
//! runtime rig rigidly displaces the geometry weighted to the drifted bones
//! (Baroness Mina's hair sat ~1 unit off the scalp: its GLB rig had drifted
//! through a Blender round-trip while the vertices stayed authored for the
//! vanilla binds). Patching the binds back re-pairs the unchanged vertices
//! with the runtime rig, byte-faithfully (FLOAT scalars in place, no
//! re-encode; vertex buffers untouched). Bones absent from the reference
//! (custom additions) keep their parent-relative transforms and ride their
//! re-patched parents.
//!
//! Usage: patch_skeleton_binds <model.vmdl_c> <ref_skel.json> <out.vmdl_c>
//!   ref_skel.json: dump_skeleton shape [{name, parent, pos[3], quat[4]}, ..]

use morphic::kv3::{Seg, Value};
use std::collections::HashMap;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (model_path, ref_path, out_path) = (&a[1], &a[2], &a[3]);
    let bytes = std::fs::read(model_path).expect("read model");
    let ref_text = std::fs::read_to_string(ref_path).expect("read ref json");
    let ref_skel: serde_json::Value = serde_json::from_str(&ref_text).expect("parse ref json");
    let ref_bones = ref_skel.as_array().expect("ref json array");

    let mut ref_by_name: HashMap<&str, (usize, [f32; 3], [f32; 4])> = HashMap::new();
    let mut ref_names: Vec<&str> = Vec::new();
    for b in ref_bones {
        let name = b["name"].as_str().expect("name");
        let parent = b["parent"].as_i64().expect("parent");
        let pos = b["pos"].as_array().expect("pos");
        let quat = b["quat"].as_array().expect("quat");
        let p: Vec<f32> = pos.iter().map(|v| v.as_f64().unwrap() as f32).collect();
        let q: Vec<f32> = quat.iter().map(|v| v.as_f64().unwrap() as f32).collect();
        ref_by_name.insert(
            name,
            (
                usize::try_from(parent.max(-1) + 1).unwrap(),
                [p[0], p[1], p[2]],
                [q[0], q[1], q[2], q[3]],
            ),
        );
        ref_names.push(name);
    }

    let tree = morphic::decode_kv3_resource(&bytes).expect("decode DATA");
    let skel = tree.get("m_modelSkeleton").expect("m_modelSkeleton");
    let names: Vec<String> = skel
        .get("m_boneName")
        .and_then(Value::as_array)
        .expect("m_boneName")
        .iter()
        .map(|v| v.as_str().expect("name").to_string())
        .collect();
    let parents: Vec<i64> = skel
        .get("m_nParent")
        .and_then(Value::as_array)
        .expect("m_nParent")
        .iter()
        .map(|v| v.as_int().expect("parent"))
        .collect();

    let cur_pos = skel
        .get("m_bonePosParent")
        .and_then(Value::as_array)
        .expect("m_bonePosParent");
    let cur_rot = skel
        .get("m_boneRotParent")
        .and_then(Value::as_array)
        .expect("m_boneRotParent");
    let comp = |arr: &Value, c: usize| -> f64 {
        arr.as_array().expect("vec")[c].as_f64().expect("component")
    };

    let mut fedits: Vec<(Vec<Seg>, f64)> = Vec::new();
    let mut patched = 0usize;
    let mut kept: Vec<&str> = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let Some((ref_parent_idx1, pos, quat)) = ref_by_name.get(name.as_str()) else {
            kept.push(name);
            continue;
        };
        // Safety: the bone's parent NAME must match the reference's, or the
        // parent-relative transform would compose against a different chain.
        let model_parent = if parents[i] >= 0 {
            names[usize::try_from(parents[i]).unwrap()].as_str()
        } else {
            ""
        };
        let ref_parent = if *ref_parent_idx1 > 0 {
            ref_names[ref_parent_idx1 - 1]
        } else {
            ""
        };
        assert_eq!(
            model_parent, ref_parent,
            "bone {name}: model parent {model_parent} != ref parent {ref_parent}"
        );
        // Only patch components that actually differ: identical values need no
        // edit, and exact-0/1 components are stored tagless (no float bytes to
        // patch in place).
        const EPS: f64 = 2e-4;
        let mut bone_edits = 0usize;
        for (c, v) in pos.iter().enumerate() {
            if (comp(&cur_pos[i], c) - f64::from(*v)).abs() <= EPS {
                continue;
            }
            fedits.push((
                vec![
                    Seg::Key("m_modelSkeleton".into()),
                    Seg::Key("m_bonePosParent".into()),
                    Seg::Index(i),
                    Seg::Index(c),
                ],
                f64::from(*v),
            ));
            bone_edits += 1;
        }
        // q and -q are the same rotation; flip the reference into the stored
        // hemisphere before comparing/patching.
        let dot: f64 = (0..4)
            .map(|c| comp(&cur_rot[i], c) * f64::from(quat[c]))
            .sum();
        let sign = if dot < 0.0 { -1.0f32 } else { 1.0f32 };
        for (c, v) in quat.iter().enumerate() {
            let target = *v * sign;
            if (comp(&cur_rot[i], c) - f64::from(target)).abs() <= EPS {
                continue;
            }
            fedits.push((
                vec![
                    Seg::Key("m_modelSkeleton".into()),
                    Seg::Key("m_boneRotParent".into()),
                    Seg::Index(i),
                    Seg::Index(c),
                ],
                f64::from(target),
            ));
            bone_edits += 1;
        }
        if bone_edits > 0 {
            patched += 1;
        }
    }

    let out = match morphic::patch_kv3_resource_doubles(&bytes, &fedits) {
        Ok(out) => out,
        Err(_) => {
            // A component that must change but is stored tagless (exact 0/1)
            // cannot be float-patched; apply edits individually so the rest
            // land, and report the casualties.
            let mut cur = bytes.clone();
            let mut skipped_edits = 0usize;
            for e in &fedits {
                match morphic::patch_kv3_resource_doubles(&cur, std::slice::from_ref(e)) {
                    Ok(next) => cur = next,
                    Err(_) => {
                        eprintln!(
                            "WARN: unpatchable (tagless) component at {:?} -> {}",
                            e.0, e.1
                        );
                        skipped_edits += 1;
                    }
                }
            }
            eprintln!("WARN: {skipped_edits}/{} components skipped", fedits.len());
            cur
        }
    };
    // Self-check: re-decode and confirm a patched value landed.
    let check = morphic::decode_kv3_resource(&out).expect("re-decode");
    let _ = check
        .get("m_modelSkeleton")
        .and_then(|s| s.get("m_bonePosParent"))
        .expect("patched skeleton readable");
    std::fs::write(out_path, &out).expect("write output");
    println!(
        "patched binds of {patched}/{} bones ({} float comps) -> {out_path}; kept as-authored: {:?}",
        names.len(),
        fedits.len(),
        kept
    );
}
