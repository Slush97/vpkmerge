//! Binary glTF (`.glb`) writer for a decoded [`Model`]. Builds the glTF 2.0
//! document with `gltf-json` and frames the GLB container by hand.
//!
//! This is the M5a slice: geometry + skeleton + skin + *untextured* materials
//! (a named default-PBR material per draw-call material path). Texture
//! resolution/decode/embedding is M5b/M6 (needs the cross-VPK loader).
//!
//! Coordinate handling mirrors VRF `GltfModelExporter`: vertex positions and
//! bone local transforms stay in Source space; a wrapper node over the skeleton
//! and each mesh carries `TRANSFORMSOURCETOGLTF` (inches->meters + Z-up->Y-up),
//! and the inverse-bind matrices are the Source-space `inverse(globalBind)`
//! (computed before that transform, per VRF's "order matters"). Keeping bone
//! local transforms in Source space is what lets Grimoire retarget the shared
//! clips by bone name.
//!
//! morphic's [`Mat4`] is row-major / row-vector; glTF stores column-major /
//! column-vector. Row-major storage of `M` equals column-major storage of
//! `Mᵀ`, and `Mᵀ` is exactly the column-vector form of a row-vector `M`, so a
//! `Mat4.m` array is emitted into glTF verbatim.

// Matrices/positions widen between f32 and JSON f64 (serde) losslessly enough;
// buffer lengths are small. These casts are deliberate. `default_trait_access`
// is allowed because gltf-json structs carry many `extras`/`extensions` fields
// that are idiomatically initialized with `Default::default()`.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::default_trait_access
)]

use std::collections::{BTreeMap, BTreeSet};

use gltf_json as json;
use json::validation::Checked::Valid;
use json::validation::USize64;
use serde_json::json as jval;

use crate::error::DecodeError;

use super::math::{Mat4, Quat};
use super::mesh::{MeshPart, VertexBuffer};
use super::{Clip, Model, Skeleton};

/// Source space (inches, Z-up) to glTF space (meters, Y-up). Matches VRF
/// `TRANSFORMSOURCETOGLTF = CreateScale(0.0254) * CreateFromYawPitchRoll(0, -pi/2, -pi/2)`.
fn transform_source_to_gltf() -> Mat4 {
    use std::f32::consts::FRAC_PI_2;
    let rot = Mat4::from_quaternion(Quat::from_yaw_pitch_roll(0.0, -FRAC_PI_2, -FRAC_PI_2));
    Mat4::from_scale(0.0254).mul(&rot)
}

/// Resolves a compiled resource path (e.g. `models/.../foo.vtex_c`) to its
/// bytes. Implemented by the caller, which owns the VPK I/O (skin VPK first,
/// base pak second). Keeps `morphic` free of file/VPK access.
pub trait FileResolver {
    fn resolve(&self, compiled_path: &str) -> Option<Vec<u8>>;
}

/// Writes `model` as a binary glTF (`.glb`) byte stream, untextured (named
/// default-PBR materials only).
pub fn to_glb(model: &Model) -> Result<Vec<u8>, DecodeError> {
    build(model, None)
}

/// Writes `model` as a `.glb` with materials textured from the resolved
/// `.vmat_c` / `.vtex_c` files: base color, normal, metallic-roughness
/// (roughness split out of the packed normal texture), occlusion, and emissive.
/// Materials that fail to resolve fall back to a named default.
pub fn to_glb_textured(model: &Model, files: &dyn FileResolver) -> Result<Vec<u8>, DecodeError> {
    build(model, Some(files))
}

/// Writes a single vertex buffer as a minimal `.glb` for external editing
/// (Blender): one mesh, one triangle primitive over `vb` with POSITION + NORMAL,
/// plus a custom `_ORIGID` SCALAR attribute holding each vertex's original index.
///
/// Positions are emitted in raw Source space (no Y-up/scale transform), so the
/// edited positions read straight back into [`super::replace_vertex_positions`]
/// with no inverse transform (the model just appears Z-up in a glTF viewer). The
/// `_ORIGID` carrier survives Blender's import/export vertex split + reorder, so
/// the edited mesh maps back to the original buffer by id. `indices` is the
/// buffer's triangulation (concatenated draw-call indices).
pub fn to_edit_glb(vb: &VertexBuffer, indices: &[u32]) -> Result<Vec<u8>, DecodeError> {
    let mut b = Builder::default();
    b.root.asset.generator = Some("morphic".to_owned());
    let count = vb.element_count;

    let pos_bytes: Vec<u8> = vb.positions.iter().flat_map(f32x).collect();
    let pos_view = b.add_view(&pos_bytes, json::buffer::Target::ArrayBuffer);
    let (min, max) = bounds(&vb.positions);
    let position = b.add_accessor(
        pos_view,
        count,
        json::accessor::ComponentType::F32,
        json::accessor::Type::Vec3,
        Some((min, max)),
    );

    let mut attributes = BTreeMap::new();
    attributes.insert(Valid(json::mesh::Semantic::Positions), position);

    if vb.normals.len() == count {
        let nb: Vec<u8> = vb.normals.iter().flat_map(f32x).collect();
        let nv = b.add_view(&nb, json::buffer::Target::ArrayBuffer);
        let na = b.add_accessor(
            nv,
            count,
            json::accessor::ComponentType::F32,
            json::accessor::Type::Vec3,
            None,
        );
        attributes.insert(Valid(json::mesh::Semantic::Normals), na);
    }

    // _ORIGID carrier: per-vertex f32 index (exact for counts < 2^24).
    let id_bytes: Vec<u8> = (0..count).flat_map(|i| (i as f32).to_le_bytes()).collect();
    let id_view = b.add_view(&id_bytes, json::buffer::Target::ArrayBuffer);
    let id_acc = b.add_accessor(
        id_view,
        count,
        json::accessor::ComponentType::F32,
        json::accessor::Type::Scalar,
        None,
    );
    attributes.insert(
        Valid(json::mesh::Semantic::Extras(ORIGID_ATTR.to_string())),
        id_acc,
    );

    let tri_bytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    let tri_view = b.add_view(&tri_bytes, json::buffer::Target::ElementArrayBuffer);
    let tri_acc = b.add_accessor(
        tri_view,
        indices.len(),
        json::accessor::ComponentType::U32,
        json::accessor::Type::Scalar,
        None,
    );

    let primitive = json::mesh::Primitive {
        attributes,
        indices: Some(tri_acc),
        material: None,
        mode: Valid(json::mesh::Mode::Triangles),
        targets: None,
        extensions: None,
        extras: Default::default(),
    };
    let mesh = b.root.push(json::Mesh {
        primitives: vec![primitive],
        weights: None,
        name: Some("edit".to_string()),
        extensions: None,
        extras: Default::default(),
    });
    let node = b.root.push(json::Node {
        mesh: Some(mesh),
        name: Some("edit".to_string()),
        ..default_node()
    });
    b.root.push(json::Scene {
        nodes: vec![node],
        extensions: None,
        extras: Default::default(),
        name: None,
    });
    b.root.scene = Some(json::Index::new(0));
    b.finish()
}

