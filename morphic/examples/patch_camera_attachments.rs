//! Dump / retarget a compiled model's MDAT `CRenderMesh.m_attachments`.
//!
//! The hero over-the-shoulder camera is anchored by the MDAT mesh block's
//! attachments (portrait_camera / standing_pivot / crouching_pivot / near_00 /
//! far_00 / gunaim_00). Headless resourcecompiler drops AttachmentCameraData,
//! and an RC compile from a staged AttachmentList carries euler-rounded values,
//! so a hero-model build patches the exact f32 transforms back from a known-good
//! reference model (the mechanism proven in-game by meshswap_build.rs).
//!
//! Usage:
//!   patch_camera_attachments dump <model.vmdl_c> [--kv3 OUT.kv3]
//!       Print every MDAT attachment; with --kv3, write a ModelDoc
//!       AttachmentList node consumable via S2_ATTACHMENT_FILE
//!       (emit_skeleton_vmdl.py).
//!   patch_camera_attachments patch <target.vmdl_c> <ref.vmdl_c> <out.vmdl_c> [--all]
//!       Copy the camera attachments' m_vInfluenceRotations /
//!       m_vInfluenceOffsets f32 components from ref onto target, matched by
//!       attachment NAME (order-independent). --all retargets every attachment
//!       present in both. In-place set_floats; no re-encode.

use morphic::kv3::{decode, Seg, Value};
use morphic::resource::Resource;

const CAM: [&str; 6] = [
    "portrait_camera",
    "standing_pivot",
    "crouching_pivot",
    "near_00",
    "far_00",
    "gunaim_00",
];

fn attachments(tree: &Value) -> &[Value] {
    tree.get("m_attachments")
        .and_then(Value::as_array)
        .expect("MDAT has no m_attachments array")
}

fn vecf(v: &Value) -> Vec<f64> {
    v.as_array()
        .expect("vector")
        .iter()
        .map(|x| x.as_f64().expect("component"))
        .collect()
}

/// Source QuaternionMatrix -> QAngle (pitch, yaw, roll), mirroring
/// emit_skeleton_vmdl.quat_to_qangle.
fn quat_to_qangle(x: f64, y: f64, z: f64, w: f64) -> (f64, f64, f64) {
    let (fwd0, fwd1, fwd2) = (
        1.0 - 2.0 * y * y - 2.0 * z * z,
        2.0 * x * y + 2.0 * w * z,
        2.0 * x * z - 2.0 * w * y,
    );
    let (left0, left1, left2) = (
        2.0 * x * y - 2.0 * w * z,
        1.0 - 2.0 * x * x - 2.0 * z * z,
        2.0 * y * z + 2.0 * w * x,
    );
    let up2 = 1.0 - 2.0 * x * x - 2.0 * y * y;
    let xy = fwd0.hypot(fwd1);
    if xy > 1e-3 {
        (
            (-fwd2).atan2(xy).to_degrees(),
            fwd1.atan2(fwd0).to_degrees(),
            left2.atan2(up2).to_degrees(),
        )
    } else {
        (
            (-fwd2).atan2(xy).to_degrees(),
            (-left0).atan2(left1).to_degrees(),
            0.0,
        )
    }
}

struct Att {
    name: String,
    parent: String,
    offset: [f64; 3],
    quat: [f64; 4],
    ignore_rotation: bool,
    weight: f64,
    influences: i64,
}

fn read_atts(tree: &Value) -> Vec<Att> {
    attachments(tree)
        .iter()
        .map(|att| {
            let name = att
                .get("key")
                .and_then(Value::as_str)
                .expect("key")
                .to_string();
            let val = att.get("value").expect("value");
            let parent = val
                .get("m_influenceNames")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let off = vecf(
                &val.get("m_vInfluenceOffsets")
                    .and_then(Value::as_array)
                    .expect("offsets")[0],
            );
            let rot = vecf(
                &val.get("m_vInfluenceRotations")
                    .and_then(Value::as_array)
                    .expect("rotations")[0],
            );
            let ignore_rotation = val
                .get("m_bIgnoreRotation")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let weight = val
                .get("m_influenceWeights")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_f64)
                .unwrap_or(1.0);
            let influences = val
                .get("m_nInfluences")
                .and_then(Value::as_int)
                .unwrap_or(1);
            Att {
                name,
                parent,
                offset: [off[0], off[1], off[2]],
                quat: [rot[0], rot[1], rot[2], rot[3]],
                ignore_rotation,
                weight,
                influences,
            }
        })
        .collect()
}

