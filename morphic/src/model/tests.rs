//! M3 validation. These tests run in CI off the committed KV3 fixtures, so they
//! cover the parts of model decode that do not need the multi-megabyte vertex
//! buffers: the skeleton (from `DATA`), the LOD0 embedded-mesh registry +
//! vertex layouts (from `CTRL`), and the body mesh's draw calls + scene bounds
//! (from `MDAT[0]`). Everything is diffed against the oracle golden
//! `hornet_model_meta.json` (produced by `morphic-oracle model-meta`, which
//! wraps `ValveResourceFormat`). The full buffer-decode path (positions, joints,
//! the vertex/index totals, the position bbox) is exercised by the gated
//! `tests/model_local.rs` against a real VPK, diffed against the same golden.

use std::path::PathBuf;

use serde::Deserialize;

use super::{mesh, skeleton, topology};
use crate::kv3::{self, Value};

#[derive(Deserialize)]
struct Golden {
    bone_count: usize,
    bone_names: Vec<String>,
    meshes: Vec<GMesh>,
}

#[derive(Deserialize)]
struct GMesh {
    name: String,
    mesh_index: usize,
    scene_min: [f32; 3],
    scene_max: [f32; 3],
    vertex_buffers: Vec<GVb>,
    index_buffers: Vec<GIb>,
    primitives: Vec<GPrim>,
}

#[derive(Deserialize)]
struct GVb {
    element_count: usize,
    element_size: usize,
    fields: Vec<GField>,
}

#[derive(Deserialize)]
struct GField {
    semantic: String,
    semantic_index: i32,
    format: u32,
    offset: usize,
}

#[derive(Deserialize)]
struct GIb {
    element_count: usize,
    element_size: usize,
}

#[derive(Deserialize)]
struct GPrim {
    vertex_buffer: usize,
    vertex_count: usize,
    index_count: usize,
    material: String,
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/kv3")
}

fn parse_fixture(name: &str) -> Value {
    let bytes = std::fs::read(fixtures().join(name)).expect("read kv3 fixture");
    kv3::decode(&bytes).expect("parse kv3 fixture")
}

fn load_golden() -> Golden {
    let text = std::fs::read_to_string(fixtures().join("hornet_model_meta.json"))
        .expect("read model-meta golden");
    serde_json::from_str(&text).expect("parse model-meta golden")
}

fn approx3(a: [f32; 3], b: [f32; 3], what: &str) {
    for i in 0..3 {
        assert!(
            (a[i] - b[i]).abs() < 1e-3,
            "{what}[{i}]: {} vs golden {}",
            a[i],
            b[i]
        );
    }
}

#[test]
fn skeleton_matches_golden() {
    let data = parse_fixture("hornet_data.kv3bin");
    let skel = skeleton::Skeleton::from_model_data(&data).expect("build skeleton");
    let g = load_golden();

    assert_eq!(skel.bones.len(), g.bone_count, "bone count");
    assert_eq!(skel.sorted_bone_names(), g.bone_names, "bone-name set");

    // Hierarchy is well formed: exactly one root, every parent in range and
    // earlier than its child (so global bind poses resolve in one pass).
    let roots = skel.bones.iter().filter(|b| b.parent.is_none()).count();
    assert_eq!(roots, 1, "expected a single root bone");
    for (i, b) in skel.bones.iter().enumerate() {
        if let Some(p) = b.parent {
            assert!(p < i, "bone {i} parent {p} not earlier");
        }
        // Inverse-bind round-trips its global bind to identity (sanity on the
        // matrix math), within float tolerance.
        let prod = b.global_bind.mul(&b.inverse_bind);
        for r in 0..4 {
            for c in 0..4 {
                let expect = if r == c { 1.0 } else { 0.0 };
                assert!(
                    (prod.m[r * 4 + c] - expect).abs() < 1e-3,
                    "bone {i} bind*inverse not identity"
                );
            }
        }
    }
}

#[test]
fn embedded_meshes_and_layouts_match_golden() {
    let data = parse_fixture("hornet_data.kv3bin");
    let ctrl = parse_fixture("hornet_ctrl.kv3bin");
    let embedded = mesh::EmbeddedMesh::parse_all(&ctrl).expect("parse embedded meshes");
    let lod0 = mesh::lod0_indices(&data, &embedded).expect("lod0 filter");
    let g = load_golden();

    assert_eq!(lod0.len(), g.meshes.len(), "LOD0 mesh count");

    for (gm, &idx) in g.meshes.iter().zip(&lod0) {
        let em = &embedded[idx];
        assert_eq!(em.name, gm.name, "mesh name");
        assert_eq!(em.mesh_index, gm.mesh_index, "mesh index");

        assert_eq!(
            em.vertex_buffers.len(),
            gm.vertex_buffers.len(),
            "{}: vertex buffer count",
            gm.name
        );
        for (vb, gvb) in em.vertex_buffers.iter().zip(&gm.vertex_buffers) {
            assert_eq!(vb.element_count, gvb.element_count, "{}: vb count", gm.name);
            assert_eq!(vb.element_size, gvb.element_size, "{}: vb stride", gm.name);
            assert_eq!(
                vb.fields.len(),
                gvb.fields.len(),
                "{}: field count",
                gm.name
            );
            for (f, gf) in vb.fields.iter().zip(&gvb.fields) {
                assert_eq!(f.semantic_name, gf.semantic, "{}: field semantic", gm.name);
                assert_eq!(
                    f.semantic_index, gf.semantic_index,
                    "{}: sem index",
                    gm.name
                );
                assert_eq!(f.format as u32, gf.format, "{}: field format", gm.name);
                assert_eq!(f.offset, gf.offset, "{}: field offset", gm.name);
            }
        }

        assert_eq!(
            em.index_buffers.len(),
            gm.index_buffers.len(),
            "{}: index buffer count",
            gm.name
        );
        for (ib, gib) in em.index_buffers.iter().zip(&gm.index_buffers) {
            assert_eq!(ib.element_count, gib.element_count, "{}: ib count", gm.name);
            assert_eq!(ib.element_size, gib.element_size, "{}: ib width", gm.name);
        }
    }
}