/// Semantic name for the original-vertex-index carrier. `gltf-json`'s
/// `Semantic::Extras` serialization prepends one `_`, so this string `"ORIGID"`
/// lands on disk as the conventional `_ORIGID` custom attribute (a single
/// underscore, which Blender round-trips cleanly).
pub(crate) const ORIGID_ATTR: &str = "ORIGID";

fn build(model: &Model, files: Option<&dyn FileResolver>) -> Result<Vec<u8>, DecodeError> {
    let mut b = Builder::default();
    b.root.asset.generator = Some("morphic".to_owned());

    let skin = b.add_skin(&model.skeleton);

    let mut mat_index: BTreeMap<String, json::Index<json::Material>> = BTreeMap::new();
    let mut scene_nodes: Vec<json::Index<json::Node>> = Vec::new();

    // Skeleton wrapper node carries the axis/scale transform; bone nodes hang
    // off it with Source-space local transforms.
    if let Some(s) = &skin {
        scene_nodes.push(s.root_node);
    }

    for part in &model.meshes {
        let Some(mesh) = b.add_mesh(part, &mut mat_index, files) else {
            continue; // every primitive was an outline shell
        };
        let node = b.root.push(json::Node {
            mesh: Some(mesh),
            skin: skin.as_ref().map(|s| s.skin),
            matrix: Some(transform_source_to_gltf().m),
            name: Some(part.name.clone()),
            ..default_node()
        });
        scene_nodes.push(node);
    }

    // Animation clips drive the skin's joint nodes; they need no scene node.
    if let Some(s) = &skin {
        b.add_animations(model, s);
    }

    b.root.push(json::Scene {
        nodes: scene_nodes,
        extensions: None,
        extras: Default::default(),
        name: None,
    });
    b.root.scene = Some(json::Index::new(0));

    b.finish()
}

/// Accumulates the single GLB binary buffer alongside the glTF document.
#[derive(Default)]
struct Builder {
    root: json::Root,
    bin: Vec<u8>,
    /// Shared sampler for all textures, created lazily.
    sampler: Option<json::Index<json::texture::Sampler>>,
    /// KHR material extensions gltf-json 1.x cannot represent with our feature
    /// set (sheen has no cargo feature at all), keyed by material index. They
    /// are injected into the serialized JSON by [`inject_material_extensions`]
    /// in [`Builder::finish`]; one mechanism covers every extension we emit.
    material_extensions: BTreeMap<usize, serde_json::Map<String, serde_json::Value>>,
}

struct SkinRefs {
    skin: json::Index<json::Skin>,
    root_node: json::Index<json::Node>,
    /// Joint node index per model-skeleton bone, in bone order. Animation
    /// channels target these by bone index.
    bone_nodes: Vec<json::Index<json::Node>>,
}

impl Builder {
    /// Appends `bytes` as a new buffer view (4-byte aligned), returning its index.
    fn add_view(
        &mut self,
        bytes: &[u8],
        target: json::buffer::Target,
    ) -> json::Index<json::buffer::View> {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        self.root.push(json::buffer::View {
            buffer: json::Index::new(0),
            byte_length: USize64(bytes.len() as u64),
            byte_offset: Some(USize64(offset as u64)),
            byte_stride: None,
            target: Some(Valid(target)),
            name: None,
            extensions: None,
            extras: Default::default(),
        })
    }

    fn add_accessor(
        &mut self,
        view: json::Index<json::buffer::View>,
        count: usize,
        component_type: json::accessor::ComponentType,
        type_: json::accessor::Type,
        min_max: Option<(json::Value, json::Value)>,
    ) -> json::Index<json::Accessor> {
        let (min, max) = match min_max {
            Some((mn, mx)) => (Some(mn), Some(mx)),
            None => (None, None),
        };
        self.root.push(json::Accessor {
            buffer_view: Some(view),
            byte_offset: Some(USize64(0)),
            count: USize64(count as u64),
            component_type: Valid(json::accessor::GenericComponentType(component_type)),
            type_: Valid(type_),
            min,
            max,
            name: None,
            normalized: false,
            sparse: None,
            extensions: None,
            extras: Default::default(),
        })
    }

    /// Builds the model skin: a wrapper node, the bone hierarchy, and the
    /// inverse-bind-matrix accessor. Returns `None` for an empty skeleton.
    fn add_skin(&mut self, skeleton: &Skeleton) -> Option<SkinRefs> {
        if skeleton.bones.is_empty() {
            return None;
        }

        // One node per bone, with Source-space local translation + rotation.
        let bone_nodes: Vec<json::Index<json::Node>> = skeleton
            .bones
            .iter()
            .map(|bone| {
                self.root.push(json::Node {
                    translation: Some([bone.position.x, bone.position.y, bone.position.z]),
                    rotation: Some(json::scene::UnitQuaternion([
                        bone.rotation.x,
                        bone.rotation.y,
                        bone.rotation.z,
                        bone.rotation.w,
                    ])),
                    name: Some(bone.name.clone()),
                    ..default_node()
                })
            })
            .collect();

        // Wire children and collect roots.
        let mut children: Vec<Vec<json::Index<json::Node>>> =
            vec![Vec::new(); skeleton.bones.len()];
        let mut roots: Vec<json::Index<json::Node>> = Vec::new();
        for (i, bone) in skeleton.bones.iter().enumerate() {
            match bone.parent {
                Some(p) => children[p].push(bone_nodes[i]),
                None => roots.push(bone_nodes[i]),
            }
        }
        for (i, kids) in children.into_iter().enumerate() {
            if !kids.is_empty() {
                self.root.nodes[bone_nodes[i].value()].children = Some(kids);
            }
        }

        // Inverse-bind matrices, one MAT4 per bone, in bone-index order.
        let mut ibm = Vec::with_capacity(skeleton.bones.len() * 64);
        for bone in &skeleton.bones {
            for f in bone.inverse_bind.m {
                ibm.extend_from_slice(&f.to_le_bytes());
            }
        }
        let ibm_view = self.add_view(&ibm, json::buffer::Target::ArrayBuffer);
        let ibm_accessor = self.add_accessor(
            ibm_view,
            skeleton.bones.len(),
            json::accessor::ComponentType::F32,
            json::accessor::Type::Mat4,
            None,
        );

        let root_node = self.root.push(json::Node {
            children: Some(roots),
            matrix: Some(transform_source_to_gltf().m),
            name: Some("skeleton".to_owned()),
            ..default_node()
        });

        let skin = self.root.push(json::Skin {
            joints: bone_nodes.clone(),
            inverse_bind_matrices: Some(ibm_accessor),
            skeleton: Some(root_node),
            name: None,
            extensions: None,
            extras: Default::default(),
        });

        Some(SkinRefs {
            skin,
            root_node,
            bone_nodes,
        })
    }

