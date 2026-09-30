//! Splice a donor model's PHYS block (FeModel cloth + ragdoll bodies) into a
//! compiled model that ships none. PHYS references bones purely by name
//! (m_boneNames / m_pFeModel.m_CtrlName), so the splice is valid whenever the
//! target skeleton carries every referenced bone. Built for the Baroness Mina
//! combined mod: the hero-model-compiler output is mesh-only, so the kitty
//! source mod's ear/purse-charm cloth and the standard 22-body ragdoll never
//! made it into the recompile.
//!
//! NOT SUFFICIENT ALONE: the engine locates embedded physics through the CTRL
//! registry (`embedded_physics.phys_data_block`, a block-table index), not by
//! FOURCC scan. Run `register_embedded_phys` on the result, or cloth-flagged
//! bones get no transforms at all and any geometry weighted to them warps
//! (observed in-game on the v5 build's ears).
//!
//! Usage: splice_phys <target.vmdl_c> <donor.vmdl_c> <out.vmdl_c>
use morphic::kv3::Value;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 4 {
        eprintln!("usage: splice_phys <target.vmdl_c> <donor.vmdl_c> <out.vmdl_c>");
        std::process::exit(2);
    }
    let target = std::fs::read(&a[1]).expect("read target");
    let donor = std::fs::read(&a[2]).expect("read donor");

    let tres = morphic::resource::Resource::parse(&target).expect("parse target");
    assert!(
        tres.find_block(*b"PHYS").is_none(),
        "target already has a PHYS block"
    );
    let dres = morphic::resource::Resource::parse(&donor).expect("parse donor");
    let phys = dres.find_block(*b"PHYS").expect("donor has no PHYS block");

    // Every bone PHYS names must exist in the target skeleton.
    let tskel = morphic::model::decode_skeleton(&target).expect("target skeleton");
    let tnames: std::collections::HashSet<&str> =
        tskel.bones.iter().map(|b| b.name.as_str()).collect();
    let ptree = morphic::kv3::decode(phys).expect("PHYS kv3");
    let mut referenced: Vec<String> = Vec::new();
    if let Some(arr) = ptree.get("m_boneNames").and_then(Value::as_array) {
        referenced.extend(arr.iter().filter_map(|v| v.as_str()).map(str::to_owned));
    }
    if let Some(arr) = ptree
        .get("m_pFeModel")
        .and_then(|f| f.get("m_CtrlName"))
        .and_then(Value::as_array)
    {
        referenced.extend(arr.iter().filter_map(|v| v.as_str()).map(str::to_owned));
    }
    let missing: Vec<&String> = referenced
        .iter()
        .filter(|n| !tnames.contains(n.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "target skeleton is missing PHYS-referenced bones: {missing:?}"
    );
    println!(
        "PHYS references {} bones, all present in target ({} bones)",
        referenced.len(),
        tskel.bones.len()
    );

    let out = tres
        .rebuild_with_appended_block(*b"PHYS", phys)
        .expect("append PHYS");

    // Sanity: result parses, PHYS decodes, every prior block survived.
    let ores = morphic::resource::Resource::parse(&out).expect("parse result");
    assert_eq!(ores.blocks().len(), tres.blocks().len() + 1);
    for (i, b) in tres.blocks().iter().enumerate() {
        let ob = &ores.blocks()[i];
        assert_eq!(b.kind, ob.kind, "block {i} kind changed");
        let old = &target[b.offset as usize..(b.offset + b.size) as usize];
        let new = &out[ob.offset as usize..(ob.offset + ob.size) as usize];
        assert_eq!(old, new, "block {i} payload changed");
    }
    let ophys = ores.find_block(*b"PHYS").expect("PHYS in result");
    assert_eq!(ophys, phys, "PHYS payload changed");
    morphic::kv3::decode(ophys).expect("result PHYS decodes");

    std::fs::write(&a[3], &out).expect("write out");
    println!(
        "spliced PHYS ({} bytes): {} -> {} ({} bytes)",
        phys.len(),
        a[1],
        a[3],
        out.len()
    );
}
