//! Holliday boot-tassel cloth spike (EXPERIMENTAL, in-game-gated).
//!
//! Deadlock secondary motion lives in `PHYS.m_pFeModel` (a finite-element cloth
//! sim). Holliday's tassel bones `flaps_0_L/R` exist but are not in that sim, so
//! they're static. This grows the FeModel to add them. morphic re-emits the block
//! as KV3 v4 uncompressed; the engine accepts that (proven in-game).
//!
//! Modes (each builds an addon `.vpk`; install as citadel/addons/pak15_dir.vpk):
//!   noop    : re-encode PHYS unchanged.                         [PASSED in-game]
//!   probe1  : insert 1 invisible anchored free node + rebuild BVH.[PASSED in-game]
//!   one     : add a real swinging cloth rig for flaps_0_L only.
//!   both    : add rigs for flaps_0_L and flaps_0_R.
//!
//! Rig per tassel (mirrors one scarf segment): a static ankle bone-node + 2 static
//! `$cc` anchor nodes (follow the foot) + 4 free simulated `$cc` nodes + 1
//! position-driven bone (the flap). The flap's orientation is reconstructed by a
//! NodeBase from the 4 free nodes; we get it right WITHOUT knowing the engine's
//! frame formula by exploiting rotation-equivariance: place the free nodes as
//! `R*(scarf_ref - scarf_center)` with `R = Q_flap * Q_scarf^-1` and REUSE
//! scarf_2's exact qAdjust (both frame and bone rotate by R, so qAdjust is
//! invariant). The collision BVH is rebuilt over the grown dynamic set.
//!
//! Usage: cargo run -p vpkmerge-core --example tassel_phys -- <pak.vpk> <out_dir.vpk> --mode <noop|probe1|one|both>

use morphic::kv3::{self, Value};
use morphic::model::decode_skeleton;

const ENTRY: &str = "models/heroes_staging/astro/astro.vmdl_c";

// ---------------------------------------------------------------------------
// resource block plumbing
// ---------------------------------------------------------------------------

fn parse_blocks(b: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let bo = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    let c = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
    let base = 8 + bo;
    (0..c)
        .map(|i| {
            let e = base + i * 12;
            let mut k = [0u8; 4];
            k.copy_from_slice(&b[e..e + 4]);
            let rel = u32::from_le_bytes(b[e + 4..e + 8].try_into().unwrap()) as usize;
            let sz = u32::from_le_bytes(b[e + 8..e + 12].try_into().unwrap()) as usize;
            (k, (e + 4) + rel, sz)
        })
        .collect()
}

fn align16(n: usize) -> usize {
    (n + 15) & !15
}

fn rebuild_with_block(raw: &[u8], target_index: usize, new_payload: &[u8]) -> Vec<u8> {
    let blocks = parse_blocks(raw);
    let n = blocks.len();
    let resource_version = u16::from_le_bytes([raw[6], raw[7]]);
    let payloads: Vec<&[u8]> = blocks
        .iter()
        .enumerate()
        .map(|(i, (_, off, sz))| {
            if i == target_index {
                new_payload
            } else {
                &raw[*off..*off + *sz]
            }
        })
        .collect();
    let table_len = n * 12;
    let mut cursor = align16(16 + table_len);
    let mut abs = Vec::with_capacity(n);
    for p in &payloads {
        abs.push(cursor);
        cursor = align16(cursor + p.len());
    }
    let mut out = vec![0u8; cursor];
    out[0..4].copy_from_slice(&(cursor as u32).to_le_bytes());
    out[4..6].copy_from_slice(&12u16.to_le_bytes());
    out[6..8].copy_from_slice(&resource_version.to_le_bytes());
    out[8..12].copy_from_slice(&8u32.to_le_bytes());
    out[12..16].copy_from_slice(&(n as u32).to_le_bytes());
    for (i, (kind, _, _)) in blocks.iter().enumerate() {
        let entry = 16 + i * 12;
        out[entry..entry + 4].copy_from_slice(kind);
        let off_field = entry + 4;
        out[off_field..off_field + 4].copy_from_slice(&((abs[i] - off_field) as u32).to_le_bytes());
        out[off_field + 4..off_field + 8]
            .copy_from_slice(&(payloads[i].len() as u32).to_le_bytes());
    }
    for (off, p) in abs.iter().zip(&payloads) {
        out[*off..*off + p.len()].copy_from_slice(p);
    }
    out
}

// ---------------------------------------------------------------------------
// Value tree helpers
// ---------------------------------------------------------------------------