    /// Emits one glTF animation per decoded clip. Each animated bone+channel
    /// becomes a `(sampler, channel)` pair: the sampler shares the clip's time
    /// accessor (`frame / fps`) as input and the channel targets that bone's
    /// joint node. Keyframe values are raw Source/local space, exactly like the
    /// bind-pose bone nodes; the source->glTF transform lives on the skeleton
    /// wrapper node above them and must not be applied here.
    fn add_animations(&mut self, model: &Model, skin: &SkinRefs) {
        use json::animation::Property;

        for clip in &model.animations {
            if clip.frame_count == 0 {
                continue;
            }
            let input = self.time_accessor(clip);

            let mut samplers: Vec<json::animation::Sampler> = Vec::new();
            let mut channels: Vec<json::animation::Channel> = Vec::new();

            for track in &clip.tracks {
                let Some(&node) = skin.bone_nodes.get(track.bone) else {
                    continue; // a decoded bone with no joint node (defensive)
                };
                if let Some(t) = &track.translations {
                    let out = self.f32_accessor(t.iter().flat_map(|v| [v.x, v.y, v.z]), 3);
                    push_channel(
                        &mut samplers,
                        &mut channels,
                        input,
                        out,
                        node,
                        Property::Translation,
                    );
                }
                if let Some(r) = &track.rotations {
                    let out = self.f32_accessor(r.iter().flat_map(|q| [q.x, q.y, q.z, q.w]), 4);
                    push_channel(
                        &mut samplers,
                        &mut channels,
                        input,
                        out,
                        node,
                        Property::Rotation,
                    );
                }
                if let Some(s) = &track.scales {
                    // Source bone scale is uniform; glTF wants a Vec3.
                    let out = self.f32_accessor(s.iter().flat_map(|&v| [v, v, v]), 3);
                    push_channel(
                        &mut samplers,
                        &mut channels,
                        input,
                        out,
                        node,
                        Property::Scale,
                    );
                }
            }

            if channels.is_empty() {
                continue; // clip animated nothing on this skeleton
            }
            self.root.push(json::Animation {
                name: Some(clip.name.clone()),
                channels,
                samplers,
                extensions: None,
                extras: Default::default(),
            });
        }
    }

    /// SCALAR f32 time accessor for a clip (`frame / fps`). glTF requires
    /// `min`/`max` on a sampler's input accessor; times are monotonic so they
    /// are the first and last sample.
    fn time_accessor(&mut self, clip: &Clip) -> json::Index<json::Accessor> {
        let fps = if clip.fps > 0.0 { clip.fps } else { 1.0 };
        let times: Vec<f32> = (0..clip.frame_count).map(|f| f as f32 / fps).collect();
        let bytes: Vec<u8> = times.iter().flat_map(|t| t.to_le_bytes()).collect();
        let view = self.add_image_view(&bytes); // targetless: animation data
        let min = json::serialize::to_value(vec![times.first().copied().unwrap_or(0.0)]).unwrap();
        let max = json::serialize::to_value(vec![times.last().copied().unwrap_or(0.0)]).unwrap();
        self.add_accessor(
            view,
            times.len(),
            json::accessor::ComponentType::F32,
            json::accessor::Type::Scalar,
            Some((min, max)),
        )
    }

    /// Output accessor for animation samples: `components` f32 per element
    /// (3 = VEC3 translation/scale, 4 = VEC4 rotation). No `min`/`max` (only the
    /// input accessor needs them).
    fn f32_accessor(
        &mut self,
        values: impl Iterator<Item = f32>,
        components: usize,
    ) -> json::Index<json::Accessor> {
        let bytes: Vec<u8> = values.flat_map(f32::to_le_bytes).collect();
        let count = bytes.len() / 4 / components;
        let view = self.add_image_view(&bytes); // targetless: animation data
        let type_ = if components == 4 {
            json::accessor::Type::Vec4
        } else {
            json::accessor::Type::Vec3
        };
        self.add_accessor(view, count, json::accessor::ComponentType::F32, type_, None)
    }

    /// Builds one glTF mesh (its primitives + shared per-vertex-buffer
    /// accessors), or `None` when the whole part is dropped ([`is_dropped`]).
    /// Deadlock's NPR shells (the inverted-hull `*_outline` and the additive
    /// `*_glow` effect meshes) are dropped: as plain glTF geometry they collapse
    /// to an opaque hull that whitewashes the model. Reproducing them is a
    /// renderer-side (three.js) concern, not a baked one. Hidden-by-default
    /// alt-forms (Viscous's `inflated` Goo Ball) are dropped too: see
    /// [`is_alt_form`].
    fn add_mesh(
        &mut self,
        part: &MeshPart,
        mat_index: &mut BTreeMap<String, json::Index<json::Material>>,
        files: Option<&dyn FileResolver>,
    ) -> Option<json::Index<json::Mesh>> {
        if is_dropped(&part.name) {
            return None;
        }
        let renderable: Vec<_> = part
            .primitives
            .iter()
            .filter(|p| !is_dropped(&p.material))
            .collect();
        if renderable.is_empty() {
            return None;
        }

        // Deinterleaved attributes are shared by every primitive over a buffer.
        let vb_attrs: Vec<VertexAccessors> = part
            .vertex_buffers
            .iter()
            .map(|vb| self.add_vertex_buffer(vb, part.bone_weight_count))
            .collect();

        let mut primitives = Vec::with_capacity(renderable.len());
        for prim in renderable {
            let Some(attrs) = vb_attrs.get(prim.vertex_buffer) else {
                continue;
            };
            let Some(position) = attrs.position else {
                continue;
            };

            let mut attributes = BTreeMap::new();
            attributes.insert(Valid(json::mesh::Semantic::Positions), position);
            if let Some(a) = attrs.normal {
                attributes.insert(Valid(json::mesh::Semantic::Normals), a);
            }
            if let Some(a) = attrs.tangent {
                attributes.insert(Valid(json::mesh::Semantic::Tangents), a);
            }
            for (i, a) in attrs.texcoords.iter().enumerate() {
                attributes.insert(Valid(json::mesh::Semantic::TexCoords(i as u32)), *a);
            }
            if material_uses_vertex_color(&prim.material, files) {
                let mut colors = Vec::new();
                for &stream in &prim.vertex_buffers {
                    if let Some(stream_attrs) = vb_attrs.get(stream) {
                        colors.extend(stream_attrs.colors.iter().copied());
                    }
                }
                if colors.is_empty() {
                    colors.extend(attrs.colors.iter().copied());
                }
                for (i, a) in colors.into_iter().enumerate() {
                    attributes.insert(Valid(json::mesh::Semantic::Colors(i as u32)), a);
                }
            }
            if let Some(a) = attrs.joints0 {
                attributes.insert(Valid(json::mesh::Semantic::Joints(0)), a);
            }
            if let Some(a) = attrs.weights0 {
                attributes.insert(Valid(json::mesh::Semantic::Weights(0)), a);
            }

            let indices: Vec<u8> = prim.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            let idx_view = self.add_view(&indices, json::buffer::Target::ElementArrayBuffer);
            let idx_accessor = self.add_accessor(
                idx_view,
                prim.indices.len(),
                json::accessor::ComponentType::U32,
                json::accessor::Type::Scalar,
                None,
            );

            let material = self.material_for(&prim.material, mat_index, files);

            primitives.push(json::mesh::Primitive {
                attributes,
                indices: Some(idx_accessor),
                material: Some(material),
                mode: Valid(json::mesh::Mode::Triangles),
                targets: None,
                extensions: None,
                extras: Default::default(),
            });
        }

        if primitives.is_empty() {
            return None;
        }

        Some(self.root.push(json::Mesh {
            primitives,
            weights: None,
            name: Some(part.name.clone()),
            extensions: None,
            extras: Default::default(),
        }))
    }

