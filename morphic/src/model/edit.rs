//! Vertex-displacement model edits (Tier 0): reshape existing geometry by
//! rewriting the `POSITION` lane of a mesh's vertex buffer, without changing
//! topology. The model-world analog of the texture splice in `crate::edit`:
//! decode the `MVTX` block, overwrite positions, re-encode in the same
//! meshoptimizer codec, and splice it back into the resource container.
//!
//! Vertex buffers are addressed by their global block index (the `m_nBlockIndex`
//! the `CTRL` buffer registry stores), enumerated by [`vertex_targets`].

use crate::error::DecodeError;
use crate::kv3::{set_bools, Seg, Value};
use crate::meshopt::{encode_index_buffer, encode_vertex_buffer};
use crate::resource::Resource;

use super::dxgi::DxgiFormat;
use super::glb;
use super::mesh::{
    assemble_to_layout, assemble_vertex_buffer, AssembledBuffer, EmbeddedMesh, VertexBuffer,
};
use super::vbib::{BufferDesc, InputLayoutField};

const CTRL: [u8; 4] = *b"CTRL";

/// One editable (or inspectable) vertex buffer in a `.vmdl_c`.
#[derive(Debug, Clone)]
pub struct VertexTarget {
    /// The owning embedded mesh's name (e.g. `body`, `gun`).
    pub mesh_name: String,
    pub mesh_index: usize,
    /// Global block index of this buffer's `MVTX` payload; the handle passed to
    /// [`read_vertex_positions`] / [`replace_vertex_positions`].
    pub block_index: usize,
    pub vertex_count: usize,
    pub stride: usize,
    pub meshopt: bool,
    /// True when this buffer can be displacement-edited: meshopt-compressed,
    /// not ZSTD, and carrying an `R32G32B32_FLOAT` `POSITION`.
    pub editable: bool,
    /// True when this buffer carries a `COLOR` attribute (a baked per-vertex
    /// tint), i.e. is a candidate for [`recolor_vertex_buffer`].
    pub has_color: bool,
}

pub(super) fn parse_embedded(
    bytes: &[u8],
) -> Result<(Resource<'_>, Vec<EmbeddedMesh>), DecodeError> {
    let resource = Resource::parse(bytes)?;
    let ctrl_bytes = resource
        .find_block(CTRL)
        .ok_or(DecodeError::Model("model has no CTRL block"))?;
    let ctrl = crate::kv3::decode(ctrl_bytes)?;
    let embedded = EmbeddedMesh::parse_all(&ctrl)?;
    Ok((resource, embedded))
}

fn find_vertex_buffer(embedded: &[EmbeddedMesh], block_index: usize) -> Option<&BufferDesc> {
    embedded
        .iter()
        .flat_map(|em| &em.vertex_buffers)
        .find(|d| d.block_index == block_index)
}

fn has_float_position(desc: &BufferDesc) -> bool {
    desc.fields
        .iter()
        .any(|f| f.semantic_name == "POSITION" && f.format == DxgiFormat::R32G32B32Float)
}

/// Lists every vertex buffer in a `.vmdl_c`, with the block index used to edit
/// it and whether a displacement edit is supported.
pub fn vertex_targets(vmdl_bytes: &[u8]) -> Result<Vec<VertexTarget>, DecodeError> {
    let (_resource, embedded) = parse_embedded(vmdl_bytes)?;
    let mut out = Vec::new();
    for em in &embedded {
        for d in &em.vertex_buffers {
            out.push(VertexTarget {
                mesh_name: em.name.clone(),
                mesh_index: em.mesh_index,
                block_index: d.block_index,
                vertex_count: d.element_count,
                stride: d.element_size,
                meshopt: d.meshopt,
                editable: d.meshopt && !d.zstd && has_float_position(d),
                has_color: d.fields.iter().any(|f| f.semantic_name == "COLOR"),
            });
        }
    }
    Ok(out)
}

/// Exports the vertex buffer at `block_index` as a standalone `.glb` for editing
/// in Blender (or any glTF tool): one triangle mesh with POSITION + NORMAL and a
/// custom `_ORIGID` per-vertex attribute carrying the original index. Reshape it
/// (displacement only, topology preserved) and feed the result back to
/// [`apply_edited_glb`].
pub fn export_buffer_for_edit(
    vmdl_bytes: &[u8],
    block_index: usize,
) -> Result<Vec<u8>, DecodeError> {
    let (resource, embedded) = parse_embedded(vmdl_bytes)?;
    let (em, local) = embedded
        .iter()
        .find_map(|em| {
            em.vertex_buffers
                .iter()
                .position(|d| d.block_index == block_index)
                .map(|local| (em, local))
        })
        .ok_or(DecodeError::Model("no vertex buffer at that block index"))?;

    // Assemble unskinned (remap None): we only need positions, normals, and the
    // draw-call triangulation for this buffer.
    let part = super::mesh::assemble(em, &resource, None)?;
    let vb = part.vertex_buffers.get(local).ok_or(DecodeError::Model(
        "buffer index out of range after assemble",
    ))?;
    let indices: Vec<u32> = part
        .primitives
        .iter()
        .filter(|p| p.vertex_buffer == local)
        .flat_map(|p| p.indices.iter().copied())
        .collect();

    glb::to_edit_glb(vb, &indices)
}

