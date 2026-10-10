//! Inject the precomputed hero animgraph fields (m_animGraph2Refs +
//! m_vecNmSkeletonRefs) into a CSDK-compiled mesh-only hero .vmdl_c DATA block,
//! so the runtime NM animgraph binds to the swapped mesh.
//!
//! Usage: inject_animgraph <in.vmdl_c> <out.vmdl_c> <codename> <model_rel> [mode]
//! mode: both (default) | anim (only m_animGraph2Refs) | nm (only m_vecNmSkeletonRefs)
use morphic::kv3::{Seg, Value};

/// KV3 value flag byte for a resource-handle string (`CStrongHandle`).
const FLAG_RESOURCE: u8 = 1;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (inp, out, code, model_rel) = (&a[1], &a[2], &a[3], &a[4]);
    let mode = a.get(5).map(String::as_str).unwrap_or("both");
    let bytes = std::fs::read(inp).expect("read input");

    // m_hGraph is a CStrongHandle -> RESOURCE-flagged string; m_sIdentifier is
    // a plain string. m_vecNmSkeletonRefs elements are RESOURCE-flagged too.
    let animgraph = Value::Array(vec![
        Value::Object(vec![
            ("m_sIdentifier".into(), Value::String(String::new())),
            (
                "m_hGraph".into(),
                Value::String(format!(
                    "animgraphs/animgraph2/hero/hero.vnmgraph+{code}.vnmgraph"
                )),
            ),
        ]),
        Value::Object(vec![
            ("m_sIdentifier".into(), Value::String("UI".into())),
            (
                "m_hGraph".into(),
                Value::String(format!(
                    "animgraphs/animgraph2/hero/hero_ui.vnmgraph+{code}.vnmgraph"
                )),
            ),
        ]),
    ]);
    let animgraph_flags: Vec<(Vec<Seg>, u8)> = vec![
        (
            vec![Seg::Index(0), Seg::Key("m_hGraph".into())],
            FLAG_RESOURCE,
        ),
        (
            vec![Seg::Index(1), Seg::Key("m_hGraph".into())],
            FLAG_RESOURCE,
        ),
    ];
    let nmskel = Value::Array(vec![Value::String(format!("{model_rel}/{code}.vnmskel"))]);
    let nmskel_flags: Vec<(Vec<Seg>, u8)> = vec![(vec![Seg::Index(0)], FLAG_RESOURCE)];

    let mut cur = bytes;
    if mode == "both" || mode == "anim" {
        cur = morphic::patch_kv3_resource_object_insert(
            &cur,
            &[],
            "m_animGraph2Refs",
            &animgraph,
            &animgraph_flags,
        )
        .expect("inject m_animGraph2Refs");
    }
    if mode == "both" || mode == "nm" {
        cur = morphic::patch_kv3_resource_object_insert(
            &cur,
            &[],
            "m_vecNmSkeletonRefs",
            &nmskel,
            &nmskel_flags,
        )
        .expect("inject m_vecNmSkeletonRefs");
    }

    // Self-check: morphic must re-decode its own output.
    let tree = morphic::decode_kv3_resource(&cur).expect("re-decode output");
    std::fs::write(out, &cur).expect("write output");
    println!(
        "injected mode={mode} -> {out} ({} bytes), animGraph2Refs={} nmSkelRefs={}",
        cur.len(),
        tree.get("m_animGraph2Refs").is_some(),
        tree.get("m_vecNmSkeletonRefs").is_some(),
    );
}