fn fmt_f(v: f64) -> String {
    // Full float precision, no exponent, trailing-zero trimmed like ModelDoc text.
    let s = format!("{v:.6}");
    s.to_string()
}

fn dump(path: &str, kv3_out: Option<&str>) {
    let bytes = std::fs::read(path).expect("read model");
    let res = Resource::parse(&bytes).expect("parse model");
    let tree = decode(res.find_block(*b"MDAT").expect("MDAT block")).expect("decode MDAT");
    let atts = read_atts(&tree);
    println!("{} attachments in MDAT of {path}", atts.len());
    for a in &atts {
        let (p, y, r) = quat_to_qangle(a.quat[0], a.quat[1], a.quat[2], a.quat[3]);
        let cam = if CAM.contains(&a.name.as_str()) {
            "  [CAMERA]"
        } else {
            ""
        };
        println!(
            "  {:24} parent={:16} off=[{:9.4} {:9.4} {:9.4}] quat=[{:8.5} {:8.5} {:8.5} {:8.5}] ang=[{:8.3} {:8.3} {:8.3}]{}",
            a.name, a.parent, a.offset[0], a.offset[1], a.offset[2],
            a.quat[0], a.quat[1], a.quat[2], a.quat[3], p, y, r, cam
        );
        if a.influences > 1 {
            println!(
                "    WARN: {} influences; only influence 0 exported",
                a.influences
            );
        }
    }
    if let Some(out) = kv3_out {
        let mut s = String::new();
        s.push_str(
            "\t\t\t{\n\t\t\t\t_class = \"AttachmentList\"\n\t\t\t\tchildren = \n\t\t\t\t[\n",
        );
        for a in &atts {
            let (p, y, r) = quat_to_qangle(a.quat[0], a.quat[1], a.quat[2], a.quat[3]);
            s.push_str(&format!(
                "\t\t\t\t\t{{\n\t\t\t\t\t\t_class = \"Attachment\"\n\t\t\t\t\t\tname = \"{}\"\n\t\t\t\t\t\tignore_rotation = {}\n\t\t\t\t\t\tparent_bone = \"{}\"\n\t\t\t\t\t\trelative_origin = [ {}, {}, {} ]\n\t\t\t\t\t\trelative_angles = [ {}, {}, {} ]\n\t\t\t\t\t\tweight = {}\n\t\t\t\t\t}},\n",
                a.name,
                a.ignore_rotation,
                a.parent,
                fmt_f(a.offset[0]), fmt_f(a.offset[1]), fmt_f(a.offset[2]),
                fmt_f(p), fmt_f(y), fmt_f(r),
                fmt_f(a.weight),
            ));
        }
        s.push_str("\t\t\t\t]\n\t\t\t},");
        std::fs::write(out, &s).expect("write kv3");
        println!(
            "wrote AttachmentList node ({} attachments) to {out}",
            atts.len()
        );
    }
}

