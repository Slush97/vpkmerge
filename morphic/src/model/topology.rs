//! Tier-1 topology edits: change *which* geometry a model renders, as opposed to
//! the Tier-0 vertex displacement in [`super::edit`] (which moves existing
//! vertices without changing the draw-call set).
//!
//! The first operation is **remove**: drop the draw call(s) for a given material
//! so that garment/part stops rendering. Removing a draw call touches only the
//! `MDAT` KV3 block (`m_sceneObjects[].m_drawCalls[]`); the shared `MVTX`/`MIDX`
//! buffers and every *surviving* draw call's index range are left untouched (the
//! removed call's slice of the index buffer simply goes unreferenced), so no
//! buffer re-encode or index renumbering is needed. The edited `MDAT` is
//! re-encoded as uncompressed KV3 v4 (the engine-accepted writer) and spliced
//! back with [`Resource::rebuild_with_block`].
//!
//! This is also the cheapest probe of the model-KV3 rewrite path: Tier 0 only
//! ever re-encoded `MVTX`, never a model KV3 block. See
//! `docs/handoff-model-edit.md` (T1a).

use crate::error::DecodeError;
use crate::kv3::{self, Seg, Value};
use crate::resource::Resource;

use super::edit::{build_mesh_buffers_to_layout, parse_embedded, EditedPrimitive, EncodedMesh};
use super::mesh::{self, VertexBuffer};
use super::skeleton;
use super::vbib::BufferDesc;

/// One draw call in a model, located by the blocks/indices needed to address it.
#[derive(Debug, Clone)]
pub struct DrawCallInfo {
    /// Owning embedded mesh name (e.g. `body`, `gun`).
    pub mesh_name: String,
    pub mesh_index: usize,
    /// Primitive ordinal within the owning mesh part's LOD0 draw-call list.
    pub primitive_index: usize,
    /// Global block index of the `MDAT` this draw call lives in.
    pub data_block: usize,
    /// Index into `MDAT.m_sceneObjects`.
    pub scene_object: usize,
    /// Index into that scene object's `m_drawCalls`.
    pub draw_call: usize,
    /// Every local vertex-buffer stream handle referenced by this draw call.
    pub vertex_buffers: Vec<usize>,
    /// Primary local vertex-buffer stream handle.
    pub vertex_buffer: usize,
    /// Local index-buffer handle.
    pub index_buffer: usize,
    /// Global `MVTX` block indices for [`DrawCallInfo::vertex_buffers`].
    pub vertex_blocks: Vec<usize>,
    /// Global primary `MVTX` block index.
    pub vertex_block: usize,
    /// Global `MIDX` block index.
    pub index_block: usize,
    pub start_index: usize,
    pub applied_index_offset: usize,
    pub base_vertex: u32,
    pub primitive_type: String,
    /// Material path the draw call renders with (`m_material`), e.g.
    /// `models/.../vindicta_dress.vmat`.
    pub material: String,
    pub vertex_count: usize,
    pub index_count: usize,
}

/// One draw call dropped by [`remove_draw_calls_by_material`].
#[derive(Debug, Clone)]
pub struct RemovedDrawCall {
    pub mesh_name: String,
    pub data_block: usize,
    pub material: String,
    pub vertex_count: usize,
    pub index_count: usize,
}

/// Lists the renderable (LOD0) draw calls of a `.vmdl_c`, with the material each
/// renders and where it sits. Use it to discover the exact material string to
/// pass to [`remove_draw_calls_by_material`].
///
/// Only LOD0 is listed (the set the player sees up close), to keep the output
/// readable; [`remove_draw_calls_by_material`] still removes a matching material
/// from *every* LOD so the part disappears at all distances.
pub fn draw_call_targets(vmdl_bytes: &[u8]) -> Result<Vec<DrawCallInfo>, DecodeError> {
    let (resource, embedded) = parse_embedded(vmdl_bytes)?;
    let data = kv3::decode(resource.data_block()?)?;
    let lod0 = mesh::lod0_indices(&data, &embedded)?;

    let mut out = Vec::new();
    for &i in &lod0 {
        let em = &embedded[i];
        let mdat_bytes = resource
            .get_block_by_index(em.data_block)
            .ok_or(DecodeError::Model("MDAT block index out of range"))?;
        let mdat = kv3::decode(mdat_bytes)?;
        let Some(scene_objects) = mdat.get("m_sceneObjects").and_then(Value::as_array) else {
            continue;
        };
        let mut primitive_index = 0usize;
        for (so_i, so) in scene_objects.iter().enumerate() {
            let Some(draw_calls) = so.get("m_drawCalls").and_then(Value::as_array) else {
                continue;
            };
            for (dc_i, dc) in draw_calls.iter().enumerate() {
                let vertex_buffers = vertex_buffers_of(dc);
                let vertex_buffer = vertex_buffers.first().copied().unwrap_or(0);
                let index_buffer = dc
                    .get("m_indexBuffer")
                    .map_or(0, |b| usize_field(b, "m_hBuffer"));
                let vertex_blocks: Vec<usize> = vertex_buffers
                    .iter()
                    .filter_map(|&vb| em.vertex_buffers.get(vb).map(|d| d.block_index))
                    .collect();
                let vertex_block = em
                    .vertex_buffers
                    .get(vertex_buffer)
                    .map_or(0, |d| d.block_index);
                let index_block = em
                    .index_buffers
                    .get(index_buffer)
                    .map_or(0, |d| d.block_index);
                out.push(DrawCallInfo {
                    mesh_name: em.name.clone(),
                    mesh_index: em.mesh_index,
                    primitive_index,
                    data_block: em.data_block,
                    scene_object: so_i,
                    draw_call: dc_i,
                    vertex_buffers,
                    vertex_buffer,
                    index_buffer,
                    vertex_blocks,
                    vertex_block,
                    index_block,
                    start_index: usize_field(dc, "m_nStartIndex"),
                    applied_index_offset: usize_field(dc, "m_nAppliedIndexOffset"),
                    base_vertex: u32::try_from(usize_field(dc, "m_nBaseVertex")).unwrap_or(0),
                    primitive_type: dc
                        .get("m_nPrimitiveType")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    material: material_of(dc).to_owned(),
                    vertex_count: usize_field(dc, "m_nVertexCount"),
                    index_count: usize_field(dc, "m_nIndexCount"),
                });
                primitive_index += 1;
            }
        }
    }
    Ok(out)
}