fn amut<'a>(v: &'a mut Value, k: &str) -> &'a mut Vec<Value> {
    match v.get_mut(k) {
        Some(Value::Array(a)) => a,
        _ => panic!("expected array at {k}"),
    }
}
fn aget<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k).and_then(Value::as_array).unwrap_or(&[])
}
fn set_uint(v: &mut Value, k: &str, n: u64) {
    if let Some(s) = v.get_mut(k) {
        *s = Value::UInt(n);
    }
}
fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}
fn dvec(xs: &[f64]) -> Value {
    Value::Array(xs.iter().map(|x| Value::Double(*x)).collect())
}
fn uvec(xs: &[u64]) -> Value {
    Value::Array(xs.iter().map(|x| Value::UInt(*x)).collect())
}
fn getf(v: &Value, i: usize) -> f64 {
    v.as_array().unwrap()[i].as_f64().unwrap()
}
trait AsArrayMut {
    fn am(&mut self) -> &mut Vec<Value>;
}
impl AsArrayMut for Value {
    fn am(&mut self) -> &mut Vec<Value> {
        match self {
            Value::Array(a) => a,
            _ => panic!(),
        }
    }
}

fn rm(idx: u64, ins: u64) -> u64 {
    if idx >= ins {
        idx + 1
    } else {
        idx
    }
}
fn rmi(idx: i64, ins: u64) -> i64 {
    if idx >= 0 && (idx as u64) >= ins {
        idx + 1
    } else {
        idx
    }
}

/// Shift every node reference in the FeModel up by one, for an insertion at `ins`.
/// (Tree children are excluded; the tree is rebuilt wholesale at the end.)
fn remap_node_refs(fe: &mut Value, ins: u64) {
    for k in ["m_FreeNodes", "m_SourceElems"] {
        for x in amut(fe, k) {
            if let Value::UInt(u) = x {
                *u = rm(*u, ins);
            }
        }
    }
    for x in amut(fe, "m_SkelParents") {
        match x {
            Value::Int(i) => *i = rmi(*i, ins),
            Value::UInt(u) => *u = rm(*u, ins),
            _ => {}
        }
    }
    let field = |o: &mut Value, f: &str, ins: u64| {
        if let Some(s) = o.get_mut(f) {
            match s {
                Value::UInt(u) => *u = rm(*u, ins),
                Value::Int(i) => *i = rmi(*i, ins),
                _ => {}
            }
        }
    };
    for o in amut(fe, "m_Rods") {
        for x in amut(o, "nNode") {
            if let Value::UInt(u) = x {
                *u = rm(*u, ins);
            }
        }
    }
    for o in amut(fe, "m_SimdRods") {
        if let Some(Value::Array(pair)) = o.get_mut("nNode") {
            for lane in pair {
                for x in lane.am() {
                    if let Value::UInt(u) = x {
                        *u = rm(*u, ins);
                    }
                }
            }
        }
    }
    for o in amut(fe, "m_NodeBases") {
        for f in ["nNode", "nNodeX0", "nNodeX1", "nNodeY0", "nNodeY1"] {
            field(o, f, ins);
        }
    }
    for o in amut(fe, "m_DynNodeWindBases") {
        for f in ["nNodeX0", "nNodeX1", "nNodeY0", "nNodeY1"] {
            field(o, f, ins);
        }
    }
    for o in amut(fe, "m_ReverseOffsets") {
        for f in ["nBoneCtrl", "nTargetNode"] {
            field(o, f, ins);
        }
    }
    for o in amut(fe, "m_CtrlOffsets") {
        for f in ["nCtrlParent", "nCtrlChild"] {
            field(o, f, ins);
        }
    }
    for k in ["m_TaperedCapsuleRigids", "m_SphereRigids"] {
        for o in amut(fe, k) {
            field(o, "nNode", ins);
        }
    }
}

// ---------------------------------------------------------------------------
// quaternion / vector math (model space)
// ---------------------------------------------------------------------------

type V3 = [f64; 3];
type Q = [f64; 4]; // x, y, z, w

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn norm(a: V3) -> V3 {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if l > 1e-9 {
        scale(a, 1.0 / l)
    } else {
        a
    }
}
fn dist(a: V3, b: V3) -> f64 {
    let d = sub(a, b);
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn qmul(a: Q, b: Q) -> Q {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}
fn qconj(q: Q) -> Q {
    [-q[0], -q[1], -q[2], q[3]]
}
fn qrot(q: Q, v: V3) -> V3 {
    let p = [v[0], v[1], v[2], 0.0];
    let r = qmul(qmul(q, p), qconj(q));
    [r[0], r[1], r[2]]
}
/// Row-vector-convention rotation matrix (morphic `global_bind`) to quaternion.
fn mat_to_quat(m: &[f32; 16]) -> Q {
    let r = |i: usize, j: usize| m[i * 4 + j] as f64;
    let tr = r(0, 0) + r(1, 1) + r(2, 2);
    if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        [
            (r(1, 2) - r(2, 1)) / s,
            (r(2, 0) - r(0, 2)) / s,
            (r(0, 1) - r(1, 0)) / s,
            0.25 * s,
        ]
    } else if r(0, 0) > r(1, 1) && r(0, 0) > r(2, 2) {
        let s = (1.0 + r(0, 0) - r(1, 1) - r(2, 2)).sqrt() * 2.0;
        [
            0.25 * s,
            (r(1, 0) + r(0, 1)) / s,
            (r(2, 0) + r(0, 2)) / s,
            (r(1, 2) - r(2, 1)) / s,
        ]
    } else if r(1, 1) > r(2, 2) {
        let s = (1.0 + r(1, 1) - r(0, 0) - r(2, 2)).sqrt() * 2.0;
        [
            (r(1, 0) + r(0, 1)) / s,
            0.25 * s,
            (r(2, 1) + r(1, 2)) / s,
            (r(2, 0) - r(0, 2)) / s,
        ]
    } else {
        let s = (1.0 + r(2, 2) - r(0, 0) - r(1, 1)).sqrt() * 2.0;
        [
            (r(2, 0) + r(0, 2)) / s,
            (r(2, 1) + r(1, 2)) / s,
            0.25 * s,
            (r(0, 1) - r(1, 0)) / s,
        ]
    }
}