/// Reads the current `POSITION` array of the vertex buffer at `block_index`,
/// in the buffer's native vertex order. Pair with [`replace_vertex_positions`]
/// to apply a transform: read, transform each position, write back.
pub fn read_vertex_positions(
    vmdl_bytes: &[u8],
    block_index: usize,
) -> Result<Vec<[f32; 3]>, DecodeError> {
    let (resource, embedded) = parse_embedded(vmdl_bytes)?;
    let desc = find_vertex_buffer(&embedded, block_index)
        .ok_or(DecodeError::Model("no vertex buffer at that block index"))?;
    let raw = resource
        .get_block_by_index(block_index)
        .ok_or(DecodeError::Model("MVTX block index out of range"))?;
    let on_disk = desc.decode(raw, true)?;
    on_disk.positions()
}

/// Split copies of one original vertex (Blender duplicates a vertex per
/// normal/UV seam, each carrying the same `_ORIGID`) must agree on position to
/// within this tolerance (source units = inches). A larger spread means a seam
/// was pulled apart, which displacement editing cannot represent.
const SPLIT_TOL: f32 = 1.0e-2;

/// Applies an edited `.glb` (as produced by [`export_buffer_for_edit`] and
/// reshaped in Blender) back onto the model: recovers each original vertex's new
/// position via the `_ORIGID` carrier and splices the buffer at `block_index`.
/// Topology must be preserved: every original vertex must be present exactly once
/// (after regrouping split copies), or this errors.
pub fn apply_edited_glb(
    vmdl_bytes: &[u8],
    block_index: usize,
    glb_bytes: &[u8],
) -> Result<Vec<u8>, DecodeError> {
    let (_resource, embedded) = parse_embedded(vmdl_bytes)?;
    let desc = find_vertex_buffer(&embedded, block_index)
        .ok_or(DecodeError::Model("no vertex buffer at that block index"))?;
    let new_positions = read_edited_positions(glb_bytes, desc.element_count)?;
    replace_vertex_positions(vmdl_bytes, block_index, &new_positions)
}

/// Reads an edited `.glb` and returns one position per original vertex, indexed by
/// `_ORIGID` (0..`count`). Positions are taken in node-world space so a baked
/// import/export axis transform is undone. Split copies are regrouped by id.
fn read_edited_positions(glb_bytes: &[u8], count: usize) -> Result<Vec<[f32; 3]>, DecodeError> {
    let (doc, buffers, _images) = gltf::import_slice(glb_bytes)
        .map_err(|_| DecodeError::Model("failed to parse edited glb"))?;
    let world = node_world_transforms(&doc);

    let mut acc: Vec<Option<[f32; 3]>> = vec![None; count];
    for node in doc.nodes() {
        let Some(mesh) = node.mesh() else { continue };
        let m = world[node.index()];
        for prim in mesh.primitives() {
            let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
            let Some(positions) = reader.read_positions() else {
                continue;
            };
            let positions: Vec<[f32; 3]> = positions.collect();

            // Match the carrier tolerant of underscore handling: gltf-json wrote
            // it as `_ORIGID`, but a Blender import/export round-trip can shift the
            // leading underscores, so key off the "ORIGID" stem.
            let id_acc = prim
                .attributes()
                .find_map(|(sem, a)| match sem {
                    gltf::Semantic::Extras(name) if name.to_ascii_uppercase().contains("ORIGID") => {
                        Some(a)
                    }
                    _ => None,
                })
                .ok_or(DecodeError::Model(
                    "edited glb has no _ORIGID attribute (re-export with custom attributes enabled)",
                ))?;
            let ids = read_origid(&id_acc, &buffers)
                .ok_or(DecodeError::Model("could not read _ORIGID accessor"))?;
            if ids.len() != positions.len() {
                return Err(DecodeError::Model("_ORIGID / POSITION count mismatch"));
            }

            for (&id, &p) in ids.iter().zip(&positions) {
                let id = id as usize;
                if id >= count {
                    return Err(DecodeError::Model(
                        "_ORIGID out of range (topology changed?)",
                    ));
                }
                let pw = transform_point(&m, p);
                match acc[id] {
                    None => acc[id] = Some(pw),
                    Some(existing) => {
                        let spread = (0..3)
                            .map(|k| (existing[k] - pw[k]).abs())
                            .fold(0.0_f32, f32::max);
                        if spread > SPLIT_TOL {
                            return Err(DecodeError::Model(
                                "split vertices moved apart (a UV/normal seam was separated); \
                                 not supported by displacement editing",
                            ));
                        }
                    }
                }
            }
        }
    }

    acc.into_iter()
        .map(|p| {
            p.ok_or(DecodeError::Model(
                "edited glb is missing original vertices (topology changed: vertices removed)",
            ))
        })
        .collect()
}

/// Reads a SCALAR `_ORIGID` accessor as `u32` indices, handling float or integer
/// component types (Blender re-exports our float carrier as float).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn read_origid(acc: &gltf::Accessor, buffers: &[gltf::buffer::Data]) -> Option<Vec<u32>> {
    use gltf::accessor::{DataType, Dimensions};
    if acc.dimensions() != Dimensions::Scalar {
        return None;
    }
    let view = acc.view()?;
    let buf = buffers.get(view.buffer().index())?;
    let dt = acc.data_type();
    let comp = match dt {
        DataType::I8 | DataType::U8 => 1,
        DataType::I16 | DataType::U16 => 2,
        DataType::U32 | DataType::F32 => 4,
    };
    let stride = view.stride().unwrap_or(comp);
    let start = view.offset() + acc.offset();
    let mut out = Vec::with_capacity(acc.count());
    for i in 0..acc.count() {
        let o = start + i * stride;
        let id = match dt {
            DataType::F32 => {
                f32::from_le_bytes(buf.0.get(o..o + 4)?.try_into().ok()?).round() as u32
            }
            DataType::U32 => u32::from_le_bytes(buf.0.get(o..o + 4)?.try_into().ok()?),
            DataType::U16 => u32::from(u16::from_le_bytes(buf.0.get(o..o + 2)?.try_into().ok()?)),
            DataType::U8 => u32::from(*buf.0.get(o)?),
            // A signed carrier never occurs (we write F32); reject rather than
            // guess a reinterpretation.
            DataType::I8 | DataType::I16 => return None,
        };
        out.push(id);
    }
    Some(out)
}

