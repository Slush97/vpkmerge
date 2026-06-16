//! Dump a hero model's `PHYS.m_pFeModel` (finite-element cloth sim) in a
//! cross-referenced, human-readable form: nodes <-> ctrl bones, rest poses,
//! masses, skeleton parents, rods (distance constraints), the collision BVH,
//! and the static/dynamic node bitmasks.
//!
//! Built to understand how Deadlock's cloth/jiggle simulation is encoded so we
//! can assess adding new dynamic bones (e.g. Holliday's static boot tassels).
//!
//! Usage: cargo run -p vpkmerge-core --example femodel_dump -- <pak.vpk> <entry.vmdl_c>

use morphic::kv3::Value;

fn blocks(b: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let bo = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    let c = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
    let base = 8 + bo;
    let mut v = vec![];
    for i in 0..c {
        let e = base + i * 12;
        let mut k = [0u8; 4];
        k.copy_from_slice(&b[e..e + 4]);
        let r = u32::from_le_bytes(b[e + 4..e + 8].try_into().unwrap()) as usize;
        let l = u32::from_le_bytes(b[e + 8..e + 12].try_into().unwrap()) as usize;
        v.push((k, (e + 4) + r, l));
    }
    v
}

fn arr(v: &Value, k: &str) -> Vec<Value> {
    v.get(k)
        .and_then(Value::as_array)
        .map(<[Value]>::to_vec)
        .unwrap_or_default()
}
fn u(v: &Value) -> u64 {
    v.as_uint().unwrap_or(0)
}
fn i(v: &Value) -> i64 {
    v.as_int().unwrap_or(0)
}

