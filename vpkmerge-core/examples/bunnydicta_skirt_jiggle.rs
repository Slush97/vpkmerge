//! Bunnydicta tennis skirt secondary motion: a ring of jiggle-bone panels
//! grafted onto the installed tennis variant's `hornet.vmdl_c`.
//!
//! The simulation data comes from Valve's compiler, not hand-built FeModel
//! guesses. `rig` fits STRIPS radial panels (one jiggle bone each, hinged just
//! under the waistband) to the skirt geometry already in the model and writes a
//! ModelDoc probe with a `JiggleBoneList`, the one cloth authoring path the
//! reduced CSDK's resourcecompiler is known to emit an `m_pFeModel` for. Compile
//! it with `tools/hero-model-compiler/compile_resource.py`; `build` then grafts
//! the compiled result into the model using only in-place patches:
//!
//! - Skeleton: the skeleton's numeric arrays live in KV3's auxiliary buffer,
//!   which the byte-faithful array insert can't grow, and a full DATA re-encode
//!   is engine-invalid. So the panels REUSE leftover stock-Vindicta bones the
//!   bunnysuit never skins (bell and back spikes; referenced by
//!   nothing but the skeleton and palettes): renamed to `skirt_NN` (so the
//!   animgraph, which binds by name, stops driving them), reparented to the
//!   pelvis, rebound, and flagged `Cloth|Procedural` like the ear/braid bones.
//!   The mesh skeletons in MDAT mirror the rename and inverse binds.
//! - Palette: only bones that already own a slot in every mesh's fixed 94-entry
//!   palette are used (bell + back spikes, ten panels), so the remap is untouched.
//! - Skinning: the skirt component of `clothes` draw call 0 (an uncompressed
//!   buffer the outfit already owns) is re-weighted: the top stays on the body,
//!   the rest blends onto the two nearest panels, keeping `BODY_HEM` of the
//!   original hip/leg weights at the hem so the skirt still follows lifted legs.
//! - PHYS: the compiled jiggle nodes go at the end of the model's dynamic node
//!   range (every node reference past it shifts), `m_JiggleBones` is appended and
//!   the collision BVH rebuilt. The braid/ear sim is otherwise untouched. PHYS is
//!   re-emitted as KV3 v4 uncompressed (engine-accepted, proven on Holliday).
//!
//! Chained (multi-segment) jiggles were avoided on purpose: the compiled
//! `m_nJiggleParent` is ambiguous between a node index and a jiggle index, and
//! the two stop agreeing once the nodes are shifted into this model.
//!
//! Usage:
//!   bunnydicta_skirt_jiggle rig   <tennis_dir.vpk> <out.vmdl>
//!   bunnydicta_skirt_jiggle build <tennis_dir.vpk> <probe.vmdl_c> <out_dir.vpk>

use anyhow::{bail, ensure, Context, Result};
use morphic::kv3::{self, Seg, Value};
use morphic::model::{Model, VertexBuffer};
use morphic::resource::Resource;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::fmt::Write as _;

const ENTRY: &str = "models/heroes_staging/hornet_v3/hornet.vmdl_c";
const STRIPS: usize = 10;
/// Panels hang from just under the waistband (which stays rigid on the body).
const Z_BASE: f64 = 62.0;
/// Share of the original (hip/leg) skinning kept at the hem.
const BODY_HEM: f64 = 0.7;
/// Jiggle tuning. No tip mass: gravity would sag the flared panels back to
/// vertical, into the hips. Damping ~0.7 of critical (2*sqrt(k)) so a panel
/// settles instead of ringing out of phase with its neighbours.
const STIFFNESS: f64 = 120.0;
const DAMPING: f64 = 15.0;
/// Swing limits (degrees). Panels are framed local X = out of the skirt, so the
/// Source jiggle "yaw" (tip toward X) is in/out and "pitch" is sideways; inward
/// is kept near zero so panels can't swing into the legs or rear.
const SWING_OUT: f64 = 25.0;
const SWING_IN: f64 = 0.0;
const SWING_SIDE: f64 = 12.0;
/// Outward ease at the hem (inches), all round and extra over the rear.
const EASE_HEM: f64 = 0.25;
const EASE_REAR: f64 = 0.45;
/// Unskinned stock-Vindicta bones repurposed as the panels, in strip order.
const SPARE: [&str; STRIPS] = [
    "bell_0",
    "bell_1",
    "spike_3_R",
    "spike_3_end_R",
    "spike_2_R",
    "spike_2_end_R",
    "spike_1_R",
    "spike_1_end_R",
    "spike_0_R",
    "spike_0_end_R",
];
/// `m_nFlag` of the model's own FeModel-driven bones (ears, braid):
/// Procedural | vertex-LOD bits | Mesh | Animation | Cloth.
const CLOTH_BONE_FLAGS: i64 = 0x43_fcc8;

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}
fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn panel_name(strip: usize) -> String {
    format!("skirt_{strip:02}")
}

fn key(k: &str) -> Seg {
    Seg::Key(k.into())
}

/// A bone frame: `axes[i]` is the image of local axis i (Source forward, left,
/// up) in the containing space, matching the rows of morphic's row-vector
/// matrices.
#[derive(Clone, Copy)]
struct Frame {
    origin: V3,
    axes: [V3; 3],
}

