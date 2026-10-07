//! Source 2 cloth (`FeModel`) anchor extraction from a model's `PHYS` block.
//!
//! Deadlock fabric is driven at runtime by the `FeModel` finite-element cloth
//! solver, which writes the world transforms of dedicated `$cloth_*` bones every
//! frame. Those bones are skeleton ROOTS with no animation track, so a static
//! posed bake (no solver) leaves them at bind while the body moves and the
//! fabric detaches (or, with a naive nearest-bone guess, smears). The `FeModel`
//! records, per cloth node, the body bone that drives it: `m_SkelParents` forms a
//! node tree that terminates at driver nodes whose `m_CtrlName` is a real
//! skeleton bone (`pelvis`, `clavicle_R`, `coat_e_0`, ...). [`ClothAnchors`]
//! exposes that `$cloth` bone -> anchor bone mapping so the pose baker can rigidly
//! carry each cloth root with its TRUE anchor instead of guessing.
//!
//! This is the static-export fix: it reproduces the engine's settled rest drape
//! (kinematic nodes exactly, hanging nodes at their authored rest shape). It does
//! not run the cloth solver, so it does not reproduce live sway/collision under
//! an arbitrary action pose; for a standing menu/idle snapshot the rest drape is
//! exactly what the engine shows.
//!
//! [`decode_fe_model`] is the dynamic-sim counterpart: it returns the full typed
//! [`FeModel`] (per-node solver constants, the distance-constraint rod graph, and
//! the collision capsules) that a renderer-side XPBD preview consumes, where
//! [`ClothAnchors`] only records the static rest-drape anchor map.

#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::collections::HashMap;

use crate::kv3::Value;
use crate::resource::Resource;

/// Maps a cloth bone name (`$cloth_*`) to the skeleton bone that drives it.
#[derive(Debug, Clone, Default)]
pub struct ClothAnchors {
    anchor: HashMap<String, String>,
}

impl ClothAnchors {
    /// The driver/anchor bone name for `cloth_bone`, if the `FeModel` records one.
    #[must_use]
    pub fn anchor_of(&self, cloth_bone: &str) -> Option<&str> {
        self.anchor.get(cloth_bone).map(String::as_str)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.anchor.len()
    }
}

/// Parse the cloth-anchor map from a `.vmdl_c`'s `PHYS` block. Returns `None`
/// when the model carries no `PHYS` block or no `FeModel` (weapons, most props,
/// heroes whose only secondary motion is parented hair/coat that follows FK).
#[must_use]
pub fn decode_cloth_anchors(model_bytes: &[u8]) -> Option<ClothAnchors> {
    let resource = Resource::parse(model_bytes).ok()?;
    let phys = resource.find_block(*b"PHYS")?;
    let root = crate::kv3::decode(phys).ok()?;
    anchors_from_phys(&root)
}

fn anchors_from_phys(root: &Value) -> Option<ClothAnchors> {
    let fe = find_fe_model(root)?;
    let names = fe.get("m_CtrlName").and_then(Value::as_array)?;
    let parents = fe.get("m_SkelParents").and_then(Value::as_array)?;
    if names.len() != parents.len() {
        return None;
    }
    let node_name: Vec<&str> = names.iter().map(|v| v.as_str().unwrap_or("")).collect();
    let node_parent: Vec<i64> = parents.iter().map(|v| v.as_int().unwrap_or(-1)).collect();

    let mut anchor = HashMap::new();
    for (i, name) in node_name.iter().enumerate() {
        // Only rootless cloth nodes need an anchor; a node whose own name is not a
        // `$cloth*` bone is a driver bone the skeleton already poses via FK.
        if !is_cloth_node_name(name) {
            continue;
        }
        let Some(terminal) = walk_to_terminal(i, &node_parent) else {
            continue;
        };
        let anchor_name = node_name[terminal];
        // The terminal must be a real (non-cloth) driver bone for the map to help.
        if !anchor_name.is_empty() && !is_cloth_node_name(anchor_name) {
            anchor.insert((*name).to_string(), anchor_name.to_string());
        }
    }
    if anchor.is_empty() {
        None
    } else {
        Some(ClothAnchors { anchor })
    }
}

/// A decoded Source 2 cloth simulation (`FeModel`): the typed form of the
/// `PHYS.m_pFeModel` KV3 sub-tree a renderer-side XPBD preview consumes. Arrays
/// are node-indexed; rod/capsule node fields index into [`FeModel::nodes`].
#[derive(Debug, Clone)]
pub struct FeModel {
    /// One per `m_CtrlName`; index = node id used by rods/capsules.
    pub nodes: Vec<FeNode>,
    /// Distance constraints (`m_Rods`): the real cloth topology. Cloth nodes are
    /// connected through these, NOT through the skeleton hierarchy.
    pub rods: Vec<FeRod>,
    /// Tapered collision capsules (`m_TaperedCapsuleRigids`); spheres are LOCAL
    /// to the anchor node's frame (the caller transforms by `nodes[node].init_*`),
    /// mirroring the file so nothing is silently baked.
    pub capsules: Vec<FeCapsule>,
    /// `m_nExtraIterations` solver passes.
    pub extra_iterations: u32,
    /// `m_nExtraGoalIterations` goal-constraint passes.
    pub extra_goal_iterations: u32,
    /// `m_NodeBases`: per-bone frame reconstruction (orientation comes from the
    /// solved positions of four neighbor nodes plus `q_adjust`).
    pub node_bases: Vec<FeNodeBase>,
    /// `m_CtrlOffsets`: rigid parent-control -> child-control position binding.
    pub ctrl_offsets: Vec<FeCtrlOffset>,
    /// `m_ReverseOffsets`: recover a driven bone-control position from a solved
    /// target node plus the authored offset.
    pub reverse_offsets: Vec<FeReverseOffset>,
    /// `m_CtrlSoftOffsets`: weighted (soft) parent -> child control binding.
    pub ctrl_soft_offsets: Vec<FeCtrlSoftOffset>,
    /// `m_SphereRigids`: single-sphere collision rigids (degenerate capsules).
    pub spheres: Vec<FeSphere>,
    /// `m_BoxRigids`: oriented-box collision rigids, anchored to (local to) a node.
    pub boxes: Vec<FeBox>,
    /// `m_AnimStrayRadii`: per-node-pair max-stray-distance limits that pull a node
    /// back toward its anim target within `max_dist` (an anti-blow-up constraint).
    pub anim_stray_radii: Vec<FeStrayRadius>,
    /// `m_FitMatrices`: groups of dynamic nodes fitted back to a driven bone/control.
    pub fit_matrices: Vec<FeFitMatrix>,
    /// `m_FitWeights`: per-node weights consumed by [`FeModel::fit_matrices`].
    pub fit_weights: Vec<FeFitWeight>,
    /// `m_FreeNodes`: node indices that participate in the free/dynamic solve.
    pub free_nodes: Vec<usize>,
    /// `m_LockToParent`: hard parent-control -> child-control locks.
    pub lock_to_parent: Vec<FeLockToParent>,
    /// `m_LockToGoal`: node/control indices locked to their animation goal.
    pub lock_to_goal: Vec<usize>,
    /// `m_nFirstPositionDrivenNode`: first node whose position is animation-driven.
    pub first_position_driven_node: Option<u32>,
    /// `m_flDefaultGravityScale`: global multiplier applied on top of each node's
    /// per-node `gravity`.
    pub default_gravity_scale: f32,
    /// `m_SkelParents[i]`: node-tree parent of node `i` (`-1` = root).
    pub skel_parents: Vec<i32>,
    /// `m_nStaticNodes`: count of leading static/pinned nodes.
    pub static_node_count: u32,
    /// `m_flAddWorldCollisionRadius`: added to every node's collision radius.
    pub add_world_collision_radius: f32,
    /// `m_TreeParents`/`m_TreeChildren`/`m_TreeCollisionMasks`: the raw collision BVH.
    /// Its leaves carry the per-node collision layer (folded into
    /// [`FeNode::collision_mask`]); kept here in full for callers that walk the
    /// broadphase tree. See [`FeCollisionTree`].
    pub collision_tree: Option<FeCollisionTree>,
}