fn murmur2(key: &[u8], seed: u32) -> u32 {
    let m = 0x5bd1_e995u32;
    let mut h = seed ^ (key.len() as u32);
    let mut d = key;
    while d.len() >= 4 {
        let mut k = u32::from_le_bytes([d[0], d[1], d[2], d[3]]);
        k = k.wrapping_mul(m);
        k ^= k >> 24;
        k = k.wrapping_mul(m);
        h = h.wrapping_mul(m);
        h ^= k;
        d = &d[4..];
    }
    if d.len() == 3 {
        h ^= (d[2] as u32) << 16;
    }
    if d.len() >= 2 {
        h ^= (d[1] as u32) << 8;
    }
    if !d.is_empty() {
        h ^= d[0] as u32;
        h = h.wrapping_mul(m);
    }
    h ^= h >> 13;
    h = h.wrapping_mul(m);
    h ^= h >> 15;
    h
}
fn ctrl_hash(name: &str) -> u64 {
    u64::from(murmur2(name.to_lowercase().as_bytes(), 0x3141_5926))
}

// ---------------------------------------------------------------------------
// node insertion
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Static,
    Free,
    Driven,
}

struct Node {
    name: String,
    pose: [f64; 8], // px,py,pz,1, qx,qy,qz,qw
    inv_mass: f64,
    gravity: f64,
}

/// Insert a node of `kind`, remapping every existing reference, extending the
/// parallel arrays, and bumping the partition counts. Dynamic (free/driven) nodes
/// also extend the per-dynamic arrays. SkelParents/windbase get placeholders that
/// callers fix up by name afterwards. Returns nothing; look the node up by name.
fn insert_node(fe: &mut Value, kind: Kind, nd: &Node) {
    let n_static = fe.get("m_nStaticNodes").and_then(Value::as_uint).unwrap();
    let first_driven = fe
        .get("m_nFirstPositionDrivenNode")
        .and_then(Value::as_uint)
        .unwrap();
    let count = fe.get("m_nNodeCount").and_then(Value::as_uint).unwrap();
    let ins = match kind {
        Kind::Static => n_static,
        Kind::Free => first_driven,
        Kind::Driven => count,
    };

    remap_node_refs(fe, ins);

    let i = ins as usize;
    amut(fe, "m_CtrlName").insert(i, Value::String(nd.name.clone()));
    amut(fe, "m_CtrlHash").insert(i, Value::UInt(ctrl_hash(&nd.name)));
    amut(fe, "m_InitPose").insert(i, dvec(&nd.pose));
    amut(fe, "m_NodeInvMasses").insert(i, Value::Double(nd.inv_mass));
    amut(fe, "m_NodeIntegrator").insert(
        i,
        obj(vec![
            ("flPointDamping", Value::Double(0.0)),
            (
                "flAnimationForceAttraction",
                Value::Double(0.728_999_912_738_8),
            ),
            (
                "flAnimationVertexAttraction",
                Value::Double(0.730_216_503_143_310_5),
            ),
            ("flGravity", Value::Double(nd.gravity)),
        ]),
    );
    amut(fe, "m_SkelParents").insert(i, Value::Int(-1)); // fixed up by caller

    if kind != Kind::Static {
        let di = i - n_static as usize;
        amut(fe, "m_NodeCollisionRadii").insert(di, Value::Double(1.0));
        amut(fe, "m_DynNodeFriction").insert(di, Value::Double(0.0));
        amut(fe, "m_DynNodeWindBases").insert(
            di,
            obj(vec![
                ("nNodeX0", Value::UInt(ins)),
                ("nNodeX1", Value::UInt(ins)),
                ("nNodeY0", Value::UInt(ins)),
                ("nNodeY1", Value::UInt(ins)),
            ]),
        );
    }
    if kind == Kind::Free {
        amut(fe, "m_FreeNodes").push(Value::UInt(ins));
    }

    set_uint(fe, "m_nNodeCount", count + 1);
    match kind {
        Kind::Static => {
            set_uint(fe, "m_nStaticNodes", n_static + 1);
            set_uint(
                fe,
                "m_nRotLockStaticNodes",
                fe.get("m_nRotLockStaticNodes")
                    .and_then(Value::as_uint)
                    .unwrap()
                    + 1,
            );
            set_uint(fe, "m_nFirstPositionDrivenNode", first_driven + 1);
        }
        Kind::Free => set_uint(fe, "m_nFirstPositionDrivenNode", first_driven + 1),
        Kind::Driven => {}
    }
}

