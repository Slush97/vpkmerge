//! Print the bone names a model's per-bone distance fields (DSTF, the hero's
//! AO occluders) are parented to, one per line. DSTF stores each field's bone
//! only as a string token (`m_nParentBoneNameHash`), so names are recovered by
//! hashing the model's own skeleton. build_hero_model.py's
//! `--distance-fields-from-donor` uses this list to have resourcecompiler build
//! the same per-bone fields from the swapped mesh.
//!
//! Usage: dstf_bones <model.vmdl_c>
use morphic::kv3::Value;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: dstf_bones <model.vmdl_c>");
    let bytes = std::fs::read(&path).expect("read model");
    let res = morphic::resource::Resource::parse(&bytes).expect("parse");
    let Some(dstf) = res.find_block(*b"DSTF") else {
        eprintln!("{path}: no DSTF block");
        return;
    };
    let tree = morphic::kv3::decode(dstf).expect("DSTF kv3");
    let skel = morphic::model::decode_skeleton(&bytes).expect("skeleton");
    let by_token: std::collections::HashMap<u32, &str> = skel
        .bones
        .iter()
        .map(|b| (morphic::vfx_expr::attribute_token(&b.name), b.name.as_str()))
        .collect();
    let fields = tree
        .get("m_distanceFields")
        .and_then(Value::as_array)
        .unwrap_or_default();
    let mut unresolved = 0;
    for f in fields {
        let token = f
            .get("m_nParentBoneNameHash")
            .and_then(Value::as_uint)
            .and_then(|t| u32::try_from(t).ok());
        match token.and_then(|t| by_token.get(&t)) {
            Some(name) => println!("{name}"),
            None => unresolved += 1,
        }
    }
    if unresolved > 0 {
        eprintln!(
            "{unresolved} of {} fields have no matching bone",
            fields.len()
        );
    }
}