impl FeModel {
    /// Number of pinned/kinematic nodes (`inv_mass == 0`): the static anchors the
    /// solver holds at their animated target each frame.
    #[must_use]
    pub fn pinned_count(&self) -> usize {
        self.nodes.iter().filter(|n| n.is_pinned()).count()
    }
}

/// A single cloth node: its bone name, solver constants, and authored rest pose.
#[derive(Debug, Clone)]
pub struct FeNode {
    /// `m_CtrlName[i]`: the bone this node drives (`$cloth_*` or a semantic name).
    pub name: String,
    /// `m_NodeInvMasses[i]`: inverse mass; `0` = pinned/kinematic.
    pub inv_mass: f32,
    /// `m_NodeIntegrator[i].flGravity`.
    pub gravity: f32,
    /// `m_NodeIntegrator[i].flPointDamping`.
    pub damping: f32,
    /// `m_NodeIntegrator[i].flAnimationForceAttraction`: spring strength pulling
    /// the node back to its animated rest shape.
    pub anim_force: f32,
    /// `m_NodeIntegrator[i].flAnimationVertexAttraction`: per-vertex sibling of
    /// `anim_force` (the lane the throwaway JSON exporter dropped).
    pub anim_vertex: f32,
    /// `m_InitPose[i]` translation (Source 2 Z-up, cm/Hammer units).
    pub init_pos: [f32; 3],
    /// `m_InitPose[i]` rotation quaternion `[x, y, z, w]`.
    pub init_rot: [f32; 4],
    /// `m_NodeCollisionRadii[dyn_slot]`: per-node collision radius (0 for pinned).
    pub collide_radius: f32,
    /// `m_DynNodeFriction[dyn_slot]`: collision friction (0 for pinned).
    pub friction: f32,
    /// Collision layer gating which capsules/boxes this node hits (AND-tested against
    /// the rigid's `mask`). Folded from the collision BVH leaves
    /// ([`FeModel::collision_tree`]): leaf index = dynamic slot, so dynamic node `k`
    /// gets `masks[k]`. Falls back to `0xFFFF` (collide-all) for static nodes and when
    /// the tree is absent or its leaf count != `2 * dynamic_count - 1` (so a model with
    /// an unexpected tree layout degrades safely instead of mis-folding).
    pub collision_mask: u32,
}

impl FeNode {
    /// A pinned/kinematic node (inverse mass 0): driven by the body, not solved.
    #[must_use]
    pub fn is_pinned(&self) -> bool {
        self.inv_mass <= 0.0
    }
}

/// A distance constraint between two nodes (`m_Rods`). The solver keeps the
/// segment length within `[min, max]` (slack inside the band), corrected by
/// `relax` (stiffness) weighted by the two nodes' inverse masses.
#[derive(Debug, Clone)]
pub struct FeRod {
    pub a: usize,
    pub b: usize,
    pub min: f32,
    pub max: f32,
    /// `flRelaxationFactor`: ~1.0 = structural/stiff, ~0.02 = bend/soft.
    pub relax: f32,
    /// `flWeight0`: authored mass bias toward node `a`.
    pub weight: f32,
}

/// A tapered collision capsule (`m_TaperedCapsuleRigids`): two spheres in the
/// LOCAL frame of anchor node `node`. Radius lerps `sphere0[3] -> sphere1[3]`
/// along the segment.
#[derive(Debug, Clone)]
pub struct FeCapsule {
    /// `[x, y, z, radius]` of the first sphere, local to `node`.
    pub sphere0: [f32; 4],
    /// `[x, y, z, radius]` of the second sphere, local to `node`.
    pub sphere1: [f32; 4],
    /// Anchor node index (into [`FeModel::nodes`]) whose pose places the capsule.
    pub node: usize,
    /// `nCollisionMask`: gates which cloth nodes collide with this capsule.
    pub mask: u32,
}

/// `m_NodeBases[]`: reconstructs a bone's orientation from the solved positions of
/// four neighbor nodes (an x axis from `x0 - x1`, a y axis from `y0 - y1`) plus a
/// fixed `q_adjust` rotation. The faithful writeback uses this so a bone's twist
/// comes from the cloth solve, not a static pose.
#[derive(Debug, Clone)]
pub struct FeNodeBase {
    /// The node whose bone this frame orients.
    pub node: usize,
    /// `nNodeX0`/`nNodeX1`: nodes defining the local x axis.
    pub x0: usize,
    pub x1: usize,
    /// `nNodeY0`/`nNodeY1`: nodes defining the local y axis.
    pub y0: usize,
    pub y1: usize,
    /// `qAdjust` `[x, y, z, w]`: fixed rotation from the reconstructed frame to the bone.
    pub q_adjust: [f32; 4],
}