#[test]
fn body_draw_calls_match_golden() {
    // The committed MDAT[0] is the body mesh (golden mesh index 0).
    let mdat = parse_fixture("hornet_mdat0.kv3bin");
    let scene = mesh::SceneObject::parse_all(&mdat).expect("parse scene objects");
    let body = &load_golden().meshes[0];
    assert_eq!(body.name, "body", "golden mesh 0 is body");

    let mut prims = Vec::new();
    let mut smin = [f32::INFINITY; 3];
    let mut smax = [f32::NEG_INFINITY; 3];
    for so in &scene {
        for i in 0..3 {
            smin[i] = smin[i].min(so.min_bounds[i]);
            smax[i] = smax[i].max(so.max_bounds[i]);
        }
        prims.extend(so.draw_calls.iter());
    }

    assert_eq!(prims.len(), body.primitives.len(), "body primitive count");
    for (dc, gp) in prims.iter().zip(&body.primitives) {
        assert_eq!(
            dc.vertex_buffer, gp.vertex_buffer,
            "draw call vertex buffer"
        );
        assert_eq!(dc.vertex_count, gp.vertex_count, "draw call vertex count");
        assert_eq!(dc.index_count, gp.index_count, "draw call index count");
        assert_eq!(dc.material, gp.material, "draw call material");
        assert_eq!(dc.primitive_type, "RENDER_PRIM_TRIANGLES");
    }

    approx3(smin, body.scene_min, "body scene min");
    approx3(smax, body.scene_max, "body scene max");
}

/// Re-encoding the body `MDAT` as uncompressed KV3 v4 and decoding it again must
/// reproduce the exact same tree. This is the offline crux for Tier 1a: it proves
/// the KV3 writer faithfully round-trips a *model* metadata block (not just the
/// soundevents `tests/kv3.rs` already covers), which is what lets a re-encoded
/// `MDAT` splice back in. Source 2 reads the uncompressed buffer; the in-game
/// confirm (a removed draw call) is tracked in `docs/handoff-model-edit.md`.
#[test]
fn mdat_reencodes_round_trip() {
    let bytes = std::fs::read(fixtures().join("hornet_mdat0.kv3bin")).expect("read mdat fixture");
    let format = kv3::Format::from_payload(&bytes).expect("read kv3 format guid");
    let tree = kv3::decode(&bytes).expect("decode mdat");

    let reencoded = kv3::encode(&tree, &format);
    let back = kv3::decode(&reencoded).expect("decode re-encoded mdat");

    assert_eq!(tree, back, "MDAT tree changed across encode/decode");
}

/// The faithful uncompressed re-wrap decodes to the exact same tree as the
/// original compressed block, and is genuinely uncompressed (larger, compression
/// method 0). Unlike the lossy `kv3::encode` round-trip, this preserves the
/// original type stream verbatim (value flags + typed-array tags), which the
/// engine's model loader needs (see `docs/handoff-model-edit.md` T1a).
#[test]
fn mdat_rewrap_uncompressed_is_value_faithful() {
    let bytes = std::fs::read(fixtures().join("hornet_mdat0.kv3bin")).expect("read mdat fixture");
    let original = kv3::decode(&bytes).expect("decode original");

    let rewrapped = kv3::rewrap_uncompressed(&bytes).expect("rewrap");
    assert!(
        rewrapped.len() > bytes.len(),
        "uncompressed re-wrap should be larger than the LZ4 original"
    );
    // compressionMethod field (offset 20) must be 0.
    assert_eq!(
        u32::from_le_bytes([rewrapped[20], rewrapped[21], rewrapped[22], rewrapped[23]]),
        0,
        "re-wrap must be uncompressed"
    );

    let back = kv3::decode(&rewrapped).expect("decode re-wrapped");
    assert_eq!(original, back, "re-wrap changed the decoded tree");
}

/// `find_matching_draw_calls` locates the dress draw call in the body `MDAT` at
/// its `(scene_object, draw_call)` indices without mutating anything.
#[test]
fn find_dress_draw_call_locates_it() {
    let bytes = std::fs::read(fixtures().join("hornet_mdat0.kv3bin")).expect("read mdat fixture");
    let tree = kv3::decode(&bytes).expect("decode mdat");

    let matches = |m: &str| m.to_ascii_lowercase().contains("vindicta_dress");
    let found = topology::find_matching_draw_calls(&tree, &matches);

    assert_eq!(found.len(), 1, "exactly one dress draw call");
    assert_eq!(
        (found[0].scene_object, found[0].draw_call),
        (0, 2),
        "dress is scene object 0, draw call 2"
    );
    assert!(found[0].material.contains("vindicta_dress"));
    assert!(found[0].index_count > 0, "dress carries indices");
}

/// Neutralizing the dress draw call zeros exactly its `m_nIndexCount` and leaves
/// every other byte of the block's decoded tree identical. This is the in-place,
/// byte-faithful edit (no lossy re-encode) that the engine's model loader accepts:
/// the draw call survives but submits zero primitives, so the dress stops drawing.
#[test]
fn neutralizing_dress_zeros_only_its_index_count() {
    let bytes = std::fs::read(fixtures().join("hornet_mdat0.kv3bin")).expect("read mdat fixture");
    let original = kv3::decode(&bytes).expect("decode original");

    // Dress is scene object 0, draw call 2 (see find_dress_draw_call_locates_it).
    let patched = kv3::neutralize_draw_calls(&bytes, &[(0, 2)]).expect("neutralize");
    let edited = kv3::decode(&patched).expect("decode patched");

    // Expected tree: the original with scene object 0 / draw call 2's
    // m_nIndexCount set to 0, and nothing else changed.
    let mut expected = original.clone();
    let dress = expected
        .get_mut("m_sceneObjects")
        .and_then(|v| match v {
            Value::Array(a) => a.get_mut(0),
            _ => None,
        })
        .and_then(|so| so.get_mut("m_drawCalls"))
        .and_then(|v| match v {
            Value::Array(a) => a.get_mut(2),
            _ => None,
        })
        .expect("locate dress draw call");
    let idx = dress.get_mut("m_nIndexCount").expect("m_nIndexCount field");
    assert!(
        matches!(idx, Value::Int(n) if *n > 0),
        "dress index count should start positive, got {idx:?}"
    );
    *idx = Value::Int(0);

    assert_eq!(
        edited, expected,
        "only the dress m_nIndexCount changed to 0"
    );
}

