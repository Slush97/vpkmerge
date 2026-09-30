//! Swap the donor hero's PHYS payload (ragdoll bodies + joints, FeModel cloth)
//! into a compiled model whose own PHYS is a resourcecompiler stub.
//!
//! Headless RC emits and registers a PHYS block (`CTRL.embedded_physics`) as soon
//! as the source .vmdl carries any PhysicsShapeList, so build_hero_model.py's
//! `--physics-from-donor` stages a one-capsule stub and this replaces the stub's
//! bytes in place. Block count, order and the CTRL registry are untouched, which
//! is why this needs no CTRL edit (unlike appending a new PHYS block).
//!
//! PHYS references bones purely by name (m_boneNames / m_pFeModel.m_CtrlName),
//! so every referenced name that is a bone in the donor must exist in the
//! target skeleton. Take the donor from the same pak the skeleton was dumped
//! from, or cloth nodes added by a game update will be missing.
//!
//! Usage: replace_phys <target.vmdl_c> <donor.vmdl_c> <out.vmdl_c>
use morphic::kv3::Value;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 4 {
        eprintln!("usage: replace_phys <target.vmdl_c> <donor.vmdl_c> <out.vmdl_c>");
        std::process::exit(2);
    }
    let target = std::fs::read(&a[1]).expect("read target");
    let donor = std::fs::read(&a[2]).expect("read donor");

    let tres = morphic::resource::Resource::parse(&target).expect("parse target");
    let phys_idx = tres
        .blocks()
        .iter()
        .position(|b| &b.kind == b"PHYS")
        .expect("target has no PHYS block: compile with a PhysicsShapeList stub");
    let ctrl = morphic::kv3::decode(tres.find_block(*b"CTRL").expect("target has no CTRL"))
        .expect("CTRL kv3");
    let registered = ctrl
        .get("embedded_physics")
        .and_then(|p| p.get("phys_data_block"))
        .and_then(Value::as_int)
        .expect("CTRL has no embedded_physics.phys_data_block");
    assert_eq!(
        usize::try_from(registered).ok(),
        Some(phys_idx),
        "CTRL registers PHYS at block {registered}, but PHYS is block {phys_idx}"
    );

    let dres = morphic::resource::Resource::parse(&donor).expect("parse donor");
    let phys = dres.find_block(*b"PHYS").expect("donor has no PHYS block");

    let tskel = morphic::model::decode_skeleton(&target).expect("target skeleton");
    let tnames: std::collections::HashSet<&str> =
        tskel.bones.iter().map(|b| b.name.as_str()).collect();
    let ptree = morphic::kv3::decode(phys).expect("donor PHYS kv3");
    let mut referenced: Vec<String> = Vec::new();
    if let Some(arr) = ptree.get("m_boneNames").and_then(Value::as_array) {
        referenced.extend(arr.iter().filter_map(|v| v.as_str()).map(str::to_owned));
    }
    let bodies = referenced.len();
    if let Some(arr) = ptree
        .get("m_pFeModel")
        .and_then(|f| f.get("m_CtrlName"))
        .and_then(Value::as_array)
    {
        referenced.extend(arr.iter().filter_map(|v| v.as_str()).map(str::to_owned));
    }
    // Vanilla FeModels carry cloth ctrl nodes that are not skeleton bones in the
    // donor either (e.g. wraith's 27 bone-less $cloth_m0pN of 336), so only the
    // names the donor skeleton itself has must be present in the target.
    let dskel = morphic::model::decode_skeleton(&donor).expect("donor skeleton");
    let dnames: std::collections::HashSet<&str> =
        dskel.bones.iter().map(|b| b.name.as_str()).collect();
    let missing: Vec<&String> = referenced
        .iter()
        .filter(|n| dnames.contains(n.as_str()) && !tnames.contains(n.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "target skeleton is missing PHYS-referenced bones: {missing:?}"
    );

    let out = tres.rebuild_with_block(phys_idx, phys).expect("rebuild");

    let ores = morphic::resource::Resource::parse(&out).expect("parse result");
    assert_eq!(ores.blocks().len(), tres.blocks().len());
    for (i, b) in tres.blocks().iter().enumerate() {
        let ob = &ores.blocks()[i];
        assert_eq!(b.kind, ob.kind, "block {i} kind changed");
        if i == phys_idx {
            continue;
        }
        let old = &target[b.offset as usize..(b.offset + b.size) as usize];
        let new = &out[ob.offset as usize..(ob.offset + ob.size) as usize];
        assert_eq!(old, new, "block {i} payload changed");
    }
    assert_eq!(ores.get_block_by_index(phys_idx), Some(phys));

    std::fs::write(&a[3], &out).expect("write out");
    println!(
        "replaced PHYS block {phys_idx} with donor's ({} bytes; {bodies} bodies, {} cloth ctrls, all bones present): {}",
        phys.len(),
        referenced.len() - bodies,
        a[3]
    );
}