/// `m_CtrlOffsets[]`: a rigid position binding placing control `child` at
/// `parent + offset` (offset rotated into the parent's frame).
#[derive(Debug, Clone)]
pub struct FeCtrlOffset {
    pub offset: [f32; 3],
    pub parent: usize,
    pub child: usize,
}

/// `m_ReverseOffsets[]`: recover a driven bone-control position from a solved
/// `target_node` plus the authored `offset`.
#[derive(Debug, Clone)]
pub struct FeReverseOffset {
    pub offset: [f32; 3],
    pub bone_ctrl: usize,
    pub target_node: usize,
}

/// `m_CtrlSoftOffsets[]`: a weighted (soft) `parent -> child` binding; `alpha`
/// (`flAlpha`) blends the offset target into the child rather than hard-setting it.
#[derive(Debug, Clone)]
pub struct FeCtrlSoftOffset {
    pub offset: [f32; 3],
    pub parent: usize,
    pub child: usize,
    pub alpha: f32,
}

/// `m_SphereRigids[]`: a single-sphere collision rigid (a degenerate capsule),
/// `[x, y, z, radius]` local to anchor `node`.
#[derive(Debug, Clone)]
pub struct FeSphere {
    pub sphere: [f32; 4],
    pub node: usize,
    pub mask: u32,
}

/// `m_BoxRigids[]`: an oriented box collider local to anchor `node`. `tmFrame2`
/// gives the box center (`pos`) and orientation (`rot`); `size` is its full extents.
#[derive(Debug, Clone)]
pub struct FeBox {
    /// `tmFrame2` translation (box center, local to `node`).
    pub pos: [f32; 3],
    /// `tmFrame2` rotation quaternion `[x, y, z, w]`.
    pub rot: [f32; 4],
    /// `vSize`: full box extents along each local axis.
    pub size: [f32; 3],
    pub node: usize,
    pub mask: u32,
}

/// `m_AnimStrayRadii[]`: a max-stray-distance constraint between a node pair, keeping
/// a node within `max_dist` of its anim target (corrected by `relax`). Anti-blow-up.
#[derive(Debug, Clone)]
pub struct FeStrayRadius {
    pub node: [usize; 2],
    pub max_dist: f32,
    pub relax: f32,
}

/// `m_FitMatrices[]`: a fitted bone/control transform and the dynamic-node weight
/// span it consumes.
#[derive(Debug, Clone)]
pub struct FeFitMatrix {
    /// `bone`: `[x, y, z, 1, qx, qy, qz, qw]`.
    pub bone: [f32; 8],
    /// `vCenter`: center of the fitted dynamic-node group.
    pub center: [f32; 3],
    pub end: usize,
    pub node: usize,
    pub begin_dynamic: usize,
    /// `nCtrl`, present on some model versions only.
    pub ctrl: Option<usize>,
}

/// `m_FitWeights[]`: weight for one node in a fit-matrix span.
#[derive(Debug, Clone)]
pub struct FeFitWeight {
    pub weight: f32,
    pub node: usize,
    pub dummy: u32,
}

/// `m_LockToParent[]`: a hard position lock from parent control to child control.
#[derive(Debug, Clone)]
pub struct FeLockToParent {
    pub offset: [f32; 3],
    pub parent: usize,
    pub child: usize,
}

/// The raw collision BVH (`m_TreeParents`/`m_TreeChildren`/`m_TreeCollisionMasks`),
/// carried verbatim. Layout (`D` = dynamic-node count): tree indices `[0, D)` are
/// LEAVES (leaf `k` = the `k`-th dynamic node), `[D, 2*D-1)` are internal nodes, and
/// `2*D-2` is the root. Leaf masks are folded into [`FeNode::collision_mask`]; internal
/// masks are aggregate broadphase bounds (mostly `0xFFFF`), kept for tree walkers.
#[derive(Debug, Clone)]
pub struct FeCollisionTree {
    /// `m_TreeParents[i]`: parent tree-index of tree node `i` (`65535` = root).
    pub parents: Vec<u32>,
    /// `m_TreeChildren[k].nChild`: the two child tree-indices of internal node `D + k`.
    pub children: Vec<[usize; 2]>,
    /// `m_TreeCollisionMasks[i]`: collision layer of tree node `i`.
    pub masks: Vec<u32>,
}

/// Decode the full cloth simulation from a `.vmdl_c`'s `PHYS` block. Returns
/// `None` when the model carries no `PHYS` block or no `FeModel`. Counterpart to
/// [`decode_cloth_anchors`]: this exposes the dynamic solver data (nodes, rods,
/// capsules), not just the static anchor map.
#[must_use]
pub fn decode_fe_model(model_bytes: &[u8]) -> Option<FeModel> {
    let resource = Resource::parse(model_bytes).ok()?;
    let phys = resource.find_block(*b"PHYS")?;
    let root = crate::kv3::decode(phys).ok()?;
    fe_model_from_phys(&root)
}

