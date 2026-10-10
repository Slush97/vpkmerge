//! Build a hat-man->Wraith model that registers vanilla's embedded animation +
//! physics via a merged CTRL (the registry the engine uses to find embedded
//! blocks). Hat-man's mesh + DATA + RED2 are kept verbatim (render unchanged);
//! only the small CTRL is re-encoded.
//!
//! Final block layout:
//!   0 MVTX 1 MIDX 2 MVTX 3 MDAT   (hat-man mesh, verbatim)
//!   4 ANIM 5 ASEQ 6 AGRP 7 PHYS   (vanilla envelope, verbatim)
//!   8 CTRL                        (merged: hat-man meshes + vanilla anim/phys, re-pointed)
//!   9 RERL                        (vanilla, for the envelope's external refs)
//!   10 RED2 11 DATA               (hat-man, verbatim)
//!
//! Usage: build_camera_test <out> <hatman.vmdl_c> <vanilla.vmdl_c>
use morphic::kv3::{self, Value};
use morphic::resource::Resource;

fn align16(n: usize) -> usize {
    (n + 15) & !15
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let out = &a[1];
    let hb = std::fs::read(&a[2]).expect("read hatman");
    let vb = std::fs::read(&a[3]).expect("read vanilla");
    let hat = Resource::parse(&hb).expect("parse hatman");
    let van = Resource::parse(&vb).expect("parse vanilla");

    // Merged CTRL: vanilla's CTRL format, hat-man's embedded_meshes, vanilla's
    // embedded_animation/physics re-pointed to the new block indices.
    let van_ctrl_payload = van.find_block(*b"CTRL").expect("vanilla CTRL");
    let van_ctrl = kv3::decode(van_ctrl_payload).expect("decode vanilla CTRL");
    let hat_ctrl = kv3::decode(hat.find_block(*b"CTRL").expect("hatman CTRL")).expect("decode");
    let embedded_meshes = hat_ctrl.get("embedded_meshes").expect("meshes").clone();

    let new_ctrl = obj(vec![
        ("embedded_meshes", embedded_meshes),
        (
            "embedded_animation",
            obj(vec![
                ("group_data_block", Value::Int(6)),    // AGRP
                ("anim_data_block", Value::Int(4)),     // ANIM
                ("seqgroup_data_block", Value::Int(5)), // ASEQ
            ]),
        ),
        (
            "embedded_physics",
            obj(vec![("phys_data_block", Value::Int(7))]), // PHYS
        ),
    ]);
    let ctrl_format = kv3::Format::from_payload(van_ctrl_payload).expect("ctrl format");
    let new_ctrl_bytes = kv3::encode(&new_ctrl, &ctrl_format);
    // sanity: re-decode
    let rd = kv3::decode(&new_ctrl_bytes).expect("re-decode CTRL");
    assert!(rd.get("embedded_animation").is_some(), "anim survived");
    assert!(rd.get("embedded_physics").is_some(), "phys survived");
    // keep van_ctrl referenced (silence unused)
    let _ = van_ctrl.get("embedded_meshes");

    // Block list in final order.
    let blocks: Vec<([u8; 4], &[u8])> = vec![
        (*b"MVTX", hat.get_block_by_index(0).unwrap()),
        (*b"MIDX", hat.get_block_by_index(1).unwrap()),
        (*b"MVTX", hat.get_block_by_index(2).unwrap()),
        (*b"MDAT", hat.get_block_by_index(3).unwrap()),
        (*b"ANIM", van.find_block(*b"ANIM").unwrap()),
        (*b"ASEQ", van.find_block(*b"ASEQ").unwrap()),
        (*b"AGRP", van.find_block(*b"AGRP").unwrap()),
        (*b"PHYS", van.find_block(*b"PHYS").unwrap()),
        (*b"CTRL", &new_ctrl_bytes),
        (*b"RERL", van.find_block(*b"RERL").unwrap()),
        (*b"RED2", hat.find_block(*b"RED2").unwrap()),
        (*b"DATA", hat.find_block(*b"DATA").unwrap()),
    ];

    let resource_version = u16::from_le_bytes([hb[6], hb[7]]);
    let block_count = blocks.len();
    let table_len = block_count * 12;
    let mut cursor = align16(16 + table_len);
    let mut offs = Vec::new();
    for (_, p) in &blocks {
        offs.push(cursor);
        cursor = align16(cursor + p.len());
    }
    let total = cursor;
    let mut o = vec![0u8; total];
    o[0..4].copy_from_slice(&(u32::try_from(total).unwrap()).to_le_bytes());
    o[4..6].copy_from_slice(&12u16.to_le_bytes());
    o[6..8].copy_from_slice(&resource_version.to_le_bytes());
    o[8..12].copy_from_slice(&8u32.to_le_bytes());
    o[12..16].copy_from_slice(&(u32::try_from(block_count).unwrap()).to_le_bytes());
    for (i, ((kind, p), off)) in blocks.iter().zip(&offs).enumerate() {
        let e = 16 + i * 12;
        o[e..e + 4].copy_from_slice(kind);
        let ofp = e + 4;
        o[ofp..ofp + 4].copy_from_slice(&(u32::try_from(off - ofp).unwrap()).to_le_bytes());
        o[ofp + 4..ofp + 8].copy_from_slice(&(u32::try_from(p.len()).unwrap()).to_le_bytes());
        o[*off..*off + p.len()].copy_from_slice(p);
    }
    std::fs::write(out, &o).expect("write");
    println!("wrote {out} ({total} bytes, {block_count} blocks)");
    // validate
    let r2 = Resource::parse(&o).expect("reparse");
    let ctrl2 = kv3::decode(r2.find_block(*b"CTRL").unwrap()).expect("ctrl decode");
    println!(
        "CTRL keys: meshes={} anim={} phys={}",
        ctrl2
            .get("embedded_meshes")
            .and_then(Value::as_array)
            .map_or(0, <[_]>::len),
        ctrl2.get("embedded_animation").is_some(),
        ctrl2.get("embedded_physics").is_some(),
    );
}