    /// Writes one vertex buffer's attribute accessors. Skinned buffers with
    /// joints but no weights get VRF's default `1/bone_weight_count` spread.
    #[allow(clippy::too_many_lines)]
    fn add_vertex_buffer(
        &mut self,
        vb: &VertexBuffer,
        bone_weight_count: usize,
    ) -> VertexAccessors {
        let count = vb.element_count;

        let position = (vb.positions.len() == count).then(|| {
            let pos_bytes: Vec<u8> = vb.positions.iter().flat_map(f32x).collect();
            let pos_view = self.add_view(&pos_bytes, json::buffer::Target::ArrayBuffer);
            let (min, max) = bounds(&vb.positions);
            self.add_accessor(
                pos_view,
                count,
                json::accessor::ComponentType::F32,
                json::accessor::Type::Vec3,
                Some((min, max)),
            )
        });

        let normal = (vb.normals.len() == count).then(|| {
            let bytes: Vec<u8> = vb.normals.iter().flat_map(f32x).collect();
            let view = self.add_view(&bytes, json::buffer::Target::ArrayBuffer);
            self.add_accessor(
                view,
                count,
                json::accessor::ComponentType::F32,
                json::accessor::Type::Vec3,
                None,
            )
        });
        let tangent = (vb.tangents.len() == count).then(|| {
            let bytes: Vec<u8> = vb.tangents.iter().flat_map(f32x).collect();
            let view = self.add_view(&bytes, json::buffer::Target::ArrayBuffer);
            self.add_accessor(
                view,
                count,
                json::accessor::ComponentType::F32,
                json::accessor::Type::Vec4,
                None,
            )
        });
        // Every decoded TEXCOORD stream is emitted (TEXCOORD_0, TEXCOORD_1,
        // ...); detail/AO passes sample the second set.
        let texcoords: Vec<_> = vb
            .texcoords
            .iter()
            .filter(|uv| uv.len() == count)
            .map(|uv| {
                let bytes: Vec<u8> = uv.iter().flat_map(f32x).collect();
                let view = self.add_view(&bytes, json::buffer::Target::ArrayBuffer);
                self.add_accessor(
                    view,
                    count,
                    json::accessor::ComponentType::F32,
                    json::accessor::Type::Vec2,
                    None,
                )
            })
            .collect();
        let colors = vb
            .colors
            .iter()
            .filter(|c| c.len() == count)
            .map(|c| {
                let bytes: Vec<u8> = c.iter().flat_map(f32x).collect();
                let view = self.add_view(&bytes, json::buffer::Target::ArrayBuffer);
                self.add_accessor(
                    view,
                    count,
                    json::accessor::ComponentType::F32,
                    json::accessor::Type::Vec4,
                    None,
                )
            })
            .collect();

        let (joints0, weights0) = if vb.joints.is_empty() {
            (None, None)
        } else {
            let mut joints = vb.joints.clone();
            let mut weights = if vb.weights.is_empty() {
                default_weights(count, bone_weight_count)
            } else {
                vb.weights.clone()
            };
            fix_duplicate_joints(&mut joints, &mut weights);

            let jbytes: Vec<u8> = joints.iter().copied().flat_map(u16x).collect();
            let jview = self.add_view(&jbytes, json::buffer::Target::ArrayBuffer);
            let ja = self.add_accessor(
                jview,
                count,
                json::accessor::ComponentType::U16,
                json::accessor::Type::Vec4,
                None,
            );

            let wbytes: Vec<u8> = weights.iter().flat_map(f32x).collect();
            let wview = self.add_view(&wbytes, json::buffer::Target::ArrayBuffer);
            let wa = self.add_accessor(
                wview,
                count,
                json::accessor::ComponentType::F32,
                json::accessor::Type::Vec4,
                None,
            );

            (Some(ja), Some(wa))
        };

        VertexAccessors {
            position,
            normal,
            tangent,
            texcoords,
            colors,
            joints0,
            weights0,
        }
    }

    fn material_for(
        &mut self,
        path: &str,
        mat_index: &mut BTreeMap<String, json::Index<json::Material>>,
        files: Option<&dyn FileResolver>,
    ) -> json::Index<json::Material> {
        if let Some(i) = mat_index.get(path) {
            return *i;
        }
        let name = path.rsplit('/').next().unwrap_or(path).to_owned();
        let mut material = json::Material {
            name: Some(name),
            pbr_metallic_roughness: json::material::PbrMetallicRoughness {
                base_color_factor: json::material::PbrBaseColorFactor([1.0, 1.0, 1.0, 1.0]),
                metallic_factor: json::material::StrengthFactor(0.0),
                roughness_factor: json::material::StrengthFactor(1.0),
                base_color_texture: None,
                metallic_roughness_texture: None,
                extensions: None,
                extras: Default::default(),
            },
            ..Default::default()
        };

        let mut extensions = serde_json::Map::new();
        if let Some(files) = files {
            extensions = self.apply_textures(&mut material, path, files);
        }

        let index = self.root.push(material);
        if !extensions.is_empty() {
            self.material_extensions.insert(index.value(), extensions);
        }
        mat_index.insert(path.to_owned(), index);
        index
    }

