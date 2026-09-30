//! Weld the Baroness Mina kitty ears rigidly onto `head`: re-skin every ear
//! bone influence lane to `head` (weights preserved), the ears-only subset of
//! `fix_mina_artifacts` (whose duster half is already applied in the v5+
//! lineage and whose assertion refuses to re-run). The v4-proven fix for
//! geometry riding FeModel ctrl bones the engine never poses (the kitty donor
//! FeModel is structurally dead, see docs/handoff-baroness-mina.md).
//!
//! Usage: weld_mina_ears <in.vmdl_c> <out.vmdl_c>

use morphic::model::{decode, decode_skeleton, invert_remap, remap_table, reskin_vertex_buffer};

const EARS: [&str; 4] = ["earL", "earL_001", "earR", "earR_001"];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: weld_mina_ears <in.vmdl_c> <out.vmdl_c>");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[1]).expect("read model");

    let sk = decode_skeleton(&bytes).expect("skeleton");
    let names: Vec<&str> = sk.bones.iter().map(|b| b.name.as_str()).collect();
    let bidx = |n: &str| -> u16 {
        u16::try_from(names.iter().position(|x| *x == n).expect("bone"))
            .expect("bone index fits u16")
    };
    let ear_model: Vec<u16> = EARS.iter().map(|n| bidx(n)).collect();
    let head_model = bidx("head");

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
    let ear_local: Vec<u16> = ear_model.iter().filter_map(|&b| local(b)).collect();
    let head_local = local(head_model).expect("head not in mesh palette");
    assert_eq!(ear_local.len(), 4, "ear bones missing from mesh palette");

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
                if w[k] > 0.0 && ear_local.contains(&j[k]) {
                    j[k] = head_local;
                }
            }
            j
        })
        .expect("reskin");
        new_model = reskinned;
        total += changed;
    }
    assert!(
        total > 100,
        "expected ~199 ear verts, re-skinned only {total}"
    );

    // Verify: no vertex carries weight on an ear bone anymore; positions and
    // everything else untouched (reskin only rewrites the joint lane).
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
                    !(w4[k] > 0.0 && ear_model.contains(&j4[k])),
                    "vert {i} in buffer {bi} still rides an ear bone"
                );
            }
        }
    }

    std::fs::write(&args[2], &new_model).expect("write out");
    println!(
        "wrote {} ({} bytes); {} ear verts welded to head",
        args[2],
        new_model.len(),
        total
    );
}