#[allow(clippy::too_many_lines)]
fn fe_model_from_phys(root: &Value) -> Option<FeModel> {
    let fe = find_fe_model(root)?;
    let names = fe.get("m_CtrlName").and_then(Value::as_array)?;
    let inv = fe.get("m_NodeInvMasses").and_then(Value::as_array);
    let integ = fe.get("m_NodeIntegrator").and_then(Value::as_array);
    let pose = fe.get("m_InitPose").and_then(Value::as_array);
    let radii = fe.get("m_NodeCollisionRadii").and_then(Value::as_array);
    let friction = fe.get("m_DynNodeFriction").and_then(Value::as_array);

    // The per-node collision layer lives in the collision BVH leaves. Dynamic-node
    // count = nodes with inv_mass > 0 (the dyn-slot space). Leaves are tree indices
    // [0, D) (so masks.len() == 2*D - 1) and leaf k = the k-th dynamic node; fold the
    // mask only when that layout holds, else degrade to collide-all rather than
    // mis-fold a tree built over a different node subset.
    let collision_tree = read_collision_tree(fe);
    let dynamic_count = inv.map_or(0, |a| {
        a.iter().filter(|v| v.as_f64().unwrap_or(0.0) > 0.0).count()
    });
    let leaf_masks = collision_tree.as_ref().and_then(|t| {
        (dynamic_count > 0 && t.masks.len() == 2 * dynamic_count - 1).then_some(t.masks.as_slice())
    });

    // m_NodeCollisionRadii / m_DynNodeFriction are DYNAMIC-slot indexed (k-th node
    // with inv_mass > 0), as is the BVH leaf. Walk m_CtrlName in order, advancing the
    // slot only on dynamic nodes, matching the file layout.
    let mut dyn_slot = 0usize;
    let mut nodes = Vec::with_capacity(names.len());
    for (i, name) in names.iter().enumerate() {
        let it = integ.and_then(|a| a.get(i));
        let (init_pos, init_rot) = pose
            .and_then(|a| a.get(i))
            .and_then(Value::as_array)
            .map_or(([0.0; 3], [0.0, 0.0, 0.0, 1.0]), read_pose);
        let inv_mass = inv
            .and_then(|a| a.get(i))
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as f32;
        let (collide_radius, fric, collision_mask) = if inv_mass > 0.0 {
            let r = slot_f32(radii, dyn_slot);
            let f = slot_f32(friction, dyn_slot);
            let mask = leaf_masks
                .and_then(|m| m.get(dyn_slot))
                .copied()
                .unwrap_or(0xFFFF);
            dyn_slot += 1;
            (r, f, mask)
        } else {
            // Static/pinned nodes are not in the tree and never collide: collide-all.
            (0.0, 0.0, 0xFFFF)
        };
        nodes.push(FeNode {
            name: name.as_str().unwrap_or("").to_string(),
            inv_mass,
            gravity: integ_f32(it, "flGravity"),
            damping: integ_f32(it, "flPointDamping"),
            anim_force: integ_f32(it, "flAnimationForceAttraction"),
            anim_vertex: integ_f32(it, "flAnimationVertexAttraction"),
            init_pos,
            init_rot,
            collide_radius,
            friction: fric,
            collision_mask,
        });
    }

    let rods = fe
        .get("m_Rods")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(read_rod).collect())
        .unwrap_or_default();

    let capsules = fe
        .get("m_TaperedCapsuleRigids")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(read_capsule).collect())
        .unwrap_or_default();

    let skel_parents = fe
        .get("m_SkelParents")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(read_i32).collect())
        .unwrap_or_default();

    // Named, not silently dropped: constraint types empty on necro+dynamo (element
    // shape unverifiable until a hero ships them) -- m_Quads, m_Tris, m_Twists,
    // m_HingeLimits, m_KelagerBends, m_AxialEdges; and fields with no consumer in the
    // bone-driven preview -- m_DynNodeWindBases/m_flWindage (wind), m_Ropes
    // (rope sim), m_n{Static,Dynamic}NodeFlags (behavior bitflags), m_CtrlHash (node
    // identity), m_Simd* (redundant SIMD copies of m_Rods/m_NodeBases).
    Some(FeModel {
        nodes,
        rods,
        capsules,
        extra_iterations: u32_field(fe, "m_nExtraIterations"),
        extra_goal_iterations: u32_field(fe, "m_nExtraGoalIterations"),
        node_bases: array_map(fe, "m_NodeBases", read_node_base),
        ctrl_offsets: array_map(fe, "m_CtrlOffsets", read_ctrl_offset),
        reverse_offsets: array_map(fe, "m_ReverseOffsets", read_reverse_offset),
        ctrl_soft_offsets: array_map(fe, "m_CtrlSoftOffsets", read_ctrl_soft_offset),
        spheres: array_map(fe, "m_SphereRigids", read_fe_sphere),
        boxes: array_map(fe, "m_BoxRigids", read_box),
        anim_stray_radii: array_map(fe, "m_AnimStrayRadii", read_stray_radius),
        fit_matrices: array_map(fe, "m_FitMatrices", read_fit_matrix),
        fit_weights: array_map(fe, "m_FitWeights", read_fit_weight),
        free_nodes: index_array(fe, "m_FreeNodes"),
        lock_to_parent: array_map(fe, "m_LockToParent", read_lock_to_parent),
        lock_to_goal: index_array(fe, "m_LockToGoal"),
        first_position_driven_node: fe.get("m_nFirstPositionDrivenNode").map(as_u32),
        default_gravity_scale: fe
            .get("m_flDefaultGravityScale")
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32,
        skel_parents,
        static_node_count: u32_field(fe, "m_nStaticNodes"),
        add_world_collision_radius: fe
            .get("m_flAddWorldCollisionRadius")
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as f32,
        collision_tree,
    })
}

/// Collect a named array field, mapping each element through `f` (skipping any that
/// fail to parse). Empty when the field is absent.
fn array_map<T>(fe: &Value, key: &str, f: fn(&Value) -> Option<T>) -> Vec<T> {
    fe.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(f).collect())
        .unwrap_or_default()
}

/// Read the `slot`-th f32 out of an optional dynamic-slot array (missing -> 0.0).
fn slot_f32(arr: Option<&[Value]>, slot: usize) -> f32 {
    arr.and_then(|a| a.get(slot))
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32
}

/// A KV3 integer (signed or unsigned) as `i32`; missing -> `-1` (the root sentinel).
fn read_i32(v: &Value) -> i32 {
    v.as_int()
        .or_else(|| v.as_uint().and_then(|u| i64::try_from(u).ok()))
        .unwrap_or(-1) as i32
}

/// Read a 3-vector (`vOffset` etc.) as `[f32; 3]`.
fn read_vec3(v: &Value) -> [f32; 3] {
    let a = v.as_array();
    let g = |i: usize| {
        a.and_then(|a| a.get(i))
            .and_then(Value::as_f64)
            .unwrap_or(0.0) as f32
    };
    [g(0), g(1), g(2)]
}

/// Read a quaternion (`qAdjust`) as `[x, y, z, w]`, defaulting to identity.
fn read_quat(v: &Value) -> [f32; 4] {
    let a = v.as_array();
    let g = |i: usize, d: f64| {
        a.and_then(|a| a.get(i))
            .and_then(Value::as_f64)
            .unwrap_or(d) as f32
    };
    [g(0, 0.0), g(1, 0.0), g(2, 0.0), g(3, 1.0)]
}

fn read_node_base(v: &Value) -> Option<FeNodeBase> {
    Some(FeNodeBase {
        node: v.get("nNode").and_then(as_index)?,
        x0: v.get("nNodeX0").and_then(as_index)?,
        x1: v.get("nNodeX1").and_then(as_index)?,
        y0: v.get("nNodeY0").and_then(as_index)?,
        y1: v.get("nNodeY1").and_then(as_index)?,
        q_adjust: read_quat(v.get("qAdjust")?),
    })
}

