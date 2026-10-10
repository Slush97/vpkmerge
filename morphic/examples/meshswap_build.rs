//! Binary MESH-SWAP build (camera Path A): produce a wraith.vmdl_c that is
//! vanilla Wraith's COMPLETE compiled model (camera/attachments/anim/phys baked,
//! the thing headless RC can't reproduce) with ONLY its body mesh replaced by the
//! hat-man (gen_man_ghost) mesh. Keeps vanilla's whole DATA + envelope + RERL so
//! whatever gates the over-the-shoulder camera survives.
//!
//! All edits are block-swaps (rebuild_with_block) or in-place DATA patches
//! (set_scalars / array_insert) EXCEPT the CTRL block, which is rebuilt as a
//! hybrid (vanilla CTRL + hat-man's mesh-0 buffer metadata) and re-encoded. CTRL
//! re-encode validity is the one unproven step -- de-risked offline (VRF reads it)
//! and via the standalone probe_ctrl_reenc model; the in-game test is the gate.
//!
//! Usage: meshswap_build <vanilla.vmdl_c> <hatman.vmdl_c> <out.vmdl_c>
//!
//! Layout facts (re-derived fresh, both files):
//!   vanilla mesh-0 "wraith_model": data=14 vtx[0,2,5,8,11] idx[1,3,6,9,12]
//!   hat-man mesh-0 "wraith":       data=6  vtx[0,3]        idx[1,4] tools[2,5]
//!   We reuse vanilla slots: vtx{0,2} idx{1,3} data{14} take hat-man's content.
//!   vanilla remap mesh-0 slice = [0,421); hat-man remap = 425 -> grow +4.

use morphic::kv3::{decode, encode, Format, Seg, Value};
use morphic::resource::Resource;
use std::collections::HashMap;

// hat-man block indices for its mesh-0 buffers (within the hat-man file).
const HAT_VTX: [usize; 2] = [0, 3];
const HAT_IDX: [usize; 2] = [1, 4];
const HAT_MDAT: usize = 6;
// vanilla slots that receive them (reused mesh-0 slots).
const VAN_VTX: [usize; 2] = [0, 2];
const VAN_IDX: [usize; 2] = [1, 3];
const VAN_MDAT: usize = 14;

fn names_of(tree: &Value) -> Vec<String> {
    tree.get("m_modelSkeleton")
        .and_then(|s| s.get("m_boneName"))
        .and_then(Value::as_array)
        .expect("m_boneName")
        .iter()
        .map(|x| x.as_str().expect("name").to_string())
        .collect()
}