impl Frame {
    fn from_global(m: &[f32; 16]) -> Frame {
        let r = |i: usize| {
            [
                f64::from(m[i * 4]),
                f64::from(m[i * 4 + 1]),
                f64::from(m[i * 4 + 2]),
            ]
        };
        Frame {
            origin: r(3),
            axes: [r(0), r(1), r(2)],
        }
    }

    /// `child` expressed in this frame.
    fn relative(&self, child: &Frame) -> Frame {
        let to_local = |v: V3| {
            [
                dot(v, self.axes[0]),
                dot(v, self.axes[1]),
                dot(v, self.axes[2]),
            ]
        };
        Frame {
            origin: to_local(sub(child.origin, self.origin)),
            axes: child.axes.map(to_local),
        }
    }

    /// ModelDoc `angles` ([pitch, yaw, roll] degrees): the QuaternionMatrix
    /// convention of `tools/hero-model-compiler/emit_skeleton_vmdl.py`.
    fn qangle(&self) -> V3 {
        let [fwd, left, up] = self.axes;
        let xy = fwd[0].hypot(fwd[1]);
        if xy > 1e-3 {
            [
                (-fwd[2]).atan2(xy).to_degrees(),
                fwd[1].atan2(fwd[0]).to_degrees(),
                left[2].atan2(up[2]).to_degrees(),
            ]
        } else {
            [
                (-fwd[2]).atan2(xy).to_degrees(),
                (-left[0]).atan2(left[1]).to_degrees(),
                0.0,
            ]
        }
    }

    /// Rotation as an (x, y, z, w) quaternion, the inverse of
    /// `morphic::model::Mat4::from_quaternion` (rows = axes).
    fn quat(&self) -> [f64; 4] {
        let r = |i: usize, j: usize| self.axes[i][j];
        let tr = r(0, 0) + r(1, 1) + r(2, 2);
        let q = if tr > 0.0 {
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
        };
        if q[3] < 0.0 {
            q.map(|c| -c)
        } else {
            q
        }
    }

    /// MDAT `m_invBindPose`: the inverse model-space bind as 3 rows of 4.
    fn inverse_rows(&self) -> [f64; 12] {
        let mut out = [0.0; 12];
        for i in 0..3 {
            out[i * 4..i * 4 + 3].copy_from_slice(&self.axes[i]);
            out[i * 4 + 3] = -dot(self.axes[i], self.origin);
        }
        out
    }
}

/// The skirt fit shared by `rig` and `build`, so the probe's compiled bones and
/// the grafted skinning always describe the same panels.
struct Fit {
    /// Vertex indices of the skirt within `clothes` draw call 0.
    skirt: Vec<usize>,
    /// Waist centre (XY) the panels fan out from.
    centre: [f64; 2],
    /// Per panel: model-space frame (local Z runs down to the hem, local X
    /// points out of the skirt) and hinge-to-hem length.
    panels: Vec<(Frame, f64)>,
    z_hem: f64,
}

fn clothes_dc0(model: &Model) -> Result<(&VertexBuffer, &[u32])> {
    let mp = model
        .meshes
        .iter()
        .find(|m| m.name == "clothes")
        .context("no clothes mesh")?;
    let prim = &mp.primitives[0];
    Ok((&mp.vertex_buffers[prim.vertex_buffer], &prim.indices))
}

fn fit(model: &Model) -> Result<Fit> {
    let (vb, indices) = clothes_dc0(model)?;
    let mut parent: Vec<usize> = (0..vb.element_count).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for t in indices.chunks(3) {
        for k in 1..3 {
            let (a, b) = (
                find(&mut parent, t[0] as usize),
                find(&mut parent, t[k] as usize),
            );
            parent[a] = b;
        }
    }
    let mut comps: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..vb.element_count {
        let r = find(&mut parent, i);
        comps.entry(r).or_default().push(i);
    }
    let pos = |i: usize| vb.positions[i].map(f64::from);
    let skirt = comps
        .into_values()
        .filter(|vs| vs.iter().any(|&i| pos(i)[2] < 55.0))
        .max_by_key(Vec::len)
        .context("no skirt component below the hips")?;
    let top: Vec<V3> = skirt
        .iter()
        .map(|&i| pos(i))
        .filter(|p| p[2] > Z_BASE - 1.0)
        .collect();
    let n = top.len() as f64;
    let c = [
        top.iter().map(|p| p[0]).sum::<f64>() / n,
        top.iter().map(|p| p[1]).sum::<f64>() / n,
    ];
    let z_hem = skirt.iter().map(|&i| pos(i)[2]).fold(f64::MAX, f64::min);

    let half = PI / STRIPS as f64;
    let mut panels = Vec::with_capacity(STRIPS);
    for s in 0..STRIPS {
        let th = 2.0 * half * s as f64;
        let sector: Vec<V3> = skirt
            .iter()
            .map(|&i| pos(i))
            .filter(|p| {
                let d = ((p[1] - c[1]).atan2(p[0] - c[0]) - th + PI).rem_euclid(2.0 * PI) - PI;
                d.abs() < half
            })
            .collect();
        let hem = sector.iter().map(|p| p[2]).fold(f64::MAX, f64::min);
        let at = |z: f64| {
            let band: Vec<f64> = sector
                .iter()
                .filter(|p| (p[2] - z).abs() < 0.75)
                .map(|p| (p[0] - c[0]).hypot(p[1] - c[1]))
                .collect();
            let r = band.iter().sum::<f64>() / band.len().max(1) as f64;
            [c[0] + r * th.cos(), c[1] + r * th.sin(), z]
        };
        let (p0, p1) = (at(Z_BASE), at(hem));
        let z = norm(sub(p1, p0));
        let out = [th.cos(), th.sin(), 0.0];
        let x = norm(sub(out, z.map(|v| v * dot(out, z))));
        panels.push((
            Frame {
                origin: p0,
                axes: [x, cross(z, x), z],
            },
            dot(sub(p1, p0), z),
        ));
    }
    Ok(Fit {
        skirt,
        centre: c,
        panels,
        z_hem,
    })
}