fn read_ctrl_offset(v: &Value) -> Option<FeCtrlOffset> {
    Some(FeCtrlOffset {
        offset: read_vec3(v.get("vOffset")?),
        parent: v.get("nCtrlParent").and_then(as_index)?,
        child: v.get("nCtrlChild").and_then(as_index)?,
    })
}

fn read_reverse_offset(v: &Value) -> Option<FeReverseOffset> {
    Some(FeReverseOffset {
        offset: read_vec3(v.get("vOffset")?),
        bone_ctrl: v.get("nBoneCtrl").and_then(as_index)?,
        target_node: v.get("nTargetNode").and_then(as_index)?,
    })
}

fn read_ctrl_soft_offset(v: &Value) -> Option<FeCtrlSoftOffset> {
    Some(FeCtrlSoftOffset {
        offset: read_vec3(v.get("vOffset")?),
        parent: v.get("nCtrlParent").and_then(as_index)?,
        child: v.get("nCtrlChild").and_then(as_index)?,
        alpha: v.get("flAlpha").and_then(Value::as_f64).unwrap_or(0.0) as f32,
    })
}

fn read_fe_sphere(v: &Value) -> Option<FeSphere> {
    Some(FeSphere {
        sphere: read_sphere(v.get("vSphere")?)?,
        node: v.get("nNode").and_then(as_index).unwrap_or(0),
        mask: v.get("nCollisionMask").map_or(0, as_u32),
    })
}

fn read_box(v: &Value) -> Option<FeBox> {
    let tm = v.get("tmFrame2").and_then(Value::as_array)?;
    // tmFrame2 has the InitPose layout: [x, y, z, 1, qx, qy, qz, qw].
    let g = |i: usize, d: f64| tm.get(i).and_then(Value::as_f64).unwrap_or(d) as f32;
    Some(FeBox {
        pos: [g(0, 0.0), g(1, 0.0), g(2, 0.0)],
        rot: [g(4, 0.0), g(5, 0.0), g(6, 0.0), g(7, 1.0)],
        size: read_vec3(v.get("vSize")?),
        node: v.get("nNode").and_then(as_index).unwrap_or(0),
        mask: v.get("nCollisionMask").map_or(0, as_u32),
    })
}

fn read_stray_radius(v: &Value) -> Option<FeStrayRadius> {
    let nn = v.get("nNode").and_then(Value::as_array)?;
    Some(FeStrayRadius {
        node: [as_index(nn.first()?)?, as_index(nn.get(1)?)?],
        max_dist: v.get("flMaxDist").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        relax: v
            .get("flRelaxationFactor")
            .and_then(Value::as_f64)
            .unwrap_or(1.0) as f32,
    })
}

fn read_fit_matrix(v: &Value) -> Option<FeFitMatrix> {
    Some(FeFitMatrix {
        bone: read_bone_fit(v.get("bone")?),
        center: read_vec3(v.get("vCenter")?),
        end: v.get("nEnd").and_then(as_index)?,
        node: v.get("nNode").and_then(as_index)?,
        begin_dynamic: v.get("nBeginDynamic").and_then(as_index)?,
        ctrl: v.get("nCtrl").and_then(as_index),
    })
}

fn read_fit_weight(v: &Value) -> Option<FeFitWeight> {
    Some(FeFitWeight {
        weight: v.get("flWeight").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        node: v.get("nNode").and_then(as_index)?,
        dummy: v.get("nDummy").map_or(0, as_u32),
    })
}

fn read_lock_to_parent(v: &Value) -> Option<FeLockToParent> {
    Some(FeLockToParent {
        offset: read_vec3(v.get("vOffset")?),
        parent: v.get("nCtrlParent").and_then(as_index)?,
        child: v.get("nCtrlChild").and_then(as_index)?,
    })
}

fn read_tree_children(v: &Value) -> Option<[usize; 2]> {
    let nc = v.get("nChild").and_then(Value::as_array)?;
    Some([as_index(nc.first()?)?, as_index(nc.get(1)?)?])
}

/// Carry the raw collision BVH if present (`None` when the model ships no tree).
fn read_collision_tree(fe: &Value) -> Option<FeCollisionTree> {
    let masks = fe.get("m_TreeCollisionMasks").and_then(Value::as_array)?;
    Some(FeCollisionTree {
        parents: fe
            .get("m_TreeParents")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(as_u32).collect())
            .unwrap_or_default(),
        children: array_map(fe, "m_TreeChildren", read_tree_children),
        masks: masks.iter().map(as_u32).collect(),
    })
}

/// Read a named `f32` out of a node integrator object (missing -> 0.0).
fn integ_f32(integrator: Option<&Value>, key: &str) -> f32 {
    integrator
        .and_then(|it| it.get(key))
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32
}

/// `m_InitPose[i]` is `[x, y, z, 1, qx, qy, qz, qw]`; pull position + quaternion.
fn read_pose(arr: &[Value]) -> ([f32; 3], [f32; 4]) {
    let g = |i: usize, d: f64| arr.get(i).and_then(Value::as_f64).unwrap_or(d) as f32;
    (
        [g(0, 0.0), g(1, 0.0), g(2, 0.0)],
        [g(4, 0.0), g(5, 0.0), g(6, 0.0), g(7, 1.0)],
    )
}

fn read_rod(v: &Value) -> Option<FeRod> {
    let nn = v.get("nNode").and_then(Value::as_array)?;
    let f = |key: &str, d: f64| v.get(key).and_then(Value::as_f64).unwrap_or(d) as f32;
    Some(FeRod {
        a: as_index(nn.first()?)?,
        b: as_index(nn.get(1)?)?,
        min: f("flMinDist", 0.0),
        max: f("flMaxDist", 0.0),
        relax: f("flRelaxationFactor", 1.0),
        weight: f("flWeight0", 0.0),
    })
}

fn read_capsule(v: &Value) -> Option<FeCapsule> {
    let sp = v.get("vSphere").and_then(Value::as_array)?;
    Some(FeCapsule {
        sphere0: read_sphere(sp.first()?)?,
        sphere1: read_sphere(sp.get(1)?)?,
        node: v.get("nNode").and_then(as_index).unwrap_or(0),
        mask: v.get("nCollisionMask").map_or(0, as_u32),
    })
}

fn read_sphere(v: &Value) -> Option<[f32; 4]> {
    let a = v.as_array()?;
    let g = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    Some([g(0), g(1), g(2), g(3)])
}