/// World transform per node index (column-major 4x4), composing parent chains so
/// positions can be lifted out of any baked import/export axis transform.
fn node_world_transforms(doc: &gltf::Document) -> Vec<[[f32; 4]; 4]> {
    let mut out = vec![IDENTITY4; doc.nodes().count()];
    for scene in doc.scenes() {
        for node in scene.nodes() {
            accumulate(&node, IDENTITY4, &mut out);
        }
    }
    out
}

fn accumulate(node: &gltf::Node, parent: [[f32; 4]; 4], out: &mut [[[f32; 4]; 4]]) {
    let world = mat_mul(parent, node.transform().matrix());
    out[node.index()] = world;
    for child in node.children() {
        accumulate(&child, world, out);
    }
}

const IDENTITY4: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// Column-major 4x4 multiply: `(a*b)[col][row] = sum_k a[k][row] * b[col][k]`.
#[allow(clippy::many_single_char_names)]
fn mat_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut r = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k][row] * b[col][k];
            }
            r[col][row] = s;
        }
    }
    r
}

/// Transforms a point by a column-major 4x4 (with implicit w=1).
#[allow(clippy::many_single_char_names)]
fn transform_point(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    let v = [p[0], p[1], p[2], 1.0];
    let mut o = [0.0f32; 3];
    for (row, oo) in o.iter_mut().enumerate() {
        let mut s = 0.0;
        for (col, &vc) in v.iter().enumerate() {
            s += m[col][row] * vc;
        }
        *oo = s;
    }
    o
}

/// Replaces the `POSITION` lane of the vertex buffer at `block_index` with
/// `new_positions` (same count, same order), re-encodes the buffer in its native
/// meshoptimizer codec, and splices it back, returning the new `.vmdl_c` bytes.
/// All other vertex attributes, every other block, and the model topology are
/// preserved.
///
/// Errors if the buffer is not meshopt-compressed (only those can be re-encoded),
/// is ZSTD, lacks a float `POSITION`, or if `new_positions.len()` differs from
/// the vertex count.
pub fn replace_vertex_positions(
    vmdl_bytes: &[u8],
    block_index: usize,
    new_positions: &[[f32; 3]],
) -> Result<Vec<u8>, DecodeError> {
    let (resource, embedded) = parse_embedded(vmdl_bytes)?;
    let desc = find_vertex_buffer(&embedded, block_index)
        .ok_or(DecodeError::Model("no vertex buffer at that block index"))?;
    if !desc.meshopt {
        return Err(DecodeError::Model(
            "only meshopt-compressed vertex buffers can be re-encoded",
        ));
    }
    if desc.zstd {
        return Err(DecodeError::Model("ZSTD vertex buffers not supported"));
    }

    let raw = resource
        .get_block_by_index(block_index)
        .ok_or(DecodeError::Model("MVTX block index out of range"))?;
    let mut on_disk = desc.decode(raw, true)?;
    on_disk.write_positions(new_positions)?;

    let new_mvtx = encode_vertex_buffer(desc.element_count, desc.element_size, &on_disk.data)?;
    resource.rebuild_with_block(block_index, &new_mvtx)
}

/// Reads the first `COLOR` attribute of the vertex buffer at `block_index` as
/// RGBA in 0..1 (in native vertex order), or `None` if the buffer carries no
/// `COLOR`. The diagnostic read for a vertex-color recolor: pair with
/// [`recolor_vertex_buffer`] to sample a baked tint before editing it.
pub fn read_vertex_colors(
    vmdl_bytes: &[u8],
    block_index: usize,
) -> Result<Option<Vec<[f32; 4]>>, DecodeError> {
    let (resource, embedded) = parse_embedded(vmdl_bytes)?;
    let desc = find_vertex_buffer(&embedded, block_index)
        .ok_or(DecodeError::Model("no vertex buffer at that block index"))?;
    let raw = resource
        .get_block_by_index(block_index)
        .ok_or(DecodeError::Model("MVTX block index out of range"))?;
    let on_disk = desc.decode(raw, true)?;
    match on_disk.color_fields().first() {
        Some(attr) => Ok(Some(on_disk.vector4(attr)?)),
        None => Ok(None),
    }
}

