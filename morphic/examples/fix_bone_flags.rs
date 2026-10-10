//! Copy a reference model's per-bone `m_modelSkeleton.m_nFlag` onto a compiled
//! hero `.vmdl_c`, matched BY BONE NAME, in place (every other byte preserved).
//!
//! Why: a CSDK mesh-only hero compile (skeleton injected, no animation knowledge
//! at compile time) stamps every bone with a uniform deform flag
//! (`Mesh | VertexLod0..7` = 0x3fc80). A real hero compile gives each bone its
//! true flags: `Animation` (0x40, on ~all bones), `Attachment`, `Procedural`
//! (twist bones), `Cloth` (dishcloth / hair), `Physics`, `Hitbox`, and only the
//! vertex LODs the bone actually deforms. Those flags drive how the runtime
//! treats each bone; the wrong ones make the swap "animate but subtly wrong"
//! (twist bones don't twist, etc). This makes our flags byte-identical to the
//! known-good live model for every shared bone, guessing no semantics.
//!
//! Usage: fix_bone_flags <in.vmdl_c> <ref.vmdl_c> <out.vmdl_c>
//!   in  = our compiled model (flags to fix)
//!   ref = the live hero model (source of truth for flags), e.g. live inferno
//!   out = patched model
use morphic::kv3::{Seg, Value};

fn skeleton_flags(bytes: &[u8]) -> (Vec<String>, Vec<i64>) {
    let tree = morphic::decode_kv3_resource(bytes).expect("decode DATA");
    let skel = tree.get("m_modelSkeleton").expect("m_modelSkeleton");
    let names = skel
        .get("m_boneName")
        .and_then(Value::as_array)
        .expect("m_boneName")
        .iter()
        .map(|v| v.as_str().expect("bone name str").to_owned())
        .collect::<Vec<_>>();
    let flags = skel
        .get("m_nFlag")
        .and_then(Value::as_array)
        .expect("m_nFlag")
        .iter()
        .map(|v| {
            v.as_int()
                .or_else(|| v.as_uint().and_then(|u| i64::try_from(u).ok()))
                .expect("flag int")
        })
        .collect::<Vec<_>>();
    assert_eq!(names.len(), flags.len(), "skeleton arrays length mismatch");
    (names, flags)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 4 {
        eprintln!("usage: fix_bone_flags <in.vmdl_c> <ref.vmdl_c> <out.vmdl_c>");
        std::process::exit(2);
    }
    let (inp, refp, out) = (&a[1], &a[2], &a[3]);
    let in_bytes = std::fs::read(inp).expect("read in");
    let ref_bytes = std::fs::read(refp).expect("read ref");

    let (in_names, in_flags) = skeleton_flags(&in_bytes);
    let (ref_names, ref_flags) = skeleton_flags(&ref_bytes);
    let ref_map: std::collections::HashMap<&str, i64> = ref_names
        .iter()
        .map(String::as_str)
        .zip(ref_flags.iter().copied())
        .collect();

    let mut edits: Vec<(Vec<Seg>, i64)> = Vec::new();
    let mut missing = Vec::new();
    let mut unchanged = 0usize;
    for (i, name) in in_names.iter().enumerate() {
        match ref_map.get(name.as_str()) {
            Some(&want) => {
                if want == in_flags[i] {
                    unchanged += 1;
                } else {
                    edits.push((
                        vec![
                            Seg::Key("m_modelSkeleton".into()),
                            Seg::Key("m_nFlag".into()),
                            Seg::Index(i),
                        ],
                        want,
                    ));
                }
            }
            None => missing.push(name.clone()),
        }
    }
    if !missing.is_empty() {
        eprintln!(
            "WARN: {} bone(s) absent from ref, left unchanged: {:?}",
            missing.len(),
            missing
        );
    }
    println!(
        "bones={} edits={} unchanged={} missing-from-ref={}",
        in_names.len(),
        edits.len(),
        unchanged,
        missing.len()
    );

    let patched = morphic::patch_kv3_resource_scalars(&in_bytes, &edits).expect("patch m_nFlag");

    // Self-check: re-decode and confirm every shared bone now equals the ref flag.
    let (chk_names, chk_flags) = skeleton_flags(&patched);
    let mut bad = 0;
    for (name, &f) in chk_names.iter().zip(chk_flags.iter()) {
        if let Some(&want) = ref_map.get(name.as_str()) {
            if f != want {
                bad += 1;
                if bad <= 5 {
                    eprintln!("  MISMATCH {name}: got 0x{f:x} want 0x{want:x}");
                }
            }
        }
    }
    assert_eq!(bad, 0, "{bad} bone flags did not take");

    std::fs::write(out, &patched).expect("write out");
    println!(
        "wrote {out} ({} bytes, was {}); all shared bone flags now match ref",
        patched.len(),
        in_bytes.len()
    );
}