fn pelvis_frame(model: &Model) -> Result<(usize, Frame)> {
    let i = model
        .skeleton
        .bones
        .iter()
        .position(|b| b.name == "pelvis")
        .context("no pelvis")?;
    Ok((
        i,
        Frame::from_global(&model.skeleton.bones[i].global_bind.m),
    ))
}

fn rig(vpk: &str, out: &str) -> Result<()> {
    let bytes = vpkmerge_core::read_vpk_entry(vpk, ENTRY)?;
    let model = morphic::model::decode(&bytes)?;
    let f = fit(&model)?;
    let (_, pelvis) = pelvis_frame(&model)?;
    println!(
        "skirt: {} verts, waist centre ({:.2}, {:.2}), hem z {:.2}",
        f.skirt.len(),
        f.centre[0],
        f.centre[1],
        f.z_hem
    );

    let v = |v: V3| format!("[{:.6}, {:.6}, {:.6}]", v[0], v[1], v[2]);
    let mut bones = String::new();
    let mut jiggles = String::new();
    for (s, (fr, len)) in f.panels.iter().enumerate() {
        let rel = pelvis.relative(fr);
        let name = panel_name(s);
        let _ = writeln!(
            bones,
            "     {{ _class = \"Bone\" name = \"{name}\" origin = {} angles = {} do_not_discard = true }},",
            v(rel.origin),
            v(rel.qangle())
        );
        let _ = writeln!(
            jiggles,
            "    {{ _class = \"JiggleBone\" name = \"jiggle_{name}\" jiggle_root_bone = \"{name}\" jiggle_type = 1 \
             has_yaw_constraint = true min_yaw = {yaw_in:.1} max_yaw = {yaw_out:.1} yaw_friction = 0.0 yaw_bounce = 0.0 \
             has_pitch_constraint = true min_pitch = {side_neg:.1} max_pitch = {side:.1} pitch_friction = 0.0 pitch_bounce = 0.0 \
             has_angle_constraint = false allow_flex_length = false length = {len:.4} tip_mass = 0.0 \
             yaw_stiffness = {k:.1} yaw_damping = {d:.1} pitch_stiffness = {k:.1} pitch_damping = {d:.1} \
             along_stiffness = 100.0 along_damping = 0.0 radius0 = 0.5 radius1 = 0.5 \
             point0 = [0.0, 0.0, 0.0] point1 = [0.0, 0.0, {len:.4}] }},",
            yaw_in = -SWING_IN,
            yaw_out = SWING_OUT,
            side_neg = -SWING_SIDE,
            side = SWING_SIDE,
            k = STIFFNESS,
            d = DAMPING,
        );
        println!("  {name}: hinge {:.2?}, length {len:.2}", fr.origin);
    }

    let vmdl = format!(
        "<!-- kv3 encoding:text:version{{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d}} format:modeldoc36:version{{972dada4-b828-45a4-bb93-7795cf0585da}} -->\n\
{{\n rootNode = {{\n  _class = \"RootNode\"\n  children = [\n\
   {{ _class = \"Skeleton\" children = [\n\
    {{ _class = \"Bone\" name = \"pelvis\" origin = {} angles = {} do_not_discard = true children = [\n{bones}    ] }}\n   ] }},\n\
   {{ _class = \"PhysicsShapeList\" children = [ {{ _class = \"PhysicsShapeSphere\" parent_bone = \"pelvis\" surface_prop = \"default\" collision_tags = \"solid\" radius = 3.0 center = [0,0,0] }} ] }},\n\
   {{ _class = \"JiggleBoneList\" children = [\n{jiggles}   ] }},\n  ]\n }}\n}}\n",
        v(pelvis.origin),
        v(pelvis.qangle()),
    );
    std::fs::write(out, vmdl)?;
    println!("wrote {out} ({STRIPS} jiggle panels)");
    Ok(())
}