/// Stops every draw call whose material path contains `needle` (case-insensitive
/// substring) from rendering, across all of the model's `MDAT` blocks (every LOD),
/// and splices the edited blocks back. Returns the new `.vmdl_c` bytes and a list
/// of what was neutralized.
///
/// The draw call is **not** deleted from the array (that would force a lossy KV3
/// re-encode the engine's model loader rejects: it drops value flags and
/// auxiliary-buffer typed-array tags that `MDAT` relies on). Instead the `MDAT`
/// block is re-wrapped uncompressed but otherwise byte-for-byte (preserving all of
/// that structure) and the matching draw calls' `m_nIndexCount` is zeroed in place,
/// so they submit no primitives. The shared vertex/index buffers and every other
/// draw call are untouched; per-scene-object bounds stay a (still-valid) superset.
///
/// Errors with [`DecodeError::Model`] if `needle` matches no draw call, so a
/// typo'd material name fails loudly instead of repacking the model unchanged.
pub fn remove_draw_calls_by_material(
    vmdl_bytes: &[u8],
    needle: &str,
) -> Result<(Vec<u8>, Vec<RemovedDrawCall>), DecodeError> {
    let needle_lc = needle.to_ascii_lowercase();
    let matches = |material: &str| material.to_ascii_lowercase().contains(&needle_lc);

    let (resource, embedded) = parse_embedded(vmdl_bytes)?;

    // Edit each distinct MDAT once, collecting (block_index, new_bytes).
    let mut edits: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut removed: Vec<RemovedDrawCall> = Vec::new();
    let mut done: Vec<usize> = Vec::new();
    for em in &embedded {
        if done.contains(&em.data_block) {
            continue;
        }
        done.push(em.data_block);

        let mdat_bytes = resource
            .get_block_by_index(em.data_block)
            .ok_or(DecodeError::Model("MDAT block index out of range"))?;
        let tree = kv3::decode(mdat_bytes)?;
        let found = find_matching_draw_calls(&tree, &matches);
        if found.is_empty() {
            continue;
        }

        let targets: Vec<(usize, usize)> = found
            .iter()
            .map(|f| (f.scene_object, f.draw_call))
            .collect();
        let patched = kv3::neutralize_draw_calls(mdat_bytes, &targets)?;
        for f in found {
            removed.push(RemovedDrawCall {
                mesh_name: em.name.clone(),
                data_block: em.data_block,
                material: f.material,
                vertex_count: f.vertex_count,
                index_count: f.index_count,
            });
        }
        edits.push((em.data_block, patched));
    }

    if edits.is_empty() {
        return Err(DecodeError::Model(
            "no draw call matched the given material",
        ));
    }

    // Apply each MDAT swap. `rebuild_with_block` preserves block order/count, so
    // the indices collected from the original parse stay valid across rebuilds.
    let mut bytes = vmdl_bytes.to_vec();
    for (block_index, new_mdat) in &edits {
        let resource = Resource::parse(&bytes)?;
        bytes = resource.rebuild_with_block(*block_index, new_mdat)?;
    }
    Ok((bytes, removed))
}

/// Diagnostic: re-emits every distinct `MDAT` block **uncompressed but otherwise
/// byte-faithful** (via [`kv3::rewrap_uncompressed`], preserving value flags and
/// typed-array tags), then splices them all back. The decoded tree is identical to
/// the input; its only purpose is the in-game probe "does the engine accept our
/// re-emitted model KV3 blocks at all?" independent of any edit. If even this fails
/// to load, the problem is the resource container rebuild, not the KV3 encoding.
/// Returns the new bytes and the number of `MDAT` blocks re-emitted.
pub fn reencode_all_mdat_identity(vmdl_bytes: &[u8]) -> Result<(Vec<u8>, usize), DecodeError> {
    let (resource, embedded) = parse_embedded(vmdl_bytes)?;
    let mut edits: Vec<(usize, Vec<u8>)> = Vec::new();
    let mut done: Vec<usize> = Vec::new();
    for em in &embedded {
        if done.contains(&em.data_block) {
            continue;
        }
        done.push(em.data_block);
        let mdat_bytes = resource
            .get_block_by_index(em.data_block)
            .ok_or(DecodeError::Model("MDAT block index out of range"))?;
        edits.push((em.data_block, kv3::rewrap_uncompressed(mdat_bytes)?));
    }
    let count = edits.len();
    let mut bytes = vmdl_bytes.to_vec();
    for (block_index, new_mdat) in &edits {
        let resource = Resource::parse(&bytes)?;
        bytes = resource.rebuild_with_block(*block_index, new_mdat)?;
    }
    Ok((bytes, count))
}

/// One draw call located by [`find_matching_draw_calls`]: its `(scene_object,
/// draw_call)` indices within the `MDAT` plus the info reported back to the caller.
pub(super) struct FoundDrawCall {
    pub scene_object: usize,
    pub draw_call: usize,
    pub material: String,
    pub vertex_count: usize,
    pub index_count: usize,
}

/// Locates every draw call matching `matches` in an `MDAT` tree, returning its
/// `m_sceneObjects`/`m_drawCalls` indices (for [`kv3::neutralize_draw_calls`]) and
/// the reportable counts. Read-only: nothing is mutated. Split out so the offline
/// fixture test can exercise it on a bare `MDAT` tree without a full `.vmdl_c`.
pub(super) fn find_matching_draw_calls(
    mdat: &Value,
    matches: &impl Fn(&str) -> bool,
) -> Vec<FoundDrawCall> {
    let mut out = Vec::new();
    let Some(scene_objects) = mdat.get("m_sceneObjects").and_then(Value::as_array) else {
        return out;
    };
    for (so_i, so) in scene_objects.iter().enumerate() {
        let Some(draw_calls) = so.get("m_drawCalls").and_then(Value::as_array) else {
            continue;
        };
        for (dc_i, dc) in draw_calls.iter().enumerate() {
            let material = material_of(dc);
            if matches(material) {
                out.push(FoundDrawCall {
                    scene_object: so_i,
                    draw_call: dc_i,
                    material: material.to_owned(),
                    vertex_count: usize_field(dc, "m_nVertexCount"),
                    index_count: usize_field(dc, "m_nIndexCount"),
                });
            }
        }
    }
    out
}

// --- T1d-d: replace one mesh part's geometry in place (the wedge) ---

/// What [`replace_mesh_part`] swapped, for the CLI report.
#[derive(Debug, Clone)]
pub struct ReplacedMeshPart {
    pub mesh_name: String,
    pub material: String,
    pub old_vertex_count: usize,
    pub new_vertex_count: usize,
    pub old_index_count: usize,
    pub new_index_count: usize,
    /// New vertex stride in bytes.
    pub stride: usize,
    /// New index width in bytes (2 or 4).
    pub index_size: usize,
}

/// A selected primitive within a target model, addressed by stable draw-call
/// metadata from [`draw_call_targets`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PrimitiveSelection {
    pub mesh_index: usize,
    pub primitive_index: usize,
}

/// What [`replace_mesh_group`] swapped.
#[derive(Debug, Clone)]
pub struct ReplacedMeshGroup {
    pub replaced_parts: Vec<ReplacedMeshPart>,
    pub replaced_draw_calls: usize,
}