fn node_index(fe: &Value, name: &str) -> u64 {
    aget(fe, "m_CtrlName")
        .iter()
        .position(|v| v.as_str() == Some(name))
        .expect("node not found") as u64
}
fn node_pos(fe: &Value, name: &str) -> V3 {
    let i = node_index(fe, name) as usize;
    let p = &aget(fe, "m_InitPose")[i];
    [getf(p, 0), getf(p, 1), getf(p, 2)]
}
fn set_skel_parent(fe: &mut Value, name: &str, parent: i64) {
    let i = node_index(fe, name) as usize;
    amut(fe, "m_SkelParents")[i] = Value::Int(parent);
}

fn add_rod(fe: &mut Value, a: u64, b: u64, dist: f64, relax: f64) {
    amut(fe, "m_Rods").push(obj(vec![
        ("nNode", uvec(&[a, b])),
        ("flMaxDist", Value::Double(dist)),
        ("flMinDist", Value::Double(dist)),
        ("flWeight0", Value::Double(0.0)),
        ("flRelaxationFactor", Value::Double(relax)),
    ]));
    amut(fe, "m_SimdRods").push(obj(vec![
        (
            "nNode",
            Value::Array(vec![uvec(&[a, a, a, a]), uvec(&[b, a, a, a])]),
        ),
        ("f4MaxDist", dvec(&[dist, 0.0, 0.0, 0.0])),
        ("f4MinDist", dvec(&[dist, 0.0, 0.0, 0.0])),
        ("f4Weight0", dvec(&[0.0, 0.0, 0.0, 0.0])),
        ("f4RelaxationFactor", dvec(&[relax, 1.0, 1.0, 1.0])),
    ]));
}

// ---------------------------------------------------------------------------
// BVH rebuild
// ---------------------------------------------------------------------------

fn build_tree(n_leaves: usize) -> (Vec<i64>, Vec<(u64, u64)>, u64) {
    let total = 2 * n_leaves - 1;
    let mut parents = vec![0i64; total];
    let mut children = vec![(0u64, 0u64); n_leaves - 1];
    let mut next_internal = n_leaves;
    let mut max_depth = 0u64;
    fn rec(
        lo: usize,
        hi: usize,
        depth: u64,
        ni: &mut usize,
        par: &mut [i64],
        ch: &mut [(u64, u64)],
        md: &mut u64,
        n_leaves: usize,
    ) -> usize {
        *md = (*md).max(depth);
        if hi - lo == 1 {
            return lo;
        }
        let mid = lo + (hi - lo) / 2;
        let l = rec(lo, mid, depth + 1, ni, par, ch, md, n_leaves);
        let r = rec(mid, hi, depth + 1, ni, par, ch, md, n_leaves);
        let me = *ni;
        *ni += 1;
        par[l] = me as i64;
        par[r] = me as i64;
        ch[me - n_leaves] = (l as u64, r as u64);
        me
    }
    let root = rec(
        0,
        n_leaves,
        1,
        &mut next_internal,
        &mut parents,
        &mut children,
        &mut max_depth,
        n_leaves,
    );
    parents[root] = 65535;
    (parents, children, max_depth + 1)
}
fn rebuild_tree(fe: &mut Value) {
    let nc = fe.get("m_nNodeCount").and_then(Value::as_uint).unwrap() as usize;
    let ns = fe.get("m_nStaticNodes").and_then(Value::as_uint).unwrap() as usize;
    let (parents, children, depth) = build_tree(nc - ns);
    *amut(fe, "m_TreeParents") = parents.iter().map(|p| Value::UInt(*p as u64)).collect();
    *amut(fe, "m_TreeChildren") = children
        .iter()
        .map(|(a, b)| obj(vec![("nChild", uvec(&[*a, *b]))]))
        .collect();
    *amut(fe, "m_TreeCollisionMasks") = vec![Value::UInt(15); parents.len()];
    set_uint(fe, "m_nTreeDepth", depth);
}