/// Applies `transform` to every `COLOR` attribute (RGBA in 0..1) of the vertex
/// buffer at `block_index`, re-encodes the buffer in its native meshoptimizer
/// codec, and splices it back, returning the new `.vmdl_c` bytes and the number
/// of color lanes edited. The color analog of [`replace_vertex_positions`]:
/// rewrite the baked per-vertex tint without touching topology or any other
/// attribute (position, normal, uv, skin weights).
///
/// Returns `(bytes, 0)` unchanged if the buffer has no `COLOR`. Handles both a
/// meshopt-compressed buffer (re-encoded through the vertex codec) and an
/// uncompressed buffer (the `COLOR` lane is patched in place); errors only on a
/// ZSTD buffer. In Deadlock hero models the per-vertex color often lives in a
/// standalone *uncompressed* second buffer, distinct from the meshopt geometry
/// buffer, so the uncompressed path is the common case here.
pub fn recolor_vertex_buffer(
    vmdl_bytes: &[u8],
    block_index: usize,
    transform: impl Fn([f32; 4]) -> [f32; 4],
) -> Result<(Vec<u8>, usize), DecodeError> {
    let (resource, embedded) = parse_embedded(vmdl_bytes)?;
    let desc = find_vertex_buffer(&embedded, block_index)
        .ok_or(DecodeError::Model("no vertex buffer at that block index"))?;
    if desc.zstd {
        return Err(DecodeError::Model("ZSTD vertex buffers not supported"));
    }

    let block_offset = resource
        .blocks()
        .get(block_index)
        .ok_or(DecodeError::Model("MVTX block index out of range"))?
        .offset as usize;
    let raw = resource
        .get_block_by_index(block_index)
        .ok_or(DecodeError::Model("MVTX block index out of range"))?;
    let mut on_disk = desc.decode(raw, true)?;

    let color_fields = on_disk.color_fields();
    if color_fields.is_empty() {
        return Ok((vmdl_bytes.to_vec(), 0));
    }
    for attr in &color_fields {
        let recolored: Vec<[f32; 4]> = on_disk.vector4(attr)?.into_iter().map(&transform).collect();
        on_disk.write_colors(attr, &recolored)?;
    }

    if desc.meshopt {
        // Re-encoding meshopt is not byte-compatible with the engine's decoder
        // (my encoder round-trips only through morphic's own decoder, and garbles
        // in game). Instead convert this buffer to UNCOMPRESSED: splice the edited
        // interleaved bytes raw and flip m_bMeshoptCompressed=false in CTRL. The
        // engine reads uncompressed vertex buffers natively (hero models ship them).
        return convert_meshopt_to_uncompressed(vmdl_bytes, block_index, &on_disk.data)
            .map(|bytes| (bytes, color_fields.len()));
    }

    // An uncompressed buffer's block *is* the interleaved bytes, so patch the
    // edited color lane straight into a copy of the original file at the block's
    // absolute offset. Nothing else moves: the output is byte-identical to the
    // input except the COLOR bytes (no container rebuild, no re-alignment). This
    // is the strictest, byte-faithful edit, which the engine accepts where a
    // re-encode / re-layout can be rejected (see docs/handoff-model-edit.md).
    let mut out = vmdl_bytes.to_vec();
    out.get_mut(block_offset..block_offset + on_disk.data.len())
        .ok_or(DecodeError::Model("uncompressed block past end of file"))?
        .copy_from_slice(&on_disk.data);
    Ok((out, color_fields.len()))
}

/// Re-skins the vertex buffer at `block_index`: for each vertex, `edit` receives
/// its buffer-order index, model-space position, current mesh-local
/// `BLENDINDICES` (four palette slots), and `BLENDWEIGHT`s, and returns the new
/// local joints (or the same to leave it). Weights, positions, normals, and uvs
/// are untouched, so only *which bones* drive each vertex changes. The index lets
/// a caller key into a precomputed per-vertex mask (e.g. connected-component
/// membership). Returns `(bytes, n_changed)`.
///
/// Use to bind geometry to a bone the cloth sim drives but that no vertex
/// currently follows (e.g. Holliday's boot fringe rides `leg_lower_*` while the
/// `FeModel` drives the unused `flaps_0_*`). The caller resolves bone names to local
/// palette slots via the mesh's remap table ([`invert_remap`] over
/// [`remap_table`]); both source and target bone must already be in the palette
/// (this never grows the remap table). Same meshopt handling as
/// [`recolor_vertex_buffer`]: a meshopt buffer is converted to uncompressed (the
/// engine reads that natively); an uncompressed buffer is patched in place.
///
/// Errors on a ZSTD buffer, an 8-influence buffer, or a buffer with no
/// `BLENDINDICES`.
///
/// [`invert_remap`]: super::invert_remap
/// [`remap_table`]: super::remap_table
#[allow(clippy::similar_names)]
pub fn reskin_vertex_buffer(
    vmdl_bytes: &[u8],
    block_index: usize,
    edit: impl Fn(usize, [f32; 3], [u16; 4], [f32; 4]) -> [u16; 4],
) -> Result<(Vec<u8>, usize), DecodeError> {
    let (resource, embedded) = parse_embedded(vmdl_bytes)?;
    let desc = find_vertex_buffer(&embedded, block_index)
        .ok_or(DecodeError::Model("no vertex buffer at that block index"))?;
    if desc.zstd {
        return Err(DecodeError::Model("ZSTD vertex buffers not supported"));
    }
    let block_offset = resource
        .blocks()
        .get(block_index)
        .ok_or(DecodeError::Model("MVTX block index out of range"))?
        .offset as usize;
    let raw = resource
        .get_block_by_index(block_index)
        .ok_or(DecodeError::Model("MVTX block index out of range"))?;
    let mut on_disk = desc.decode(raw, true)?;

    let bi_field = on_disk
        .fields
        .iter()
        .find(|f| f.semantic_name == "BLENDINDICES")
        .cloned()
        .ok_or(DecodeError::Model("buffer has no BLENDINDICES"))?;
    let bw_field = on_disk
        .fields
        .iter()
        .find(|f| f.semantic_name == "BLENDWEIGHT")
        .cloned();

    let n = on_disk.element_count;
    let positions = on_disk.positions()?;
    let joints_flat = on_disk.blend_indices(&bi_field, None)?;
    if joints_flat.len() != n * 4 {
        return Err(DecodeError::Model(
            "re-skin supports 4-influence buffers only",
        ));
    }
    let weights_flat = match &bw_field {
        Some(f) => on_disk.blend_weights(f)?,
        None => vec![],
    };

    let mut new_joints = Vec::with_capacity(n);
    let mut changed = 0usize;
    for i in 0..n {
        let j = [
            joints_flat[i * 4],
            joints_flat[i * 4 + 1],
            joints_flat[i * 4 + 2],
            joints_flat[i * 4 + 3],
        ];
        let w = if weights_flat.len() >= (i + 1) * 4 {
            [
                weights_flat[i * 4],
                weights_flat[i * 4 + 1],
                weights_flat[i * 4 + 2],
                weights_flat[i * 4 + 3],
            ]
        } else {
            [1.0, 0.0, 0.0, 0.0]
        };
        let nj = edit(i, positions[i], j, w);
        if nj != j {
            changed += 1;
        }
        new_joints.push(nj);
    }
    if changed == 0 {
        return Ok((vmdl_bytes.to_vec(), 0));
    }
    on_disk.write_blend_indices(&bi_field, &new_joints)?;

    if desc.meshopt {
        return convert_meshopt_to_uncompressed(vmdl_bytes, block_index, &on_disk.data)
            .map(|bytes| (bytes, changed));
    }
    let mut out = vmdl_bytes.to_vec();
    out.get_mut(block_offset..block_offset + on_disk.data.len())
        .ok_or(DecodeError::Model("uncompressed block past end of file"))?
        .copy_from_slice(&on_disk.data);
    Ok((out, changed))
}