    /// Resolves `path`'s `.vmat_c`, decodes its PBR-slot `.vtex_c` textures, and
    /// wires them onto `material`. Best-effort: any slot that fails to resolve
    /// or decode is simply left off (the material keeps its default factors).
    ///
    /// Returns the material's KHR extension objects (emissive strength, sheen,
    /// transmission/ior, unlit) for [`inject_material_extensions`]; gltf-json
    /// 1.x has no `KHR_materials_sheen` support at all, so every extension goes
    /// through the one serialized-JSON injection pass. Also attaches the
    /// `morphic` extras payload (NPR shader params + mask textures) the viewer
    /// reads back as `material.userData.morphic`.
    #[allow(clippy::too_many_lines)]
    fn apply_textures(
        &mut self,
        material: &mut json::Material,
        mat_path: &str,
        files: &dyn FileResolver,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut extensions = serde_json::Map::new();
        let Some(vmat) = files.resolve(&compiled(mat_path)) else {
            return extensions;
        };
        let Ok(mat) = crate::material::parse(&vmat) else {
            return extensions;
        };
        let pbr = mat.pbr();
        let alpha_mode = mat.alpha_mode();
        // Glass surfaces render through transmission + ior, not alpha blending
        // (same dispatch as the alpha-mode path: the flag or a *_glass shader).
        let is_glass = mat.int_params.get("F_GLASS").copied().unwrap_or(0) > 0
            || mat.shader_name.ends_with("_glass.vfx");
        // Source 2 albedo carries non-opacity data in its alpha channel (masks);
        // for non-blended materials that alpha must not become glTF transparency.
        // Glass counts as non-blended here: its final alphaMode is OPAQUE.
        let opaque = !matches!(alpha_mode, crate::material::AlphaMode::Blend) || is_glass;

        // g_vColorTint1 is a linear albedo multiplier the engine applies on top
        // of the base-color texture; glTF baseColorFactor is also linear, so it
        // maps 1:1. Without it, tinted materials (e.g. mcginnis_greengoo
        // [0.16, 0.25, 0.29]) render at full albedo. The alpha lane is kept.
        if let Some(t) = mat.vector_params.get("g_vColorTint1") {
            let a = material.pbr_metallic_roughness.base_color_factor.0[3];
            material.pbr_metallic_roughness.base_color_factor =
                json::material::PbrBaseColorFactor([t[0], t[1], t[2], a]);
        }

        // Base color (sRGB albedo).
        if let Some(p) = pbr.base_color {
            if let Some((w, h, mut rgba)) = decode_slot(files, p) {
                if opaque {
                    for px in rgba.chunks_exact_mut(4) {
                        px[3] = 255;
                    }
                }
                if let Some(png) = png_encode(w, h, &rgba) {
                    if let Some(tex) = self.texture_png(&png) {
                        material.pbr_metallic_roughness.base_color_texture = Some(tex_info(tex));
                    }
                }
            }
        }

        // Metalness mask (R channel) for the ORM blue channel. Decoded up
        // front so the metallic-roughness image below can pack it; like the
        // other non-albedo slots, a 4x4 `default_*` placeholder is skipped.
        // No size filter here (unlike the normal placeholder): Deadlock authors
        // constant metalness as a real 4x4 BC4 texture (e.g. shiv_glasses R=255),
        // so a 4x4 metalness is meaningful data, not a no-op placeholder.
        let metalness = pbr.metalness.and_then(|p| decode_slot(files, p));
        let mut metalness_wired = false;

        // Normal map (RGB) + roughness (its alpha) from the packed normal texture.
        // Skip the 4x4 default_normal placeholder (a flat normal is a no-op).
        if let Some(p) = pbr.normal {
            if let Some((w, h, rgba)) = decode_slot(files, p).filter(|&(w, h, _)| w.min(h) > 4) {
                if let Some(t) = self.texture_png(&normal_png(w, h, &rgba)) {
                    material.normal_texture = Some(json::material::NormalTexture {
                        index: t,
                        tex_coord: 0,
                        scale: 1.0,
                        extensions: None,
                        extras: Default::default(),
                    });
                }
                if let Some(t) = self.texture_png(&metal_rough_png(w, h, &rgba, metalness.as_ref()))
                {
                    material.pbr_metallic_roughness.metallic_roughness_texture = Some(tex_info(t));
                    material.pbr_metallic_roughness.roughness_factor =
                        json::material::StrengthFactor(1.0);
                    if metalness.is_some() {
                        // The texture multiplies the factor; a wired metalness
                        // mask needs metallicFactor 1.0, not the 0.0 default.
                        material.pbr_metallic_roughness.metallic_factor =
                            json::material::StrengthFactor(1.0);
                        metalness_wired = true;
                    }
                }
            }
        }
        // Constant roughness fallback: materials with no normal-roughness texture
        // keep the default factor 1.0 (fully matte) unless TextureRoughness1 sets
        // it. Gated on no MR texture so the textured path's factor-1.0 multiplier
        // is not clobbered. Verified: greenglass=0.188 (glossy) was stuck at 1.0.
        if material
            .pbr_metallic_roughness
            .metallic_roughness_texture
            .is_none()
        {
            if let Some(v) = mat.vector_params.get("TextureRoughness1") {
                material.pbr_metallic_roughness.roughness_factor =
                    json::material::StrengthFactor(v[0].clamp(0.0, 1.0));
            }
        }
        if !metalness_wired && metalness.is_none() {
            // No metalness mask: the unbound-sampler constant TextureMetalness1
            // (the vector param Deadlock actually sets) or the rare g_flMetalness
            // float still sets the factor. Cloth = [0,0,0,0] stays non-metal;
            // metal accessories = [1,1,1,0] recover their metalness.
            let m = mat
                .vector_params
                .get("TextureMetalness1")
                .map(|v| v[0])
                .or_else(|| mat.float_params.get("g_flMetalness").copied());
            if let Some(m) = m {
                material.pbr_metallic_roughness.metallic_factor =
                    json::material::StrengthFactor(m.clamp(0.0, 1.0));
            }
        }

        // Occlusion (R channel sampled by glTF).
        if let Some(tex) = pbr.occlusion.and_then(|p| self.texture_from(files, p)) {
            material.occlusion_texture = Some(json::material::OcclusionTexture {
                index: tex,
                tex_coord: 0,
                strength: json::material::StrengthFactor(1.0),
                extensions: None,
                extras: Default::default(),
            });
        }

        // Emissive (self-illum mask). g_flSelfIllumScale1 can run well past 1
        // (Chrono's clock face: 3.649); KHR_materials_emissive_strength carries
        // the overbright part, since emissiveFactor clamps at 1.
        if let Some(tex) = pbr.emissive.and_then(|p| self.texture_from(files, p)) {
            material.emissive_texture = Some(tex_info(tex));
            material.emissive_factor = json::material::EmissiveFactor([1.0, 1.0, 1.0]);
            let scale = mat
                .float_params
                .get("g_flSelfIllumScale1")
                .or_else(|| mat.float_params.get("g_flSelfIllumScale"))
                .copied();
            if let Some(s) = scale.filter(|&s| s > 1.0) {
                extensions.insert(
                    "KHR_materials_emissive_strength".to_owned(),
                    jval!({ "emissiveStrength": s }),
                );
            }
        }

        // F_SHEEN: the Charlie-sheen cloth lobe maps onto KHR_materials_sheen.
        // The `g_tSheen` texture packs sheen color in RGB and sheen roughness in
        // ALPHA (verified: body sheen 4x4 = RGB[144,197,225]/A58 == the vector
        // params TextureSheenColor1 [0.56,0.77,0.88] / TextureSheenRoughness1
        // 0.227; ghost2 sheen RGB is the flat color [0.37,0.4,0.38] with the
        // roughness varying per-texel in A, 0.21..0.82). The vector params are
        // the unbound-sampler constants for that texture's RGB / A.
        //
        // The sheen COLOR is `TextureSheenColor1 * g_vSheenColorTint1` (the base
        // sheen color times an artist tint). The old code read only
        // g_vSheenColorTint1, which is [1,1,1] (neutral) on ~22 of 26 sheen
        // materials, so it emitted WHITE sheen on nearly every cloth surface and
        // dropped the real tinted color living in TextureSheenColor1.
        if mat.int_params.get("F_SHEEN").copied().unwrap_or(0) > 0 {
            let base = mat
                .vector_params
                .get("TextureSheenColor1")
                .map_or([1.0f32; 3], |v| [v[0], v[1], v[2]]);
            let tint = mat
                .vector_params
                .get("g_vSheenColorTint1")
                .or_else(|| mat.vector_params.get("g_vSheenColorTint"))
                .map_or([1.0f32; 3], |v| [v[0], v[1], v[2]]);
            let color = [
                (base[0] * tint[0]).clamp(0.0, 1.0),
                (base[1] * tint[1]).clamp(0.0, 1.0),
                (base[2] * tint[2]).clamp(0.0, 1.0),
            ];
            let roughness = mat
                .vector_params
                .get("TextureSheenRoughness1")
                .map(|v| v[0])
                .or_else(|| mat.float_params.get("g_flSheenRoughness").copied())
                .map_or(0.5, |r| r.clamp(0.0, 1.0));

            let mut sheen = serde_json::Map::new();
            sheen.insert("sheenColorFactor".to_owned(), jval!(color));
            sheen.insert("sheenRoughnessFactor".to_owned(), jval!(roughness));

            // Bind the real per-texel sheen texture when one is present (a >4px
            // g_tSheen): RGB drives sheenColorTexture, ALPHA drives
            // sheenRoughnessTexture. Embedded twice (one sRGB color view, one
            // linear roughness view) since glTF wants distinct images/channels.
            if let Some(p) = mat.texture("g_tSheen") {
                if let Some((w, h, rgba)) = decode_slot(files, p).filter(|&(w, h, _)| w.min(h) > 4)
                {
                    if let Some(t) = self.embed_rgba_png(w, h, &rgba) {
                        sheen.insert(
                            "sheenColorTexture".to_owned(),
                            jval!({ "index": t.value() }),
                        );
                    }
                    if let Some(t) = self.texture_png(&sheen_roughness_png(w, h, &rgba)) {
                        sheen.insert(
                            "sheenRoughnessTexture".to_owned(),
                            jval!({ "index": t.value() }),
                        );
                        // The texture's alpha multiplies the factor; wire the
                        // factor to 1.0 so a bound roughness map is not scaled.
                        sheen.insert("sheenRoughnessFactor".to_owned(), jval!(1.0));
                    }
                    // Color texture multiplies the factor; keep the tint, drop
                    // the base color into the factor as 1.0*tint so it does not
                    // double-apply the base sheen color (already in RGB).
                    let factor = [
                        tint[0].clamp(0.0, 1.0),
                        tint[1].clamp(0.0, 1.0),
                        tint[2].clamp(0.0, 1.0),
                    ];
                    sheen.insert("sheenColorFactor".to_owned(), jval!(factor));
                }
            }
            extensions.insert(
                "KHR_materials_sheen".to_owned(),
                serde_json::Value::Object(sheen),
            );
        }

        // Glass: transmission + ior replace alpha blending entirely. Honor an
        // authored g_flIOR (e.g. necro_jar_glass) instead of assuming 1.5 for
        // window glass.
        if is_glass {
            let ior = mat
                .float_params
                .get("g_flIOR")
                .copied()
                .filter(|v| (1.0..=3.0).contains(v))
                .unwrap_or(1.5);
            extensions.insert(
                "KHR_materials_transmission".to_owned(),
                jval!({ "transmissionFactor": 0.9 }),
            );
            extensions.insert("KHR_materials_ior".to_owned(), jval!({ "ior": ior }));
        }

        // F_UNLIT: lighting ignored, albedo as-is.
        if mat.int_params.get("F_UNLIT").copied().unwrap_or(0) > 0 {
            extensions.insert("KHR_materials_unlit".to_owned(), jval!({}));
        }

        material.alpha_mode = Valid(if is_glass {
            json::material::AlphaMode::Opaque
        } else {
            match alpha_mode {
                crate::material::AlphaMode::Blend => json::material::AlphaMode::Blend,
                crate::material::AlphaMode::Mask => json::material::AlphaMode::Mask,
                crate::material::AlphaMode::Opaque => json::material::AlphaMode::Opaque,
            }
        });
        if let Some(c) = mat.alpha_cutoff() {
            material.alpha_cutoff = Some(json::material::AlphaCutoff(c));
        }

        material.extras = self.morphic_extras(&mat, files);
        extensions
    }