fn read_bone_fit(v: &Value) -> [f32; 8] {
    let a = v.as_array();
    let g = |i: usize, d: f64| {
        a.and_then(|a| a.get(i))
            .and_then(Value::as_f64)
            .unwrap_or(d) as f32
    };
    [
        g(0, 0.0),
        g(1, 0.0),
        g(2, 0.0),
        g(3, 1.0),
        g(4, 0.0),
        g(5, 0.0),
        g(6, 0.0),
        g(7, 1.0),
    ]
}

/// A KV3 integer index that may be stored signed or unsigned.
fn as_index(v: &Value) -> Option<usize> {
    if let Some(i) = v.as_int() {
        return usize::try_from(i).ok();
    }
    usize::try_from(v.as_uint()?).ok()
}

/// A KV3 unsigned scalar (mask / count) stored signed or unsigned.
fn as_u32(v: &Value) -> u32 {
    v.as_uint()
        .or_else(|| v.as_int().and_then(|i| u64::try_from(i).ok()))
        .unwrap_or(0) as u32
}

fn u32_field(fe: &Value, key: &str) -> u32 {
    fe.get(key).map_or(0, as_u32)
}

fn index_array(fe: &Value, key: &str) -> Vec<usize> {
    fe.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(as_index).collect())
        .unwrap_or_default()
}

/// Walk the `m_SkelParents` node tree from `start` to its terminal (parent < 0),
/// guarding against cycles and out-of-range indices. Returns the terminal node
/// index, or `None` on a malformed (cyclic) chain.
fn walk_to_terminal(start: usize, parent: &[i64]) -> Option<usize> {
    let mut cur = start;
    for _ in 0..=parent.len() {
        let p = *parent.get(cur)?;
        if p < 0 {
            return Some(cur);
        }
        let p = usize::try_from(p).ok()?;
        if p >= parent.len() || p == cur {
            return Some(cur);
        }
        cur = p;
    }
    None
}

fn is_cloth_node_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("$cloth") || lower.starts_with("cloth")
}