/// Converts the meshopt vertex buffer at `block_index` into an uncompressed one
/// carrying `raw` (its decoded, edited interleaved bytes): splices the raw bytes
/// as the block and flips `m_bMeshoptCompressed` to false for that buffer in the
/// `CTRL` registry (byte-faithfully, via [`set_bools`]). The engine then reads the
/// buffer uncompressed. This sidesteps re-encoding meshopt, which is not
/// byte-compatible with the engine's decoder. Shared by the color recolor and the
/// blend-index re-skin paths.
fn convert_meshopt_to_uncompressed(
    vmdl_bytes: &[u8],
    block_index: usize,
    raw: &[u8],
) -> Result<Vec<u8>, DecodeError> {
    let resource = Resource::parse(vmdl_bytes)?;
    let ctrl_index = resource
        .blocks()
        .iter()
        .position(|b| b.kind == CTRL)
        .ok_or(DecodeError::Model("model has no CTRL block"))?;
    let ctrl_bytes = resource
        .get_block_by_index(ctrl_index)
        .ok_or(DecodeError::Model("CTRL block out of range"))?;
    let ctrl = crate::kv3::decode(ctrl_bytes)?;
    let path = meshopt_flag_path(&ctrl, block_index)?;
    let new_ctrl = set_bools(ctrl_bytes, &[(path, false)])?;

    // Two block swaps: the flipped CTRL, then the raw vertex bytes. Block indices
    // are preserved across a rebuild, so the second still targets `block_index`.
    let with_ctrl = resource.rebuild_with_block(ctrl_index, &new_ctrl)?;
    let resource2 = Resource::parse(&with_ctrl)?;
    resource2.rebuild_with_block(block_index, raw)
}

/// Builds the `CTRL` KV3 path to `m_bMeshoptCompressed` of the vertex buffer whose
/// `m_nBlockIndex` equals `block_index`
/// (`embedded_meshes[mi].m_vertexBuffers[vi].m_bMeshoptCompressed`).
fn meshopt_flag_path(ctrl: &Value, block_index: usize) -> Result<Vec<Seg>, DecodeError> {
    let target =
        i64::try_from(block_index).map_err(|_| DecodeError::Model("block index too large"))?;
    let meshes = ctrl
        .get("embedded_meshes")
        .and_then(Value::as_array)
        .ok_or(DecodeError::Model("CTRL has no embedded_meshes"))?;
    for (mi, em) in meshes.iter().enumerate() {
        let Some(vbs) = em.get("m_vertexBuffers").and_then(Value::as_array) else {
            continue;
        };
        for (vi, buf) in vbs.iter().enumerate() {
            if buf.get("m_nBlockIndex").and_then(Value::as_int) == Some(target) {
                return Ok(vec![
                    Seg::Key("embedded_meshes".to_string()),
                    Seg::Index(mi),
                    Seg::Key("m_vertexBuffers".to_string()),
                    Seg::Index(vi),
                    Seg::Key("m_bMeshoptCompressed".to_string()),
                ]);
            }
        }
    }
    Err(DecodeError::Model(
        "no CTRL vertex buffer references that block index",
    ))
}

// --- T1c: assemble a brand-new mesh from an edited glb (add-geometry path) ---