    /// Builds the `morphic` extras payload the Grimoire viewer reads as
    /// `material.userData.morphic`: the full shader name + int/float/vector
    /// param tables, plus the NPR mask textures embedded exactly like the PBR
    /// ones and referenced by glTF texture index. (glTF validators flag
    /// textures referenced only from extras as unused; that is intentional,
    /// matching VRF's exporter.) The masks are data textures (linear); the PNG
    /// embed path applies no color-space conversion.
    fn morphic_extras(
        &mut self,
        mat: &crate::material::Material,
        files: &dyn FileResolver,
    ) -> json::Extras {
        let mut textures = serde_json::Map::new();
        for slot in NPR_TEXTURE_SLOTS {
            if let Some(tex) = mat
                .texture(slot)
                .and_then(|p| self.texture_from_any(files, p))
            {
                textures.insert((*slot).to_owned(), jval!(tex.value()));
            }
        }
        let extras = jval!({
            "morphic": {
                "shader": mat.shader_name,
                "ints": mat.int_params,
                "floats": mat.float_params,
                "vectors": mat.vector_params,
                "textures": textures,
            }
        });
        serde_json::value::to_raw_value(&extras).ok()
    }

    /// Resolves + decodes a `.vtex` slot and embeds it verbatim as a glTF
    /// texture. Skips Source's 4x4 `default_*` placeholders (used by occlusion +
    /// emissive, where a placeholder is a no-op or, for the solid-white default
    /// self-illum mask, actively harmful).
    fn texture_from(
        &mut self,
        files: &dyn FileResolver,
        vtex_path: &str,
    ) -> Option<json::Index<json::Texture>> {
        let (w, h, rgba) = decode_slot(files, vtex_path)?;
        if w.min(h) <= 4 {
            return None;
        }
        self.embed_rgba_png(w, h, &rgba)
    }

    /// Like `texture_from`, but keeps placeholder-sized textures. Used for the
    /// NPR mask slots, where every shipped skin binds a flat 4x4: there the
    /// texel values are the actual per-material constants (e.g. the rim light
    /// strength in G), so small means data, not a no-op placeholder.
    fn texture_from_any(
        &mut self,
        files: &dyn FileResolver,
        vtex_path: &str,
    ) -> Option<json::Index<json::Texture>> {
        let (w, h, rgba) = decode_slot(files, vtex_path)?;
        self.embed_rgba_png(w, h, &rgba)
    }

    fn embed_rgba_png(
        &mut self,
        w: u32,
        h: u32,
        rgba: &[u8],
    ) -> Option<json::Index<json::Texture>> {
        let png = png_encode(w, h, rgba)?;
        self.texture_png(&png)
    }

    /// Embeds PNG bytes as an image + texture (sharing one sampler), returning
    /// the texture index.
    fn texture_png(&mut self, png: &[u8]) -> Option<json::Index<json::Texture>> {
        if png.is_empty() {
            return None;
        }
        let view = self.add_image_view(png);
        let image = self.root.push(json::Image {
            buffer_view: Some(view),
            mime_type: Some(json::image::MimeType("image/png".to_owned())),
            uri: None,
            name: None,
            extensions: None,
            extras: Default::default(),
        });
        let sampler = self.ensure_sampler();
        Some(self.root.push(json::Texture {
            sampler: Some(sampler),
            source: image,
            name: None,
            extensions: None,
            extras: Default::default(),
        }))
    }

    /// A buffer view with no target, for embedded image data.
    fn add_image_view(&mut self, bytes: &[u8]) -> json::Index<json::buffer::View> {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        self.root.push(json::buffer::View {
            buffer: json::Index::new(0),
            byte_length: USize64(bytes.len() as u64),
            byte_offset: Some(USize64(offset as u64)),
            byte_stride: None,
            target: None,
            name: None,
            extensions: None,
            extras: Default::default(),
        })
    }

