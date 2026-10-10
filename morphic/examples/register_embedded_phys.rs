//! Register an embedded PHYS block in a model's CTRL registry. The engine
//! locates embedded physics through `CTRL.embedded_physics.phys_data_block`
//! (a block-table index), NOT by FOURCC scan, so a PHYS block spliced into the
//! block table (`splice_phys`) is invisible to it until registered here.
//! Byte-faithful member insert (`kv3::insert_object_member_adding`): the
//! existing CTRL lanes are carried byte-for-byte.
//!
//! Usage: register_embedded_phys <in.vmdl_c> <out.vmdl_c>
use morphic::kv3::Value;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 3 {
        eprintln!("usage: register_embedded_phys <in.vmdl_c> <out.vmdl_c>");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&a[1]).expect("read model");
    let res = morphic::resource::Resource::parse(&bytes).expect("parse");

    let phys_idx = res
        .blocks()
        .iter()
        .position(|b| &b.kind == b"PHYS")
        .expect("model has no PHYS block");
    let ctrl_idx = res
        .blocks()
        .iter()
        .position(|b| &b.kind == b"CTRL")
        .expect("model has no CTRL block");
    let ctrl = res.get_block_by_index(ctrl_idx).expect("CTRL payload");

    let tree = morphic::kv3::decode(ctrl).expect("CTRL kv3");
    assert!(
        tree.get("embedded_physics").is_none(),
        "CTRL already registers embedded_physics"
    );

    let value = Value::Object(vec![(
        "phys_data_block".into(),
        Value::Int(i64::try_from(phys_idx).unwrap()),
    )]);
    let new_ctrl =
        morphic::kv3::insert_object_member_adding(ctrl, &[], "embedded_physics", &value, &[])
            .expect("insert embedded_physics");

    // Verify the insert decodes and points at the PHYS block.
    let check = morphic::kv3::decode(&new_ctrl).expect("new CTRL decodes");
    let got = check
        .get("embedded_physics")
        .and_then(|o| o.get("phys_data_block"))
        .and_then(Value::as_int)
        .expect("embedded_physics readback");
    assert_eq!(got, i64::try_from(phys_idx).unwrap());

    let out = res
        .rebuild_with_block(ctrl_idx, &new_ctrl)
        .expect("rebuild with new CTRL");
    std::fs::write(&a[2], &out).expect("write out");
    println!(
        "registered embedded_physics.phys_data_block={phys_idx} in CTRL (block {ctrl_idx}): {} -> {}",
        a[1], a[2]
    );
}