// ---------------------------------------------------------------------------
// tassel rig (one segment, rotation-cloned from scarf_2)
// ---------------------------------------------------------------------------

/// scarf_2 template captured up front (so later inserts don't disturb it).
struct ScarfTemplate {
    center: V3,         // scarf_2 model pos
    q_scarf: Q,         // scarf_2 model quat
    refs: [(V3, Q); 4], // pos+quat of NodeBase refs in order [X0=17, X1=16, Y0=15, Y1=18]
    q_adjust: [f64; 4],
    rev_offset: V3, // ReverseOffset, bone-local
    rod_relax: f64,
}
fn capture_scarf(fe: &Value) -> ScarfTemplate {
    let ip = aget(fe, "m_InitPose");
    let p = |i: usize| -> V3 { [getf(&ip[i], 0), getf(&ip[i], 1), getf(&ip[i], 2)] };
    let q = |i: usize| -> Q {
        [
            getf(&ip[i], 4),
            getf(&ip[i], 5),
            getf(&ip[i], 6),
            getf(&ip[i], 7),
        ]
    };
    let nb = &aget(fe, "m_NodeBases")[0]; // scarf_2
    let qa: Vec<f64> = aget(nb, "qAdjust")
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    let ro = &aget(fe, "m_ReverseOffsets")[0];
    let ro_v = aget(ro, "vOffset");
    let rod0 = &aget(fe, "m_Rods")[0];
    ScarfTemplate {
        center: p(31),
        q_scarf: q(31),
        refs: [
            (p(17), q(17)),
            (p(16), q(16)),
            (p(15), q(15)),
            (p(18), q(18)),
        ],
        q_adjust: [qa[0], qa[1], qa[2], qa[3]],
        rev_offset: [
            ro_v[0].as_f64().unwrap(),
            ro_v[1].as_f64().unwrap(),
            ro_v[2].as_f64().unwrap(),
        ],
        rod_relax: aget(rod0, "flRelaxationFactor").first().map_or(0.005, |_| {
            rod0.get("flRelaxationFactor").unwrap().as_f64().unwrap()
        }),
    }
}

fn pose(p: V3, q: Q) -> [f64; 8] {
    [p[0], p[1], p[2], 1.0, q[0], q[1], q[2], q[3]]
}

/// Cloth-owned bone flags. The scarf's FeModel-driven bones (scarf_2..9) carry
/// `Cloth | Procedural` (VRF's `ModelSkeletonBoneFlags::ProceduralCloth`, m_nFlag
/// base 0x403cc8). `Procedural` (0x400000) alone only marks "driven at runtime";
/// the cloth pass keys off `Cloth` (0x8) to decide which bones it OWNS and writes
/// back to. Setting Procedural without Cloth -> the bone is never claimed by the
/// FeModel and stays at bind pose (confirmed in-game: diag left the flap static).
const BONE_FLAG_CLOTH: u64 = 0x8;
const BONE_FLAG_PROCEDURAL: u64 = 0x40_0000;

/// Edit the model's DATA block: OR `Cloth | Procedural` into `m_nFlag` for each
/// named bone, so the FeModel claims and drives it. Returns the rebuilt model.
fn set_procedural_flags(model: &[u8], bones: &[&str]) -> anyhow::Result<Vec<u8>> {
    let blocks = parse_blocks(model);
    let (idx, (_, off, len)) = blocks
        .iter()
        .enumerate()
        .find(|(_, (k, _, _))| k == b"DATA")
        .map(|(i, b)| (i, *b))
        .ok_or_else(|| anyhow::anyhow!("no DATA block"))?;
    let raw = &model[off..off + len];
    let fmt = kv3::Format::from_payload(raw).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let mut data = kv3::decode(raw).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let sk = data.get_mut("m_modelSkeleton").unwrap();
    let names: Vec<String> = aget(sk, "m_boneName")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let flags = amut(sk, "m_nFlag");
    for b in bones {
        let i = names
            .iter()
            .position(|n| n == b)
            .ok_or_else(|| anyhow::anyhow!("no bone {b}"))?;
        let cur = flags[i].as_uint().unwrap();
        let new = cur | BONE_FLAG_CLOTH | BONE_FLAG_PROCEDURAL;
        flags[i] = Value::UInt(new);
        println!("  bone flag {b}: {cur:#x} -> {new:#x}");
    }
    let edited = kv3::encode(&data, &fmt);
    Ok(rebuild_with_block(model, idx, &edited))
}