/// Locate the `FeModel` object inside the `PHYS` KV3 tree. It sits under
/// `m_feModel`/`m_pFeModel`, optionally nested in `m_parts[*]`.
fn find_fe_model(root: &Value) -> Option<&Value> {
    if let Some(fe) = root.get("m_feModel").or_else(|| root.get("m_pFeModel")) {
        return Some(fe);
    }
    let parts = root.get("m_parts").and_then(Value::as_array)?;
    parts
        .iter()
        .find_map(|p| p.get("m_pFeModel").or_else(|| p.get("m_feModel")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv3::Value;

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }
    fn i(v: i64) -> Value {
        Value::Int(v)
    }
    fn d(v: f64) -> Value {
        Value::Double(v)
    }
    fn integrator(g: f64, damp: f64, af: f64, av: f64) -> Value {
        Value::Object(vec![
            ("flGravity".into(), d(g)),
            ("flPointDamping".into(), d(damp)),
            ("flAnimationForceAttraction".into(), d(af)),
            ("flAnimationVertexAttraction".into(), d(av)),
        ])
    }
    fn pose(x: f64, y: f64, z: f64) -> Value {
        Value::Array(vec![
            d(x),
            d(y),
            d(z),
            d(1.0),
            d(0.0),
            d(0.0),
            d(0.0),
            d(1.0),
        ])
    }
    fn rod(a: i64, b: i64, min: f64, max: f64, relax: f64, w: f64) -> Value {
        Value::Object(vec![
            ("nNode".into(), Value::Array(vec![i(a), i(b)])),
            ("flMinDist".into(), d(min)),
            ("flMaxDist".into(), d(max)),
            ("flRelaxationFactor".into(), d(relax)),
            ("flWeight0".into(), d(w)),
        ])
    }
    fn capsule(node: i64, mask: i64) -> Value {
        Value::Object(vec![
            (
                "vSphere".into(),
                Value::Array(vec![
                    Value::Array(vec![d(0.0), d(0.0), d(0.0), d(2.0)]),
                    Value::Array(vec![d(0.0), d(0.0), d(5.0), d(1.5)]),
                ]),
            ),
            ("nNode".into(), i(node)),
            ("nCollisionMask".into(), i(mask)),
        ])
    }

    /// A small `FeModel` decodes its nodes/rods/capsules with constants intact,
    /// counts pinned (`inv_mass` 0) nodes, and keeps the 4th integrator lane
    /// (`flAnimationVertexAttraction`) the throwaway JSON exporter dropped.
    #[test]
    fn decodes_nodes_rods_capsules() {
        let fe = Value::Object(vec![
            (
                "m_CtrlName".into(),
                Value::Array(vec![s("pelvis"), s("$cloth_a")]),
            ),
            ("m_NodeInvMasses".into(), Value::Array(vec![d(0.0), d(2.0)])),
            (
                "m_NodeIntegrator".into(),
                Value::Array(vec![
                    integrator(0.0, 0.0, 0.0, 0.0),
                    integrator(360.0, 0.5, 1.0, 0.25),
                ]),
            ),
            (
                "m_InitPose".into(),
                Value::Array(vec![pose(0.0, 0.0, 0.0), pose(1.0, 2.0, 3.0)]),
            ),
            (
                "m_Rods".into(),
                Value::Array(vec![rod(0, 1, 1.0, 2.0, 0.8, 0.5)]),
            ),
            (
                "m_TaperedCapsuleRigids".into(),
                Value::Array(vec![capsule(0, 15)]),
            ),
            ("m_nExtraIterations".into(), Value::UInt(18)),
            ("m_nExtraGoalIterations".into(), Value::UInt(12)),
        ]);
        let root = Value::Object(vec![("m_feModel".into(), fe)]);
        let m = fe_model_from_phys(&root).expect("fe model");

        assert_eq!(m.nodes.len(), 2);
        assert_eq!(m.rods.len(), 1);
        assert_eq!(m.capsules.len(), 1);
        assert_eq!(m.pinned_count(), 1);
        assert_eq!(m.extra_iterations, 18);
        assert_eq!(m.extra_goal_iterations, 12);
        assert!(m.fit_matrices.is_empty());
        assert!(m.fit_weights.is_empty());
        assert_eq!(m.free_nodes, [] as [usize; 0]);
        assert!(m.lock_to_parent.is_empty());
        assert_eq!(m.lock_to_goal, [] as [usize; 0]);
        assert_eq!(m.first_position_driven_node, None);

        assert!(m.nodes[0].is_pinned());
        let cloth = &m.nodes[1];
        assert_eq!(cloth.name, "$cloth_a");
        assert!(!cloth.is_pinned());
        assert!((cloth.gravity - 360.0).abs() < 1e-3);
        assert!((cloth.anim_force - 1.0).abs() < 1e-3);
        assert!((cloth.anim_vertex - 0.25).abs() < 1e-3);
        assert!((cloth.init_pos[0] - 1.0).abs() < 1e-3);

        let r = &m.rods[0];
        assert_eq!((r.a, r.b), (0, 1));
        assert!((r.relax - 0.8).abs() < 1e-3);

        let c = &m.capsules[0];
        assert_eq!(c.node, 0);
        assert_eq!(c.mask, 15);
        assert!((c.sphere0[3] - 2.0).abs() < 1e-3);
    }

    /// The binding fields decode: the dyn-slot fold of collision radius/friction
    /// (static node skipped), the node-base frame, the three offset maps, sphere
    /// rigids, fit-matrix fields, skel parents, and the raw collision tree carried
    /// verbatim.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn decodes_binding_fields() {
        let obj = |pairs: Vec<(&str, Value)>| {
            Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
        };
        let vec3 = |x: f64, y: f64, z: f64| Value::Array(vec![d(x), d(y), d(z)]);
        let bone_fit = || {
            Value::Array(vec![
                d(1.0),
                d(2.0),
                d(3.0),
                d(1.0),
                d(0.1),
                d(0.2),
                d(0.3),
                d(0.9),
            ])
        };
        let fe = obj(vec![
            (
                "m_CtrlName",
                Value::Array(vec![s("pelvis"), s("$cloth_a"), s("$cloth_b")]),
            ),
            // node 0 static, nodes 1+2 dynamic -> dyn slots 0,1
            (
                "m_NodeInvMasses",
                Value::Array(vec![d(0.0), d(2.0), d(2.0)]),
            ),
            ("m_nStaticNodes", Value::UInt(1)),
            ("m_NodeCollisionRadii", Value::Array(vec![d(1.5), d(2.5)])),
            ("m_DynNodeFriction", Value::Array(vec![d(0.3), d(0.6)])),
            ("m_flAddWorldCollisionRadius", d(2.0)),
            ("m_SkelParents", Value::Array(vec![i(-1), i(0), i(1)])),
            (
                "m_NodeBases",
                Value::Array(vec![obj(vec![
                    ("nNode", i(2)),
                    ("nNodeX0", i(1)),
                    ("nNodeX1", i(0)),
                    ("nNodeY0", i(2)),
                    ("nNodeY1", i(1)),
                    (
                        "qAdjust",
                        Value::Array(vec![d(0.0), d(0.0), d(0.0), d(1.0)]),
                    ),
                ])]),
            ),
            (
                "m_CtrlOffsets",
                Value::Array(vec![obj(vec![
                    ("vOffset", vec3(1.0, 2.0, 3.0)),
                    ("nCtrlParent", i(0)),
                    ("nCtrlChild", i(1)),
                ])]),
            ),
            (
                "m_ReverseOffsets",
                Value::Array(vec![obj(vec![
                    ("vOffset", vec3(4.0, 5.0, 6.0)),
                    ("nBoneCtrl", i(1)),
                    ("nTargetNode", i(2)),
                ])]),
            ),
            (
                "m_CtrlSoftOffsets",
                Value::Array(vec![obj(vec![
                    ("vOffset", vec3(7.0, 8.0, 9.0)),
                    ("nCtrlParent", i(1)),
                    ("nCtrlChild", i(2)),
                    ("flAlpha", d(0.5)),
                ])]),
            ),
            (
                "m_SphereRigids",
                Value::Array(vec![obj(vec![
                    (
                        "vSphere",
                        Value::Array(vec![d(0.0), d(0.0), d(0.0), d(3.0)]),
                    ),
                    ("nNode", i(0)),
                    ("nCollisionMask", i(7)),
                ])]),
            ),
            (
                "m_BoxRigids",
                Value::Array(vec![obj(vec![
                    (
                        "tmFrame2",
                        Value::Array(vec![
                            d(1.0),
                            d(2.0),
                            d(3.0),
                            d(1.0),
                            d(0.0),
                            d(0.0),
                            d(0.0),
                            d(1.0),
                        ]),
                    ),
                    ("vSize", vec3(4.0, 5.0, 6.0)),
                    ("nNode", i(0)),
                    ("nCollisionMask", i(15)),
                ])]),
            ),
            (
                "m_AnimStrayRadii",
                Value::Array(vec![obj(vec![
                    ("nNode", Value::Array(vec![i(1), i(2)])),
                    ("flMaxDist", d(7.0)),
                    ("flRelaxationFactor", d(1.0)),
                ])]),
            ),
            (
                "m_FitMatrices",
                Value::Array(vec![
                    obj(vec![
                        ("bone", bone_fit()),
                        ("vCenter", vec3(10.0, 11.0, 12.0)),
                        ("nEnd", i(4)),
                        ("nNode", i(2)),
                        ("nBeginDynamic", i(1)),
                        ("nCtrl", i(7)),
                    ]),
                    obj(vec![
                        ("bone", bone_fit()),
                        ("vCenter", vec3(20.0, 21.0, 22.0)),
                        ("nEnd", i(8)),
                        ("nNode", i(1)),
                        ("nBeginDynamic", i(4)),
                    ]),
                ]),
            ),
            (
                "m_FitWeights",
                Value::Array(vec![obj(vec![
                    ("flWeight", d(0.75)),
                    ("nNode", i(2)),
                    ("nDummy", i(3)),
                ])]),
            ),
            ("m_FreeNodes", Value::Array(vec![i(1), i(2)])),
            (
                "m_LockToParent",
                Value::Array(vec![obj(vec![
                    ("vOffset", vec3(2.0, 4.0, 6.0)),
                    ("nCtrlParent", i(0)),
                    ("nCtrlChild", i(2)),
                ])]),
            ),
            ("m_LockToGoal", Value::Array(vec![i(1), i(2)])),
            ("m_nFirstPositionDrivenNode", Value::UInt(2)),
            ("m_flDefaultGravityScale", d(1.5)),
            // D=2 dynamic nodes -> leaves [0,2), internal node 2, root=2; masks.len()
            // == 2*D-1 == 3 so per-node masks fold (leaf k -> the k-th dynamic node).
            (
                "m_TreeCollisionMasks",
                Value::Array(vec![i(7), i(15), i(65535)]),
            ),
            ("m_TreeParents", Value::Array(vec![i(2), i(2), i(65535)])),
            (
                "m_TreeChildren",
                Value::Array(vec![obj(vec![("nChild", Value::Array(vec![i(1), i(0)]))])]),
            ),
        ]);
        let root = Value::Object(vec![("m_feModel".into(), fe)]);
        let m = fe_model_from_phys(&root).expect("fe model");

        // dyn-slot fold: static node 0 gets 0; dynamic nodes take radii/friction in order.
        assert_eq!(m.static_node_count, 1);
        assert!((m.nodes[0].collide_radius - 0.0).abs() < 1e-6);
        assert!((m.nodes[1].collide_radius - 1.5).abs() < 1e-6);
        assert!((m.nodes[2].collide_radius - 2.5).abs() < 1e-6);
        assert!((m.nodes[1].friction - 0.3).abs() < 1e-6);
        assert!((m.nodes[2].friction - 0.6).abs() < 1e-6);
        assert!((m.add_world_collision_radius - 2.0).abs() < 1e-6);
        // per-node masks fold from the BVH leaves: leaf k -> the k-th dynamic node.
        assert_eq!(m.nodes[0].collision_mask, 0xFFFF); // static node -> collide-all
        assert_eq!(m.nodes[1].collision_mask, 7); // dyn slot 0 -> masks[0]
        assert_eq!(m.nodes[2].collision_mask, 15); // dyn slot 1 -> masks[1]

        assert_eq!(m.skel_parents, vec![-1, 0, 1]);

        assert_eq!(m.node_bases.len(), 1);
        let nb = &m.node_bases[0];
        assert_eq!((nb.node, nb.x0, nb.x1, nb.y0, nb.y1), (2, 1, 0, 2, 1));
        assert!((nb.q_adjust[3] - 1.0).abs() < 1e-6);

        assert_eq!(m.ctrl_offsets.len(), 1);
        assert_eq!((m.ctrl_offsets[0].parent, m.ctrl_offsets[0].child), (0, 1));
        assert!((m.ctrl_offsets[0].offset[0] - 1.0).abs() < 1e-6);

        assert_eq!(m.reverse_offsets.len(), 1);
        assert_eq!(
            (
                m.reverse_offsets[0].bone_ctrl,
                m.reverse_offsets[0].target_node
            ),
            (1, 2)
        );

        assert_eq!(m.ctrl_soft_offsets.len(), 1);
        assert!((m.ctrl_soft_offsets[0].alpha - 0.5).abs() < 1e-6);

        assert_eq!(m.spheres.len(), 1);
        assert_eq!(m.spheres[0].mask, 7);
        assert!((m.spheres[0].sphere[3] - 3.0).abs() < 1e-6);

        assert_eq!(m.boxes.len(), 1);
        assert_eq!(m.boxes[0].node, 0);
        assert_eq!(m.boxes[0].mask, 15);
        assert!((m.boxes[0].pos[0] - 1.0).abs() < 1e-6);
        assert!((m.boxes[0].rot[3] - 1.0).abs() < 1e-6);
        assert!((m.boxes[0].size[0] - 4.0).abs() < 1e-6);

        assert_eq!(m.anim_stray_radii.len(), 1);
        assert_eq!(m.anim_stray_radii[0].node, [1, 2]);
        assert!((m.anim_stray_radii[0].max_dist - 7.0).abs() < 1e-6);

        assert_eq!(m.fit_matrices.len(), 2);
        let fm = &m.fit_matrices[0];
        assert_eq!(
            (fm.end, fm.node, fm.begin_dynamic, fm.ctrl),
            (4, 2, 1, Some(7))
        );
        assert!((fm.bone[0] - 1.0).abs() < 1e-6);
        assert!((fm.bone[7] - 0.9).abs() < 1e-6);
        assert!((fm.center[2] - 12.0).abs() < 1e-6);
        assert_eq!(m.fit_matrices[1].ctrl, None);

        assert_eq!(m.fit_weights.len(), 1);
        assert!((m.fit_weights[0].weight - 0.75).abs() < 1e-6);
        assert_eq!((m.fit_weights[0].node, m.fit_weights[0].dummy), (2, 3));

        assert_eq!(m.free_nodes, vec![1, 2]);
        assert_eq!(m.lock_to_parent.len(), 1);
        assert_eq!(
            (m.lock_to_parent[0].parent, m.lock_to_parent[0].child),
            (0, 2)
        );
        assert!((m.lock_to_parent[0].offset[1] - 4.0).abs() < 1e-6);
        assert_eq!(m.lock_to_goal, vec![1, 2]);
        assert_eq!(m.first_position_driven_node, Some(2));

        assert!((m.default_gravity_scale - 1.5).abs() < 1e-6);

        let tree = m.collision_tree.expect("collision tree");
        assert_eq!(tree.masks, vec![7, 15, 65535]);
        assert_eq!(tree.children, vec![[1, 0]]);
        assert_eq!(tree.parents, vec![2, 2, 65535]);
    }

    /// A node tree where two cloth nodes chain up through cloth parents to a real
    /// driver bone resolves each cloth bone to that bone.
    #[test]
    fn resolves_cloth_nodes_to_terminal_driver_bone() {
        // nodes: 0 driver "pelvis" (root), 1 "$cloth_a" -> 0, 2 "$cloth_b" -> 1
        let fe = Value::Object(vec![
            (
                "m_CtrlName".into(),
                Value::Array(vec![s("pelvis"), s("$cloth_a"), s("$cloth_b")]),
            ),
            (
                "m_SkelParents".into(),
                Value::Array(vec![i(-1), i(0), i(1)]),
            ),
        ]);
        let root = Value::Object(vec![("m_feModel".into(), fe)]);
        let anchors = anchors_from_phys(&root).expect("anchors");
        assert_eq!(anchors.anchor_of("$cloth_a"), Some("pelvis"));
        assert_eq!(anchors.anchor_of("$cloth_b"), Some("pelvis"));
        // The driver bone itself is not in the map (it is FK-posed).
        assert_eq!(anchors.anchor_of("pelvis"), None);
    }

    /// A cyclic chain is dropped rather than looping forever.
    #[test]
    fn cyclic_chain_is_ignored() {
        let fe = Value::Object(vec![
            (
                "m_CtrlName".into(),
                Value::Array(vec![s("$cloth_a"), s("$cloth_b")]),
            ),
            ("m_SkelParents".into(), Value::Array(vec![i(1), i(0)])),
        ]);
        let root = Value::Object(vec![("m_feModel".into(), fe)]);
        assert!(anchors_from_phys(&root).is_none());
    }
}