/// Pushes a skirt vertex radially out from the waist centre, zero at the hinge
/// line and growing to the hem, more over the rear (-X) where the hips and thighs
/// swing back into the skirt.
fn ease_out(p: V3, f: &Fit) -> V3 {
    let t = ((Z_BASE - p[2]) / (Z_BASE - f.z_hem)).clamp(0.0, 1.0);
    let (dx, dy) = (p[0] - f.centre[0], p[1] - f.centre[1]);
    let r = dx.hypot(dy);
    let rear = (-dx / r).max(0.0);
    let d = t * (EASE_HEM + EASE_REAR * rear);
    [p[0] + dx / r * d, p[1] + dy / r * d, p[2]]
}

/// New skin for one skirt vertex: body weights fade to `BODY_HEM` over the top
/// third, the remainder goes to the two panels either side of it.
fn skirt_weights(
    p: V3,
    joints: [u16; 4],
    weights: [f32; 4],
    f: &Fit,
    panel_bone: &[u16],
) -> ([u16; 4], [f32; 4]) {
    let t = ((Z_BASE - p[2]) / (Z_BASE - f.z_hem)).clamp(0.0, 1.0);
    let body = 1.0 - (1.0 - BODY_HEM) * smoothstep(0.0, 0.35, t);
    let mut acc: Vec<(u16, f64)> = Vec::new();
    let mut add = |b: u16, w: f64| {
        if w <= 0.0 {
            return;
        }
        match acc.iter_mut().find(|(j, _)| *j == b) {
            Some(e) => e.1 += w,
            None => acc.push((b, w)),
        }
    };
    for k in 0..4 {
        add(joints[k], f64::from(weights[k]) * body);
    }
    let u = ((p[1] - f.centre[1])
        .atan2(p[0] - f.centre[0])
        .rem_euclid(2.0 * PI))
        / (2.0 * PI / STRIPS as f64);
    let s0 = u.floor() as usize % STRIPS;
    let frac = u - u.floor();
    add(panel_bone[s0], (1.0 - body) * (1.0 - frac));
    add(panel_bone[(s0 + 1) % STRIPS], (1.0 - body) * frac);

    acc.sort_by(|a, b| b.1.total_cmp(&a.1));
    acc.truncate(4);
    let total: f64 = acc.iter().map(|e| e.1).sum();
    let mut j = [acc[0].0; 4];
    let mut w = [0.0f32; 4];
    for (k, (b, x)) in acc.iter().enumerate() {
        j[k] = *b;
        w[k] = (x / total) as f32;
    }
    (j, w)
}

/// Every FeModel field that holds a node index.
const NODE_REF_KEYS: [&str; 9] = [
    "nNode",
    "nNodeX0",
    "nNodeX1",
    "nNodeY0",
    "nNodeY1",
    "nBoneCtrl",
    "nTargetNode",
    "nCtrlParent",
    "nCtrlChild",
];

fn shift(v: &mut Value, from: u64, by: u64) {
    match v {
        Value::UInt(u) if *u >= from => *u += by,
        Value::Int(i) if *i >= 0 && (*i as u64) >= from => *i += by as i64,
        Value::Array(a) => a.iter_mut().for_each(|x| shift(x, from, by)),
        _ => {}
    }
}