/// The T1d-a scalar-set primitive locates a field by KV3 path and sets it. Setting
/// the dress draw call's `m_nIndexCount` to 0 must produce byte-identical output to
/// `neutralize_draw_calls` (cross-checking the path walker against the proven one),
/// and setting it to a new value must change only that field in the decoded tree.
#[test]
fn set_scalars_edits_field_by_path() {
    let bytes = std::fs::read(fixtures().join("hornet_mdat0.kv3bin")).expect("read mdat fixture");
    let original = kv3::decode(&bytes).expect("decode original");

    // Dress is scene object 0, draw call 2 (see find_dress_draw_call_locates_it).
    let path = vec![
        kv3::Seg::Key("m_sceneObjects".into()),
        kv3::Seg::Index(0),
        kv3::Seg::Key("m_drawCalls".into()),
        kv3::Seg::Index(2),
        kv3::Seg::Key("m_nIndexCount".into()),
    ];

    // Setting to 0 by path == zeroing via neutralize_draw_calls, byte for byte.
    let via_set = kv3::set_scalars(&bytes, &[(path.clone(), 0)]).expect("set 0");
    let via_neutralize = kv3::neutralize_draw_calls(&bytes, &[(0, 2)]).expect("neutralize");
    assert_eq!(
        via_set, via_neutralize,
        "set-to-0 by path should byte-match neutralize_draw_calls"
    );

    // Setting to a new positive value changes only that field in the tree.
    let patched = kv3::set_scalars(&bytes, &[(path, 12_345)]).expect("set value");
    let edited = kv3::decode(&patched).expect("decode patched");
    let mut expected = original;
    let dress = expected
        .get_mut("m_sceneObjects")
        .and_then(|v| match v {
            Value::Array(a) => a.get_mut(0),
            _ => None,
        })
        .and_then(|so| so.get_mut("m_drawCalls"))
        .and_then(|v| match v {
            Value::Array(a) => a.get_mut(2),
            _ => None,
        })
        .expect("locate dress draw call");
    *dress.get_mut("m_nIndexCount").expect("m_nIndexCount") = Value::Int(12_345);
    assert_eq!(edited, expected, "only the targeted m_nIndexCount changed");
}

/// A path that does not resolve to an integer scalar is rejected, not silently
/// ignored.
#[test]
fn set_scalars_rejects_missing_path() {
    let bytes = std::fs::read(fixtures().join("hornet_mdat0.kv3bin")).expect("read mdat fixture");
    let bogus = vec![kv3::Seg::Key("m_doesNotExist".into())];
    assert!(kv3::set_scalars(&bytes, &[(bogus, 1)]).is_err());
}

/// T1d-d's CTRL edits, proven on the committed `CTRL` block (which `set_scalars`
/// has not been exercised on before, only `MDAT`): set the gun's vertex/index element
/// counts and a layout field's format+offset by path, and confirm exactly those
/// scalars change in the decoded tree, everything else byte-faithful. This is the
/// buffer-registry half of replace-in-place, without needing the full pak.
#[test]
fn set_scalars_edits_ctrl_buffer_registry() {
    let bytes = std::fs::read(fixtures().join("hornet_ctrl.kv3bin")).expect("read ctrl fixture");
    let original = kv3::decode(&bytes).expect("decode ctrl");

    // The gun is embedded_meshes[1] with one vertex buffer and one index buffer.
    let vb = |k: &str| {
        vec![
            kv3::Seg::Key("embedded_meshes".into()),
            kv3::Seg::Index(1),
            kv3::Seg::Key("m_vertexBuffers".into()),
            kv3::Seg::Index(0),
            kv3::Seg::Key(k.into()),
        ]
    };
    let ib = |k: &str| {
        vec![
            kv3::Seg::Key("embedded_meshes".into()),
            kv3::Seg::Index(1),
            kv3::Seg::Key("m_indexBuffers".into()),
            kv3::Seg::Index(0),
            kv3::Seg::Key(k.into()),
        ]
    };
    let field = |i: usize, k: &str| {
        vec![
            kv3::Seg::Key("embedded_meshes".into()),
            kv3::Seg::Index(1),
            kv3::Seg::Key("m_vertexBuffers".into()),
            kv3::Seg::Index(0),
            kv3::Seg::Key("m_inputLayoutFields".into()),
            kv3::Seg::Index(i),
            kv3::Seg::Key(k.into()),
        ]
    };

    let edits = vec![
        (vb("m_nElementCount"), 9999),
        (vb("m_nElementSizeInBytes"), 60),
        (ib("m_nElementCount"), 8888),
        // field 0 (POSITION) m_Format is a small INT32_AS_BYTE; field 1 (TEXCOORD)
        // m_nOffset is 12. Both are byte-stored scalars, so settable. (POSITION's
        // own m_nOffset is the tagless 0 constant and is deliberately not touched.)
        (field(0, "m_Format"), 2),
        (field(1, "m_nOffset"), 16),
    ];
    let patched = kv3::set_scalars(&bytes, &edits).expect("set ctrl scalars");
    let edited = kv3::decode(&patched).expect("decode patched ctrl");

    // The gun's buffers reflect the edits (read variant-agnostically: CTRL counts
    // may decode as UInt where MDAT counts decode as Int).
    let gun = &edited
        .get("embedded_meshes")
        .and_then(Value::as_array)
        .unwrap()[1];
    let v0 = &gun
        .get("m_vertexBuffers")
        .and_then(Value::as_array)
        .unwrap()[0];
    let i0 = &gun.get("m_indexBuffers").and_then(Value::as_array).unwrap()[0];
    let fields = v0
        .get("m_inputLayoutFields")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(
        v0.get("m_nElementCount").and_then(Value::as_int),
        Some(9999)
    );
    assert_eq!(
        v0.get("m_nElementSizeInBytes").and_then(Value::as_int),
        Some(60)
    );
    assert_eq!(
        i0.get("m_nElementCount").and_then(Value::as_int),
        Some(8888)
    );
    assert_eq!(fields[0].get("m_Format").and_then(Value::as_int), Some(2));
    assert_eq!(fields[1].get("m_nOffset").and_then(Value::as_int), Some(16));

    // POSITION's offset (the untouched tagless 0) is intact, and every mesh other
    // than the gun is byte-faithful through the rewrap.
    assert_eq!(fields[0].get("m_nOffset").and_then(Value::as_int), Some(0));
    let orig_meshes = original
        .get("embedded_meshes")
        .and_then(Value::as_array)
        .unwrap();
    let new_meshes = edited
        .get("embedded_meshes")
        .and_then(Value::as_array)
        .unwrap();
    for (i, (o, n)) in orig_meshes.iter().zip(new_meshes).enumerate() {
        if i != 1 {
            assert_eq!(o, n, "mesh {i} (not the gun) is unchanged");
        }
    }
}