/// Add one swinging tassel rig for bone `flap` (anchored to bone `ankle`).
fn add_tassel(
    fe: &mut Value,
    sk: &morphic::model::Skeleton,
    t: &ScarfTemplate,
    flap: &str,
    ankle: &str,
    tag: &str,
) {
    let bone = |nm: &str| {
        sk.bones
            .iter()
            .find(|b| b.name == nm)
            .unwrap_or_else(|| panic!("no bone {nm}"))
    };
    let bpos = |nm: &str| -> V3 {
        let m = &bone(nm).global_bind.m;
        [m[12] as f64, m[13] as f64, m[14] as f64]
    };
    let bquat = |nm: &str| mat_to_quat(&bone(nm).global_bind.m);

    let c_f = bpos(flap);
    let q_f = bquat(flap);
    let a_pos = bpos(ankle);
    let q_a = bquat(ankle);

    // rotation taking scarf_2's frame to the flap's frame (rotation-equivariance)
    let r = qmul(q_f, qconj(t.q_scarf));

    // 4 free nodes: rotation-cloned scarf refs, around the flap origin
    let free_names = [
        format!("$cc{tag}_x0"),
        format!("$cc{tag}_x1"),
        format!("$cc{tag}_y0"),
        format!("$cc{tag}_y1"),
    ];
    let mut free_pos = [[0.0; 3]; 4];
    for i in 0..4 {
        let (rp, rq) = t.refs[i];
        free_pos[i] = add(c_f, qrot(r, sub(rp, t.center)));
        let q = qmul(r, rq);
        insert_node(
            fe,
            Kind::Free,
            &Node {
                name: free_names[i].clone(),
                pose: pose(free_pos[i], q),
                inv_mass: 0.0,
                gravity: 540.0,
            },
        );
    }

    // 2 static anchors: offset toward the ankle from the flap, ±5 across (cloth width)
    let toward = norm(sub(a_pos, c_f));
    let anchor_c = add(c_f, scale(toward, 5.0));
    let a0p = add(anchor_c, qrot(r, [0.0, 5.0, 0.0]));
    let a1p = add(anchor_c, qrot(r, [0.0, -5.0, 0.0]));
    let an0 = format!("$cc{tag}_a0");
    let an1 = format!("$cc{tag}_a1");
    insert_node(
        fe,
        Kind::Static,
        &Node {
            name: ankle.to_string(),
            pose: pose(a_pos, q_a),
            inv_mass: 0.0,
            gravity: 0.0,
        },
    );
    insert_node(
        fe,
        Kind::Static,
        &Node {
            name: an0.clone(),
            pose: pose(a0p, q_a),
            inv_mass: 0.0,
            gravity: 0.0,
        },
    );
    insert_node(
        fe,
        Kind::Static,
        &Node {
            name: an1.clone(),
            pose: pose(a1p, q_a),
            inv_mass: 0.0,
            gravity: 0.0,
        },
    );

    // driven bone node
    insert_node(
        fe,
        Kind::Driven,
        &Node {
            name: flap.to_string(),
            pose: pose(c_f, q_f),
            inv_mass: 1.0,
            gravity: 0.0,
        },
    );

    // ---- relations (by final index) ----
    let fx0 = node_index(fe, &free_names[0]);
    let fx1 = node_index(fe, &free_names[1]);
    let fy0 = node_index(fe, &free_names[2]);
    let fy1 = node_index(fe, &free_names[3]);
    let ai = node_index(fe, ankle);
    let a0i = node_index(fe, &an0);
    let a1i = node_index(fe, &an1);
    let bi = node_index(fe, flap);

    // skeleton parenting (free -> bone, anchors -> ankle, bone -> ankle)
    for n in &free_names {
        set_skel_parent(fe, n, bi as i64);
    }
    set_skel_parent(fe, &an0, ai as i64);
    set_skel_parent(fe, &an1, ai as i64);
    set_skel_parent(fe, flap, ai as i64);

    // CtrlOffsets so the static anchors follow the ankle bone (offset in ankle local frame)
    let local = |p: V3| -> V3 { qrot(qconj(q_a), sub(p, a_pos)) };
    for (child, p) in [(a0i, a0p), (a1i, a1p)] {
        let o = local(p);
        amut(fe, "m_CtrlOffsets").push(obj(vec![
            ("vOffset", dvec(&[o[0], o[1], o[2]])),
            ("nCtrlParent", Value::UInt(ai)),
            ("nCtrlChild", Value::UInt(child)),
        ]));
    }

    // NodeBase: reconstruct the flap's orientation (reuse scarf_2 qAdjust)
    amut(fe, "m_NodeBases").push(obj(vec![
        ("nNode", Value::UInt(bi)),
        ("nDummy", uvec(&[0, 0, 0])),
        ("nNodeX0", Value::UInt(fx0)),
        ("nNodeX1", Value::UInt(fx1)),
        ("nNodeY0", Value::UInt(fy0)),
        ("nNodeY1", Value::UInt(fy1)),
        ("qAdjust", dvec(&t.q_adjust)),
    ]));
    // ReverseOffset: flap position from free node Y0, bone-local offset
    amut(fe, "m_ReverseOffsets").push(obj(vec![
        ("vOffset", dvec(&t.rev_offset)),
        ("nBoneCtrl", Value::UInt(bi)),
        ("nTargetNode", Value::UInt(fy0)),
    ]));

    // Rods: tether ONLY the near edge (x1,y0 = the scarf-15/16 side, closest to the
    // bone) to the two anchors, then make the rest a rigid panel. The panel hangs
    // from that top edge like a hinged flap and swings freely under gravity --
    // tethering every node to both anchors (as before) welds it rigid -> static.
    add_rod(fe, a0i, fx1, dist(a0p, free_pos[1]), t.rod_relax); // anchor -> near node
    add_rod(fe, a1i, fy0, dist(a1p, free_pos[2]), t.rod_relax);
    add_rod(fe, fx1, fy0, dist(free_pos[1], free_pos[2]), t.rod_relax); // near edge
    add_rod(fe, fx0, fy1, dist(free_pos[0], free_pos[3]), t.rod_relax); // far edge
    add_rod(fe, fx1, fx0, dist(free_pos[1], free_pos[0]), t.rod_relax); // side
    add_rod(fe, fy0, fy1, dist(free_pos[2], free_pos[3]), t.rod_relax); // side
    add_rod(fe, fx1, fy1, dist(free_pos[1], free_pos[3]), t.rod_relax); // diagonal (rigidify)

    println!(
        "  tassel {flap}: free@({:.1},{:.1},{:.1}) anchored to {ankle}",
        c_f[0], c_f[1], c_f[2]
    );
}