fn shift_node_refs(v: &mut Value, from: u64, by: u64) {
    match v {
        Value::Object(kv) => {
            for (k, x) in kv {
                if NODE_REF_KEYS.contains(&k.as_str()) {
                    shift(x, from, by);
                } else {
                    shift_node_refs(x, from, by);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| shift_node_refs(x, from, by)),
        _ => {}
    }
}

fn arr<'a>(v: &'a mut Value, k: &str) -> Result<&'a mut Vec<Value>> {
    match v.get_mut(k) {
        Some(Value::Array(a)) => Ok(a),
        _ => bail!("FeModel has no array {k}"),
    }
}
fn uint(v: &Value, k: &str) -> Result<u64> {
    v.get(k)
        .and_then(|x| {
            x.as_uint()
                .or_else(|| x.as_int().and_then(|i| u64::try_from(i).ok()))
        })
        .with_context(|| format!("FeModel has no {k}"))
}
fn f64s(v: &Value) -> Vec<f64> {
    v.as_array()
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// Spatial median-split BVH over the dynamic nodes, in the layout the compiler
/// emits: leaves are dynamic-node ordinals, internals are numbered after them
/// in post-order (root last, its parent 65535), masks OR up from the leaves.
fn rebuild_tree(fe: &mut Value, leaf_masks: &[u64]) -> Result<()> {
    let n_static = uint(fe, "m_nStaticNodes")? as usize;
    let poses: Vec<V3> = arr(fe, "m_InitPose")?
        .iter()
        .skip(n_static)
        .map(|p| {
            let x = f64s(p);
            [x[0], x[1], x[2]]
        })
        .collect();
    let leaves = poses.len();
    ensure!(
        leaves == leaf_masks.len() && leaves > 1,
        "tree leaf count mismatch"
    );
    let mut parents = vec![0u64; 2 * leaves - 1];
    let mut masks = vec![0u64; 2 * leaves - 1];
    let mut children = vec![[0u64; 2]; leaves - 1];
    masks[..leaves].copy_from_slice(leaf_masks);
    let mut next = leaves;
    let mut depth = 0;
    fn build(
        ids: &mut [usize],
        d: usize,
        poses: &[V3],
        next: &mut usize,
        parents: &mut [u64],
        masks: &mut [u64],
        children: &mut [[u64; 2]],
        depth: &mut usize,
    ) -> usize {
        *depth = (*depth).max(d);
        if ids.len() == 1 {
            return ids[0];
        }
        let ext = |k: usize| {
            let (lo, hi) = ids.iter().fold((f64::MAX, f64::MIN), |(lo, hi), &i| {
                (lo.min(poses[i][k]), hi.max(poses[i][k]))
            });
            hi - lo
        };
        let axis = (0..3).max_by(|&a, &b| ext(a).total_cmp(&ext(b))).unwrap();
        ids.sort_by(|&a, &b| poses[a][axis].total_cmp(&poses[b][axis]));
        let mid = ids.len() / 2;
        let (l, r) = ids.split_at_mut(mid);
        let a = build(l, d + 1, poses, next, parents, masks, children, depth);
        let b = build(r, d + 1, poses, next, parents, masks, children, depth);
        let me = *next;
        *next += 1;
        parents[a] = me as u64;
        parents[b] = me as u64;
        masks[me] = masks[a] | masks[b];
        children[me - poses.len()] = [a as u64, b as u64];
        me
    }
    let mut ids: Vec<usize> = (0..leaves).collect();
    let root = build(
        &mut ids,
        0,
        &poses,
        &mut next,
        &mut parents,
        &mut masks,
        &mut children,
        &mut depth,
    );
    ensure!(root == 2 * leaves - 2, "BVH root is not last");
    parents[root] = 65535;
    *arr(fe, "m_TreeParents")? = parents.into_iter().map(Value::UInt).collect();
    *arr(fe, "m_TreeCollisionMasks")? = masks.into_iter().map(Value::UInt).collect();
    *arr(fe, "m_TreeChildren")? = children
        .into_iter()
        .map(|c| {
            Value::Object(vec![(
                "nChild".into(),
                Value::Array(c.map(Value::UInt).to_vec()),
            )])
        })
        .collect();
    *fe.get_mut("m_nTreeDepth").context("no m_nTreeDepth")? = Value::UInt(depth as u64);
    Ok(())
}

/// Inserts the probe's compiled jiggle nodes at the end of `fe`'s dynamic range.
fn merge_jiggles(fe: &mut Value, probe: &Value) -> Result<()> {
    ensure!(
        arr(fe, "m_JiggleBones")?.is_empty(),
        "model already has jiggle bones (grafted twice?)"
    );
    let n_static = uint(fe, "m_nStaticNodes")?;
    let ins = uint(fe, "m_nFirstPositionDrivenNode")?;
    let count = uint(fe, "m_nNodeCount")?;
    ensure!(
        uint(probe, "m_nStaticNodes")? == 0,
        "probe has static nodes"
    );
    let k = uint(probe, "m_nNodeCount")?;
    ensure!(
        uint(probe, "m_nFirstPositionDrivenNode")? == k,
        "probe has driven nodes"
    );
    let old_dynamic = (count - n_static) as usize;

    for (name, x) in match fe {
        Value::Object(kv) => kv.iter_mut(),
        _ => bail!("FeModel is not an object"),
    } {
        if name.starts_with("m_Tree") {
            continue;
        }
        if [
            "m_FreeNodes",
            "m_SourceElems",
            "m_LockToGoal",
            "m_SkelParents",
        ]
        .contains(&name.as_str())
        {
            shift(x, ins, k);
        } else {
            shift_node_refs(x, ins, k);
        }
    }

    let probe_arr = |key: &str| -> Result<Vec<Value>> {
        let a = probe
            .get(key)
            .and_then(Value::as_array)
            .with_context(|| format!("probe has no {key}"))?;
        ensure!(a.len() as u64 == k, "probe {key} is not per-node");
        Ok(a.to_vec())
    };
    let at = ins as usize;
    for key in [
        "m_CtrlName",
        "m_CtrlHash",
        "m_InitPose",
        "m_NodeInvMasses",
        "m_NodeIntegrator",
    ] {
        let add = probe_arr(key)?;
        arr(fe, key)?.splice(at..at, add);
    }
    let mut parents = probe_arr("m_SkelParents")?;
    parents.iter_mut().for_each(|p| shift(p, 0, ins));
    arr(fe, "m_SkelParents")?.splice(at..at, parents);

    let dyn_at = (ins - n_static) as usize;
    let winds: Vec<Value> = (0..k)
        .map(|j| {
            let me = Value::UInt(ins + j);
            Value::Object(
                ["nNodeX0", "nNodeX1", "nNodeY0", "nNodeY1"]
                    .iter()
                    .map(|f| ((*f).to_string(), me.clone()))
                    .collect(),
            )
        })
        .collect();
    arr(fe, "m_DynNodeWindBases")?.splice(dyn_at..dyn_at, winds);
    if !arr(fe, "m_NodeCollisionRadii")?.is_empty() {
        arr(fe, "m_NodeCollisionRadii")?.splice(dyn_at..dyn_at, (0..k).map(|_| Value::Double(0.0)));
    }

    let mut jiggles = probe_arr("m_JiggleBones")?;
    for j in &mut jiggles {
        let node = j.get_mut("m_nNode").context("jiggle without m_nNode")?;
        shift(node, 0, ins);
        ensure!(
            j.get("m_nJiggleParent").and_then(Value::as_uint) == Some(u64::from(u32::MAX)),
            "probe jiggle has a jiggle parent"
        );
    }
    *arr(fe, "m_JiggleBones")? = jiggles;

    *fe.get_mut("m_nNodeCount").unwrap() = Value::UInt(count + k);
    *fe.get_mut("m_nFirstPositionDrivenNode").unwrap() = Value::UInt(ins + k);

    let mut masks = vec![15u64; old_dynamic + k as usize];
    masks[dyn_at..dyn_at + k as usize].fill(0);
    rebuild_tree(fe, &masks)
}

/// Every per-node / per-dynamic-node array must match the partition counts.
fn validate_fe(fe: &Value) -> Result<()> {
    let n = uint(fe, "m_nNodeCount")? as usize;
    let dynamic = n - uint(fe, "m_nStaticNodes")? as usize;
    let len = |k: &str| {
        fe.get(k)
            .and_then(Value::as_array)
            .map_or(0, <[Value]>::len)
    };
    for k in [
        "m_CtrlName",
        "m_CtrlHash",
        "m_InitPose",
        "m_NodeInvMasses",
        "m_NodeIntegrator",
        "m_SkelParents",
    ] {
        ensure!(len(k) == n, "{k} has {} entries for {n} nodes", len(k));
    }
    for k in ["m_DynNodeWindBases", "m_NodeCollisionRadii"] {
        ensure!(
            len(k) == dynamic,
            "{k} has {} entries for {dynamic} dynamic nodes",
            len(k)
        );
    }
    ensure!(len("m_TreeParents") == 2 * dynamic - 1, "tree size");
    fn max_ref(v: &Value, m: &mut u64) {
        match v {
            Value::Object(kv) => {
                for (k, x) in kv {
                    if NODE_REF_KEYS.contains(&k.as_str()) {
                        let mut all = Vec::new();
                        flat(x, &mut all);
                        *m = all.into_iter().fold(*m, u64::max);
                    } else {
                        max_ref(x, m);
                    }
                }
            }
            Value::Array(a) => a.iter().for_each(|x| max_ref(x, m)),
            _ => {}
        }
    }
    fn flat(v: &Value, out: &mut Vec<u64>) {
        match v {
            Value::UInt(u) => out.push(*u),
            Value::Int(i) if *i >= 0 => out.push(*i as u64),
            Value::Array(a) => a.iter().for_each(|x| flat(x, out)),
            _ => {}
        }
    }
    let mut m = 0;
    max_ref(fe, &mut m);
    ensure!(
        (m as usize) < n,
        "node reference {m} out of range ({n} nodes)"
    );
    Ok(())
}

fn block_index(res: &Resource, kind: &[u8; 4]) -> Result<usize> {
    res.blocks()
        .iter()
        .position(|b| &b.kind == kind)
        .with_context(|| format!("no {} block", String::from_utf8_lossy(kind)))
}

#[allow(clippy::too_many_lines)]
fn build(vpk: &str, probe_path: &str, out: &str) -> Result<()> {
    let original = vpkmerge_core::read_vpk_entry(vpk, ENTRY)?;
    let model = morphic::model::decode(&original)?;
    let names: Vec<&str> = model
        .skeleton
        .bones
        .iter()
        .map(|b| b.name.as_str())
        .collect();
    ensure!(
        !names.contains(&"skirt_00"),
        "model already has skirt bones"
    );
    let spare_ix: Vec<usize> = SPARE
        .iter()
        .map(|s| {
            names
                .iter()
                .position(|n| n == s)
                .with_context(|| format!("no spare bone {s}"))
        })
        .collect::<Result<_>>()?;
    let f = fit(&model)?;
    let (pelvis_ix, pelvis) = pelvis_frame(&model)?;
    ensure!(
        spare_ix.iter().all(|&i| i > pelvis_ix),
        "spare bone precedes the pelvis"
    );

    // Skin first: the bone spheres are measured from it.
    let (vb, indices) = clothes_dc0(&model)?;
    let mut vb = vb.clone();
    let panel_bone: Vec<u16> = spare_ix
        .iter()
        .map(|&i| u16::try_from(i).unwrap())
        .collect();
    let mut sphere = vec![0.0f64; STRIPS];
    for &i in &f.skirt {
        let p = ease_out(vb.positions[i].map(f64::from), &f);
        vb.positions[i] = p.map(|x| x as f32);
        let (j, w) = skirt_weights(p, vb.joints[i], vb.weights[i], &f, &panel_bone);
        for k in 0..4 {
            if let Some(s) = panel_bone.iter().position(|&b| b == j[k]) {
                if w[k] > 0.0 {
                    let d = sub(p, f.panels[s].0.origin);
                    sphere[s] = sphere[s].max(dot(d, d).sqrt());
                }
            }
        }
        vb.joints[i] = j;
        vb.weights[i] = w;
    }

    // DATA, in place: rename, reparent, rebind, flag.
    let mut bytes = original.clone();
    let sk = |field: &str, i: usize| vec![key("m_modelSkeleton"), key(field), Seg::Index(i)];
    let renames: Vec<(Vec<Seg>, String)> = spare_ix
        .iter()
        .enumerate()
        .map(|(s, &i)| (sk("m_boneName", i), panel_name(s)))
        .collect();
    bytes = morphic::patch_kv3_resource_strings_adding(&bytes, &renames)?;
    let mut ints = Vec::new();
    let mut reals = Vec::new();
    for (s, &i) in spare_ix.iter().enumerate() {
        ints.push((sk("m_nParent", i), pelvis_ix as i64));
        ints.push((sk("m_nFlag", i), CLOTH_BONE_FLAGS));
        let rel = pelvis.relative(&f.panels[s].0);
        for (c, x) in rel.origin.iter().enumerate() {
            reals.push((
                vec![
                    key("m_modelSkeleton"),
                    key("m_bonePosParent"),
                    Seg::Index(i),
                    Seg::Index(c),
                ],
                *x,
            ));
        }
        for (c, x) in rel.quat().iter().enumerate() {
            reals.push((
                vec![
                    key("m_modelSkeleton"),
                    key("m_boneRotParent"),
                    Seg::Index(i),
                    Seg::Index(c),
                ],
                *x,
            ));
        }
        reals.push((sk("m_boneSphere", i), sphere[s]));
    }
    bytes = morphic::patch_kv3_resource_scalars(&bytes, &ints)?;
    bytes = morphic::patch_kv3_resource_floats(
        &bytes,
        &reals
            .iter()
            .map(|(p, x)| (p.clone(), *x as f32))
            .collect::<Vec<_>>(),
    )
    .or_else(|_| morphic::patch_kv3_resource_doubles(&bytes, &reals))
    .context("patch bone binds")?;

    // Palette: every panel must already have a slot in the clothes mesh's
    // fixed 94-entry palette. Growing the slice put vertices on slots past the
    // instance's bone range, which read the NEXT hero's bone matrices in game
    // (a panel stretched across to another Vindicta).
    let data = kv3::decode(Resource::parse(&bytes)?.data_block()?)?;
    let clothes_mesh = model
        .meshes
        .iter()
        .find(|m| m.name == "clothes")
        .unwrap()
        .mesh_index;
    let slice = morphic::model::remap_table(&data, clothes_mesh).context("no clothes remap")?;
    for (s, &b) in spare_ix.iter().enumerate() {
        ensure!(
            slice.contains(&b),
            "{} is not in the clothes palette",
            SPARE[s]
        );
    }

    // Skinning.
    let (edited, rep) =
        morphic::model::replace_draw_call_uncompressed(&bytes, "clothes", 0, &vb, indices)?;
    bytes = edited;

    // MDAT mesh skeletons: mirror the rename + inverse bind where listed.
    for (bi, blk) in Resource::parse(&bytes)?
        .blocks()
        .to_vec()
        .iter()
        .enumerate()
    {
        if &blk.kind != b"MDAT" {
            continue;
        }
        let res = Resource::parse(&bytes)?;
        let raw = res.get_block_by_index(bi).context("MDAT")?;
        let mdat = kv3::decode(raw)?;
        let Some(list) = mdat
            .get("m_skeleton")
            .and_then(|s| s.get("m_bones"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        let mut strs = Vec::new();
        let mut inv = Vec::new();
        for (j, b) in list.iter().enumerate() {
            let Some(s) = b
                .get("m_boneName")
                .and_then(Value::as_str)
                .and_then(|n| SPARE.iter().position(|x| *x == n))
            else {
                continue;
            };
            let at = |f: &str| vec![key("m_skeleton"), key("m_bones"), Seg::Index(j), key(f)];
            strs.push((at("m_boneName"), panel_name(s)));
            strs.push((at("m_parentName"), "pelvis".to_string()));
            for (c, x) in f.panels[s].0.inverse_rows().iter().enumerate() {
                let mut p = at("m_invBindPose");
                p.push(Seg::Index(c));
                inv.push((p, *x));
            }
        }
        if strs.is_empty() {
            continue;
        }
        let mut payload = kv3::set_strings_adding(raw, &strs)?;
        payload = kv3::set_floats(
            &payload,
            &inv.iter()
                .map(|(p, x)| (p.clone(), *x as f32))
                .collect::<Vec<_>>(),
        )
        .or_else(|_| kv3::set_doubles(&payload, &inv))
        .context("patch MDAT inverse binds")?;
        bytes = res.rebuild_with_block(bi, &payload)?;
    }

    // PHYS: merge the compiled jiggles.
    let probe_bytes = std::fs::read(probe_path).with_context(|| format!("read {probe_path}"))?;
    let probe_res = Resource::parse(&probe_bytes)?;
    let probe_phys = kv3::decode(
        probe_res
            .find_block(*b"PHYS")
            .context("probe has no PHYS")?,
    )?;
    let probe_fe = probe_phys
        .get("m_pFeModel")
        .context("probe has no FeModel")?;
    let probe_names: Vec<&str> = probe_fe
        .get("m_CtrlName")
        .and_then(Value::as_array)
        .context("probe ctrl names")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let want: Vec<String> = (0..STRIPS).map(panel_name).collect();
    ensure!(
        probe_names == want,
        "probe was compiled from a different rig: {probe_names:?}"
    );

    let res = Resource::parse(&bytes)?;
    let phys_ix = block_index(&res, b"PHYS")?;
    let phys_raw = res.get_block_by_index(phys_ix).context("PHYS")?;
    let fmt = kv3::Format::from_payload(phys_raw)?;
    let mut phys = kv3::decode(phys_raw)?;
    let fe = phys.get_mut("m_pFeModel").context("model has no FeModel")?;
    merge_jiggles(fe, probe_fe)?;
    validate_fe(fe)?;
    bytes = res.rebuild_with_block(phys_ix, &kv3::encode(&phys, &fmt))?;

    verify(&bytes, &f)?;
    println!(
        "grafted {STRIPS} jiggle panels; skirt {} verts re-weighted, clothes dc0 {} verts {} idx",
        f.skirt.len(),
        rep.new_vertex_count,
        rep.new_index_count
    );

    let overlay = format!("{out}.overlay_dir.vpk");
    vpkmerge_core::pack(&[(ENTRY, bytes.as_slice())], &overlay)?;
    let _ = std::fs::remove_file(out);
    vpkmerge_core::merge(
        &[vpk, overlay.as_str()],
        out,
        &vpkmerge_core::MergeOptions::default(),
    )?;
    std::fs::remove_file(&overlay)?;
    println!("wrote {out}");
    Ok(())
}

/// Re-decode the result and check the graft agrees with itself: every panel
/// bone's model-space bind (from DATA) sits where its FeModel node's InitPose
/// says (from the compiler), and every skirt vertex skins where it was.
fn verify(bytes: &[u8], f: &Fit) -> Result<()> {
    let model = morphic::model::decode(bytes)?;
    let res = Resource::parse(bytes)?;
    let phys = kv3::decode(res.find_block(*b"PHYS").context("PHYS")?)?;
    let fe = phys.get("m_pFeModel").context("FeModel")?;
    let ctrl: Vec<&str> = fe
        .get("m_CtrlName")
        .and_then(Value::as_array)
        .context("ctrl names")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let poses = fe
        .get("m_InitPose")
        .and_then(Value::as_array)
        .context("init pose")?;
    let mut worst = (0.0f64, 0.0f64);
    for s in 0..STRIPS {
        let name = panel_name(s);
        let bone = model
            .skeleton
            .bones
            .iter()
            .find(|b| b.name == name)
            .with_context(|| format!("{name} missing after graft"))?;
        let got = Frame::from_global(&bone.global_bind.m);
        let node = ctrl
            .iter()
            .position(|n| *n == name)
            .with_context(|| format!("no node {name}"))?;
        let p = f64s(&poses[node]);
        let dp = sub(got.origin, [p[0], p[1], p[2]]);
        let q = got.quat();
        let qd = (q[0] * p[4] + q[1] * p[5] + q[2] * p[6] + q[3] * p[7])
            .abs()
            .min(1.0);
        worst.0 = worst.0.max(dot(dp, dp).sqrt());
        worst.1 = worst.1.max(2.0 * qd.acos().to_degrees());
        let want = f.panels[s].0;
        ensure!(
            dot(sub(got.origin, want.origin), sub(got.origin, want.origin)).sqrt() < 1e-3,
            "{name} bind drifted from the fit"
        );
    }
    println!(
        "verify: DATA binds vs compiled InitPose: max {:.4} units, {:.3} deg",
        worst.0, worst.1
    );
    ensure!(
        worst.0 < 0.01 && worst.1 < 0.1,
        "grafted binds disagree with the compiled jiggle poses"
    );

    let (vb, _) = clothes_dc0(&model)?;
    let rest: Vec<morphic::model::Mat4> = model
        .skeleton
        .bones
        .iter()
        .map(|b| b.inverse_bind.mul(&b.global_bind))
        .collect();
    let mut max_err = 0.0f64;
    for &i in &f.skirt {
        let p = vb.positions[i];
        let v = morphic::model::Vec3 {
            x: p[0],
            y: p[1],
            z: p[2],
        };
        let mut acc = [0.0f64; 3];
        for k in 0..4 {
            let w = f64::from(vb.weights[i][k]);
            let q = rest[usize::from(vb.joints[i][k])].transform_point(v);
            acc[0] += w * f64::from(q.x);
            acc[1] += w * f64::from(q.y);
            acc[2] += w * f64::from(q.z);
        }
        max_err = max_err.max(dot(sub(acc, p.map(f64::from)), sub(acc, p.map(f64::from))).sqrt());
    }
    println!("verify: skirt rest-pose skinning error max {max_err:.5}");
    ensure!(max_err < 1e-2, "skirt does not skin to its own rest pose");
    Ok(())
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("rig") if a.len() == 3 => rig(&a[1], &a[2]),
        Some("build") if a.len() == 4 => build(&a[1], &a[2], &a[3]),
        _ => bail!(
            "usage: bunnydicta_skirt_jiggle rig <tennis_dir.vpk> <out.vmdl>\n       \
             bunnydicta_skirt_jiggle build <tennis_dir.vpk> <probe.vmdl_c> <out_dir.vpk>"
        ),
    }
}