#[test]
fn remap_table_partitions_by_mesh() {
    // Each mesh's blend-index remap slice is non-empty and maps into the model
    // skeleton, so deinterleaved joints stay in range.
    let data = parse_fixture("hornet_data.kv3bin");
    let skel = skeleton::Skeleton::from_model_data(&data).expect("skeleton");

    let body = skeleton::remap_table(&data, 0).expect("body remap table");
    assert!(!body.is_empty(), "body remap table empty");
    assert!(
        body.iter().all(|&b| b < skel.bones.len()),
        "remap maps outside skeleton"
    );
}

// --- GLB writer (M5a) ---

use super::math::{Mat4, Quat, Vec3};
use super::mesh::{MeshPart, Primitive, VertexBuffer};
use super::skeleton::Bone;
use super::{BoneTrack, Clip, Model};

/// A minimal skinned model: one root bone, one triangle bound to it. Lets the
/// GLB writer be exercised in CI without the multi-megabyte hornet buffers.
fn synthetic_model() -> Model {
    let bone = Bone {
        name: "root".to_owned(),
        parent: None,
        flags: 0,
        position: Vec3::default(),
        rotation: Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        },
        local_bind: Mat4::IDENTITY,
        global_bind: Mat4::IDENTITY,
        inverse_bind: Mat4::IDENTITY,
    };
    let vb = VertexBuffer {
        element_count: 3,
        stride: 0,
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        tangents: Vec::new(),
        texcoords: vec![
            vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            // A second UV set (detail/AO pass) exercises TEXCOORD_1 emission.
            vec![[0.5, 0.5], [1.0, 0.5], [0.5, 1.0]],
        ],
        colors: vec![vec![
            [0.8, 0.3, 0.2, 1.0],
            [0.7, 0.25, 0.2, 1.0],
            [0.9, 0.35, 0.25, 1.0],
        ]],
        joints: vec![[0, 0, 0, 0]; 3],
        weights: vec![[1.0, 0.0, 0.0, 0.0]; 3],
        layout: Vec::new(),
    };
    let part = MeshPart {
        name: "tri".to_owned(),
        mesh_index: 0,
        vertex_buffers: vec![vb],
        primitives: vec![Primitive {
            vertex_buffer: 0,
            vertex_buffers: vec![0],
            material: "test/mat.vmat".to_owned(),
            vertex_count: 3,
            indices: vec![0, 1, 2],
        }],
        min_bounds: [0.0; 3],
        max_bounds: [1.0, 1.0, 0.0],
        bone_weight_count: 1,
    };
    Model {
        skeleton: skeleton::Skeleton { bones: vec![bone] },
        meshes: vec![part],
        animations: Vec::new(),
        cloth: None,
    }
}

#[test]
fn glb_writes_and_reloads() {
    let glb = super::to_glb(&synthetic_model()).expect("write glb");

    // Valid container: "glTF" magic, version 2, length matches.
    assert_eq!(&glb[0..4], b"glTF", "GLB magic");
    assert_eq!(
        u32::from_le_bytes(glb[4..8].try_into().unwrap()),
        2,
        "GLB version"
    );
    assert_eq!(
        u32::from_le_bytes(glb[8..12].try_into().unwrap()) as usize,
        glb.len(),
        "GLB declared length"
    );

    // The `gltf` reader parses + validates it.
    let g = gltf::Gltf::from_slice(&glb).expect("re-read glb");
    let doc = &g.document;

    assert_eq!(doc.meshes().count(), 1);
    let prim_count: usize = doc.meshes().map(|m| m.primitives().count()).sum();
    assert_eq!(prim_count, 1);

    let skin = doc.skins().next().expect("has skin");
    assert_eq!(skin.joints().count(), 1, "one joint");
    assert_eq!(
        skin.joints().next().unwrap().name(),
        Some("root"),
        "bone name preserved"
    );

    let prim = doc.meshes().next().unwrap().primitives().next().unwrap();
    assert!(prim.get(&gltf::Semantic::Positions).is_some(), "POSITION");
    assert!(prim.get(&gltf::Semantic::Colors(0)).is_some(), "COLOR_0");
    assert!(prim.get(&gltf::Semantic::Joints(0)).is_some(), "JOINTS_0");
    assert!(prim.get(&gltf::Semantic::Weights(0)).is_some(), "WEIGHTS_0");
    assert!(
        prim.get(&gltf::Semantic::TexCoords(0)).is_some(),
        "TEXCOORD_0"
    );
    assert!(
        prim.get(&gltf::Semantic::TexCoords(1)).is_some(),
        "TEXCOORD_1 (second UV set)"
    );
    assert!(prim.indices().is_some(), "indices");
}

/// [`synthetic_model`] with a single 2-frame rotation clip on its one bone:
/// exercises the animation emit path (samplers/channels/accessors) in CI with no
/// VPK dependency.
fn synthetic_animated_model() -> Model {
    let mut m = synthetic_model();
    m.animations = vec![Clip {
        name: "spin".to_owned(),
        fps: 2.0,
        frame_count: 2,
        looping: true,
        tracks: vec![BoneTrack {
            bone: 0,
            translations: None,
            rotations: Some(vec![
                Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
                Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 1.0,
                    w: 0.0,
                },
            ]),
            scales: None,
        }],
    }];
    m
}

#[test]
fn glb_animation_writes_and_reloads() {
    let glb = super::to_glb(&synthetic_animated_model()).expect("write glb");
    let g = gltf::Gltf::from_slice(&glb).expect("re-read glb");
    let doc = &g.document;

    assert_eq!(doc.animations().count(), 1, "one clip");
    let anim = doc.animations().next().unwrap();
    assert_eq!(anim.name(), Some("spin"));
    assert_eq!(anim.channels().count(), 1, "one rotation channel");
    assert_eq!(anim.samplers().count(), 1, "one sampler");

    let chan = anim.channels().next().unwrap();
    assert!(
        matches!(
            chan.target().property(),
            gltf::animation::Property::Rotation
        ),
        "targets rotation"
    );
    assert_eq!(
        chan.target().node().name(),
        Some("root"),
        "targets the joint node"
    );

    let s = chan.sampler();
    assert!(
        matches!(s.interpolation(), gltf::animation::Interpolation::Linear),
        "linear interpolation"
    );
    assert_eq!(s.input().count(), 2, "two time samples");
    assert_eq!(s.output().count(), 2, "two rotation samples");
    assert_eq!(s.input().dimensions(), gltf::accessor::Dimensions::Scalar);
    assert!(s.input().min().is_some(), "input accessor has min");
    assert!(s.input().max().is_some(), "input accessor has max");
    assert_eq!(s.output().dimensions(), gltf::accessor::Dimensions::Vec4);
}

