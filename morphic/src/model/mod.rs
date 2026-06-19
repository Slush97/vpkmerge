//! Source 2 model (`.vmdl_c`) decoding.
//!
//! For now this exposes [`inspect`], a cheap structural read of a compiled
//! model: it parses the resource container and summarizes the block table so
//! callers (and `vpkmerge model inspect`) can see how heavy a model is and
//! whether it carries embedded geometry vs. only material overrides. Full
//! mesh decode (meshoptimizer `MVTX`/`MIDX` + KV3 `MDAT`) lands in a later
//! milestone; see `vpkmerge/docs/vmdl-glb-exporter.md`.
//!
//! [`decode`] performs the full M3 read: skeleton (from `DATA`), LOD0 mesh
//! assembly (from `CTRL` + `MDAT` + `MVTX`/`MIDX`), and per-vertex skin
//! weights remapped onto the model skeleton. Materials/textures (M4) and the
//! `.glb` writer (M5) build on the [`Model`] this returns.

mod animation;
mod dxgi;
mod edit;
mod femodel;
mod glb;
mod gltf_import;
mod math;
mod mesh;
mod nm;
mod pose;
mod skeleton;
mod topology;
mod uvmask;
mod vbib;

#[cfg(test)]
mod tests;

pub use animation::{BoneTrack, Clip};
pub use edit::{
    apply_edited_glb, build_mesh_buffers, build_mesh_buffers_from_glb,
    build_mesh_buffers_to_layout, export_buffer_for_edit, read_edited_mesh, read_edited_primitives,
    read_vertex_colors, read_vertex_positions, recolor_vertex_buffer, replace_vertex_positions,
    reskin_vertex_buffer, vertex_targets, EditedPrimitive, EncodedMesh, VertexTarget,
};
pub use femodel::{decode_cloth_anchors, ClothAnchors};
pub use glb::{to_glb, to_glb_textured, FileResolver};
pub use gltf_import::{
    apply_animation, import_glb_onto_nm_clip, read_glb_animation, GltfAnimation, GltfBoneTrack,
};
pub use math::{Mat4, Quat, Vec3};
pub use mesh::{
    assemble_to_layout, assemble_vertex_buffer, AssembledBuffer, MeshPart, Primitive, VertexBuffer,
};
pub use nm::{
    bake_nm_pose, decode_nm_clip, decode_nm_pose, decode_nm_skeleton, decode_pose_stream,
    encode_compressed_pose, nm_clip_to_clip, reencode_nm_clip, reencode_nm_clip_full, NmClip,
    NmPose, NmSkeleton, NmTrack, QuantRange, TrackSettings,
};
pub use pose::{
    bake_pose, bake_pose_from, bake_pose_named, secondary_motion_pose_report, LocalPose,
    SecondaryMotionBoneInfluence, SecondaryMotionMaterialReport, SecondaryMotionPoseReport,
};
pub use skeleton::{invert_remap, localize_joints, remap_table, Bone, Skeleton};
pub use topology::{
    append_skinned_draw_call, draw_call_targets, reencode_all_mdat_identity,
    remove_draw_calls_by_material, replace_mesh_group, replace_mesh_group_uncompressed,
    replace_mesh_part, replace_mesh_part_uncompressed, set_draw_call_groups, set_model_material,
    DrawCallGroup, DrawCallInfo, PrimitiveSelection, RemovedDrawCall, ReplacedMeshGroup,
    ReplacedMeshPart,
};
pub use uvmask::{
    atlas_png, mask_png, segment_color, segment_coverage, segments, Segment, SegmentBy,
};

use crate::error::DecodeError;
use crate::resource::Resource;

const CTRL: [u8; 4] = *b"CTRL";