/// Replaces an existing mesh part's geometry with a brand-new mesh of *any*
/// vertex/index count, **in place** (the T1d wedge from `docs/handoff-model-edit.md`).
/// Unlike a true additive insert (deferred), this reuses the target part's block
/// slots, material, and bone palette, so it needs only block *swaps* plus in-place
/// *scalar* edits, never KV3 array growth or a container block-table grow:
///
/// 1. Localize the new mesh's model-space `JOINTS_0` into the target mesh's bone
///    palette (T1d-c), so the existing remap table needs no edit.
/// 2. Assemble + encode the new buffer to the target's exact `m_inputLayoutFields`
///    field set, uncompressed (T1d-b), so the layout's element count is unchanged.
/// 3. Swap the `MVTX`/`MIDX` blocks (reusing their block indices).
/// 4. `set_scalars` the `CTRL` registry (buffer element counts, stride, each
///    field's format/offset, index width) and the `MDAT` draw call (vertex/index
///    counts, start index 0, base vertex 0).
///
/// The target part must have exactly one vertex buffer, one index buffer, and one
/// draw call (the clean wedge; the gun part fits). The new mesh is skinned against
/// the model skeleton (glTF joint indices == model bone indices); a skinned new
/// mesh requires the target to have a bone remap table. Errors loudly on any of
/// these contract violations rather than producing a broken model.
///
/// Returns the new `.vmdl_c` bytes and a report of what changed.
///
/// The swapped buffers are written with morphic's meshopt codec (v1). That codec
/// round-trips through morphic's own decoder but is **not byte-compatible with the
/// engine's meshopt decoder** and garbles in game. For a model that must load in
/// Deadlock, use [`replace_mesh_part_uncompressed`], which writes raw buffers and
/// flips `m_bMeshoptCompressed` to false (the engine reads uncompressed natively).
pub fn replace_mesh_part(
    vmdl_bytes: &[u8],
    mesh_name: &str,
    new_mesh: &VertexBuffer,
    indices: &[u32],
) -> Result<(Vec<u8>, ReplacedMeshPart), DecodeError> {
    replace_mesh_part_impl(vmdl_bytes, mesh_name, new_mesh, indices, true)
}

/// Like [`replace_mesh_part`], but writes the new vertex/index buffers
/// **uncompressed** and flips their `m_bMeshoptCompressed` flags to false in the
/// `CTRL` registry. This is the engine-loadable path: morphic's meshopt encoder
/// emits codec v1 (`0xa1`/`0xe1`) which Deadlock's decoder garbles, whereas the
/// engine reads uncompressed vertex/index buffers natively (hero models ship
/// them). Use this when the output is meant to run in game (e.g. soul-container
/// imports); the file is larger but correct.
pub fn replace_mesh_part_uncompressed(
    vmdl_bytes: &[u8],
    mesh_name: &str,
    new_mesh: &VertexBuffer,
    indices: &[u32],
) -> Result<(Vec<u8>, ReplacedMeshPart), DecodeError> {
    replace_mesh_part_impl(vmdl_bytes, mesh_name, new_mesh, indices, false)
}

#[allow(clippy::too_many_lines)] // one cohesive wedge; splitting hurts readability
fn replace_mesh_part_impl(
    vmdl_bytes: &[u8],
    mesh_name: &str,
    new_mesh: &VertexBuffer,
    indices: &[u32],
    compressed: bool,
) -> Result<(Vec<u8>, ReplacedMeshPart), DecodeError> {
    let (resource, embedded) = parse_embedded(vmdl_bytes)?;

    let mesh_pos = embedded
        .iter()
        .position(|em| em.name == mesh_name)
        .ok_or(DecodeError::Model("no embedded mesh with that name"))?;
    let em = &embedded[mesh_pos];
    if em.vertex_buffers.len() != 1 || em.index_buffers.len() != 1 {
        return Err(DecodeError::Model(
            "replace-in-place needs a mesh part with exactly one vertex buffer and one index \
             buffer (LOD-split parts like the body are not supported by the wedge)",
        ));
    }
    let vb_desc = &em.vertex_buffers[0];
    let ib_desc = &em.index_buffers[0];
    let mdat_block = em.data_block;
    let vb_block = vb_desc.block_index;
    let ib_block = ib_desc.block_index;
    let old_vertex_count = vb_desc.element_count;
    let old_index_count = ib_desc.element_count;

    // Localize the new mesh's JOINTS_0 into the target mesh's bone palette (T1d-c).
    let data = kv3::decode(resource.data_block()?)?;
    let remap = skeleton::remap_table(&data, em.mesh_index);
    let mut local = new_mesh.clone();
    if !new_mesh.joints.is_empty() {
        let table = remap.as_deref().ok_or(DecodeError::Model(
            "new mesh is skinned but the target mesh has no bone remap table",
        ))?;
        let weights = (new_mesh.weights.len() == new_mesh.element_count)
            .then_some(new_mesh.weights.as_slice());
        local.joints = skeleton::localize_joints(&new_mesh.joints, weights, table)?;
    }

    // Assemble to the target's field set + encode both buffers (T1d-b).
    let enc = build_mesh_buffers_to_layout(&local, indices, &vb_desc.fields)?;
    let new_vertex_count = enc.vertex_count;
    let new_index_count = enc.index_count;
    let stride = enc.stride;
    let index_size = enc.index_size;

    // The single draw call this part renders (the clean-wedge contract).
    let mdat_bytes = resource
        .get_block_by_index(mdat_block)
        .ok_or(DecodeError::Model("MDAT block index out of range"))?;
    let mdat = kv3::decode(mdat_bytes)?;
    let (so_idx, dc_idx, material) = locate_single_draw_call(&mdat)?;
    let draw_call = mdat
        .get("m_sceneObjects")
        .and_then(Value::as_array)
        .and_then(|a| a.get(so_idx))
        .and_then(|so| so.get("m_drawCalls"))
        .and_then(Value::as_array)
        .and_then(|a| a.get(dc_idx))
        .ok_or(DecodeError::Model("draw call vanished after locate"))?;

    // Compute the scalar edits for the buffer registry (CTRL) and draw call (MDAT).
    let ctrl_idx = resource
        .blocks()
        .iter()
        .position(|b| &b.kind == b"CTRL")
        .ok_or(DecodeError::Model("model has no CTRL block"))?;
    let ctrl_bytes = resource
        .find_block(*b"CTRL")
        .ok_or(DecodeError::Model("model has no CTRL block"))?;
    let ctrl_edits = ctrl_edits_for(mesh_pos, vb_desc, ib_desc, &enc)?;
    let mdat_edits = mdat_edits_for(so_idx, dc_idx, draw_call, new_vertex_count, new_index_count)?;

    // Splice the changed blocks. Block order/count is preserved, so block indices
    // (and the CTRL references to them) stay valid across each rebuild. CTRL/MDAT
    // are only re-emitted when they actually changed (an unchanged-count replace
    // touches only MVTX/MIDX, like a Tier-0 edit).
    let mut swaps: Vec<(usize, Vec<u8>)> = Vec::with_capacity(4);
    // CTRL carries the buffer-count/stride scalar edits and, for the uncompressed
    // path, the two m_bMeshoptCompressed flag flips. Both land on the same CTRL
    // block, so apply scalars then bools to one working copy.
    let mut new_ctrl: Option<Vec<u8>> = if ctrl_edits.is_empty() {
        None
    } else {
        Some(kv3::set_scalars(ctrl_bytes, &ctrl_edits)?)
    };
    if !compressed {
        let base = new_ctrl.as_deref().unwrap_or(ctrl_bytes);
        let vb_flag = vec![
            seg("embedded_meshes"),
            Seg::Index(mesh_pos),
            seg("m_vertexBuffers"),
            Seg::Index(0),
            seg("m_bMeshoptCompressed"),
        ];
        let ib_flag = vec![
            seg("embedded_meshes"),
            Seg::Index(mesh_pos),
            seg("m_indexBuffers"),
            Seg::Index(0),
            seg("m_bMeshoptCompressed"),
        ];
        new_ctrl = Some(kv3::set_bools(base, &[(vb_flag, false), (ib_flag, false)])?);
    }
    if let Some(ctrl) = new_ctrl {
        swaps.push((ctrl_idx, ctrl));
    }
    if !mdat_edits.is_empty() {
        swaps.push((mdat_block, kv3::set_scalars(mdat_bytes, &mdat_edits)?));
    }
    let (vtx_payload, idx_payload) = if compressed {
        (enc.mvtx, enc.midx)
    } else {
        (enc.mvtx_raw, enc.midx_raw)
    };
    swaps.push((vb_block, vtx_payload));
    swaps.push((ib_block, idx_payload));

    let mut bytes = vmdl_bytes.to_vec();
    for (idx, payload) in swaps {
        let res = Resource::parse(&bytes)?;
        bytes = res.rebuild_with_block(idx, &payload)?;
    }

    Ok((
        bytes,
        ReplacedMeshPart {
            mesh_name: mesh_name.to_string(),
            material,
            old_vertex_count,
            new_vertex_count,
            old_index_count,
            new_index_count,
            stride,
            index_size,
        },
    ))
}