fn main() -> anyhow::Result<()> {
    let pak = std::env::args().nth(1).expect("pak");
    let entry = std::env::args().nth(2).expect("entry");
    let b = vpkmerge_core::read_vpk_entry(&pak, &entry)?;
    let (_, off, len) = blocks(&b)
        .into_iter()
        .find(|(k, _, _)| k == b"PHYS")
        .ok_or_else(|| anyhow::anyhow!("no PHYS"))?;
    let phys = morphic::kv3::decode(&b[off..off + len]).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let fe = phys
        .get("m_pFeModel")
        .ok_or_else(|| anyhow::anyhow!("no m_pFeModel"))?;

    println!("== {entry} ==");
    for k in [
        "m_nNodeCount",
        "m_nStaticNodes",
        "m_nRotLockStaticNodes",
        "m_nFirstPositionDrivenNode",
        "m_nTreeDepth",
        "m_nStaticNodeFlags",
        "m_nDynamicNodeFlags",
        "m_nNodeBaseJiggleboneDependsCount",
        "m_nRopeCount",
        "m_nSimdQuadCount1",
        "m_nSimdQuadCount2",
        "m_nQuadCount1",
        "m_nQuadCount2",
        "m_nSimdTriCount1",
        "m_nSimdTriCount2",
        "m_nTriCount1",
        "m_nTriCount2",
    ] {
        if let Some(x) = fe.get(k) {
            println!("  {k} = {x:?}");
        }
    }

    let ctrl_name = arr(fe, "m_CtrlName");
    let ctrl_hash = arr(fe, "m_CtrlHash");
    let inv_mass = arr(fe, "m_NodeInvMasses");
    let skel_par = arr(fe, "m_SkelParents");
    let init_pose = arr(fe, "m_InitPose");
    let free = arr(fe, "m_FreeNodes").iter().map(u).collect::<Vec<_>>();
    let coll_radii = arr(fe, "m_NodeCollisionRadii");
    let n = ctrl_name.len();

    println!("\n-- nodes (n={n}) -- [F]=free/simulated  invMass  skelParent  ctrlBone");
    for idx in 0..n {
        let name = ctrl_name.get(idx).and_then(Value::as_str).unwrap_or("?");
        let hash = ctrl_hash.get(idx).map(u).unwrap_or(0);
        let im = inv_mass
            .get(idx)
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        let sp = skel_par.get(idx).map(i).unwrap_or(-99);
        let isfree = free.contains(&(idx as u64));
        // init pose: array of 12 (3x4) or a struct; print translation if array
        let pose_t = init_pose
            .get(idx)
            .and_then(Value::as_array)
            .map(|a| {
                let g = |j: usize| a.get(j).and_then(Value::as_f64).unwrap_or(0.0);
                // could be vec of doubles len 12 (matrix) -> translation at 3,7,11; or struct
                if a.len() >= 12 {
                    format!("t=({:.1},{:.1},{:.1})", g(3), g(7), g(11))
                } else {
                    format!("[{} elems]", a.len())
                }
            })
            .unwrap_or_else(|| "?".into());
        let cr = coll_radii.iter().nth(idx).and_then(Value::as_f64);
        println!(
            "  [{idx:2}] {}{:<22} invM={im:>7.3} skelPar={sp:>3} hash={hash:<11} {pose_t}{}",
            if isfree { "F " } else { "  " },
            name,
            cr.map(|r| format!(" cr={r:.2}")).unwrap_or_default()
        );
    }

    let rods = arr(fe, "m_Rods");
    println!("\n-- m_Rods (n={}) node pairs --", rods.len());
    for (ri, rod) in rods.iter().enumerate() {
        let np = arr(rod, "nNode");
        let a = np.first().map(u).unwrap_or(0);
        let c = np.get(1).map(u).unwrap_or(0);
        let extra: Vec<String> = rod
            .as_object()
            .unwrap_or(&[])
            .iter()
            .filter(|(k, _)| k != "nNode")
            .map(|(k, v)| format!("{k}={v:?}"))
            .collect();
        let na = ctrl_name
            .get(a as usize)
            .and_then(Value::as_str)
            .unwrap_or("?");
        let nc = ctrl_name
            .get(c as usize)
            .and_then(Value::as_str)
            .unwrap_or("?");
        println!("  rod[{ri:2}] {a}({na}) <-> {c}({nc})  {}", extra.join(" "));
    }

    let bases = arr(fe, "m_NodeBases");
    println!("\n-- m_NodeBases (n={}) --", bases.len());
    for (bi, nb) in bases.iter().enumerate() {
        let flat: Vec<String> = nb
            .as_object()
            .unwrap_or(&[])
            .iter()
            .filter(|(k, _)| k != "qAdjust" && k != "nDummy")
            .map(|(k, v)| format!("{k}={v:?}"))
            .collect();
        println!("  base[{bi}] {}", flat.join(" "));
    }

    let tp = arr(fe, "m_TreeParents").iter().map(i).collect::<Vec<_>>();
    let tc = arr(fe, "m_TreeChildren");
    println!(
        "\n-- tree: depth={:?} parents(len={}) children(len={}) --",
        fe.get("m_nTreeDepth"),
        tp.len(),
        tc.len()
    );
    println!("  m_TreeParents = {tp:?}");
    let kids: Vec<(u64, u64)> = tc
        .iter()
        .map(|c| {
            let a = arr(c, "nChild");
            (a.first().map(u).unwrap_or(0), a.get(1).map(u).unwrap_or(0))
        })
        .collect();
    println!("  m_TreeChildren = {kids:?}");

    // other parallel arrays lengths, to know everything we must extend
    println!("\n-- parallel array lengths (must stay consistent) --");
    for k in [
        "m_CtrlName",
        "m_CtrlHash",
        "m_InitPose",
        "m_NodeInvMasses",
        "m_NodeIntegrator",
        "m_SkelParents",
        "m_NodeCollisionRadii",
        "m_DynNodeFriction",
        "m_DynNodeWindBases",
        "m_FreeNodes",
        "m_SourceElems",
        "m_Rods",
        "m_SimdRods",
        "m_NodeBases",
        "m_SimdNodeBases",
        "m_ReverseOffsets",
        "m_CtrlOffsets",
        "m_TreeParents",
        "m_TreeChildren",
        "m_TreeCollisionMasks",
        "m_TaperedCapsuleRigids",
        "m_SphereRigids",
        "m_VertexSetNames",
    ] {
        if let Some(a) = fe.get(k).and_then(Value::as_array) {
            println!("  {k}: {}", a.len());
        }
    }
    Ok(())
}