/// An axis-aligned bounding box in the model's source coordinate space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// A fully decoded model: the skin skeleton, the LOD0 meshes, and the model's
/// own animation clips (empty when the model carries no `ANIM`/`AGRP` blocks).
#[derive(Debug, Clone)]
pub struct Model {
    pub skeleton: Skeleton,
    pub meshes: Vec<MeshPart>,
    pub animations: Vec<Clip>,
    /// Cloth (`FeModel`) anchor map from the model's `PHYS` block, when present:
    /// each `$cloth_*` root bone -> the body bone that drives it. Populated by
    /// [`decode`]; the pose baker uses it to keep solver-driven fabric attached
    /// to the posed body instead of leaving it stranded at bind. `None` for
    /// models with no cloth physics (weapons, props, FK-only heroes).
    pub cloth: Option<femodel::ClothAnchors>,
}

impl Model {
    /// Total unique vertices across all LOD0 vertex buffers (matches the `glTF`
    /// accessor vertex total, since primitives index into shared buffers).
    #[must_use]
    pub fn total_vertices(&self) -> usize {
        self.meshes
            .iter()
            .flat_map(|m| &m.vertex_buffers)
            .map(|vb| vb.element_count)
            .sum()
    }

    /// Per-primitive vertex total: each primitive contributes its whole source
    /// vertex buffer's element count. This is what a glTF tool reports when it
    /// sums `POSITION` accessor counts across primitives (buffers shared by
    /// several primitives are counted once per primitive), so it is larger than
    /// [`Model::total_vertices`].
    #[must_use]
    pub fn gltf_vertex_total(&self) -> usize {
        self.meshes
            .iter()
            .flat_map(|m| {
                m.primitives
                    .iter()
                    .map(move |p| m.vertex_buffers[p.vertex_buffer].element_count)
            })
            .sum()
    }

    /// Total primitive indices across all LOD0 draw calls.
    #[must_use]
    pub fn total_indices(&self) -> usize {
        self.meshes
            .iter()
            .flat_map(|m| &m.primitives)
            .map(|p| p.indices.len())
            .sum()
    }

    /// Sorted, de-duplicated material paths referenced by LOD0 primitives.
    #[must_use]
    pub fn materials(&self) -> Vec<String> {
        let mut mats: Vec<String> = self
            .meshes
            .iter()
            .flat_map(|m| &m.primitives)
            .map(|p| p.material.clone())
            .collect();
        mats.sort();
        mats.dedup();
        mats
    }

    /// Source-space bounds over every decoded LOD0 position, or `None` when the
    /// model carries no positions.
    #[must_use]
    pub fn position_bounds(&self) -> Option<Aabb> {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut seen = false;
        for vb in self.meshes.iter().flat_map(|m| &m.vertex_buffers) {
            for p in &vb.positions {
                seen = true;
                for i in 0..3 {
                    min[i] = min[i].min(p[i]);
                    max[i] = max[i].max(p[i]);
                }
            }
        }
        seen.then_some(Aabb { min, max })
    }
}

impl mesh::BlockSource for Resource<'_> {
    fn block(&self, index: usize) -> Option<&[u8]> {
        self.get_block_by_index(index)
    }
}

/// Decodes a `.vmdl_c` into an in-memory [`Model`]: the model skeleton and the
/// LOD0 meshes with positions/normals/uv/joints/weights. Does not resolve
/// materials/textures (M4) or write a `.glb` (M5).
pub fn decode(bytes: &[u8]) -> Result<Model, DecodeError> {
    let resource = Resource::parse(bytes)?;

    let data = crate::kv3::decode(resource.data_block()?)?;
    let ctrl_bytes = resource
        .find_block(CTRL)
        .ok_or(DecodeError::Model("model has no CTRL block"))?;
    let ctrl = crate::kv3::decode(ctrl_bytes)?;

    let skeleton = Skeleton::from_model_data(&data)?;
    let embedded = mesh::EmbeddedMesh::parse_all(&ctrl)?;
    let lod0 = mesh::lod0_indices(&data, &embedded)?;
    // Drop body-group variants that are not in the default selection (alt heads,
    // props, "sleeping" sets, UI-only meshes); no-op for single-group models.
    let lod0 = mesh::filter_default_mesh_group(&data, lod0);

    let mut meshes = Vec::with_capacity(lod0.len());
    for i in lod0 {
        let em = &embedded[i];
        let remap = skeleton::remap_table(&data, em.mesh_index);
        meshes.push(mesh::assemble(em, &resource, remap.as_deref())?);
    }

    // Best-effort: a model whose animation blocks fail to decode still exports
    // its static mesh (mirrors the texture path's tolerance of bad slots).
    let animations = animation::decode_all(&resource, &skeleton).unwrap_or_default();

    Ok(Model {
        skeleton,
        meshes,
        animations,
        cloth: femodel::decode_cloth_anchors(bytes),
    })
}