// ---------------------------------------------------------------------------
// validation
// ---------------------------------------------------------------------------

fn validate(fe: &Value) -> anyhow::Result<()> {
    let nc = fe.get("m_nNodeCount").and_then(Value::as_uint).unwrap() as usize;
    let ns = fe.get("m_nStaticNodes").and_then(Value::as_uint).unwrap() as usize;
    for k in [
        "m_CtrlName",
        "m_CtrlHash",
        "m_InitPose",
        "m_NodeInvMasses",
        "m_NodeIntegrator",
        "m_SkelParents",
    ] {
        anyhow::ensure!(
            aget(fe, k).len() == nc,
            "{k} len {} != nodeCount {nc}",
            aget(fe, k).len()
        );
    }
    for k in [
        "m_NodeCollisionRadii",
        "m_DynNodeFriction",
        "m_DynNodeWindBases",
    ] {
        anyhow::ensure!(
            aget(fe, k).len() == nc - ns,
            "{k} len {} != dyn {}",
            aget(fe, k).len(),
            nc - ns
        );
    }
    let chk = |u: u64| -> anyhow::Result<()> {
        anyhow::ensure!((u as usize) < nc, "node ref {u} >= {nc}");
        Ok(())
    };
    for x in aget(fe, "m_FreeNodes") {
        chk(x.as_uint().unwrap())?;
    }
    for o in aget(fe, "m_Rods") {
        for x in aget(o, "nNode") {
            chk(x.as_uint().unwrap())?;
        }
    }
    for o in aget(fe, "m_NodeBases") {
        for f in ["nNode", "nNodeX0", "nNodeX1", "nNodeY0", "nNodeY1"] {
            chk(o.get(f).unwrap().as_uint().unwrap())?;
        }
    }
    for o in aget(fe, "m_ReverseOffsets") {
        chk(o.get("nBoneCtrl").unwrap().as_uint().unwrap())?;
        chk(o.get("nTargetNode").unwrap().as_uint().unwrap())?;
    }
    let tp = aget(fe, "m_TreeParents").len();
    anyhow::ensure!(
        tp == 2 * (nc - ns) - 1,
        "tree parents {tp} != {}",
        2 * (nc - ns) - 1
    );
    anyhow::ensure!(
        aget(fe, "m_TreeChildren").len() == (nc - ns) - 1,
        "tree children mismatch"
    );
    anyhow::ensure!(
        aget(fe, "m_TreeCollisionMasks").len() == tp,
        "tree masks mismatch"
    );
    Ok(())
}