/// Repoints the model's single draw-call material to `new_material` (a `.vmat`
/// path), in place: the new path is appended to the `MDAT` string table and the
/// draw call's `m_material` (or legacy `m_pMaterial`) field is redirected to it.
///
/// This is what makes a reskinned soul container **additive** rather than a graft
/// over vanilla: the model references a fresh, uniquely-named material (e.g.
/// `.../my_skin.vmat`) that the mod ships alongside, instead of overriding the
/// stock `soul_container.vmat`. The engine binds by this `m_material` string at
/// render time; the `RERL` precache list (which still names the old material) is a
/// dependency hint, not the render binding, so it is intentionally left untouched.
///
/// Requires the model to have exactly one draw call (the soul-container shape) and
/// a KV3 v5 `MDAT` (so a brand-new string can be appended). Errors otherwise.
pub fn set_model_material(vmdl_bytes: &[u8], new_material: &str) -> Result<Vec<u8>, DecodeError> {
    let targets = draw_call_targets(vmdl_bytes)?;
    let [dc] = targets.as_slice() else {
        return Err(DecodeError::Model(
            "set_model_material requires exactly one draw call",
        ));
    };

    let resource = Resource::parse(vmdl_bytes)?;
    let mdat_bytes = resource
        .get_block_by_index(dc.data_block)
        .ok_or(DecodeError::Model("MDAT block index out of range"))?;

    // Pick the key the draw call actually uses (modern m_material / legacy
    // m_pMaterial), mirroring material_of.
    let mdat = kv3::decode(mdat_bytes)?;
    let draw_call = mdat
        .get("m_sceneObjects")
        .and_then(Value::as_array)
        .and_then(|a| a.get(dc.scene_object))
        .and_then(|so| so.get("m_drawCalls"))
        .and_then(Value::as_array)
        .and_then(|a| a.get(dc.draw_call))
        .ok_or(DecodeError::Model("draw call not found in MDAT"))?;
    let key = if draw_call.get("m_material").is_some() {
        "m_material"
    } else if draw_call.get("m_pMaterial").is_some() {
        "m_pMaterial"
    } else {
        return Err(DecodeError::Model("draw call has no material field"));
    };

    let path = vec![
        Seg::Key("m_sceneObjects".to_string()),
        Seg::Index(dc.scene_object),
        Seg::Key("m_drawCalls".to_string()),
        Seg::Index(dc.draw_call),
        Seg::Key(key.to_string()),
    ];
    let new_mdat = kv3::set_strings_adding(mdat_bytes, &[(path, new_material.to_string())])?;
    resource.rebuild_with_block(dc.data_block, &new_mdat)
}

/// One material's draw call when expanding a single-draw-call model into a
/// multi-material one. The group owns a contiguous `[start_index, start_index +
/// index_count)` slice of the model's single (already globally-rebased) index
/// buffer and renders `material`. `vertex_start` is written to
/// `m_nAppliedIndexOffset`, and `vertex_end` is written to `m_nVertexCount`,
/// matching resourcecompiler's per-material vertex range convention.
#[derive(Debug, Clone)]
pub struct DrawCallGroup {
    /// The `.vmat` path this group renders with (no `_c`).
    pub material: String,
    /// First index of this group's slice of the shared index buffer.
    pub start_index: usize,
    /// Number of indices in this group's slice.
    pub index_count: usize,
    /// First vertex in the shared vertex buffer for this group.
    pub vertex_start: usize,
    /// Exclusive end vertex in the shared vertex buffer for this group.
    pub vertex_end: usize,
}