fn set_int(entry: &mut Value, key: &str, v: i64) {
    match entry.get_mut(key).expect(key) {
        Value::Int(x) => *x = v,
        Value::UInt(x) => *x = v as u64,
        other => panic!("{key} is {other:?}, not int"),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (van_path, hat_path, out_path) = (&a[1], &a[2], &a[3]);
    let van_bytes = std::fs::read(van_path).expect("read vanilla");
    let hat_bytes = std::fs::read(hat_path).expect("read hatman");

    // ---- decode DATA trees + skeletons for the remap ----
    let van_data = morphic::decode_kv3_resource(&van_bytes).expect("decode vanilla DATA");
    let hat_data = morphic::decode_kv3_resource(&hat_bytes).expect("decode hatman DATA");
    let van_names = names_of(&van_data);
    let hat_names = names_of(&hat_data);
    assert_eq!(van_names.len(), hat_names.len(), "bone count differs");
    let van_idx: HashMap<&str, usize> = van_names
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), i))
        .collect();
    for s in &hat_names {
        assert!(
            van_idx.contains_key(s.as_str()),
            "hatman bone {s} absent from vanilla"
        );
    }
    let hat_remap: Vec<i64> = hat_data
        .get("m_remappingTable")
        .and_then(Value::as_array)
        .expect("hat remap")
        .iter()
        .map(|x| x.as_int().unwrap())
        .collect();
    // hat-man-local -> vanilla-global, by bone NAME.
    let new_remap: Vec<i64> = hat_remap
        .iter()
        .map(|&v| {
            if v < 0 {
                -1
            } else {
                van_idx[hat_names[v as usize].as_str()] as i64
            }
        })
        .collect();
    let n_remap = new_remap.len();
    println!(
        "remap: hatman {} entries -> vanilla bone order (e.g. [0..4]={:?})",
        n_remap,
        &new_remap[..4.min(n_remap)]
    );

    let van_starts: Vec<i64> = van_data
        .get("m_remappingTableStarts")
        .and_then(Value::as_array)
        .expect("starts")
        .iter()
        .map(|x| x.as_int().unwrap())
        .collect();
    let van_total: usize = van_data
        .get("m_remappingTable")
        .and_then(Value::as_array)
        .unwrap()
        .len();
    let mesh0_slice = (van_starts[1] - van_starts[0]) as usize; // 421
    let grow = n_remap as i64 - mesh0_slice as i64; // +4
    println!("vanilla mesh-0 slice={mesh0_slice}, total={van_total}, grow by {grow}");
    assert!(grow >= 0, "hat-man remap smaller than slice; unexpected");

    // ---- Camera fix: the hero camera anchors live in the MESH block's
    // CRenderMesh.m_attachments (NOT the envelope), so swapping hat-man's MDAT in
    // also swaps the camera attachments. Vanilla's near_00/far_00/gunaim_00/pivots
    // carry the over-the-shoulder ROTATION + side OFFSET; hat-man's RC-compiled
    // ones point straight back (centered). Patch hat-man's MDAT camera attachments
    // back to vanilla's transforms (FLOAT-stored => set_floats; no re-encode).
    let hat_res = Resource::parse(&hat_bytes).expect("parse hatman");
    let van_res0 = Resource::parse(&van_bytes).expect("parse vanilla");
    let hat_mdat_raw = hat_res
        .get_block_by_index(HAT_MDAT)
        .expect("hat MDAT")
        .to_vec();
    let patched_mdat = if std::env::var("MESHSWAP_NO_CAM_FIX").is_ok() {
        println!("camera fix: SKIPPED (MESHSWAP_NO_CAM_FIX)");
        hat_mdat_raw.clone()
    } else {
        const CAM: [&str; 6] = [
            "portrait_camera",
            "standing_pivot",
            "crouching_pivot",
            "near_00",
            "far_00",
            "gunaim_00",
        ];
        let van_mdat = decode(van_res0.get_block_by_index(VAN_MDAT).expect("van MDAT"))
            .expect("decode van MDAT");
        let hat_mdat = decode(&hat_mdat_raw).expect("decode hat MDAT");
        let van_atts = van_mdat
            .get("m_attachments")
            .and_then(Value::as_array)
            .expect("van atts");
        let hat_atts = hat_mdat
            .get("m_attachments")
            .and_then(Value::as_array)
            .expect("hat atts");
        let mut fedits: Vec<(Vec<Seg>, f32)> = Vec::new();
        for (i, att) in van_atts.iter().enumerate() {
            let name = att.get("key").and_then(Value::as_str).unwrap_or("");
            if !CAM.contains(&name) {
                continue;
            }
            assert_eq!(
                hat_atts[i].get("key").and_then(Value::as_str),
                Some(name),
                "attachment order mismatch at {i}"
            );
            let val = att.get("value").expect("att value");
            for field in ["m_vInfluenceRotations", "m_vInfluenceOffsets"] {
                let infs = val.get(field).and_then(Value::as_array).expect(field);
                for (j, inf) in infs.iter().enumerate() {
                    for (c, comp) in inf.as_array().expect("inf vec").iter().enumerate() {
                        if let Value::Double(d) = comp {
                            fedits.push((
                                vec![
                                    Seg::Key("m_attachments".into()),
                                    Seg::Index(i),
                                    Seg::Key("value".into()),
                                    Seg::Key(field.into()),
                                    Seg::Index(j),
                                    Seg::Index(c),
                                ],
                                *d as f32,
                            ));
                        }
                    }
                }
            }
        }
        let out =
            morphic::kv3::set_floats(&hat_mdat_raw, &fedits).expect("patch MDAT cam attachments");
        println!(
            "camera fix: patched {} float comps across {} camera attachments",
            fedits.len(),
            CAM.len()
        );
        out
    };

    // ---- Steps 1-3: swap hat-man's mesh blocks into vanilla's mesh-0 slots ----
    let payload = |i: usize| hat_res.get_block_by_index(i).expect("hat block").to_vec();
    let swaps: [(usize, Vec<u8>); 5] = [
        (VAN_VTX[0], payload(HAT_VTX[0])),
        (VAN_VTX[1], payload(HAT_VTX[1])),
        (VAN_IDX[0], payload(HAT_IDX[0])),
        (VAN_IDX[1], payload(HAT_IDX[1])),
        (VAN_MDAT, patched_mdat.clone()),
    ];
    let mut out = van_bytes.clone();
    for (slot, p) in &swaps {
        let r = Resource::parse(&out).expect("re-parse for swap");
        out = r.rebuild_with_block(*slot, p).expect("rebuild_with_block");
    }
    println!(
        "steps 1-3: swapped 5 mesh blocks into vanilla slots {:?}",
        swaps.iter().map(|(s, _)| s).collect::<Vec<_>>()
    );

    // ---- Step 4: hybrid CTRL (vanilla CTRL + hat-man mesh-0 buffer metadata) ----
    let ctrl_idx = van_res0
        .blocks()
        .iter()
        .position(|b| b.kind == *b"CTRL")
        .expect("CTRL");
    let van_ctrl_payload = van_res0.get_block_by_index(ctrl_idx).expect("ctrl payload");
    let ctrl_fmt = Format::from_payload(van_ctrl_payload).expect("ctrl format");
    let mut ctrl_tree = decode(van_ctrl_payload).expect("decode vanilla CTRL");

    let hat_res_c = Resource::parse(&hat_bytes).expect("parse hatman ctrl");
    let hat_ctrl_idx = hat_res_c
        .blocks()
        .iter()
        .position(|b| b.kind == *b"CTRL")
        .unwrap();
    let hat_ctrl =
        decode(hat_res_c.get_block_by_index(hat_ctrl_idx).unwrap()).expect("decode hat CTRL");
    let mut hat_entry = hat_ctrl
        .get("embedded_meshes")
        .and_then(Value::as_array)
        .unwrap()[0]
        .clone();

    // repoint hat-man's mesh-0 entry at the reused vanilla slots.
    set_int(&mut hat_entry, "m_nDataBlock", VAN_MDAT as i64);
    if let Value::Array(vb) = hat_entry.get_mut("m_vertexBuffers").unwrap() {
        for (k, b) in vb.iter_mut().enumerate() {
            set_int(b, "m_nBlockIndex", VAN_VTX[k] as i64);
        }
    }
    if let Value::Array(ib) = hat_entry.get_mut("m_indexBuffers").unwrap() {
        for (k, b) in ib.iter_mut().enumerate() {
            set_int(b, "m_nBlockIndex", VAN_IDX[k] as i64);
        }
    }
    // drop tools buffers (editor-only; hat-man's tools blocks are not in the merged file).
    *hat_entry.get_mut("m_toolsBuffers").unwrap() = Value::Array(vec![]);

    // graft into vanilla's embedded_meshes[0]; keep meshes 1-8 + envelope refs as-is.
    if let Value::Array(em) = ctrl_tree.get_mut("embedded_meshes").unwrap() {
        em[0] = hat_entry;
    }
    let new_ctrl = encode(&ctrl_tree, &ctrl_fmt);
    {
        let r = Resource::parse(&out).expect("re-parse for ctrl");
        out = r
            .rebuild_with_block(ctrl_idx, &new_ctrl)
            .expect("rebuild ctrl");
    }
    println!(
        "step 4: rebuilt CTRL ({} -> {} bytes) with hat-man mesh-0 entry",
        van_ctrl_payload.len(),
        new_ctrl.len()
    );

    // ---- Step 6 (do inserts before scalar edits): grow mesh-0 remap slice ----
    let rt_path = vec![Seg::Key("m_remappingTable".into())];
    for _ in 0..grow {
        out = morphic::patch_kv3_resource_array_insert(
            &out,
            &rt_path,
            mesh0_slice, // insert at end of mesh-0 slice (index 421)
            // m_remappingTable is an ARRAY_TYPED of INT32 (subtype 11); the
            // inserted value's wire type must be INT32 too. value_wire_type maps
            // 256..=i32::MAX -> INT32 (0/1/2..=255 would become INT64_ZERO/ONE/
            // INT32_AS_BYTE and mismatch). Real values are written below.
            &Value::Int(256),
        )
        .expect("array insert into remap");
    }

    // ---- Step 6: DATA remap edits (mesh-0 slice = hat-man remap-by-name + starts) ----
    let skip_neutralize = std::env::var("MESHSWAP_SKIP_NEUTRALIZE").is_ok();
    let mut scalar_edits: Vec<(Vec<Seg>, i64)> = Vec::new();
    // When isolating: mesh-0 (hat-man) must render at EVERY LOD level, because the
    // vanilla LOD body meshes (3,6) have their draw calls zeroed below. Vanilla
    // gives mesh-0 only the LOD0 bit, so crossing into LOD1/2 range (camera pitch
    // changes the hero<->camera distance enough to do this) selects an empty LOD
    // mesh and the body vanishes. LOD bits LOD0=1/LOD1=2/LOD2=4 -> 7 = all three.
    if !skip_neutralize {
        scalar_edits.push((
            vec![Seg::Key("m_refLODGroupMasks".into()), Seg::Index(0)],
            7,
        ));
    }
    for (k, &v) in new_remap.iter().enumerate() {
        scalar_edits.push((vec![Seg::Key("m_remappingTable".into()), Seg::Index(k)], v));
    }
    // bump starts[1..] by `grow` so the later slices still line up.
    for i in 1..van_starts.len() {
        scalar_edits.push((
            vec![Seg::Key("m_remappingTableStarts".into()), Seg::Index(i)],
            van_starts[i] + grow,
        ));
    }
    out = morphic::patch_kv3_resource_scalars(&out, &scalar_edits).expect("scalar patch DATA");
    println!(
        "step 6: {} DATA remap scalar edits applied",
        scalar_edits.len()
    );

    // ---- Step 5 (proper): isolate hat-man by zeroing the DRAW CALLS of vanilla
    // meshes 1-8, NOT their mesh-group/LOD masks. Mask-zeroing gutted the LOD
    // system (body culled at distance) AND perturbs the camera-critical group
    // structure; draw-call zeroing leaves every mask 100% vanilla so the camera
    // attachments + culling keep working, while those meshes submit no primitives.
    // MESHSWAP_SKIP_NEUTRALIZE=1 leaves all 9 meshes rendering (vanilla parts float).
    // MDAT block index of each vanilla mesh 1-8 (from CTRL embedded_meshes; the
    // hat-man body is mesh-0 @ block 14 and is left rendering).
    const NEUTRALIZE_MDAT: [usize; 8] = [17, 20, 35, 38, 41, 56, 59, 62];
    if !skip_neutralize {
        let mut zeroed = 0usize;
        for &blk in &NEUTRALIZE_MDAT {
            let res = Resource::parse(&out).expect("re-parse for neutralize");
            let payload = res.get_block_by_index(blk).expect("mdat block").to_vec();
            let tree = decode(&payload).expect("decode mdat");
            let mut targets: Vec<(usize, usize)> = Vec::new();
            if let Some(sos) = tree.get("m_sceneObjects").and_then(Value::as_array) {
                for (so, sobj) in sos.iter().enumerate() {
                    let dc = sobj
                        .get("m_drawCalls")
                        .and_then(Value::as_array)
                        .map_or(0, <[Value]>::len);
                    for d in 0..dc {
                        targets.push((so, d));
                    }
                }
            }
            let neut = morphic::kv3::neutralize_draw_calls(&payload, &targets)
                .expect("neutralize draw calls");
            out = res
                .rebuild_with_block(blk, &neut)
                .expect("rebuild neutralized mdat");
            zeroed += targets.len();
        }
        println!(
            "step 5: zeroed {zeroed} draw calls across vanilla meshes 1-8 (masks left vanilla)"
        );
    } else {
        println!("step 5: SKIPPED (MESHSWAP_SKIP_NEUTRALIZE) -- vanilla meshes 1-8 still render");
    }

    // ---- validate ----
    let res = Resource::parse(&out).expect("re-parse merged");
    assert_eq!(
        res.blocks().len(),
        van_res0.blocks().len(),
        "block count changed"
    );
    let merged_data = morphic::decode_kv3_resource(&out).expect("decode merged DATA");
    let mr: Vec<i64> = merged_data
        .get("m_remappingTable")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|x| x.as_int().unwrap())
        .collect();
    assert_eq!(mr.len(), van_total + grow as usize, "remap total wrong");
    assert_eq!(
        &mr[..n_remap],
        &new_remap[..],
        "mesh-0 remap slice mismatch"
    );
    let merged_starts: Vec<i64> = merged_data
        .get("m_remappingTableStarts")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|x| x.as_int().unwrap())
        .collect();
    assert_eq!(merged_starts[0], 0);
    assert_eq!(
        merged_starts[1], n_remap as i64,
        "mesh-1 start should be {n_remap}"
    );
    // every mesh-0 remap entry resolves to a real vanilla bone.
    for (k, &v) in mr[..n_remap].iter().enumerate() {
        if v >= 0 {
            assert!(
                (v as usize) < van_names.len(),
                "remap[{k}]={v} out of range"
            );
        }
    }
    // CTRL re-decodes and mesh-0 now points at the reused slots with 2 buffers.
    let mc = decode(res.find_block(*b"CTRL").unwrap()).expect("decode merged CTRL");
    let em = mc.get("embedded_meshes").and_then(Value::as_array).unwrap();
    assert_eq!(em.len(), 9, "embedded_meshes count changed");
    let m0 = &em[0];
    assert_eq!(
        m0.get("m_nDataBlock").unwrap().as_int(),
        Some(VAN_MDAT as i64)
    );
    assert_eq!(
        m0.get("m_vertexBuffers")
            .and_then(Value::as_array)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        m0.get("m_indexBuffers")
            .and_then(Value::as_array)
            .unwrap()
            .len(),
        2
    );
    // envelope refs untouched.
    let anim = mc.get("embedded_animation").unwrap();
    assert_eq!(anim.get("anim_data_block").unwrap().as_int(), Some(64));
    assert_eq!(
        mc.get("embedded_physics")
            .unwrap()
            .get("phys_data_block")
            .unwrap()
            .as_int(),
        Some(67)
    );
    // mesh-group + LOD masks must stay EXACTLY vanilla (camera-critical structure).
    let masks: Vec<i64> = merged_data
        .get("m_refMeshGroupMasks")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|x| x.as_int().unwrap())
        .collect();
    assert_eq!(
        masks,
        vec![3, 1, 3, 3, 3, 1, 3, 3, 1],
        "mesh-group masks must stay vanilla"
    );
    // camera attachments must read vanilla's over-shoulder rotation (unless cam fix off).
    if std::env::var("MESHSWAP_NO_CAM_FIX").is_err() {
        let mdat = decode(res.find_block(*b"MDAT").unwrap()).expect("decode merged MDAT");
        let near = &mdat.get("m_attachments").and_then(Value::as_array).unwrap()[26];
        let r0 = near
            .get("value")
            .unwrap()
            .get("m_vInfluenceRotations")
            .and_then(Value::as_array)
            .unwrap()[0]
            .as_array()
            .unwrap()[0]
            .as_f64()
            .unwrap();
        assert!(
            (r0 - (-0.4999999701976776)).abs() < 1e-6,
            "near_00 rot not vanilla (cam fix failed): {r0}"
        );
    }

    std::fs::write(out_path, &out).expect("write");
    println!(
        "\nOK -> {out_path} ({} bytes, {} blocks)",
        out.len(),
        res.blocks().len()
    );
    println!(
        "  mesh-0 = hat-man (data=14, vtx{VAN_VTX:?}, idx{VAN_IDX:?}); meshes 1-8 neutralized"
    );
    println!(
        "  remap: mesh-0 slice {n_remap} entries, total {} ; starts[1]={}",
        mr.len(),
        merged_starts[1]
    );
    println!("  envelope (anim/phys/dstf) + RERL + DATA(skeleton/camera/attachments) = vanilla, untouched");
}
