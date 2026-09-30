//! Duster-only variant of `fix_mina_artifacts`: collapse the baroness
//! feather-duster prop (custom `umbrella` -> `feather1..4` -> `featherend`
//! bones the runtime never drives) to a degenerate point on `spine_2`, but
//! LEAVE THE KITTY EAR WEIGHTS ALONE. Used by the v5 rebuild that restores
//! the kitty source mod's PHYS block: with cloth spliced back in, the
//! `earL/earR(+_001)` bones ARE driven (FeModel ctrls, by name), so the ears
//! must keep their original influences instead of riding `head` rigidly.
//! Pair with an ear bind-transform restore (`patch_skeleton_binds`), since
//! the merged skeleton drifted the ear binds off the geometry.
//!
//! Everything else (normals, uvs, other lanes, other entries) is preserved.
//!
//! Usage: fix_mina_duster <in_dir.vpk> <out_dir.vpk>

use std::path::Path;

use morphic::model::{decode, decode_skeleton, invert_remap, remap_table, reskin_vertex_buffer};

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";
const DUSTER: [&str; 6] = [
    "umbrella",
    "feather1",
    "feather2",
    "feather3",
    "feather4",
    "featherend",
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
        eprintln!("usage: fix_mina_duster <in_dir.vpk> <out_dir.vpk>");
        std::process::exit(2);
    }
    let (in_vpk, out_vpk) = (&args[1], &args[2]);

    let vpk = valve_pak::open(Path::new(in_vpk)).expect("open vpk");
    let bytes = vpk
        .get_file(ENTRY)
        .expect("model entry")
        .read_all()
        .expect("read model");

    // Model-space bone indices.
    let sk = decode_skeleton(&bytes).expect("skeleton");
    let names: Vec<&str> = sk.bones.iter().map(|b| b.name.as_str()).collect();
    let bidx = |n: &str| -> Option<u16> {
        names
            .iter()
            .position(|x| *x == n)
            .and_then(|i| u16::try_from(i).ok())
    };
    let duster_model: Vec<u16> = DUSTER.iter().filter_map(|n| bidx(n)).collect();
    let spine2 = names
        .iter()
        .position(|x| *x == "spine_2")
        .expect("spine_2 bone");
    let collapse = [
        sk.bones[spine2].global_bind.m[12],
        sk.bones[spine2].global_bind.m[13],
        sk.bones[spine2].global_bind.m[14],
    ];
    println!("bones: duster {duster_model:?}; collapse at {collapse:?}");
    assert_eq!(duster_model.len(), 6, "duster chain missing from skeleton");

    // Mesh-local palette slots (buffer 0 = LOD0 body).
    let (_, off, len) = blocks(&bytes)
        .into_iter()
        .find(|(k, _, _)| k == b"DATA")
        .expect("DATA block");
    let data = morphic::kv3::decode(&bytes[off..off + len]).expect("DATA kv3");
    let rt = remap_table(&data, 0).expect("mesh 0 remap");
    let inv = invert_remap(&rt);
    let local = |model_bone: u16| inv.get(&(model_bone as usize)).copied();
    let duster_local: Vec<u16> = duster_model.iter().filter_map(|&m| local(m)).collect();
    let spine2_local = local(u16::try_from(spine2).unwrap()).expect("spine_2 not in mesh palette");
    println!("palette: duster {duster_local:?} -> spine_2 {spine2_local}");
    assert_eq!(
        duster_local.len(),
        6,
        "duster bones missing from mesh palette"
    );

    // Edit EVERY vertex buffer of the LOD0 mesh: the body (incl. the duster
    // handle + kitty ears) is buffer 0, and the duster's plume canopy is its
    // own small buffer. The same mesh palette applies to all of them.
    let m0 = decode(&bytes).expect("decode model");
    let targets = morphic::model::vertex_targets(&bytes).expect("vertex targets");
    let mesh0_blocks: Vec<usize> = targets
        .iter()
        .filter(|t| t.mesh_index == m0.meshes[0].mesh_index)
        .map(|t| t.block_index)
        .collect();
    assert_eq!(
        mesh0_blocks.len(),
        m0.meshes[0].vertex_buffers.len(),
        "buffer/block count mismatch"
    );

    let mut new_model = bytes.clone();
    let mut total_duster = 0usize;
    let mut total_reskinned = 0usize;
    let mut expected_positions: Vec<Vec<[f32; 3]>> = Vec::new();
    for (bi, &block) in mesh0_blocks.iter().enumerate() {
        let vb = &m0.meshes[0].vertex_buffers[bi];
        let nverts = vb.positions.len();
        let mut positions: Vec<[f32; 3]> = vb.positions.clone();
        let mut duster_verts = 0usize;
        for i in 0..nverts {
            let j4 = vb.joints[i];
            let w4 = vb.weights[i];
            if (0..4).any(|k| w4[k] > 0.0 && duster_model.contains(&j4[k])) {
                positions[i] = collapse;
                duster_verts += 1;
            }
        }
        total_duster += duster_verts;

        // 1) duster -> spine_2 (joints only, weights preserved). Duster verts
        // also collapse to one point below; riding a single driven bone keeps
        // every collapsed triangle degenerate under any animation.
        let (reskinned, changed) = reskin_vertex_buffer(&new_model, block, |_i, _pos, mut j, w| {
            for k in 0..4 {
                if w[k] > 0.0 && duster_local.contains(&j[k]) {
                    j[k] = spine2_local;
                }
            }
            j
        })
        .expect("reskin");
        total_reskinned += changed;

        // 2) duster collapse.
        new_model = morphic::model::replace_vertex_positions(&reskinned, block, &positions)
            .expect("collapse");
        println!(
            "buffer {bi} (block {block}): {nverts} verts, {duster_verts} collapsed, {changed} re-skinned"
        );
        expected_positions.push(positions);
    }
    println!(
        "total: {total_duster} duster verts collapsed, {total_reskinned} re-skinned; model {} -> {} bytes",
        bytes.len(),
        new_model.len()
    );
    assert!(total_duster > 1000, "duster selection unexpectedly small");

    // Verify on the result: nothing references duster or ear bones with weight,
    // duster verts all sit at the collapse point, everything else unmoved.
    let mv = decode(&new_model).expect("decode result");
    for (bi, expected) in expected_positions.iter().enumerate() {
        let vb = &m0.meshes[0].vertex_buffers[bi];
        let vbn = &mv.meshes[0].vertex_buffers[bi];
        let mut bad = 0usize;
        let mut moved_other = 0usize;
        for i in 0..expected.len() {
            let j4 = vbn.joints[i];
            let w4 = vbn.weights[i];
            for k in 0..4 {
                if w4[k] > 0.0 && duster_model.contains(&j4[k]) {
                    bad += 1;
                }
            }
            if expected[i] != vbn.positions[i] {
                moved_other += 1;
            }
        }
        assert_eq!(
            bad, 0,
            "buffer {bi}: verts still weighted to artifact bones"
        );
        assert_eq!(
            moved_other, 0,
            "buffer {bi}: positions drifted beyond the edit"
        );
        assert_eq!(vbn.normals, vb.normals, "buffer {bi}: normals changed");
        assert_eq!(vbn.texcoords, vb.texcoords, "buffer {bi}: uvs changed");
    }
    println!("verify: 0 artifact-bone influences, positions exact, normals/uvs preserved");

    // Repack: every entry of the input VPK verbatim, with the model swapped.
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
    vpkmerge_core::pack(&refs, out_vpk).expect("pack vpk");
    println!("wrote {out_vpk} ({} entries)", refs.len());
}