/// Expands a model that currently has exactly ONE draw call into
/// `groups.len()` draw calls, one per material group.
///
/// This is the multi-material companion to [`set_model_material`]. Source 2
/// binds a material per draw call (`m_sceneObjects[0].m_drawCalls[].m_material`)
/// and partitions one shared index buffer between them via each call's
/// `m_nStartIndex`/`m_nIndexCount`; each draw also carries a vertex-range bound
/// via `m_nAppliedIndexOffset`/`m_nVertexCount`. So the expansion is: clone the
/// single draw call `groups.len() - 1` times (growing the `m_drawCalls` array
/// byte-faithfully via [`kv3::insert_array_element_adding`]), then repoint each
/// clone's index slice/range ([`kv3::set_scalars`]) and material
/// ([`kv3::set_strings_adding`]). No block re-encode, no RERL edit (the precache
/// list is a hint, not the render binding).
///
/// Call this AFTER [`replace_mesh_part_uncompressed`] has swapped in the merged
/// geometry (which leaves the part with one draw call covering the whole mesh).
/// `total_vertex_count` is retained as a consistency guard for older callers:
/// every group's `vertex_end` must be within that shared buffer.
pub fn set_draw_call_groups(
    vmdl_bytes: &[u8],
    groups: &[DrawCallGroup],
    total_vertex_count: usize,
) -> Result<Vec<u8>, DecodeError> {
    if groups.is_empty() {
        return Err(DecodeError::Model("need at least one draw-call group"));
    }
    for group in groups {
        if group.vertex_start > group.vertex_end || group.vertex_end > total_vertex_count {
            return Err(DecodeError::Model(
                "draw-call group vertex range is outside the shared vertex buffer",
            ));
        }
    }

    let resource = Resource::parse(vmdl_bytes)?;
    // The single draw call lives in the part's MDAT block; find it the same way
    // draw_call_targets does, then require exactly one (the wedge contract).
    let dc = {
        let targets = draw_call_targets(vmdl_bytes)?;
        let [only] = targets.as_slice() else {
            return Err(DecodeError::Model(
                "set_draw_call_groups requires the model to start with exactly one draw call",
            ));
        };
        only.clone()
    };
    let mdat_block = dc.data_block;
    let so_idx = dc.scene_object;
    let dc_idx = dc.draw_call;

    let mdat_bytes = resource
        .get_block_by_index(mdat_block)
        .ok_or(DecodeError::Model("MDAT block index out of range"))?;
    let mdat = kv3::decode(mdat_bytes)?;
    let template = mdat
        .get("m_sceneObjects")
        .and_then(Value::as_array)
        .and_then(|a| a.get(so_idx))
        .and_then(|so| so.get("m_drawCalls"))
        .and_then(Value::as_array)
        .and_then(|a| a.get(dc_idx))
        .ok_or(DecodeError::Model("draw call not found in MDAT"))?
        .clone();
    let mat_key = if template.get("m_material").is_some() {
        "m_material"
    } else if template.get("m_pMaterial").is_some() {
        "m_pMaterial"
    } else {
        return Err(DecodeError::Model("draw call has no material field"));
    };

    let dcs_path = vec![
        seg("m_sceneObjects"),
        Seg::Index(so_idx),
        seg("m_drawCalls"),
    ];

    // 1) Grow the m_drawCalls array to groups.len() by inserting clones of the
    //    single existing draw call after it (indices dc_idx+1..). Each clone has
    //    its group's index slice + material BAKED into the Value before
    //    insertion: a non-zero m_nStartIndex/m_nIndexCount must be stored with a
    //    real data lane (KV3 encodes a non-zero int that fits i32 as INT32),
    //    because a tagless 0 (the template's m_nStartIndex) has no byte to
    //    overwrite later. Baking also means the post-insert set_scalars pass sees
    //    current == new for these clones and skips them.
    let mut bytes = mdat_bytes.to_vec();
    for (i, g) in groups.iter().enumerate().skip(1) {
        let mut clone = template.clone();
        bake_draw_call(&mut clone, g, mat_key);
        bytes = kv3::insert_array_element_adding(&bytes, &dcs_path, dc_idx + i, &clone)?;
    }

    // 2) Repoint every draw call's material in one string pass.
    let str_edits: Vec<(Vec<Seg>, String)> = groups
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let mut p = dcs_path.clone();
            p.push(Seg::Index(dc_idx + i));
            p.push(seg(mat_key));
            (p, g.material.clone())
        })
        .collect();
    bytes = kv3::set_strings_adding(&bytes, &str_edits)?;

    // 3) Set each draw call's index slice (start/count), pin base vertex to 0,
    //    and record the shared vertex-count upper bound. Read current values off
    //    the grown block so push_if_changed can skip tagless 0/1 no-ops.
    let grown = kv3::decode(&bytes)?;
    let grown_dcs = grown
        .get("m_sceneObjects")
        .and_then(Value::as_array)
        .and_then(|a| a.get(so_idx))
        .and_then(|so| so.get("m_drawCalls"))
        .and_then(Value::as_array)
        .ok_or(DecodeError::Model("draw calls vanished after growth"))?;
    if grown_dcs.len() != groups.len() {
        return Err(DecodeError::Model(
            "draw-call array did not grow to the expected length",
        ));
    }
    let scal_edits = draw_call_range_edits(grown_dcs, so_idx, dc_idx, groups)?;
    if !scal_edits.is_empty() {
        bytes = kv3::set_scalars(&bytes, &scal_edits)?;
    }

    resource.rebuild_with_block(mdat_block, &bytes)
}

/// The scalar edits that pin each draw call's index slice (start/count), applied
/// index offset, base vertex (0), and vertex-range end bound. Reads current
/// values off the grown draw-call array so `push_if_changed` skips tagless-0
/// no-ops (a baked clone's fields already match).
fn draw_call_range_edits(
    grown_dcs: &[Value],
    so_idx: usize,
    dc_idx: usize,
    groups: &[DrawCallGroup],
) -> Result<Vec<(Vec<Seg>, i64)>, DecodeError> {
    let mut edits: Vec<(Vec<Seg>, i64)> = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        let dc_val = &grown_dcs[i];
        let base = vec![
            seg("m_sceneObjects"),
            Seg::Index(so_idx),
            seg("m_drawCalls"),
            Seg::Index(dc_idx + i),
        ];
        for (key, new) in [
            ("m_nStartIndex", g.start_index),
            ("m_nIndexCount", g.index_count),
            ("m_nAppliedIndexOffset", g.vertex_start),
            ("m_nVertexCount", g.vertex_end),
            ("m_nBaseVertex", 0),
        ] {
            push_if_changed(&mut edits, child(&base, key), dc_uint(dc_val, key), new)?;
        }
    }
    Ok(edits)
}

/// Replaces a semantic group that can span multiple draw calls. Each selected
/// target primitive is sourced from one donor GLB primitive; unselected draw
/// calls in the same mesh part are preserved by copying their existing geometry
/// into the rebuilt buffer.
///
/// This still uses the proven in-place wedge: no new blocks, no KV3 array growth,
/// and no material/RERL edits. Each touched mesh part must therefore have exactly
/// one vertex buffer and one index buffer. Multiple draw calls over that buffer
/// are supported.
pub fn replace_mesh_group(
    vmdl_bytes: &[u8],
    selections: &[PrimitiveSelection],
    donor_primitives: &[EditedPrimitive],
) -> Result<(Vec<u8>, ReplacedMeshGroup), DecodeError> {
    if selections.is_empty() {
        return Err(DecodeError::Model(
            "replace group matched no target draw calls",
        ));
    }
    if donor_primitives.is_empty() {
        return Err(DecodeError::Model("donor glb has no mesh primitives"));
    }

    let mut by_mesh: std::collections::BTreeMap<usize, std::collections::BTreeSet<usize>> =
        std::collections::BTreeMap::new();
    for sel in selections {
        by_mesh
            .entry(sel.mesh_index)
            .or_default()
            .insert(sel.primitive_index);
    }

    let mut bytes = vmdl_bytes.to_vec();
    let mut replaced_parts = Vec::new();
    let mut replaced_draw_calls = 0usize;
    for (mesh_index, primitive_indices) in by_mesh {
        let (next, replaced) =
            replace_mesh_group_part(&bytes, mesh_index, &primitive_indices, donor_primitives)?;
        replaced_draw_calls += primitive_indices.len();
        replaced_parts.push(replaced);
        bytes = next;
    }

    Ok((
        bytes,
        ReplacedMeshGroup {
            replaced_parts,
            replaced_draw_calls,
        },
    ))
}

