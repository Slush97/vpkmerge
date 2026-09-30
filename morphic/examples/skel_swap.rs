//! Build a skeleton-order-consistent variant of an RC-built model: take the
//! mesh model's DATA tree, replace its m_modelSkeleton with the donor's (the
//! vanilla, CS2WT-built model that has the correct hero camera), and retarget
//! the mesh's m_remappingTable BY BONE NAME so the mesh stays bound to the same
//! bones, now expressed in the donor's bone-index order. Re-encodes only the
//! DATA block (MVTX/MIDX meshopt buffers are copied verbatim, never re-encoded).
//!
//! Hypothesis under test: the Deadlock third-person camera anchors to a bone by
//! INDEX; RC reorders bones (root_motion lands at index 7, not 0) vs CS2WT, so a
//! skeleton-order-matched build should restore the over-the-shoulder camera.
//!
//! Usage: skel_swap <mesh_model.vmdl_c> <donor.vmdl_c> <out.vmdl_c>
use morphic::kv3::Value;

fn bone_names(tree: &Value) -> Vec<String> {
    tree.get("m_modelSkeleton")
        .and_then(|s| s.get("m_boneName"))
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap_or("?").to_string())
        .collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mesh_path = &a[1];
    let donor_path = &a[2];
    let out_path = &a[3];

    let mesh_bytes = std::fs::read(mesh_path).expect("read mesh");
    let donor_bytes = std::fs::read(donor_path).expect("read donor");
    let mut mesh_tree = morphic::decode_kv3_resource(&mesh_bytes).expect("decode mesh");
    let donor_tree = morphic::decode_kv3_resource(&donor_bytes).expect("decode donor");

    let mesh_names = bone_names(&mesh_tree);
    let donor_names = bone_names(&donor_tree);
    assert_eq!(mesh_names.len(), donor_names.len(), "bone count mismatch");

    // donor name -> index
    let mut donor_idx = std::collections::HashMap::new();
    for (i, n) in donor_names.iter().enumerate() {
        donor_idx.insert(n.clone(), i as i64);
    }
    // every mesh bone must exist in donor
    for n in &mesh_names {
        assert!(
            donor_idx.contains_key(n),
            "mesh bone {n} not in donor skeleton"
        );
    }

    // Retarget the remapping table: each entry is a mesh-skeleton index; map it
    // through the bone name to the donor-skeleton index. Preserve Int/UInt variant.
    let old_remap: Vec<Value> = mesh_tree
        .get("m_remappingTable")
        .and_then(Value::as_array)
        .unwrap()
        .to_vec();
    let mut new_remap = Vec::with_capacity(old_remap.len());
    for v in &old_remap {
        let mesh_skel_idx = v.as_int().expect("remap int") as usize;
        let name = &mesh_names[mesh_skel_idx];
        let donor_skel_idx = donor_idx[name];
        // keep the original variant kind
        let nv = match v {
            Value::UInt(_) => Value::UInt(donor_skel_idx as u64),
            _ => Value::Int(donor_skel_idx),
        };
        new_remap.push(nv);
    }

    // Replace m_modelSkeleton with the donor's (full clone) and remap with retargeted.
    let donor_skel = donor_tree
        .get("m_modelSkeleton")
        .expect("donor skel")
        .clone();
    *mesh_tree
        .get_mut("m_modelSkeleton")
        .expect("mesh skel slot") = donor_skel;
    *mesh_tree.get_mut("m_remappingTable").expect("remap slot") = Value::Array(new_remap);

    // Re-encode (DATA only; mesh buffers copied verbatim).
    let out = morphic::encode_kv3_resource(&mesh_bytes, &mesh_tree).expect("encode");

    // Sanity: re-decode and verify the new skeleton is the donor's order and the
    // remap resolves to the same bone names as before.
    let check = morphic::decode_kv3_resource(&out).expect("re-decode");
    let check_names = bone_names(&check);
    assert_eq!(
        check_names, donor_names,
        "skeleton not swapped to donor order"
    );
    let check_remap = check
        .get("m_remappingTable")
        .and_then(Value::as_array)
        .unwrap();
    for (i, v) in check_remap.iter().enumerate() {
        let resolved = &check_names[v.as_int().unwrap() as usize];
        let original = &mesh_names[old_remap[i].as_int().unwrap() as usize];
        assert_eq!(resolved, original, "remap entry {i} resolves to wrong bone");
    }
    println!(
        "skel-swap OK: bones={} remap={} | mesh DATA {} -> out {} bytes",
        check_names.len(),
        check_remap.len(),
        mesh_bytes.len(),
        out.len()
    );
    println!("bone[0]={} bone[7]={}", check_names[0], check_names[7]);

    std::fs::write(out_path, &out).expect("write");
    println!("wrote {out_path}");
}