    fn ensure_sampler(&mut self) -> json::Index<json::texture::Sampler> {
        if let Some(s) = self.sampler {
            return s;
        }
        let s = self.root.push(json::texture::Sampler::default());
        self.sampler = Some(s);
        s
    }

    /// Frames the document + binary buffer into a GLB byte stream.
    fn finish(mut self) -> Result<Vec<u8>, DecodeError> {
        self.root.push(json::Buffer {
            byte_length: USize64(self.bin.len() as u64),
            uri: None,
            name: None,
            extensions: None,
            extras: Default::default(),
        });

        let mut json_string = json::serialize::to_string(&self.root)
            .map_err(|_| DecodeError::Model("glTF JSON serialize failed"))?;
        if !self.material_extensions.is_empty() {
            json_string = inject_material_extensions(&json_string, &self.material_extensions)?;
        }
        let mut json_bytes = json_string.into_bytes();
        while !json_bytes.len().is_multiple_of(4) {
            json_bytes.push(b' ');
        }
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }

        let total = 12 + 8 + json_bytes.len() + 8 + self.bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());

        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_bytes);

        out.extend_from_slice(&(self.bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&self.bin);

        Ok(out)
    }
}

/// True for Deadlock's inverted-hull toon-outline shells. Their primitives are
/// dropped from the GLB: rendered as solid geometry the hull wraps and
/// whitewashes the model. Two naming conventions in the shipped heroes:
/// - `*_outline`: the standard inverted hull.
/// - `*jitter*`: the comic-style inked border (Billy/`punkgoat`, parts
///   `*_jitter01`/`_bat_jitter`, materials `*_border_jitter01`). NOT the
///   `g_tJitterMask` texture input many normal materials use for animated edges:
///   this only ever inspects material + mesh-part *names*, never texture-param
///   names, and no visible hero-body material is named `*jitter*` (verified
///   across the roster), so matching the name token is safe.
pub(crate) fn is_outline_material(path: &str) -> bool {
    let lc = path.to_ascii_lowercase();
    lc.contains("outline") || lc.contains("jitter")
}

/// True for Deadlock's additive glow-effect shells (mesh part `ghost_glow`,
/// material `*_glow.vmat`). In-game an additive NPR shader draws them; as plain
/// glTF geometry they collapse to an opaque shell ("white halo") over the model,
/// so they are dropped. Excludes `*noglow*` (a normal material with glow turned
/// off, e.g. `astro_barrelv2_noglow`), which must be kept.
pub(crate) fn is_glow_material(path: &str) -> bool {
    let lc = path.to_ascii_lowercase();
    lc.contains("glow") && !lc.contains("noglow")
}

/// True for any mesh part or material that exports as a non-renderable shell:
/// the toon outline (`is_outline_material`) or the additive glow
/// (`is_glow_material`). Such geometry is dropped from the GLB.
pub(crate) fn is_shell(name: &str) -> bool {
    is_outline_material(name) || is_glow_material(name)
}

/// True for a hero "alt-form" mesh part that is hidden in the default
/// menu/idle pose and only revealed while an ability is active. Unlike a shell
/// this is real, fully-shaded geometry; the game hides it (a zeroed bone scale /
/// visibility flag driven outside the locomotion clips we sample), so a static
/// posed export keeps it at full bind size and it swallows the body.
///
/// Currently just Viscous's Goo Ball: mesh part `inflated` (material
/// `viscous_ball`), a ~1.4x-body sphere present in EVERY clip we tried
/// (`ui_hero_select`, `primary_stand_idle`, ...), so no pose clip collapses it.
/// Matched on the part name so all of its primitives (incl. the shared
/// `black`/`viscous_swatches` ones) drop together; the material token is a
/// belt-and-braces match should a skin rename the part.
pub(crate) fn is_alt_form(name: &str) -> bool {
    let lc = name.to_ascii_lowercase();
    lc == "inflated" || lc.contains("viscous_ball")
}

/// True for any mesh part or material omitted from a static hero-card GLB: a
/// non-renderable NPR shell ([`is_shell`]) or a hidden-by-default alt-form
/// ([`is_alt_form`]).
pub(crate) fn is_dropped(name: &str) -> bool {
    is_shell(name) || is_alt_form(name)
}

/// Appends `_c` to a source resource path unless it is already a compiled path.
fn compiled(path: &str) -> String {
    if path.ends_with("_c") {
        path.to_owned()
    } else {
        format!("{path}_c")
    }
}

fn tex_info(index: json::Index<json::Texture>) -> json::texture::Info {
    json::texture::Info {
        index,
        tex_coord: 0,
        extensions: None,
        extras: Default::default(),
    }
}

/// Whether a primitive should carry `COLOR_n` attributes into glTF. When a
/// resolver is available, follow the Source material flags so vertex colors do
/// not tint materials that ignore them in-game. Without material bytes, keep the
/// geometry data rather than silently dropping it.
fn material_uses_vertex_color(path: &str, files: Option<&dyn FileResolver>) -> bool {
    let Some(files) = files else {
        return true;
    };
    let Some(vmat) = files.resolve(&compiled(path)) else {
        return true;
    };
    crate::material::parse(&vmat).map_or(true, |m| m.uses_vertex_color())
}

/// Resolves a `.vtex` slot (+ `_c`), decodes its top mip, and returns
/// `(width, height, RGBA8)`. Skips HDR textures (no PBR slot we read is HDR).
/// Tiny `4x4` placeholders are kept here (a flat base color is a real albedo,
/// e.g. Deadlock body skin); callers that must reject placeholders (occlusion,
/// emissive, normal) filter by size themselves.
fn decode_slot(files: &dyn FileResolver, vtex_path: &str) -> Option<(u32, u32, Vec<u8>)> {
    let bytes = files.resolve(&compiled(vtex_path))?;
    let img = crate::decode(&bytes).ok()?;
    match img.data {
        crate::ImageData::Rgba8(d) => Some((img.width, img.height, d)),
        crate::ImageData::Rgba16F(_) => None,
    }
}

fn png_encode(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let img = image::RgbaImage::from_raw(w, h, rgba.to_vec())?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Normal map: the packed normal texture's R,G are the tangent-space normal X,Y.
/// Source 2 stores roughness in the BLUE channel (not normal Z), so reconstruct
/// Z from X,Y to keep a valid normal map. The old code passed blue (roughness)
/// straight into the normal's Z, skewing every surface normal.
#[allow(clippy::cast_sign_loss)]
fn normal_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        let nx = (f32::from(px[0]) / 255.0) * 2.0 - 1.0;
        let ny = (f32::from(px[1]) / 255.0) * 2.0 - 1.0;
        let nz = (1.0 - (nx * nx + ny * ny)).max(0.0).sqrt();
        let bz = ((nz * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
        out.extend_from_slice(&[px[0], px[1], bz, 255]);
    }
    png_encode(w, h, &out).unwrap_or_default()
}

/// glTF metallic-roughness texture: G = roughness (from the normal texture's
/// BLUE channel; Source 2 `g_tNormalRoughness` packs roughness in B while the
/// alpha is a constant ~1.0 placeholder, so reading alpha yielded fully-rough
/// matte surfaces), B = metalness (the metalness mask's R channel,
/// nearest-neighbor resampled to the roughness image's dimensions; 0 without a
/// mask).
pub(super) fn metal_rough_png(
    w: u32,
    h: u32,
    rgba: &[u8],
    metalness: Option<&(u32, u32, Vec<u8>)>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for (i, px) in rgba.chunks_exact(4).enumerate() {
        let metal = metalness.map_or(0, |&(mw, mh, ref m)| {
            let x = i as u64 % u64::from(w);
            let y = i as u64 / u64::from(w);
            let mx = x * u64::from(mw) / u64::from(w);
            let my = y * u64::from(mh) / u64::from(h);
            m[((my * u64::from(mw) + mx) * 4) as usize]
        });
        out.extend_from_slice(&[0, px[2], metal, 255]);
    }
    png_encode(w, h, &out).unwrap_or_default()
}

/// glTF `sheenRoughnessTexture`: glTF reads sheen roughness from the ALPHA
/// channel. Source 2's `g_tSheen` packs sheen color in RGB and sheen roughness
/// in alpha, so copy alpha into the output alpha and leave RGB at 255 (the
/// glTF reader ignores them for this slot).
fn sheen_roughness_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[255, 255, 255, px[3]]);
    }
    png_encode(w, h, &out).unwrap_or_default()
}