#[allow(clippy::too_many_lines)]
fn replace_mesh_group_part(
    vmdl_bytes: &[u8],
    mesh_index: usize,
    selected_primitives: &std::collections::BTreeSet<usize>,
    donor_primitives: &[EditedPrimitive],
) -> Result<(Vec<u8>, ReplacedMeshPart), DecodeError> {
    let (resource, embedded) = parse_embedded(vmdl_bytes)?;
    let mesh_pos = embedded
        .iter()
        .position(|em| em.mesh_index == mesh_index)
        .ok_or(DecodeError::Model("no embedded mesh with that mesh index"))?;
    let em = &embedded[mesh_pos];
    if em.vertex_buffers.len() != 1 || em.index_buffers.len() != 1 {
        return Err(DecodeError::Model(
            "replace-group needs each touched mesh part to have exactly one vertex buffer and \
             one index buffer; multi-stream parts need a future structural layout editor",
        ));
    }

    let data = kv3::decode(resource.data_block()?)?;
    let remap = skeleton::remap_table(&data, em.mesh_index);
    let part = mesh::assemble(em, &resource, remap.as_deref())?;
    if selected_primitives
        .iter()
        .any(|&i| i >= part.primitives.len())
    {
        return Err(DecodeError::Model(
            "selected draw call is outside the target mesh",
        ));
    }

    let donor_assignment = assign_donor_primitives(&part, selected_primitives, donor_primitives)?;
    let mut sources = Vec::with_capacity(part.primitives.len());
    for (prim_i, prim) in part.primitives.iter().enumerate() {
        if let Some(&donor_i) = donor_assignment.get(&prim_i) {
            let donor = &donor_primitives[donor_i];
            sources.push(PrimitiveSource {
                vb: donor.vertex_buffer.clone(),
                indices: donor.indices.clone(),
            });
        } else {
            let vb = part
                .vertex_buffers
                .get(prim.vertex_buffer)
                .ok_or(DecodeError::Model("primitive vertex buffer out of range"))?
                .clone();
            sources.push(PrimitiveSource {
                vb,
                indices: prim.indices.clone(),
            });
        }
    }

    let (combined, indices, ranges) = combine_primitive_sources(&sources)?;
    let mut local = combined;
    if !local.joints.is_empty() {
        let table = remap.as_deref().ok_or(DecodeError::Model(
            "group mesh is skinned but the target mesh has no bone remap table",
        ))?;
        let weights =
            (local.weights.len() == local.element_count).then_some(local.weights.as_slice());
        local.joints = skeleton::localize_joints(&local.joints, weights, table)?;
    }

    let vb_desc = &em.vertex_buffers[0];
    let ib_desc = &em.index_buffers[0];
    let enc = build_mesh_buffers_to_layout(&local, &indices, &vb_desc.fields)?;

    let mdat_bytes = resource
        .get_block_by_index(em.data_block)
        .ok_or(DecodeError::Model("MDAT block index out of range"))?;
    let mdat = kv3::decode(mdat_bytes)?;
    let draw_calls = locate_draw_calls(&mdat)?;
    if draw_calls.len() != part.primitives.len() {
        return Err(DecodeError::Model(
            "decoded primitive count does not match MDAT draw-call count",
        ));
    }

    let ctrl_idx = resource
        .blocks()
        .iter()
        .position(|b| &b.kind == b"CTRL")
        .ok_or(DecodeError::Model("model has no CTRL block"))?;
    let ctrl_bytes = resource
        .find_block(*b"CTRL")
        .ok_or(DecodeError::Model("model has no CTRL block"))?;
    let ctrl_edits = ctrl_edits_for(mesh_pos, vb_desc, ib_desc, &enc)?;

    let mut mdat_edits = Vec::new();
    for (i, (so_idx, dc_idx, draw_call)) in draw_calls.iter().enumerate() {
        let r = ranges
            .get(i)
            .ok_or(DecodeError::Model("missing rebuilt primitive range"))?;
        mdat_edits.extend(mdat_edits_for_range(
            *so_idx,
            *dc_idx,
            draw_call,
            enc.vertex_count,
            r.index_count,
            r.start_index,
            0,
        )?);
    }

    let mut swaps: Vec<(usize, Vec<u8>)> = Vec::with_capacity(4);
    if !ctrl_edits.is_empty() {
        swaps.push((ctrl_idx, kv3::set_scalars(ctrl_bytes, &ctrl_edits)?));
    }
    if !mdat_edits.is_empty() {
        swaps.push((em.data_block, kv3::set_scalars(mdat_bytes, &mdat_edits)?));
    }
    swaps.push((vb_desc.block_index, enc.mvtx));
    swaps.push((ib_desc.block_index, enc.midx));

    let mut bytes = vmdl_bytes.to_vec();
    for (idx, payload) in swaps {
        let res = Resource::parse(&bytes)?;
        bytes = res.rebuild_with_block(idx, &payload)?;
    }

    let materials: Vec<String> = selected_primitives
        .iter()
        .filter_map(|&i| part.primitives.get(i).map(|p| p.material.clone()))
        .collect();
    Ok((
        bytes,
        ReplacedMeshPart {
            mesh_name: part.name,
            material: materials.join(", "),
            old_vertex_count: vb_desc.element_count,
            new_vertex_count: enc.vertex_count,
            old_index_count: ib_desc.element_count,
            new_index_count: enc.index_count,
            stride: enc.stride,
            index_size: enc.index_size,
        },
    ))
}

struct PrimitiveSource {
    vb: VertexBuffer,
    indices: Vec<u32>,
}

#[derive(Clone, Copy)]
struct PrimitiveRange {
    start_index: usize,
    index_count: usize,
}

fn assign_donor_primitives(
    part: &mesh::MeshPart,
    selected_primitives: &std::collections::BTreeSet<usize>,
    donors: &[EditedPrimitive],
) -> Result<std::collections::BTreeMap<usize, usize>, DecodeError> {
    let selected_count = selected_primitives.len();
    let exact_mesh: Vec<usize> = donors
        .iter()
        .enumerate()
        .filter(|(_, d)| {
            d.mesh_name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(&part.name))
        })
        .map(|(i, _)| i)
        .collect();
    let candidates: Vec<usize> = if exact_mesh.len() >= selected_count {
        exact_mesh
    } else if donors.len() == selected_count {
        (0..donors.len()).collect()
    } else {
        donors
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                d.mesh_name
                    .as_deref()
                    .is_none_or(|n| n.eq_ignore_ascii_case(&part.name))
            })
            .map(|(i, _)| i)
            .collect()
    };

    let mut used = std::collections::BTreeSet::new();
    let mut out = std::collections::BTreeMap::new();
    for &prim_i in selected_primitives {
        let target = part.primitives.get(prim_i).ok_or(DecodeError::Model(
            "selected draw call is outside the target mesh",
        ))?;
        let donor_i = candidates
            .iter()
            .copied()
            .find(|i| {
                !used.contains(i)
                    && donors[*i]
                        .material_name
                        .as_deref()
                        .is_some_and(|m| material_names_match(m, &target.material))
            })
            .or_else(|| candidates.iter().copied().find(|i| !used.contains(i)))
            .ok_or(DecodeError::Model(
                "donor glb does not contain enough matching primitives for the selected group",
            ))?;
        used.insert(donor_i);
        out.insert(prim_i, donor_i);
    }
    Ok(out)
}