// ---------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    let pak = std::env::args().nth(1).expect("pak.vpk");
    let out = std::env::args().nth(2).expect("out_dir.vpk");
    let mode = std::env::args().skip(3).collect::<Vec<_>>().join(" ");
    let mode = mode
        .split_whitespace()
        .find(|s| !s.starts_with("--") && *s != "mode")
        .unwrap_or("noop")
        .to_string();

    let model = vpkmerge_core::read_vpk_entry(&pak, ENTRY)?;
    let blocks = parse_blocks(&model);
    let (phys_idx, (_, off, len)) = blocks
        .iter()
        .enumerate()
        .find(|(_, (k, _, _))| k == b"PHYS")
        .map(|(i, b)| (i, *b))
        .ok_or_else(|| anyhow::anyhow!("no PHYS"))?;
    let orig = &model[off..off + len];
    let fmt = kv3::Format::from_payload(orig).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let mut phys = kv3::decode(orig).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let sk = decode_skeleton(&model).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let n0 = phys
        .get("m_pFeModel")
        .and_then(|f| f.get("m_nNodeCount"))
        .and_then(Value::as_uint)
        .unwrap();
    println!("PHYS {len}b v5/LZ4, FeModel nodes={n0}, mode={mode}");

    let touched = mode != "noop";
    match mode.as_str() {
        "noop" => {}
        "probe1" => {
            let fe = phys.get_mut("m_pFeModel").unwrap();
            let ins = fe
                .get("m_nFirstPositionDrivenNode")
                .and_then(Value::as_uint)
                .unwrap();
            let p = node_pos(fe, "pelvis");
            insert_node(
                fe,
                Kind::Free,
                &Node {
                    name: "$cctassel_probe".into(),
                    pose: [p[0], p[1], p[2] - 10.0, 1.0, 0.0, 0.0, 0.0, 1.0],
                    inv_mass: 1.0,
                    gravity: 540.0,
                },
            );
            add_rod(fe, 0, ins, 10.0, 1.0);
            rebuild_tree(fe);
            validate(fe)?;
            println!("probe1: 1 free node + tree rebuild");
        }
        "diag" => {
            // Diagnostic: append flaps_0_L as a driven bone reconstructed from the
            // SCARF's existing (moving) free nodes 15..18, exactly like scarf_2. No
            // new free nodes. If the fringe then moves (flies toward the neck and
            // swings), the added-bone pipeline (binding + NodeBase + ReverseOffset)
            // works and the blocker is purely my free nodes not simulating. If it
            // stays static, the reconstruction itself isn't being applied.
            let fe = phys.get_mut("m_pFeModel").unwrap();
            let t = capture_scarf(fe);
            insert_node(
                fe,
                Kind::Driven,
                &Node {
                    name: "flaps_0_L".into(),
                    pose: pose(t.center, t.q_scarf),
                    inv_mass: 1.0,
                    gravity: 0.0,
                },
            );
            let bi = node_index(fe, "flaps_0_L");
            set_skel_parent(fe, "flaps_0_L", 14);
            amut(fe, "m_NodeBases").push(obj(vec![
                ("nNode", Value::UInt(bi)),
                ("nDummy", uvec(&[0, 0, 0])),
                ("nNodeX0", Value::UInt(17)),
                ("nNodeX1", Value::UInt(16)),
                ("nNodeY0", Value::UInt(15)),
                ("nNodeY1", Value::UInt(18)),
                ("qAdjust", dvec(&t.q_adjust)),
            ]));
            amut(fe, "m_ReverseOffsets").push(obj(vec![
                ("vOffset", dvec(&t.rev_offset)),
                ("nBoneCtrl", Value::UInt(bi)),
                ("nTargetNode", Value::UInt(15)),
            ]));
            rebuild_tree(fe);
            validate(fe)?;
            println!("diag: flaps_0_L driven by scarf nodes 15-18 (should fly to neck + swing)");
        }
        "one" | "both" => {
            let fe = phys.get_mut("m_pFeModel").unwrap();
            let t = capture_scarf(fe);
            add_tassel(fe, &sk, &t, "flaps_0_L", "ankle_L", "flapsL");
            if mode == "both" {
                add_tassel(fe, &sk, &t, "flaps_0_R", "ankle_R", "flapsR");
            }
            rebuild_tree(fe);
            validate(fe)?;
            let n1 = fe.get("m_nNodeCount").and_then(Value::as_uint).unwrap();
            println!("{mode}: nodes {n0} -> {n1}, validated");
        }
        other => anyhow::bail!("unknown mode {other}"),
    }

    let edited = kv3::encode(&phys, &fmt);
    let mut new_model = rebuild_with_block(&model, phys_idx, &edited);
    // Mark the flap bones procedural so the FeModel is allowed to drive them.
    new_model = match mode.as_str() {
        "one" | "diag" => set_procedural_flags(&new_model, &["flaps_0_L"])?,
        "both" => set_procedural_flags(&new_model, &["flaps_0_L", "flaps_0_R"])?,
        _ => new_model,
    };
    let rb = parse_blocks(&new_model);
    let (_, no, nl) = rb.iter().find(|(k, _, _)| k == b"PHYS").copied().unwrap();
    let phys2 =
        kv3::decode(&new_model[no..no + nl]).map_err(|e| anyhow::anyhow!("re-decode: {e:?}"))?;
    if touched {
        validate(phys2.get("m_pFeModel").unwrap())?;
    }
    println!("re-decode OK; PHYS {} -> {} bytes", len, edited.len());

    vpkmerge_core::pack(&[(ENTRY, new_model.as_slice())], &out)?;
    println!("wrote {out}");
    Ok(())
}