/// NPR mask texture slots embedded for the viewer and referenced from the
/// `morphic` extras payload (R = tint enable, G = rim light constant for the
/// first; outline + transmissive color for the others).
const NPR_TEXTURE_SLOTS: &[&str] = &[
    "g_tTintMaskRimLightMask",
    "g_tNprOutlineMask",
    "g_tNprTransmissiveColor",
];

/// Injects per-material KHR extension objects into the serialized glTF JSON
/// and lists their names in the root `extensionsUsed` array (required for
/// validity). One post-pass covers every extension this writer emits: gltf-json
/// 1.x exposes some of them behind cargo features but has no
/// `KHR_materials_sheen` at all, so a single `serde_json` mechanism beats mixing
/// two. `per_material` is keyed by material index.
pub(super) fn inject_material_extensions(
    json_str: &str,
    per_material: &BTreeMap<usize, serde_json::Map<String, serde_json::Value>>,
) -> Result<String, DecodeError> {
    let mut doc: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|_| DecodeError::Model("glTF JSON reparse failed"))?;

    let used: BTreeSet<&String> = per_material
        .values()
        .flat_map(serde_json::Map::keys)
        .collect();

    {
        let materials = doc
            .get_mut("materials")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or(DecodeError::Model("glTF materials array missing"))?;
        for (&i, ext) in per_material {
            let slot = materials
                .get_mut(i)
                .and_then(serde_json::Value::as_object_mut)
                .ok_or(DecodeError::Model("glTF material index out of range"))?;
            let entry = slot
                .entry("extensions")
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            let obj = entry
                .as_object_mut()
                .ok_or(DecodeError::Model("glTF material extensions not an object"))?;
            for (name, value) in ext {
                obj.insert(name.clone(), value.clone());
            }
        }
    }

    let root = doc
        .as_object_mut()
        .ok_or(DecodeError::Model("glTF root not an object"))?;
    let list = root
        .entry("extensionsUsed")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    let arr = list
        .as_array_mut()
        .ok_or(DecodeError::Model("glTF extensionsUsed not an array"))?;
    for name in used {
        if !arr.iter().any(|v| v.as_str() == Some(name)) {
            arr.push(jval!(name));
        }
    }

    serde_json::to_string(&doc).map_err(|_| DecodeError::Model("glTF JSON re-serialize failed"))
}

/// The accessors written for one vertex buffer, shared across its primitives.
struct VertexAccessors {
    position: Option<json::Index<json::Accessor>>,
    normal: Option<json::Index<json::Accessor>>,
    tangent: Option<json::Index<json::Accessor>>,
    texcoords: Vec<json::Index<json::Accessor>>,
    colors: Vec<json::Index<json::Accessor>>,
    joints0: Option<json::Index<json::Accessor>>,
    weights0: Option<json::Index<json::Accessor>>,
}

/// Pushes a sampler + channel pair into one animation's local arrays. The
/// sampler index is local to the animation (channels reference samplers within
/// the same `json::Animation`, not globally).
fn push_channel(
    samplers: &mut Vec<json::animation::Sampler>,
    channels: &mut Vec<json::animation::Channel>,
    input: json::Index<json::Accessor>,
    output: json::Index<json::Accessor>,
    node: json::Index<json::Node>,
    property: json::animation::Property,
) {
    let sampler = json::Index::new(samplers.len() as u32);
    samplers.push(json::animation::Sampler {
        input,
        output,
        interpolation: Valid(json::animation::Interpolation::Linear),
        extensions: None,
        extras: Default::default(),
    });
    channels.push(json::animation::Channel {
        sampler,
        target: json::animation::Target {
            node,
            path: Valid(property),
            extensions: None,
            extras: Default::default(),
        },
        extensions: None,
        extras: Default::default(),
    });
}

fn default_node() -> json::Node {
    json::Node {
        camera: None,
        children: None,
        extensions: None,
        extras: Default::default(),
        matrix: None,
        mesh: None,
        name: None,
        rotation: None,
        scale: None,
        translation: None,
        skin: None,
        weights: None,
    }
}

fn f32x<const N: usize>(v: &[f32; N]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn u16x(v: [u16; 4]) -> Vec<u8> {
    v.iter().flat_map(|u| u.to_le_bytes()).collect()
}

fn bounds(positions: &[[f32; 3]]) -> (json::Value, json::Value) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in positions {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    if !min[0].is_finite() {
        min = [0.0; 3];
        max = [0.0; 3];
    }
    (
        json::serialize::to_value(min.to_vec()).unwrap(),
        json::serialize::to_value(max.to_vec()).unwrap(),
    )
}

/// VRF's default weights for a mesh with joints but no weight stream:
/// `1/bone_weight_count` spread over the first `bone_weight_count` influences.
pub(crate) fn default_weights(count: usize, bone_weight_count: usize) -> Vec<[f32; 4]> {
    let bwc = bone_weight_count.clamp(1, 4);
    let w = 1.0 / bwc as f32;
    let mut row = [0.0f32; 4];
    for r in row.iter_mut().take(bwc) {
        *r = w;
    }
    vec![row; count]
}

/// Port of VRF `FixDuplicateJoints` (4-influence path): zero out influences with
/// zero weight, then merge duplicate joint indices, summing their weights.
fn fix_duplicate_joints(joints: &mut [[u16; 4]], weights: &mut [[f32; 4]]) {
    for (j, w) in joints.iter_mut().zip(weights.iter_mut()) {
        for k in 0..4 {
            if w[k] == 0.0 {
                j[k] = 0;
            }
        }
        // For each influence a, fold any later duplicate b into it (summing
        // weights) and shift the rest down. Walk both ends inward, as VRF does.
        for a in (0..=2).rev() {
            for b in (a + 1..=3).rev() {
                if j[a] == j[b] {
                    for l in b..3 {
                        j[l] = j[l + 1];
                        w[l] = w[l + 1];
                    }
                    j[3] = 0;
                    w[a] += w[b];
                    w[3] = 0.0;
                }
            }
        }
    }
}