/// A [`super::FileResolver`] backed by committed fixtures: any `.vmat_c` request
/// returns the hornet head material, any `.vtex_c` returns a real BC7 texture.
struct FixtureResolver {
    vmat: Vec<u8>,
    vtex: Vec<u8>,
}

impl super::FileResolver for FixtureResolver {
    fn resolve(&self, compiled_path: &str) -> Option<Vec<u8>> {
        if compiled_path.ends_with(".vmat_c") {
            Some(self.vmat.clone())
        } else if compiled_path.ends_with(".vtex_c") {
            Some(self.vtex.clone())
        } else {
            None
        }
    }
}

#[test]
fn glb_textured_embeds_resolved_images() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let resolver = FixtureResolver {
        vmat: std::fs::read(root.join("fixtures/material/vindicta_headv2.vmat_c"))
            .expect("vmat fixture"),
        // 128x64 BC7, comfortably above the 4x4 placeholder cutoff.
        vtex: std::fs::read(root.join("fixtures/bc7/generic_sleep_icon.vtex_c"))
            .expect("vtex fixture"),
    };

    let glb = super::to_glb_textured(&synthetic_model(), &resolver).expect("textured glb");
    let g = gltf::Gltf::from_slice(&glb).expect("re-read glb");
    let doc = &g.document;

    assert!(doc.images().count() > 0, "embedded images present");
    assert!(doc.textures().count() > 0, "textures present");
    let mat = doc.materials().next().expect("a material");
    assert!(
        mat.pbr_metallic_roughness().base_color_texture().is_some(),
        "base color texture wired"
    );
    // pbr.vfx exposes g_tNormalRoughness, so a normal map is wired too.
    assert!(mat.normal_texture().is_some(), "normal texture wired");
}

/// The textured path emits the `morphic` extras payload (NPR shader params +
/// embedded mask textures) and rides the overbright self-illum scale on
/// `KHR_materials_emissive_strength` via the serialized-JSON injection pass.
#[test]
fn glb_textured_emits_npr_extras_and_emissive_strength() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let resolver = FixtureResolver {
        vmat: std::fs::read(root.join("fixtures/material/vindicta_headv2.vmat_c"))
            .expect("vmat fixture"),
        vtex: std::fs::read(root.join("fixtures/bc7/generic_sleep_icon.vtex_c"))
            .expect("vtex fixture"),
    };
    let glb = super::to_glb_textured(&synthetic_model(), &resolver).expect("textured glb");

    // Read the raw JSON chunk: the injected extensions and extras are easiest
    // to assert on structurally (the gltf crate hides unknown extensions).
    let json_len = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    assert_eq!(&glb[16..20], b"JSON");
    let doc: serde_json::Value =
        serde_json::from_slice(&glb[20..20 + json_len]).expect("glb json chunk");

    // vindicta_headv2 carries g_flSelfIllumScale1 = 3.5 (> 1) and a self-illum
    // mask, so the material gets KHR_materials_emissive_strength, and the
    // extension is declared at the root (required for valid glTF).
    let used = doc["extensionsUsed"].as_array().expect("extensionsUsed");
    assert!(
        used.iter().any(|v| v == "KHR_materials_emissive_strength"),
        "extension listed in extensionsUsed"
    );
    let mat = &doc["materials"][0];
    let strength = mat["extensions"]["KHR_materials_emissive_strength"]["emissiveStrength"]
        .as_f64()
        .expect("emissiveStrength");
    assert!((strength - 3.5).abs() < 1e-6, "scale carried: {strength}");

    // The morphic extras payload: shader + full param tables + NPR masks.
    let morphic = &mat["extras"]["morphic"];
    assert_eq!(morphic["shader"], "pbr.vfx");
    assert_eq!(morphic["ints"]["F_USE_NPR_LIGHTING"], 1);
    let scale = morphic["floats"]["g_flSelfIllumScale1"]
        .as_f64()
        .expect("float param");
    assert!((scale - 3.5).abs() < 1e-6);
    let tint = morphic["vectors"]["g_vSelfIllumTint1"]
        .as_array()
        .expect("vector param");
    assert_eq!(tint.len(), 4, "vectors are [x, y, z, w]");

    // All three NPR mask slots resolve (to the fixture BC7), embed, and are
    // referenced by glTF texture index.
    let texture_count = doc["textures"].as_array().map_or(0, Vec::len) as u64;
    for slot in [
        "g_tTintMaskRimLightMask",
        "g_tNprOutlineMask",
        "g_tNprTransmissiveColor",
    ] {
        let idx = morphic["textures"][slot]
            .as_u64()
            .unwrap_or_else(|| panic!("{slot} embedded"));
        assert!(idx < texture_count, "{slot} texture index in range");
    }
}

/// The ORM image packs the metalness mask's R channel into B, nearest-neighbor
/// resampled to the roughness image's dimensions; without a mask B stays 0.
#[test]
fn metal_rough_packs_resampled_metalness() {
    // 2x2 packed normal-roughness (BLUE = roughness, the Source 2 packing the
    // GLB writer reads; alpha is a constant placeholder) + 1x1 metalness
    // (R = 200): the single metal texel upsamples across the whole ORM image.
    #[rustfmt::skip]
    let rough = [
        10, 20, 100, 255,  10, 20, 110, 255,
        10, 20, 120, 255,  10, 20, 130, 255,
    ];
    let metal = (1u32, 1u32, vec![200u8, 0, 0, 255]);
    let png = super::glb::metal_rough_png(2, 2, &rough, Some(&metal));
    let img = image::load_from_memory(&png).expect("orm png").to_rgba8();
    let expect_rough = [100u8, 110, 120, 130];
    for (i, px) in img.pixels().enumerate() {
        assert_eq!(px[0], 0, "R unused");
        assert_eq!(px[1], expect_rough[i], "G = roughness");
        assert_eq!(px[2], 200, "B = metalness");
    }

    let png = super::glb::metal_rough_png(2, 2, &rough, None);
    let img = image::load_from_memory(&png).expect("orm png").to_rgba8();
    assert!(img.pixels().all(|px| px[2] == 0), "no mask: B = 0");
}