/// The encoded `MVTX` + `MIDX` for a newly-assembled mesh part, plus the
/// metadata T1d needs to register it in the resource container (the `CTRL`
/// buffer registry's element counts / stride / `m_inputLayoutFields`, and the
/// `MDAT` draw-call index count / width).
#[derive(Debug, Clone)]
pub struct EncodedMesh {
    /// meshopt-encoded vertex buffer (codec v1, header `0xa1`).
    pub mvtx: Vec<u8>,
    /// meshopt-encoded index buffer (codec v1, header `0xe1`).
    pub midx: Vec<u8>,
    /// Raw interleaved vertex bytes (the uncompressed MVTX block payload). The
    /// engine reads uncompressed vertex buffers natively; morphic's meshopt codec
    /// v1 (`mvtx`) is NOT byte-compatible with the engine's decoder and garbles in
    /// game, so a buffer written for the engine must use this with
    /// `m_bMeshoptCompressed = false`.
    pub mvtx_raw: Vec<u8>,
    /// Raw index bytes (u16/u32 LE), the uncompressed MIDX block payload.
    pub midx_raw: Vec<u8>,
    pub vertex_count: usize,
    /// Interleaved vertex stride in bytes.
    pub stride: usize,
    pub index_count: usize,
    /// Index width in bytes (2 or 4).
    pub index_size: usize,
    /// Vertex layout, for the `CTRL` registry's `m_inputLayoutFields`.
    pub fields: Vec<InputLayoutField>,
}

/// One mesh primitive read from a donor `.glb` for topology replacement.
#[derive(Debug, Clone)]
pub struct EditedPrimitive {
    pub mesh_name: Option<String>,
    pub material_name: Option<String>,
    pub vertex_buffer: VertexBuffer,
    pub indices: Vec<u32>,
}

/// Reads one new mesh part from an edited `.glb`: its positions, normals, UV0,
/// joints, weights, and triangle indices. Takes a single primitive (the
/// add-one-mesh-part contract); pass `mesh_name` to pick it out of a multi-mesh
/// glb, else the only primitive is used.
///
/// Positions are taken in the glb's accessor space (no world transform: the
/// caller / T1d reconciles any baked axis flip when splicing). `JOINTS_0` are
/// glTF skin-joint indices, which equal model skeleton bone indices for a mesh
/// skinned against an exported hero skeleton.
pub fn read_edited_mesh(
    glb_bytes: &[u8],
    mesh_name: Option<&str>,
) -> Result<(VertexBuffer, Vec<u32>), DecodeError> {
    let primitives = read_edited_primitives(glb_bytes)?;
    let mut chosen = None;
    for prim in primitives {
        if let Some(want) = mesh_name {
            if prim.mesh_name.as_deref() != Some(want) {
                continue;
            }
        }
        if chosen.is_some() {
            return Err(DecodeError::Model(
                "edited glb has more than one primitive; T1c assembles one mesh part at a \
                 time (pass a mesh name, or split the export)",
            ));
        }
        chosen = Some(prim);
    }
    let prim = chosen.ok_or(DecodeError::Model("edited glb has no mesh primitive"))?;
    Ok((prim.vertex_buffer, prim.indices))
}

/// Reads every primitive from an edited `.glb`, preserving its mesh/material
/// names when present. Positions are taken in accessor space, matching
/// [`read_edited_mesh`].
pub fn read_edited_primitives(glb_bytes: &[u8]) -> Result<Vec<EditedPrimitive>, DecodeError> {
    let (doc, buffers, _images) = gltf::import_slice(glb_bytes)
        .map_err(|_| DecodeError::Model("failed to parse edited glb"))?;

    let mut out = Vec::new();
    for node in doc.nodes() {
        let Some(mesh) = node.mesh() else { continue };
        let mesh_name = mesh.name().or_else(|| node.name()).map(str::to_owned);
        for prim in mesh.primitives() {
            out.push(read_primitive(&buffers, mesh_name.clone(), &prim)?);
        }
    }
    if out.is_empty() {
        return Err(DecodeError::Model("edited glb has no mesh primitive"));
    }
    Ok(out)
}

fn read_primitive(
    buffers: &[gltf::buffer::Data],
    mesh_name: Option<String>,
    prim: &gltf::Primitive<'_>,
) -> Result<EditedPrimitive, DecodeError> {
    let reader = prim.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));

    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or(DecodeError::Model("edited glb primitive has no POSITION"))?
        .collect();
    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(Iterator::collect)
        .unwrap_or_default();
    let tangents: Vec<[f32; 4]> = reader
        .read_tangents()
        .map(Iterator::collect)
        .unwrap_or_default();
    let uv: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|t| t.into_f32().collect())
        .unwrap_or_default();
    let colors0: Vec<[f32; 4]> = reader
        .read_colors(0)
        .map(|c| c.into_rgba_f32().collect())
        .unwrap_or_default();
    let joints: Vec<[u16; 4]> = reader
        .read_joints(0)
        .map(|j| j.into_u16().collect())
        .unwrap_or_default();
    let weights: Vec<[f32; 4]> = reader
        .read_weights(0)
        .map(|w| w.into_f32().collect())
        .unwrap_or_default();
    let indices: Vec<u32> = reader
        .read_indices()
        .ok_or(DecodeError::Model("edited glb primitive has no indices"))?
        .into_u32()
        .collect();

    let texcoords = if uv.is_empty() { Vec::new() } else { vec![uv] };
    let colors = if colors0.is_empty() {
        Vec::new()
    } else {
        vec![colors0]
    };
    let vb = VertexBuffer {
        element_count: positions.len(),
        positions,
        normals,
        tangents,
        texcoords,
        colors,
        joints,
        weights,
        ..VertexBuffer::default()
    };
    Ok(EditedPrimitive {
        mesh_name,
        material_name: prim.material().name().map(str::to_owned),
        vertex_buffer: vb,
        indices,
    })
}