#[allow(clippy::too_many_lines)]
fn combine_primitive_sources(
    sources: &[PrimitiveSource],
) -> Result<(VertexBuffer, Vec<u32>, Vec<PrimitiveRange>), DecodeError> {
    let total: usize = sources.iter().map(|s| s.vb.element_count).sum();
    if total == 0 {
        return Err(DecodeError::Model(
            "cannot build an empty replacement group",
        ));
    }

    let max_texcoords = sources
        .iter()
        .map(|s| s.vb.texcoords.len())
        .max()
        .unwrap_or(0);
    let max_colors = sources.iter().map(|s| s.vb.colors.len()).max().unwrap_or(0);
    let any_normals = sources
        .iter()
        .any(|s| s.vb.normals.len() == s.vb.element_count);
    let any_tangents = sources
        .iter()
        .any(|s| s.vb.tangents.len() == s.vb.element_count);
    let any_joints = sources
        .iter()
        .any(|s| s.vb.joints.len() == s.vb.element_count);
    let any_weights = sources
        .iter()
        .any(|s| s.vb.weights.len() == s.vb.element_count);

    let mut out = VertexBuffer {
        element_count: total,
        texcoords: vec![Vec::with_capacity(total); max_texcoords],
        colors: vec![Vec::with_capacity(total); max_colors],
        ..VertexBuffer::default()
    };
    if any_normals {
        out.normals = Vec::with_capacity(total);
    }
    if any_tangents {
        out.tangents = Vec::with_capacity(total);
    }
    if any_joints {
        out.joints = Vec::with_capacity(total);
    }
    if any_weights {
        out.weights = Vec::with_capacity(total);
    }

    let mut indices = Vec::new();
    let mut ranges = Vec::with_capacity(sources.len());
    let mut vertex_offset = 0u32;
    for source in sources {
        let count = source.vb.element_count;
        if source.vb.positions.len() != count {
            return Err(DecodeError::Model("replacement primitive has no POSITION"));
        }
        out.positions.extend_from_slice(&source.vb.positions);
        if any_normals {
            extend_or(&mut out.normals, &source.vb.normals, count, [0.0, 0.0, 1.0]);
        }
        if any_tangents {
            extend_or(
                &mut out.tangents,
                &source.vb.tangents,
                count,
                [1.0, 0.0, 0.0, 1.0],
            );
        }
        for channel in 0..max_texcoords {
            let fallback = source
                .vb
                .texcoords
                .first()
                .filter(|uv| uv.len() == count)
                .and_then(|uv| uv.first().copied())
                .unwrap_or([0.0, 0.0]);
            if let Some(values) = source
                .vb
                .texcoords
                .get(channel)
                .filter(|uv| uv.len() == count)
            {
                out.texcoords[channel].extend_from_slice(values);
            } else {
                out.texcoords[channel].extend(std::iter::repeat_n(fallback, count));
            }
        }
        for channel in 0..max_colors {
            if let Some(values) = source.vb.colors.get(channel).filter(|c| c.len() == count) {
                out.colors[channel].extend_from_slice(values);
            } else {
                out.colors[channel].extend(std::iter::repeat_n([1.0; 4], count));
            }
        }
        if any_joints {
            extend_or(&mut out.joints, &source.vb.joints, count, [0; 4]);
        }
        if any_weights {
            extend_or(
                &mut out.weights,
                &source.vb.weights,
                count,
                [1.0, 0.0, 0.0, 0.0],
            );
        }

        let start_index = indices.len();
        for &idx in &source.indices {
            indices.push(idx.checked_add(vertex_offset).ok_or(DecodeError::Model(
                "replacement group index exceeds u32 range",
            ))?);
        }
        ranges.push(PrimitiveRange {
            start_index,
            index_count: source.indices.len(),
        });
        vertex_offset = vertex_offset
            .checked_add(u32::try_from(count).map_err(|_| {
                DecodeError::Model("replacement group vertex count exceeds u32 range")
            })?)
            .ok_or(DecodeError::Model(
                "replacement group vertex count exceeds u32 range",
            ))?;
    }

    Ok((out, indices, ranges))
}

fn extend_or<T: Copy>(out: &mut Vec<T>, values: &[T], count: usize, fallback: T) {
    if values.len() == count {
        out.extend_from_slice(values);
    } else {
        out.extend(std::iter::repeat_n(fallback, count));
    }
}

fn locate_draw_calls(mdat: &Value) -> Result<Vec<(usize, usize, &Value)>, DecodeError> {
    let scene_objects = mdat
        .get("m_sceneObjects")
        .and_then(Value::as_array)
        .ok_or(DecodeError::Model("MDAT has no m_sceneObjects"))?;
    let mut out = Vec::new();
    for (so_i, so) in scene_objects.iter().enumerate() {
        let Some(dcs) = so.get("m_drawCalls").and_then(Value::as_array) else {
            continue;
        };
        for (dc_i, dc) in dcs.iter().enumerate() {
            out.push((so_i, dc_i, dc));
        }
    }
    Ok(out)
}

fn material_names_match(donor: &str, target: &str) -> bool {
    let d = material_key(donor);
    let t = material_key(target);
    !d.is_empty() && !t.is_empty() && (d.contains(&t) || t.contains(&d))
}

fn material_key(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches("_c")
        .trim_end_matches(".vmat")
        .to_ascii_lowercase()
}

/// The `CTRL` buffer-registry scalar edits for replacing mesh part `mesh_pos`'s
/// single vertex/index buffer with `enc`: new element counts, vertex stride, index
/// width, and each layout field's format/offset. Field count + order are unchanged
/// (so no KV3 array growth), and only values that actually differ are emitted (a
/// KV3 `0`/`1` is a tagless constant with no byte to set; the first field's offset
/// is always 0, and the gun's formats are already uncompressed float).
fn ctrl_edits_for(
    mesh_pos: usize,
    vb_desc: &BufferDesc,
    ib_desc: &BufferDesc,
    enc: &EncodedMesh,
) -> Result<Vec<(Vec<Seg>, i64)>, DecodeError> {
    let vb_path = vec![
        seg("embedded_meshes"),
        Seg::Index(mesh_pos),
        seg("m_vertexBuffers"),
        Seg::Index(0),
    ];
    let ib_path = vec![
        seg("embedded_meshes"),
        Seg::Index(mesh_pos),
        seg("m_indexBuffers"),
        Seg::Index(0),
    ];
    let mut edits: Vec<(Vec<Seg>, i64)> = Vec::new();
    push_if_changed(
        &mut edits,
        child(&vb_path, "m_nElementCount"),
        vb_desc.element_count,
        enc.vertex_count,
    )?;
    push_if_changed(
        &mut edits,
        child(&vb_path, "m_nElementSizeInBytes"),
        vb_desc.element_size,
        enc.stride,
    )?;
    push_if_changed(
        &mut edits,
        child(&ib_path, "m_nElementCount"),
        ib_desc.element_count,
        enc.index_count,
    )?;
    push_if_changed(
        &mut edits,
        child(&ib_path, "m_nElementSizeInBytes"),
        ib_desc.element_size,
        enc.index_size,
    )?;
    for (fi, f) in enc.fields.iter().enumerate() {
        let mut field_path = vb_path.clone();
        field_path.push(seg("m_inputLayoutFields"));
        field_path.push(Seg::Index(fi));
        let old = &vb_desc.fields[fi];
        if f.format.id() != old.format.id() {
            edits.push((child(&field_path, "m_Format"), i64::from(f.format.id())));
        }
        push_if_changed(
            &mut edits,
            child(&field_path, "m_nOffset"),
            old.offset,
            f.offset,
        )?;
    }
    Ok(edits)
}