/// Parses just the model skeleton from a `.vmdl_c`. Cheap relative to [`decode`]
/// (no buffer decode); useful for bone-name retarget checks.
pub fn decode_skeleton(bytes: &[u8]) -> Result<Skeleton, DecodeError> {
    let resource = Resource::parse(bytes)?;
    let data = crate::kv3::decode(resource.data_block()?)?;
    Skeleton::from_model_data(&data)
}

/// One entry in a model's block table.
#[derive(Debug, Clone)]
pub struct BlockSummary {
    /// 4-char block type (e.g. `MVTX`, `MIDX`, `MDAT`, `DATA`).
    pub kind: String,
    /// Block size in bytes.
    pub size: u32,
}

/// Structural summary of a compiled model resource.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Every block in declaration order.
    pub blocks: Vec<BlockSummary>,
    /// Number of `MVTX` vertex buffers (one per renderable mesh part).
    pub mesh_parts: usize,
    /// Number of `MIDX` index buffers.
    pub index_buffers: usize,
    /// True if the model carries its own geometry (`MVTX` present) rather than
    /// only overriding materials and referencing a base-game mesh.
    pub has_embedded_geometry: bool,
    /// True if skeleton/animation blocks (`ANIM`/`ASEQ`/`AGRP`) are present.
    pub has_skeleton_anim: bool,
    /// True if a collision block (`PHYS`) is present.
    pub has_physics: bool,
    /// Sum of all `MVTX` block sizes, a rough geometry-weight signal.
    pub vertex_bytes: u64,
}

const MVTX: [u8; 4] = *b"MVTX";
const MIDX: [u8; 4] = *b"MIDX";
const PHYS: [u8; 4] = *b"PHYS";
const ANIM: [u8; 4] = *b"ANIM";
const ASEQ: [u8; 4] = *b"ASEQ";
const AGRP: [u8; 4] = *b"AGRP";

/// Parse a `.vmdl_c` resource and summarize its block table. Does not decode
/// geometry; this is the cheap structural read.
pub fn inspect(bytes: &[u8]) -> Result<ModelInfo, DecodeError> {
    let resource = Resource::parse(bytes)?;

    let mut blocks = Vec::new();
    let mut mesh_parts = 0usize;
    let mut index_buffers = 0usize;
    let mut has_skeleton_anim = false;
    let mut has_physics = false;
    let mut vertex_bytes = 0u64;

    for b in resource.blocks() {
        match b.kind {
            MVTX => {
                mesh_parts += 1;
                vertex_bytes += u64::from(b.size);
            }
            MIDX => index_buffers += 1,
            PHYS => has_physics = true,
            ANIM | ASEQ | AGRP => has_skeleton_anim = true,
            _ => {}
        }
        blocks.push(BlockSummary {
            kind: String::from_utf8_lossy(&b.kind).into_owned(),
            size: b.size,
        });
    }

    Ok(ModelInfo {
        has_embedded_geometry: mesh_parts > 0,
        blocks,
        mesh_parts,
        index_buffers,
        has_skeleton_anim,
        has_physics,
        vertex_bytes,
    })
}