/// Assembles a [`VertexBuffer`] + triangle indices into encoded `MVTX`/`MIDX`
/// buffers (the T1c output T1d splices into the container), at this codec's
/// default uncompressed layout (the field set the mesh's own attributes imply).
/// The index width is chosen as 2 bytes unless an index exceeds `u16::MAX`.
/// Errors if the indices are not a triangle list or reference a vertex past the
/// buffer.
pub fn build_mesh_buffers(vb: &VertexBuffer, indices: &[u32]) -> Result<EncodedMesh, DecodeError> {
    encode_mesh(assemble_vertex_buffer(vb)?, indices)
}

/// Like [`build_mesh_buffers`], but conforms the new buffer to an **existing
/// target layout's field set** (T1d-b): same semantics, count, and order as
/// `target`, re-typed to uncompressed formats. This is what replace-in-place
/// (T1d-d) uses so the `CTRL` registry's `m_inputLayoutFields` element count is
/// unchanged and only each field's format/offset is a scalar edit.
pub fn build_mesh_buffers_to_layout(
    vb: &VertexBuffer,
    indices: &[u32],
    target: &[InputLayoutField],
) -> Result<EncodedMesh, DecodeError> {
    encode_mesh(assemble_to_layout(vb, target)?, indices)
}

/// Encodes an already-assembled interleaved buffer + its triangle list into
/// `MVTX`/`MIDX`. Shared by [`build_mesh_buffers`] (default layout) and
/// [`build_mesh_buffers_to_layout`] (target layout).
fn encode_mesh(asm: AssembledBuffer, indices: &[u32]) -> Result<EncodedMesh, DecodeError> {
    let mvtx = encode_vertex_buffer(asm.element_count, asm.stride, &asm.data)?;

    if !indices.len().is_multiple_of(3) {
        return Err(DecodeError::Model(
            "index count is not a multiple of 3 (expected a triangle list)",
        ));
    }
    let vertex_count = asm.element_count as u64;
    if indices.iter().any(|&i| u64::from(i) >= vertex_count) {
        return Err(DecodeError::Model(
            "index references a vertex past the assembled buffer",
        ));
    }

    let index_size = if indices.iter().any(|&i| i > u32::from(u16::MAX)) {
        4
    } else {
        2
    };
    let mut index_bytes = Vec::with_capacity(indices.len() * index_size);
    for &i in indices {
        if index_size == 2 {
            // Bounded above by u16::MAX in this branch, so the narrow is lossless.
            index_bytes.extend_from_slice(&u16::try_from(i).unwrap_or(0).to_le_bytes());
        } else {
            index_bytes.extend_from_slice(&i.to_le_bytes());
        }
    }
    let midx = encode_index_buffer(indices.len(), index_size, &index_bytes)?;

    Ok(EncodedMesh {
        mvtx,
        midx,
        mvtx_raw: asm.data,
        midx_raw: index_bytes,
        vertex_count: asm.element_count,
        stride: asm.stride,
        index_count: indices.len(),
        index_size,
        fields: asm.fields,
    })
}

/// Reads a new mesh part from an edited `.glb` and encodes it to `MVTX`/`MIDX`
/// in one step. Convenience over [`read_edited_mesh`] + [`build_mesh_buffers`].
pub fn build_mesh_buffers_from_glb(
    glb_bytes: &[u8],
    mesh_name: Option<&str>,
) -> Result<EncodedMesh, DecodeError> {
    let (vb, indices) = read_edited_mesh(glb_bytes, mesh_name)?;
    build_mesh_buffers(&vb, &indices)
}

#[cfg(test)]
mod tests {
    // Positions round-trip through our own glTF writer/reader exactly, so the
    // tests assert exact float-array equality deliberately.
    #![allow(clippy::float_cmp)]

    use super::*;
    use crate::model::VertexBuffer;

    fn quad() -> (VertexBuffer, Vec<u32>) {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let vb = VertexBuffer {
            element_count: 4,
            stride: 0,
            normals: vec![[0.0, 0.0, 1.0]; 4],
            positions,
            ..Default::default()
        };
        (vb, vec![0, 1, 2, 0, 2, 3])
    }

    /// `to_edit_glb` then `read_edited_positions` recovers the exact positions,
    /// in original vertex order, via the `_ORIGID` carrier (identity node
    /// transform, no Blender).
    #[test]
    fn edit_glb_round_trips_positions_by_id() {
        let (vb, indices) = quad();
        let glb = glb::to_edit_glb(&vb, &indices).expect("write edit glb");
        let recovered = read_edited_positions(&glb, vb.element_count).expect("read");
        assert_eq!(recovered, vb.positions);
    }

    /// A buffer with more original vertices than the glb carries (a vertex was
    /// removed in Blender) is rejected, not silently mis-spliced.
    #[test]
    fn edit_glb_missing_vertex_is_rejected() {
        let (vb, indices) = quad();
        let glb = glb::to_edit_glb(&vb, &indices).expect("write edit glb");
        // Ask for 5 originals when the glb only carries ids 0..=3.
        assert!(read_edited_positions(&glb, 5).is_err());
    }