/// The `MDAT` draw-call scalar edits: the replacement spans the whole new buffer
/// from the start, so vertex/index counts are updated and start index / base
/// vertex are pinned to 0 (only emitted when they differ from the current value).
fn mdat_edits_for(
    so_idx: usize,
    dc_idx: usize,
    draw_call: &Value,
    new_vertex_count: usize,
    new_index_count: usize,
) -> Result<Vec<(Vec<Seg>, i64)>, DecodeError> {
    mdat_edits_for_range(
        so_idx,
        dc_idx,
        draw_call,
        new_vertex_count,
        new_index_count,
        0,
        0,
    )
}

fn mdat_edits_for_range(
    so_idx: usize,
    dc_idx: usize,
    draw_call: &Value,
    new_vertex_count: usize,
    new_index_count: usize,
    new_start_index: usize,
    new_base_vertex: usize,
) -> Result<Vec<(Vec<Seg>, i64)>, DecodeError> {
    let dc_path = vec![
        seg("m_sceneObjects"),
        Seg::Index(so_idx),
        seg("m_drawCalls"),
        Seg::Index(dc_idx),
    ];
    let mut edits: Vec<(Vec<Seg>, i64)> = Vec::new();
    push_if_changed(
        &mut edits,
        child(&dc_path, "m_nVertexCount"),
        dc_uint(draw_call, "m_nVertexCount"),
        new_vertex_count,
    )?;
    push_if_changed(
        &mut edits,
        child(&dc_path, "m_nIndexCount"),
        dc_uint(draw_call, "m_nIndexCount"),
        new_index_count,
    )?;
    push_if_changed(
        &mut edits,
        child(&dc_path, "m_nStartIndex"),
        dc_uint(draw_call, "m_nStartIndex"),
        new_start_index,
    )?;
    push_if_changed(
        &mut edits,
        child(&dc_path, "m_nBaseVertex"),
        dc_uint(draw_call, "m_nBaseVertex"),
        new_base_vertex,
    )?;
    Ok(edits)
}

/// Locates the single draw call of a mesh part's `MDAT`, returning its
/// `(scene_object, draw_call)` indices and material. Errors if the part has zero
/// or more than one draw call (the wedge maps the new mesh onto exactly one).
fn locate_single_draw_call(mdat: &Value) -> Result<(usize, usize, String), DecodeError> {
    let scene_objects = mdat
        .get("m_sceneObjects")
        .and_then(Value::as_array)
        .ok_or(DecodeError::Model("MDAT has no m_sceneObjects"))?;
    let mut found: Option<(usize, usize, String)> = None;
    for (so_i, so) in scene_objects.iter().enumerate() {
        let Some(dcs) = so.get("m_drawCalls").and_then(Value::as_array) else {
            continue;
        };
        for (dc_i, dc) in dcs.iter().enumerate() {
            if found.is_some() {
                return Err(DecodeError::Model(
                    "replace-in-place needs a mesh part with exactly one draw call (this part \
                     renders more than one material)",
                ));
            }
            found = Some((so_i, dc_i, material_of(dc).to_owned()));
        }
    }
    found.ok_or(DecodeError::Model("target mesh part has no draw call"))
}

/// Appends a `(path, new)` scalar edit only when it differs from the current
/// value. A KV3 `0`/`1` is a *tagless* constant with no byte to overwrite, so a
/// no-op set (e.g. the first layout field's offset, always 0) must be skipped or
/// `set_scalars` would fail to resolve it.
fn push_if_changed(
    edits: &mut Vec<(Vec<Seg>, i64)>,
    path: Vec<Seg>,
    current: usize,
    new: usize,
) -> Result<(), DecodeError> {
    if current != new {
        edits.push((path, as_i64(new)?));
    }
    Ok(())
}

/// Bakes a draw-call group's index slice, vertex-count bound, and material into
/// a cloned draw-call `Value` (the clones grown by [`set_draw_call_groups`]).
/// Numeric fields keep their existing Int/UInt wire variant so a positive
/// count/offset round-trips to the KV3 width the engine read.
fn bake_draw_call(dc: &mut Value, group: &DrawCallGroup, mat_key: &str) {
    set_uint_field(dc, "m_nStartIndex", group.start_index);
    set_uint_field(dc, "m_nIndexCount", group.index_count);
    set_applied_index_offset(dc, group.vertex_start);
    set_uint_field(dc, "m_nVertexCount", group.vertex_end);
    set_uint_field(dc, "m_nBaseVertex", 0);
    if let Some(v) = dc.get_mut(mat_key) {
        *v = Value::String(group.material.clone());
    }
}

/// Overwrites an unsigned draw-call field in place, preserving its current
/// Int/UInt wire variant (and leaving it untouched if the template lacks the key).
fn set_uint_field(dc: &mut Value, key: &str, n: usize) {
    if let Some(v) = dc.get_mut(key) {
        *v = match v {
            Value::UInt(_) => Value::UInt(n as u64),
            _ => Value::Int(i64::try_from(n).unwrap_or(i64::MAX)),
        };
    }
}

fn set_applied_index_offset(dc: &mut Value, n: usize) {
    if let Some(v) = dc.get_mut("m_nAppliedIndexOffset") {
        *v = Value::UInt(n as u64);
    }
}

/// Reads an unsigned draw-call field's current value (for the changed-or-skip
/// comparison); a sentinel forces a set on the (never-seen) absent field.
fn dc_uint(dc: &Value, key: &str) -> usize {
    dc.get(key)
        .and_then(Value::as_int)
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(usize::MAX)
}

/// A `Seg::Key` from a string literal (the verbose `Seg::Key(s.into())` is the
/// bulk of a KV3 path).
fn seg(key: &str) -> Seg {
    Seg::Key(key.to_string())
}

/// Extends a KV3 path base with one final object key.
fn child(base: &[Seg], key: &str) -> Vec<Seg> {
    let mut p = base.to_vec();
    p.push(Seg::Key(key.to_string()));
    p
}

/// A count/size/offset as the `i64` `set_scalars` takes, erroring on the
/// (unreachable for real models) overflow rather than wrapping.
fn as_i64(n: usize) -> Result<i64, DecodeError> {
    i64::try_from(n).map_err(|_| DecodeError::Model("count exceeds i64 range"))
}

/// The material path a draw call renders with, reading either the modern
/// `m_material` or the legacy `m_pMaterial` key (mirrors `mesh::DrawCall::parse`).
fn material_of(dc: &Value) -> &str {
    dc.get("m_material")
        .or_else(|| dc.get("m_pMaterial"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn vertex_buffers_of(dc: &Value) -> Vec<usize> {
    dc.get("m_vertexBuffers")
        .and_then(Value::as_array)
        .map(|buffers| {
            buffers
                .iter()
                .filter_map(|b| {
                    b.get("m_hBuffer")
                        .and_then(Value::as_int)
                        .and_then(|v| usize::try_from(v).ok())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Reads an unsigned draw-call field, defaulting to 0 when absent (the value is
/// informational, used only for the removal report).
fn usize_field(dc: &Value, key: &str) -> usize {
    dc.get(key)
        .and_then(Value::as_int)
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(0)
}
