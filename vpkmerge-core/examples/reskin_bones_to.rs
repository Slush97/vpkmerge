//! Re-skin every influence lane on the named source bones to one target bone
//! (weights preserved), across all LOD0 vertex buffers of a loose `.vmdl_c`.
//! Generalizes `weld_mina_ears`: the rigid-lock fix for geometry whose bones
//! the runtime animates somewhere the geometry was not authored for (Baroness
//! Mina purse under vanilla purse IK) or never poses at all (dead FeModel
//! ctrl bones).
//!
//! Usage: reskin_bones_to <in.vmdl_c> <out.vmdl_c> <target_bone> <src_bone>...

use morphic::model::{decode, decode_skeleton, invert_remap, remap_table, reskin_vertex_buffer};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: reskin_bones_to <in.vmdl_c> <out.vmdl_c> <target_bone> <src_bone>...");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[1]).expect("read model");
    let target_name = &args[3];
    let src_names: Vec<&String> = args[4..].iter().collect();

    let sk = decode_skeleton(&bytes).expect("skeleton");
    let names: Vec<&str> = sk.bones.iter().map(|b| b.name.as_str()).collect();
    let bidx = |n: &str| -> u16 {
        u16::try_from(
            names
                .iter()
                .position(|x| *x == n)
                .unwrap_or_else(|| panic!("bone {n} not in skeleton")),
        )
        .expect("bone index fits u16")
    };
    let src_model: Vec<u16> = src_names.iter().map(|n| bidx(n)).collect();
    let target_model = bidx(target_name);

    let bo = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let bc = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let data_payload = (0..bc)
        .map(|i| {
            let e = 8 + bo + i * 12;
            let mut k = [0u8; 4];
            k.copy_from_slice(&bytes[e..e + 4]);
            let r = u32::from_le_bytes(bytes[e + 4..e + 8].try_into().unwrap()) as usize;
            let l = u32::from_le_bytes(bytes[e + 8..e + 12].try_into().unwrap()) as usize;
            (k, (e + 4) + r, l)
        })
        .find(|(k, _, _)| k == b"DATA")
        .map(|(_, off, len)| &bytes[off..off + len])
        .expect("DATA block");
    let data = morphic::kv3::decode(data_payload).expect("DATA kv3");
    let rt = remap_table(&data, 0).expect("mesh 0 remap");
    let inv = invert_remap(&rt);
    let local = |model_bone: u16| inv.get(&(model_bone as usize)).copied();
    let src_local: Vec<u16> = src_model.iter().filter_map(|&b| local(b)).collect();
    let target_local = local(target_model).expect("target bone not in mesh palette");
    println!(
        "reskin {} palette slot(s) -> {target_name} (slot {target_local}); {} of {} source bones in palette",
        src_local.len(),
        src_local.len(),
        src_model.len()
    );

    let m0 = decode(&bytes).expect("decode model");
    let targets = morphic::model::vertex_targets(&bytes).expect("vertex targets");
    let mesh0_blocks: Vec<usize> = targets
        .iter()
        .filter(|t| t.mesh_index == m0.meshes[0].mesh_index)
        .map(|t| t.block_index)
        .collect();

    let mut new_model = bytes.clone();
    let mut total = 0usize;
    for &block in &mesh0_blocks {
        let (reskinned, changed) = reskin_vertex_buffer(&new_model, block, |_i, _pos, mut j, w| {
            for k in 0..4 {
                if w[k] > 0.0 && src_local.contains(&j[k]) {
                    j[k] = target_local;
                }
            }
            j
        })
        .expect("reskin");
        new_model = reskinned;
        total += changed;
    }
    assert!(total > 0, "no verts rode the source bones");

    // Verify: no weight remains on the source bones, positions untouched.
    let mv = decode(&new_model).expect("decode result");
    for (bi, vb) in mv.meshes[0].vertex_buffers.iter().enumerate() {
        let old = &m0.meshes[0].vertex_buffers[bi];
        assert_eq!(
            old.positions, vb.positions,
            "positions moved in buffer {bi}"
        );
        for i in 0..vb.positions.len() {
            let (j4, w4) = (vb.joints[i], vb.weights[i]);
            for k in 0..4 {
                assert!(
                    !(w4[k] > 0.0 && src_model.contains(&j4[k])),
                    "vert {i} in buffer {bi} still rides a source bone"
                );
            }
        }
    }

    std::fs::write(&args[2], &new_model).expect("write out");
    println!(
        "wrote {} ({} bytes); {} verts re-skinned to {target_name}",
        args[2],
        new_model.len(),
        total
    );
}