    /// `build_mesh_buffers` encodes both buffers and they decode back: the quad's
    /// positions/normals survive the MVTX round-trip and its triangle list
    /// survives the MIDX round-trip (the T1c offline gate over both encoders).
    #[test]
    fn build_mesh_buffers_round_trips_quad() {
        let (vb, indices) = quad();
        let enc = build_mesh_buffers(&vb, &indices).expect("build buffers");
        assert_eq!(enc.mvtx[0], 0xa1, "vertex codec header");
        assert_eq!(enc.midx[0], 0xe1, "index codec header");
        assert_eq!(enc.stride, 24, "POSITION + NORMAL, no skinning");
        assert_eq!(enc.index_size, 2, "small mesh uses 16-bit indices");

        let vdesc = BufferDesc {
            block_index: 0,
            element_count: enc.vertex_count,
            element_size: enc.stride,
            meshopt: true,
            zstd: false,
            fields: enc.fields.clone(),
        };
        let on_disk = vdesc.decode(&enc.mvtx, true).expect("decode mvtx");
        assert_eq!(on_disk.positions().expect("positions"), vb.positions);

        let idesc = BufferDesc {
            block_index: 0,
            element_count: enc.index_count,
            element_size: enc.index_size,
            meshopt: true,
            zstd: false,
            fields: Vec::new(),
        };
        let idx = idesc.decode(&enc.midx, false).expect("decode midx");
        assert_eq!(
            idx.read_indices(0, enc.index_count, 0).expect("indices"),
            indices
        );
    }

    /// A non-triangle-list index count is rejected.
    #[test]
    fn build_mesh_buffers_rejects_non_triangle_indices() {
        let (vb, _) = quad();
        assert!(build_mesh_buffers(&vb, &[0, 1, 2, 3]).is_err());
    }

    /// End-to-end on a real edited glb (gated on `MORPHIC_EDIT_GLB`, e.g. a
    /// `to_glb_textured` export of the hornet model; `MORPHIC_EDIT_MESH` picks
    /// the single-primitive part, default `gun`): read the mesh, assemble +
    /// encode both buffers, decode them back, and assert every attribute and the
    /// triangle list round-trip (positions/normals/uv exact, joints exact,
    /// weights within the u8-unorm quantum).
    #[test]
    fn build_mesh_buffers_round_trips_real_glb_local() {
        let Ok(path) = std::env::var("MORPHIC_EDIT_GLB") else {
            eprintln!("MORPHIC_EDIT_GLB not set; skipping real glb mesh round-trip");
            return;
        };
        let mesh = std::env::var("MORPHIC_EDIT_MESH").unwrap_or_else(|_| "gun".to_string());
        let glb = std::fs::read(&path).expect("read glb");

        let (vb, indices) = read_edited_mesh(&glb, Some(&mesh)).expect("read edited mesh");
        eprintln!(
            "read {mesh}: {} verts, {} indices (joints={}, weights={}, uv={}, normals={})",
            vb.element_count,
            indices.len(),
            vb.joints.len(),
            vb.weights.len(),
            vb.texcoords.first().map_or(0, Vec::len),
            vb.normals.len(),
        );

        let enc = build_mesh_buffers(&vb, &indices).expect("build buffers");
        assert_eq!(enc.mvtx[0], 0xa1);
        assert_eq!(enc.midx[0], 0xe1);

        let vdesc = BufferDesc {
            block_index: 0,
            element_count: enc.vertex_count,
            element_size: enc.stride,
            meshopt: true,
            zstd: false,
            fields: enc.fields.clone(),
        };
        let on_disk = vdesc.decode(&enc.mvtx, true).expect("decode mvtx");

        // Uncompressed float lanes round-trip exactly.
        assert_eq!(on_disk.positions().expect("positions"), vb.positions);
        if !vb.normals.is_empty() {
            let f = on_disk
                .fields
                .iter()
                .find(|f| f.semantic_name == "NORMAL")
                .unwrap();
            let (normals, _) = on_disk.normal_tangent(f).expect("normals");
            assert_eq!(normals, vb.normals, "normals");
        }
        if let Some(uv) = vb.texcoords.first() {
            let f = on_disk
                .fields
                .iter()
                .find(|f| f.semantic_name == "TEXCOORD")
                .unwrap();
            assert_eq!(&on_disk.vector2(f).expect("uv"), uv, "uv");
        }
        if !vb.joints.is_empty() {
            let f = on_disk
                .fields
                .iter()
                .find(|f| f.semantic_name == "BLENDINDICES")
                .unwrap();
            let flat = on_disk.blend_indices(f, None).expect("joints");
            let got: Vec<[u16; 4]> = flat
                .chunks_exact(4)
                .map(|c| [c[0], c[1], c[2], c[3]])
                .collect();
            assert_eq!(got, vb.joints, "joints (identity remap)");
        }
        if !vb.weights.is_empty() {
            let f = on_disk
                .fields
                .iter()
                .find(|f| f.semantic_name == "BLENDWEIGHT")
                .unwrap();
            let flat = on_disk.blend_weights(f).expect("weights");
            for (i, want) in vb.weights.iter().enumerate() {
                for k in 0..4 {
                    assert!(
                        (flat[i * 4 + k] - want[k]).abs() <= 3.0 / 255.0,
                        "weight vertex {i} lane {k}: {} vs {}",
                        flat[i * 4 + k],
                        want[k]
                    );
                }
            }
        }

        let idesc = BufferDesc {
            block_index: 0,
            element_count: enc.index_count,
            element_size: enc.index_size,
            meshopt: true,
            zstd: false,
            fields: Vec::new(),
        };
        let idx = idesc.decode(&enc.midx, false).expect("decode midx");
        assert_eq!(
            idx.read_indices(0, enc.index_count, 0).expect("indices"),
            indices,
            "index round-trip"
        );
    }
}