fn patch(target_path: &str, ref_path: &str, out_path: &str, all: bool) {
    let mut out = std::fs::read(target_path).expect("read target");
    let ref_bytes = std::fs::read(ref_path).expect("read ref");
    let ref_res = Resource::parse(&ref_bytes).expect("parse ref");
    let ref_tree =
        decode(ref_res.find_block(*b"MDAT").expect("ref MDAT")).expect("decode ref MDAT");
    let ref_by_name: std::collections::HashMap<&str, &Value> = attachments(&ref_tree)
        .iter()
        .map(|a| (a.get("key").and_then(Value::as_str).expect("key"), a))
        .collect();

    // Every LOD mesh carries its own copy of the attachments, so an RC build
    // with a LODGroupList needs each MDAT patched, not just LOD0's.
    let mdat_indices: Vec<usize> = Resource::parse(&out)
        .expect("parse target")
        .blocks()
        .iter()
        .enumerate()
        .filter(|(_, b)| &b.kind == b"MDAT")
        .map(|(i, _)| i)
        .collect();
    assert!(!mdat_indices.is_empty(), "model has no MDAT block");

    for mdat_idx in mdat_indices {
        let tgt_res = Resource::parse(&out).expect("parse target");
        let tgt_mdat_raw = tgt_res
            .get_block_by_index(mdat_idx)
            .expect("target MDAT bytes")
            .to_vec();
        let tgt_tree = decode(&tgt_mdat_raw).expect("decode target MDAT");
        let tgt_atts = attachments(&tgt_tree);

        let wanted: Vec<&str> = if all {
            tgt_atts
                .iter()
                .map(|a| a.get("key").and_then(Value::as_str).expect("key"))
                .filter(|n| ref_by_name.contains_key(n))
                .collect()
        } else {
            CAM.to_vec()
        };

        let mut fedits: Vec<(Vec<Seg>, f32)> = Vec::new();
        let mut patched_names: Vec<&str> = Vec::new();
        for (i, att) in tgt_atts.iter().enumerate() {
            let name = att.get("key").and_then(Value::as_str).expect("key");
            if !wanted.contains(&name) {
                continue;
            }
            let Some(ref_att) = ref_by_name.get(name) else {
                panic!("attachment {name} missing from ref model");
            };
            let ref_val = ref_att.get("value").expect("ref value");
            for field in ["m_vInfluenceRotations", "m_vInfluenceOffsets"] {
                let infs = ref_val.get(field).and_then(Value::as_array).expect(field);
                for (j, inf) in infs.iter().enumerate() {
                    for (c, comp) in inf.as_array().expect("inf vec").iter().enumerate() {
                        if let Value::Double(d) = comp {
                            fedits.push((
                                vec![
                                    Seg::Key("m_attachments".into()),
                                    Seg::Index(i),
                                    Seg::Key("value".into()),
                                    Seg::Key(field.into()),
                                    Seg::Index(j),
                                    Seg::Index(c),
                                ],
                                *d as f32,
                            ));
                        }
                    }
                }
            }
            patched_names.push(name);
        }
        if !all {
            for want in CAM {
                assert!(
                    patched_names.contains(&want),
                    "camera attachment {want} not found in target MDAT block {mdat_idx}; RC dropped it (use the ModelDoc GUI route or stage it via S2_ATTACHMENT_FILE)"
                );
            }
        }
        let patched_mdat =
            morphic::kv3::set_floats(&tgt_mdat_raw, &fedits).expect("set_floats on MDAT");
        let next = tgt_res
            .rebuild_with_block(mdat_idx, &patched_mdat)
            .expect("rebuild with patched MDAT");
        println!(
            "MDAT block {mdat_idx}: patched {} float comps across {} attachments ({:?})",
            fedits.len(),
            patched_names.len(),
            patched_names
        );
        out = next;
    }
    std::fs::write(out_path, &out).expect("write output");
    println!("-> {out_path}");
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("dump") => {
            let path = a.get(2).expect("dump <model.vmdl_c> [--kv3 OUT]");
            let kv3 = a
                .iter()
                .position(|s| s == "--kv3")
                .map(|i| a[i + 1].as_str());
            dump(path, kv3);
        }
        Some("patch") => {
            let (t, r, o) = (
                a.get(2).expect("target"),
                a.get(3).expect("ref"),
                a.get(4).expect("out"),
            );
            let all = a.iter().any(|s| s == "--all");
            patch(t, r, o, all);
        }
        _ => {
            eprintln!("usage: patch_camera_attachments dump <model.vmdl_c> [--kv3 OUT.kv3]");
            eprintln!("       patch_camera_attachments patch <target.vmdl_c> <ref.vmdl_c> <out.vmdl_c> [--all]");
            std::process::exit(2);
        }
    }
}