/// [`super::glb::inject_material_extensions`] lands each extension object on
/// the right material and declares every name in `extensionsUsed`.
#[test]
fn material_extensions_inject_into_serialized_json() {
    let json = r#"{"asset":{"version":"2.0"},"materials":[{"name":"a"},{"name":"b"}]}"#;
    let mut unlit = serde_json::Map::new();
    unlit.insert("KHR_materials_unlit".to_owned(), serde_json::json!({}));
    let mut glass = serde_json::Map::new();
    glass.insert(
        "KHR_materials_transmission".to_owned(),
        serde_json::json!({ "transmissionFactor": 0.9 }),
    );
    glass.insert(
        "KHR_materials_ior".to_owned(),
        serde_json::json!({ "ior": 1.5 }),
    );
    let mut per = std::collections::BTreeMap::new();
    per.insert(0usize, unlit);
    per.insert(1usize, glass);

    let out = super::glb::inject_material_extensions(json, &per).expect("inject");
    let doc: serde_json::Value = serde_json::from_str(&out).expect("reparse");
    assert!(doc["materials"][0]["extensions"]["KHR_materials_unlit"].is_object());
    let t = doc["materials"][1]["extensions"]["KHR_materials_transmission"]["transmissionFactor"]
        .as_f64()
        .expect("transmissionFactor");
    assert!((t - 0.9).abs() < 1e-9);
    let ior = doc["materials"][1]["extensions"]["KHR_materials_ior"]["ior"]
        .as_f64()
        .expect("ior");
    assert!((ior - 1.5).abs() < 1e-9);
    let used: Vec<&str> = doc["extensionsUsed"]
        .as_array()
        .expect("extensionsUsed")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(
        used,
        [
            "KHR_materials_ior",
            "KHR_materials_transmission",
            "KHR_materials_unlit"
        ]
    );
}

#[test]
fn outline_materials_are_detected() {
    assert!(super::glb::is_outline_material(
        "models/heroes_staging/hornet_v3/materials/vindicta_outline.vmat"
    ));
    assert!(!super::glb::is_outline_material(
        "models/heroes_staging/hornet_v3/materials/skinmaterial.vmat"
    ));
}

#[test]
fn glow_shells_are_detected_but_noglow_is_kept() {
    // The additive glow effect shell (mesh part `ghost_glow`, material
    // `*_glow.vmat`) is a shell to drop.
    assert!(super::glb::is_glow_material("ghost_glow"));
    assert!(super::glb::is_glow_material(
        "models/heroes_staging/hornet_v3/materials/vindicta_glow.vmat"
    ));
    assert!(super::glb::is_shell("ghost_glow"));
    // A normal material that merely has glow turned off must be kept.
    assert!(!super::glb::is_glow_material(
        "models/heroes_staging/astro/materials/astro_barrelv2_noglow.vmat"
    ));
    assert!(!super::glb::is_shell(
        "models/heroes_staging/astro/materials/astro_barrelv2_noglow.vmat"
    ));
    assert!(!super::glb::is_shell("body"));
}

#[test]
fn viscous_goo_ball_alt_form_is_dropped() {
    // Viscous's Goo Ball alt-form: matched on the mesh-part name so all of its
    // primitives drop together, and on the `viscous_ball` material token.
    assert!(super::glb::is_alt_form("inflated"));
    assert!(super::glb::is_alt_form(
        "models/heroes_staging/viscous/materials/viscous_ball.vmat"
    ));
    assert!(super::glb::is_dropped("inflated"));
    // It is real geometry, not an NPR shell, so `is_shell` must NOT claim it.
    assert!(!super::glb::is_shell("inflated"));
    // Viscous's actual body parts/materials are kept.
    assert!(!super::glb::is_alt_form("body"));
    assert!(!super::glb::is_dropped("body"));
    assert!(!super::glb::is_alt_form(
        "models/heroes_staging/viscous/materials/viscous_body.vmat"
    ));
}

mod pose_bake {
    use super::super::animation::{BoneTrack, Clip};
    use super::super::math::{Mat4, Quat, Vec3};
    use super::super::mesh::{MeshPart, Primitive, VertexBuffer};
    use super::super::skeleton::{Bone, Skeleton};
    use super::super::{
        bake_pose, bake_pose_from, bake_pose_named, secondary_motion_pose_report, LocalPose, Model,
    };

    const ID: Quat = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    fn root_bone() -> Bone {
        root_bone_at("root", Vec3::default())
    }

    fn root_bone_at(name: &str, position: Vec3) -> Bone {
        let local_bind = Mat4::from_translation(position);
        let inverse_bind = local_bind.invert().expect("root bind inverse");
        Bone {
            name: name.into(),
            parent: None,
            flags: 0,
            position,
            rotation: ID,
            local_bind,
            global_bind: local_bind,
            inverse_bind,
        }
    }

    fn skinned_vertex(pos: [f32; 3]) -> VertexBuffer {
        skinned_vertex_on_joint(pos, 0)
    }

    fn skinned_vertex_on_joint(pos: [f32; 3], joint: u16) -> VertexBuffer {
        VertexBuffer {
            element_count: 1,
            stride: 0,
            positions: vec![pos],
            normals: vec![[0.0, 1.0, 0.0]],
            tangents: vec![],
            texcoords: vec![],
            colors: vec![],
            joints: vec![[joint, 0, 0, 0]],
            weights: vec![[1.0, 0.0, 0.0, 0.0]],
            layout: vec![],
        }
    }

    fn one_part(vb: VertexBuffer) -> MeshPart {
        MeshPart {
            name: "body".into(),
            mesh_index: 0,
            primitives: vec![Primitive {
                vertex_buffer: 0,
                vertex_buffers: vec![0],
                material: "body".into(),
                vertex_count: vb.element_count,
                indices: vec![0],
            }],
            vertex_buffers: vec![vb],
            min_bounds: [0.0; 3],
            max_bounds: [0.0; 3],
            bone_weight_count: 1,
        }
    }

    fn single_bone_clip(name: &str, t: Vec3) -> Clip {
        Clip {
            name: name.into(),
            fps: 30.0,
            frame_count: 1,
            looping: false,
            tracks: vec![BoneTrack {
                bone: 0,
                translations: Some(vec![t]),
                rotations: Some(vec![ID]),
                scales: None,
            }],
        }
    }

    fn child_bone(name: &str, parent: usize, position: Vec3) -> Bone {
        let local_bind = Mat4::from_translation(position);
        let inverse_bind = local_bind.invert().expect("child bind inverse");
        Bone {
            name: name.into(),
            parent: Some(parent),
            flags: 0,
            position,
            rotation: ID,
            local_bind,
            global_bind: local_bind,
            inverse_bind,
        }
    }

