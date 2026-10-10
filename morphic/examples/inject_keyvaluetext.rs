//! Copy a reference hero model's `m_modelInfo.m_keyValueText` (the embedded
//! `vmdlkeys4` text blob) onto a compiled `.vmdl_c`, byte-faithfully.
//!
//! Why: that blob is where Deadlock stores the model's *runtime* procedural rig
//! data, all bone-referenced BY NAME:
//!   - `BoneConstraintList`  -> CTiltTwistConstraint (forearm/upper-arm/leg/neck
//!     TWIST bones), CAimConstraint, COrientConstraint, CPointConstraint
//!   - `ikdata` / `m_IKChains`, `FeetSettings`        -> arm/leg IK + foot plant
//!   - `LookAtList`                                   -> head/neck look-at
//!   - movement/camera/particle/sound/status settings
//! A CSDK mesh-only compile emits an almost-empty blob (just the animgraph hero
//! stub), so every twist/IK/look-at bone stays at bind -> the swap "animates but
//! subtly wrong" (warped forearms/fingers, head offset from neck). The reference
//! (live) hero blob is a superset of the stub (it carries the same
//! `CCitadelHeroModelGameData_t` animgraph refs), so a wholesale copy restores the
//! full rig and keeps the animgraph wiring. Binary `m_animGraph2Refs` /
//! `m_vecNmSkeletonRefs` (separate fields) are untouched.
//!
//! `patch_kv3_resource_strings_adding` grows the KV3 string table for the (much
//! larger) blob and redirects the field, preserving the skeleton's typed tags and
//! every other byte. No re-encode.
//!
//! Usage: inject_keyvaluetext <in.vmdl_c> <ref.vmdl_c> <out.vmdl_c>
use morphic::kv3::{Seg, Value};

fn key_value_text(bytes: &[u8]) -> String {
    let tree = morphic::decode_kv3_resource(bytes).expect("decode DATA");
    tree.get("m_modelInfo")
        .and_then(|m| m.get("m_keyValueText"))
        .and_then(Value::as_str)
        .expect("m_modelInfo.m_keyValueText")
        .to_owned()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 4 {
        eprintln!("usage: inject_keyvaluetext <in.vmdl_c> <ref.vmdl_c> <out.vmdl_c>");
        std::process::exit(2);
    }
    let (inp, refp, out) = (&a[1], &a[2], &a[3]);
    let in_bytes = std::fs::read(inp).expect("read in");
    let ref_bytes = std::fs::read(refp).expect("read ref");

    let in_kvt = key_value_text(&in_bytes);
    let ref_kvt = key_value_text(&ref_bytes);
    println!(
        "in m_keyValueText {} chars -> ref {} chars (BoneConstraintList={}, ikdata={}, LookAtList={})",
        in_kvt.len(),
        ref_kvt.len(),
        ref_kvt.contains("BoneConstraintList"),
        ref_kvt.contains("ikdata"),
        ref_kvt.contains("LookAtList"),
    );
    if in_kvt == ref_kvt {
        println!("already identical; copying through unchanged");
    }
    // Sanity: the reference must carry the animgraph hero data, or we'd be
    // dropping our own animgraph wiring by overwriting the stub.
    assert!(
        ref_kvt.contains("CCitadelHeroModelGameData_t"),
        "ref blob lacks CCitadelHeroModelGameData_t; wholesale copy would drop animgraph refs"
    );

    let edit = (
        vec![
            Seg::Key("m_modelInfo".into()),
            Seg::Key("m_keyValueText".into()),
        ],
        ref_kvt.clone(),
    );
    let patched = morphic::patch_kv3_resource_strings_adding(&in_bytes, &[edit])
        .expect("patch m_keyValueText");

    // Self-check: re-decode and confirm the field now equals the ref blob.
    let got = key_value_text(&patched);
    assert_eq!(got, ref_kvt, "m_keyValueText did not take");

    std::fs::write(out, &patched).expect("write out");
    println!(
        "wrote {out} ({} bytes, was {}); m_keyValueText now matches ref",
        patched.len(),
        in_bytes.len()
    );
}
