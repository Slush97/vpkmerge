//! Make the Baroness Mina bob rigid to the head: our compiled model ships no
//! PHYS block, so the FeModel cloth that drives the hair_0/hair_1 chains on
//! the source mod never runs, and head animation pushes the pale skull dome
//! through the crown (the in-game "bald spot"). Re-point every hair-chain
//! influence lane on the mina_hair draw call's verts to `head` (weights
//! preserved): the whole bob rides the head rigidly, so the skull can never
//! exit it. The kitty tail (tail_* bones) is left untouched.
//!
//! Usage: fix_mina_crown <in_dir.vpk> <out_dir.vpk>

use std::path::Path;

use morphic::model::{decode, decode_skeleton, invert_remap, remap_table, reskin_vertex_buffer};

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";
const HAIR_BONES: [&str; 9] = [
    "hair_0",
    "hair_0_L",
    "hair_0_R",
    "hair_1",
    "hair_1_L",
    "hair_1_R",
    "hair_end",
    "hair_end_L",
    "hair_end_R",
];

fn blocks(b: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let bo = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    let c = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
    let base = 8 + bo;
    (0..c)
        .map(|i| {
            let e = base + i * 12;
            let mut k = [0u8; 4];
            k.copy_from_slice(&b[e..e + 4]);
            let r = u32::from_le_bytes(b[e + 4..e + 8].try_into().unwrap()) as usize;
            let l = u32::from_le_bytes(b[e + 8..e + 12].try_into().unwrap()) as usize;
            (k, (e + 4) + r, l)
        })
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: fix_mina_crown <in_dir.vpk> <out_dir.vpk>");
        std::process::exit(2);
    }
    let vpk = valve_pak::open(Path::new(&args[1])).expect("open vpk");
    let bytes = vpk
        .get_file(ENTRY)
        .expect("model entry")
        .read_all()
        .expect("read model");

    let sk = decode_skeleton(&bytes).expect("skeleton");
    let names: Vec<&str> = sk.bones.iter().map(|b| b.name.as_str()).collect();
    let bidx = |n: &str| -> Option<usize> { names.iter().position(|x| *x == n) };
    let hair_model: Vec<u16> = HAIR_BONES
        .iter()
        .filter_map(|n| bidx(n).and_then(|i| u16::try_from(i).ok()))
        .collect();
    println!(
        "hair chain bones in skeleton: {}/{}",
        hair_model.len(),
        HAIR_BONES.len()
    );

    let (_, off, len) = blocks(&bytes)
        .into_iter()
        .find(|(k, _, _)| k == b"DATA")
        .expect("DATA block");
    let data = morphic::kv3::decode(&bytes[off..off + len]).expect("DATA kv3");
    let rt = remap_table(&data, 0).expect("mesh 0 remap");
    let inv = invert_remap(&rt);
    let hair_local: Vec<u16> = hair_model
        .iter()
        .filter_map(|&m| inv.get(&(m as usize)).copied())
        .collect();
    let head_local = *inv
        .get(&bidx("head").expect("head bone"))
        .expect("head not in palette");
    println!("palette: hair chain {hair_local:?} -> head {head_local}");

    // Restrict to verts of the mina_hair draw calls in buffer 0 (the bob +
    // kitty ears + tail live there; the tail uses tail_* bones, untouched).
    let m0 = decode(&bytes).expect("decode model");
    let mesh = &m0.meshes[0];
    let mut in_hair = vec![false; mesh.vertex_buffers[0].positions.len()];
    for p in &mesh.primitives {
        if p.vertex_buffer == 0 && p.material.contains("mina_hair") {
            for &i in &p.indices {
                in_hair[i as usize] = true;
            }
        }
    }
    println!(
        "hair draw call verts: {}",
        in_hair.iter().filter(|b| **b).count()
    );

    let (new_model, changed) = reskin_vertex_buffer(&bytes, 0, |i, _pos, mut j, w| {
        if !in_hair[i] {
            return j;
        }
        for k in 0..4 {
            if w[k] > 0.0 && hair_local.contains(&j[k]) {
                j[k] = head_local;
            }
        }
        j
    })
    .expect("reskin crown");
    println!("re-skinned {changed} bob verts onto head");
    assert!(changed > 1000, "crown selection unexpectedly small");

    // Verify: no hair-chain influence remains on hair draw call verts.
    let mv = decode(&new_model).expect("decode result");
    let vbn = &mv.meshes[0].vertex_buffers[0];
    let mut bad = 0usize;
    for (i, flag) in in_hair.iter().enumerate() {
        if !flag {
            continue;
        }
        for k in 0..4 {
            if vbn.weights[i][k] > 0.0 && hair_model.contains(&vbn.joints[i][k]) {
                bad += 1;
            }
        }
    }
    assert_eq!(bad, 0, "hair-chain influences remain");
    let vb0 = &m0.meshes[0].vertex_buffers[0];
    assert_eq!(vbn.positions, vb0.positions, "positions changed");
    assert_eq!(vbn.normals, vb0.normals, "normals changed");
    println!("verify: bob fully rigid to head, positions/normals untouched");

    let mut paths: Vec<String> = vpk.file_paths().cloned().collect();
    paths.sort();
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for p in &paths {
        let data = if p == ENTRY {
            new_model.clone()
        } else {
            vpk.get_file(p).expect("entry").read_all().expect("read")
        };
        files.push((p.clone(), data));
    }
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(p, d)| (p.as_str(), d.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &args[2]).expect("pack vpk");
    println!("wrote {} ({} entries)", args[2], refs.len());
}