    fn two_bone_clip(name: &str, root_t: Vec3, child_t: Vec3) -> Clip {
        Clip {
            name: name.into(),
            fps: 30.0,
            frame_count: 1,
            looping: false,
            tracks: vec![
                BoneTrack {
                    bone: 0,
                    translations: Some(vec![root_t]),
                    rotations: Some(vec![ID]),
                    scales: None,
                },
                BoneTrack {
                    bone: 1,
                    translations: Some(vec![child_t]),
                    rotations: Some(vec![ID]),
                    scales: None,
                },
            ],
        }
    }

    fn approx(a: [f32; 3], b: [f32; 3]) {
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-4, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn bind_pose_clip_leaves_vertices_unchanged() {
        let model = Model {
            skeleton: Skeleton {
                bones: vec![root_bone()],
            },
            meshes: vec![one_part(skinned_vertex([1.0, 2.0, 3.0]))],
            animations: vec![single_bone_clip("ui_hero_pose", Vec3::default())],
            cloth: None,
        };
        let baked = bake_pose(&model, &["ui_hero_pose"], 0);
        assert!(baked.skeleton.bones.is_empty(), "skeleton stripped");
        assert!(baked.animations.is_empty(), "animations stripped");
        let vb = &baked.meshes[0].vertex_buffers[0];
        assert!(
            vb.joints.is_empty() && vb.weights.is_empty(),
            "skin stripped"
        );
        approx(vb.positions[0], [1.0, 2.0, 3.0]);
    }

    #[test]
    fn translation_pose_shifts_vertices() {
        let model = Model {
            skeleton: Skeleton {
                bones: vec![root_bone()],
            },
            meshes: vec![one_part(skinned_vertex([1.0, 2.0, 3.0]))],
            animations: vec![single_bone_clip(
                "ui_hero_pose",
                Vec3 {
                    x: 10.0,
                    y: 0.0,
                    z: 0.0,
                },
            )],
            cloth: None,
        };
        let baked = bake_pose(&model, &["ui_hero_pose"], 0);
        approx(
            baked.meshes[0].vertex_buffers[0].positions[0],
            [11.0, 2.0, 3.0],
        );
    }

    #[test]
    fn secondary_motion_bones_keep_bind_local_under_posed_parent() {
        let root = root_bone();
        let tail = child_bone(
            "tail_0",
            0,
            Vec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
        );
        let model = Model {
            skeleton: Skeleton {
                bones: vec![root, tail],
            },
            meshes: vec![one_part(skinned_vertex_on_joint([1.0, 0.0, 0.0], 1))],
            animations: vec![two_bone_clip(
                "ui_hero_pose",
                Vec3 {
                    x: 10.0,
                    y: 0.0,
                    z: 0.0,
                },
                Vec3 {
                    x: 100.0,
                    y: 0.0,
                    z: 0.0,
                },
            )],
            cloth: None,
        };

        let baked = bake_pose(&model, &["ui_hero_pose"], 0);
        approx(
            baked.meshes[0].vertex_buffers[0].positions[0],
            [11.0, 0.0, 0.0],
        );
    }

    #[test]
    fn root_secondary_motion_bones_follow_nearest_posed_body_bone() {
        let root = root_bone();
        let cloth = root_bone_at(
            "$cloth_m0p1",
            Vec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
        );
        let model = Model {
            skeleton: Skeleton {
                bones: vec![root, cloth],
            },
            meshes: vec![one_part(skinned_vertex_on_joint([1.0, 0.0, 0.0], 1))],
            animations: vec![single_bone_clip(
                "ui_hero_pose",
                Vec3 {
                    x: 10.0,
                    y: 0.0,
                    z: 0.0,
                },
            )],
            cloth: None,
        };

        let baked = bake_pose(&model, &["ui_hero_pose"], 0);
        approx(
            baked.meshes[0].vertex_buffers[0].positions[0],
            [11.0, 0.0, 0.0],
        );
    }

    #[test]
    fn root_secondary_motion_bones_skip_nearest_unmoved_body_bone() {
        let root = root_bone();
        let pelvis = child_bone(
            "pelvis",
            0,
            Vec3 {
                x: 5.0,
                y: 0.0,
                z: 0.0,
            },
        );
        let cloth = root_bone_at(
            "$cloth_m0p1",
            Vec3 {
                x: 0.2,
                y: 0.0,
                z: 0.0,
            },
        );
        let model = Model {
            skeleton: Skeleton {
                bones: vec![root, pelvis, cloth],
            },
            meshes: vec![one_part(skinned_vertex_on_joint([0.2, 0.0, 0.0], 2))],
            animations: vec![Clip {
                name: "ui_hero_pose".into(),
                fps: 30.0,
                frame_count: 1,
                looping: false,
                tracks: vec![
                    BoneTrack {
                        bone: 0,
                        translations: Some(vec![Vec3::default()]),
                        rotations: Some(vec![ID]),
                        scales: None,
                    },
                    BoneTrack {
                        bone: 1,
                        translations: Some(vec![Vec3 {
                            x: 15.0,
                            y: 0.0,
                            z: 0.0,
                        }]),
                        rotations: Some(vec![ID]),
                        scales: None,
                    },
                ],
            }],
            cloth: None,
        };

        let baked = bake_pose(&model, &["ui_hero_pose"], 0);
        approx(
            baked.meshes[0].vertex_buffers[0].positions[0],
            [10.2, 0.0, 0.0],
        );
    }

    #[test]
    fn secondary_motion_pose_report_flags_root_cloth_influences() {
        let root = root_bone();
        let mut cloth = root_bone();
        cloth.name = "$cloth_m0p1".into();
        let model = Model {
            skeleton: Skeleton {
                bones: vec![root, cloth],
            },
            meshes: vec![one_part(skinned_vertex_on_joint([1.0, 0.0, 0.0], 1))],
            animations: vec![single_bone_clip(
                "ui_hero_pose",
                Vec3 {
                    x: 10.0,
                    y: 0.0,
                    z: 0.0,
                },
            )],
            cloth: None,
        };

        let report =
            secondary_motion_pose_report(&model, &model, &["ui_hero_pose"]).expect("report");
        assert_eq!(report.clip_name, "ui_hero_pose");
        assert_eq!(report.secondary_bone_count, 1);
        assert_eq!(report.animated_secondary_bone_count, 0);
        assert_eq!(report.root_secondary_bone_count, 1);
        assert_eq!(report.vertices_with_secondary, 1);
        assert_eq!(report.vertices_majority_secondary, 1);
        assert_eq!(report.vertices_with_root_secondary, 1);
        assert_eq!(report.vertices_majority_root_secondary, 1);
        assert_eq!(report.materials.len(), 1);
        assert_eq!(report.materials[0].material, "body");
        assert_eq!(report.materials[0].vertices_majority_root_secondary, 1);
        assert_eq!(report.top_bones[0].bone, "$cloth_m0p1");
        assert!(report.top_bones[0].is_root);
    }

    #[test]
    fn no_matching_clip_falls_back_to_static_bind() {
        let model = Model {
            skeleton: Skeleton {
                bones: vec![root_bone()],
            },
            meshes: vec![one_part(skinned_vertex([4.0, 5.0, 6.0]))],
            animations: vec![single_bone_clip(
                "walk",
                Vec3 {
                    x: 99.0,
                    y: 0.0,
                    z: 0.0,
                },
            )],
            cloth: None,
        };
        let baked = bake_pose(&model, &["ui_hero_pose"], 0);
        assert!(baked.skeleton.bones.is_empty());
        let vb = &baked.meshes[0].vertex_buffers[0];
        assert!(vb.joints.is_empty() && vb.weights.is_empty());
        approx(vb.positions[0], [4.0, 5.0, 6.0]);
    }

    #[test]
    fn prop_without_skeleton_passes_through() {
        let model = Model {
            skeleton: Skeleton { bones: vec![] },
            meshes: vec![one_part(skinned_vertex([5.0, 6.0, 7.0]))],
            animations: vec![],
            cloth: None,
        };
        let baked = bake_pose(&model, &["ui_hero_pose"], 0);
        let vb = &baked.meshes[0].vertex_buffers[0];
        assert!(vb.joints.is_empty() && vb.weights.is_empty());
        approx(vb.positions[0], [5.0, 6.0, 7.0]);
    }

    #[test]
    fn donor_clip_poses_a_clipless_skin_by_bone_name() {
        // The skin ships the mesh + rig but no clips (like a real skin mod).
        let skin = Model {
            skeleton: Skeleton {
                bones: vec![root_bone()],
            },
            meshes: vec![one_part(skinned_vertex([1.0, 2.0, 3.0]))],
            animations: vec![],
            cloth: None,
        };
        // The base hero supplies the clip; same bone name "root".
        let donor = Model {
            skeleton: Skeleton {
                bones: vec![root_bone()],
            },
            meshes: vec![],
            animations: vec![single_bone_clip(
                "ui_hero_pose",
                Vec3 {
                    x: 10.0,
                    y: 0.0,
                    z: 0.0,
                },
            )],
            cloth: None,
        };
        let baked = bake_pose_from(&skin, &donor, &["ui_hero_pose"], 0);
        approx(
            baked.meshes[0].vertex_buffers[0].positions[0],
            [11.0, 2.0, 3.0],
        );
    }

    #[test]
    fn donor_bone_name_mismatch_keeps_bind_pose() {
        let skin = Model {
            skeleton: Skeleton {
                bones: vec![root_bone()],
            },
            meshes: vec![one_part(skinned_vertex([1.0, 2.0, 3.0]))],
            animations: vec![],
            cloth: None,
        };
        let mut donor_bone = root_bone();
        donor_bone.name = "unrelated".into();
        let donor = Model {
            skeleton: Skeleton {
                bones: vec![donor_bone],
            },
            meshes: vec![],
            animations: vec![single_bone_clip(
                "ui_hero_pose",
                Vec3 {
                    x: 10.0,
                    y: 0.0,
                    z: 0.0,
                },
            )],
            cloth: None,
        };
        // The donor's clip targets "unrelated"; the skin has "root", so no bone
        // matches and the vertex stays at its bind position.
        let baked = bake_pose_from(&skin, &donor, &["ui_hero_pose"], 0);
        approx(
            baked.meshes[0].vertex_buffers[0].positions[0],
            [1.0, 2.0, 3.0],
        );
    }

    #[test]
    fn named_pose_shifts_matched_bone_and_binds_the_rest() {
        use std::collections::HashMap;

        let model = Model {
            skeleton: Skeleton {
                bones: vec![root_bone()],
            },
            meshes: vec![one_part(skinned_vertex([1.0, 2.0, 3.0]))],
            animations: vec![],
            cloth: None,
        };

        // A pose keyed by bone name (the NM path) translates the matched bone.
        let mut by_name = HashMap::new();
        by_name.insert(
            "root".to_string(),
            LocalPose {
                translation: Vec3 {
                    x: 10.0,
                    y: 0.0,
                    z: 0.0,
                },
                rotation: ID,
                scale: 1.0,
            },
        );
        let baked = bake_pose_named(&model, &by_name);
        assert!(baked.skeleton.bones.is_empty(), "skeleton stripped");
        approx(
            baked.meshes[0].vertex_buffers[0].positions[0],
            [11.0, 2.0, 3.0],
        );

        // A name that matches no bone leaves the mesh at its bind pose (how the
        // model's cloth/twist/helper bones, absent from an NM clip, are handled).
        let mut unmatched = HashMap::new();
        unmatched.insert(
            "nonexistent".to_string(),
            LocalPose {
                translation: Vec3 {
                    x: 99.0,
                    y: 0.0,
                    z: 0.0,
                },
                rotation: ID,
                scale: 1.0,
            },
        );
        let bind = bake_pose_named(&model, &unmatched);
        approx(
            bind.meshes[0].vertex_buffers[0].positions[0],
            [1.0, 2.0, 3.0],
        );
    }

    #[test]
    fn named_pose_carries_root_secondary_bones_with_nearest_body_bone() {
        use std::collections::HashMap;

        let root = root_bone();
        let cloth = root_bone_at(
            "$cloth_m0p1",
            Vec3 {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
        );
        let model = Model {
            skeleton: Skeleton {
                bones: vec![root, cloth],
            },
            meshes: vec![one_part(skinned_vertex_on_joint([1.0, 0.0, 0.0], 1))],
            animations: vec![],
            cloth: None,
        };

        let mut by_name = HashMap::new();
        by_name.insert(
            "root".to_string(),
            LocalPose {
                translation: Vec3 {
                    x: 10.0,
                    y: 0.0,
                    z: 0.0,
                },
                rotation: ID,
                scale: 1.0,
            },
        );

        let baked = bake_pose_named(&model, &by_name);
        approx(
            baked.meshes[0].vertex_buffers[0].positions[0],
            [11.0, 0.0, 0.0],
        );
    }
}
