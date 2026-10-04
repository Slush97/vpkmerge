//! Local (non-CI) full-model decode check: the end-to-end M3 path that needs
//! the real multi-megabyte vertex buffers, so it is gated on `MORPHIC_MODEL_VPK`
//! pointing at a Deadlock `pak01_dir.vpk` and skipped otherwise. Decodes the
//! hornet hero model (buffers, deinterleave, skin) and diffs the whole result
//! against the committed oracle golden `hornet_model_meta.json`.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Deserialize)]
struct Golden {
    bone_count: usize,
    bone_names: Vec<String>,
    meshes: Vec<GMesh>,
    unique_vertices: usize,
    gltf_vertices: usize,
    total_indices: usize,
    material_count: usize,
    materials: Vec<String>,
    bbox_min: [f32; 3],
    bbox_max: [f32; 3],
}

#[derive(Deserialize)]
struct GMesh {
    name: String,
    mesh_index: usize,
    primitives: Vec<GPrim>,
}

#[derive(Deserialize)]
struct GPrim {
    vertex_buffer: usize,
    vertex_count: usize,
    index_count: usize,
    material: String,
}

fn load_golden() -> Golden {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/kv3/hornet_model_meta.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("read golden"))
        .expect("parse golden")
}

fn approx3(a: [f32; 3], b: [f32; 3], what: &str) {
    for i in 0..3 {
        assert!(
            (a[i] - b[i]).abs() < 1e-3,
            "{what}[{i}]: {} vs {}",
            a[i],
            b[i]
        );
    }
}

/// Resolves compiled resource paths across VPKs: the model's own VPK first,
/// then any fallbacks (the base `pak01_dir.vpk` for materials/textures a skin
/// references but does not ship). The caller-side I/O `to_glb_textured` needs.
struct VpkResolver {
    vpks: Vec<valve_pak::VPK>,
}

impl morphic::model::FileResolver for VpkResolver {
    fn resolve(&self, compiled_path: &str) -> Option<Vec<u8>> {
        for vpk in &self.vpks {
            if let Ok(mut vf) = vpk.get_file(compiled_path) {
                if let Ok(bytes) = vf.read_all() {
                    return Some(bytes);
                }
            }
        }
        None
    }
}

/// Diagnostic: per material, report its base-color slot's resolve + decode
/// result (dims, or why it is missing). Gated on `MORPHIC_DIAG_VPK`
/// (+ optional `MORPHIC_DIAG_BASE`, `MORPHIC_DIAG_ENTRY`).
#[test]
fn diagnose_textures() {
    use morphic::model::FileResolver as _;

    let Ok(vpk_path) = std::env::var("MORPHIC_DIAG_VPK") else {
        eprintln!("MORPHIC_DIAG_VPK not set; skipping");
        return;
    };
    let entry = std::env::var("MORPHIC_DIAG_ENTRY")
        .unwrap_or_else(|_| "models/heroes_staging/hornet_v3/hornet.vmdl_c".to_string());

    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut vf = vpk.get_file(&entry).expect("entry");
    let bytes = vf.read_all().expect("read");
    let model = morphic::model::decode(&bytes).expect("decode");

    let mut vpks = vec![vpk];
    if let Ok(base) = std::env::var("MORPHIC_DIAG_BASE") {
        vpks.push(valve_pak::open(&base).expect("base"));
    }
    let resolver = VpkResolver { vpks };
    let compiled = |p: &str| {
        if p.ends_with("_c") {
            p.to_string()
        } else {
            format!("{p}_c")
        }
    };

    for mat_path in model.materials() {
        let name = mat_path.rsplit('/').next().unwrap_or(&mat_path).to_string();
        let Some(vmat) = resolver.resolve(&compiled(&mat_path)) else {
            eprintln!("MAT {name}: vmat UNRESOLVED");
            continue;
        };
        let mat = morphic::material::parse(&vmat).expect("parse vmat");
        let Some(base) = mat.pbr().base_color else {
            eprintln!("MAT {name} [{}]: no base_color slot", mat.shader_name);
            continue;
        };
        let status = match resolver.resolve(&compiled(base)) {
            None => "UNRESOLVED".to_string(),
            Some(b) => match morphic::decode(&b) {
                Err(e) => format!("decode-err: {e}"),
                Ok(img) => format!("{}x{}", img.width, img.height),
            },
        };
        eprintln!(
            "MAT {name} [{}]: base_color {} -> {status}",
            mat.shader_name,
            base.rsplit('/').next().unwrap_or(base)
        );
    }
}

/// Gated textured GLB export of an arbitrary VPK entry (no golden diff): set
/// `MORPHIC_EXPORT_VPK` (+ optional `MORPHIC_EXPORT_ENTRY`, `MORPHIC_EXPORT_OUT`,
/// and `MORPHIC_EXPORT_BASE` for the base pak that skins reference). Decodes the
/// model and writes a textured `.glb`. A stand-in for the M6 `model export` CLI.
#[test]
fn export_glb_from_env() {
    let Ok(vpk_path) = std::env::var("MORPHIC_EXPORT_VPK") else {
        eprintln!("MORPHIC_EXPORT_VPK not set; skipping arbitrary export");
        return;
    };
    let entry = std::env::var("MORPHIC_EXPORT_ENTRY")
        .unwrap_or_else(|_| "models/heroes_staging/hornet_v3/hornet.vmdl_c".to_string());
    let out = std::env::var("MORPHIC_EXPORT_OUT").unwrap_or_else(|_| "/tmp/export.glb".to_string());

    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut vf = vpk.get_file(&entry).expect("locate entry");
    let bytes = vf.read_all().expect("read entry");
    let model = morphic::model::decode(&bytes).expect("decode model");

    // Resolver: the model's VPK first, then the base pak (if given).
    let mut vpks = vec![vpk];
    if let Ok(base) = std::env::var("MORPHIC_EXPORT_BASE") {
        vpks.push(valve_pak::open(&base).expect("open base pak"));
    }
    let resolver = VpkResolver { vpks };

    let glb =
        morphic::model::to_glb_textured(&model, &resolver, morphic::model::GlbOptions::default())
            .expect("write glb");
    std::fs::write(&out, &glb).expect("write glb file");
    eprintln!(
        "exported {entry} -> {out} ({} bytes): {} bones, {} meshes, {} unique verts, {} materials",
        glb.len(),
        model.skeleton.bones.len(),
        model.meshes.len(),
        model.total_vertices(),
        model.materials().len(),
    );
}

/// Real-data regression guard for the additive glow-overlay keep (Phase 1 of the
/// Source 2 preview plan): decode a glow-bearing hero, export the textured GLB,
/// re-read it, and confirm the additive `*_glow` overlay mesh survived export
/// while the inverted-hull `*_outline` / `*jitter*` shells are still dropped.
/// Gated on `MORPHIC_MODEL_VPK` (the base Deadlock `pak01_dir.vpk`); the default
/// entry is hornet, whose `ghost_glow` overlay is the canonical case. The
/// `blend_mode='additive'` extras the viewer reads are covered separately (the
/// grimoire `source2Preview` unit tests use the exact exported extras values).
#[test]
fn glow_overlay_survives_export_local() {
    let Ok(vpk_path) = std::env::var("MORPHIC_MODEL_VPK") else {
        eprintln!("MORPHIC_MODEL_VPK not set; skipping glow-overlay export check");
        return;
    };
    let entry = std::env::var("MORPHIC_MODEL_ENTRY")
        .unwrap_or_else(|_| "models/heroes_staging/hornet_v3/hornet.vmdl_c".to_string());

    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut vf = vpk.get_file(&entry).expect("locate entry");
    let bytes = vf.read_all().expect("read entry");
    let model = morphic::model::decode(&bytes).expect("decode model");

    let resolver = VpkResolver { vpks: vec![vpk] };
    let glb =
        morphic::model::to_glb_textured(&model, &resolver, morphic::model::GlbOptions::default())
            .expect("write glb");

    let gltf = gltf::Gltf::from_slice(&glb).expect("re-read glb");
    let mesh_names: Vec<String> = gltf
        .document
        .meshes()
        .filter_map(|m| m.name().map(str::to_string))
        .collect();

    // The additive glow overlay (hornet's `ghost_glow`) survived export.
    assert!(
        mesh_names
            .iter()
            .any(|n| n.to_ascii_lowercase().contains("glow")),
        "expected a kept *glow* overlay mesh, got {mesh_names:?}"
    );
    // The inverted-hull outline / jitter shells are still dropped.
    assert!(
        !mesh_names.iter().any(|n| {
            let lc = n.to_ascii_lowercase();
            lc.contains("outline") || lc.contains("jitter")
        }),
        "outline/jitter shells must not survive export, got {mesh_names:?}"
    );

    eprintln!("glow overlay export OK: meshes {mesh_names:?}");
}

/// Lists hero body-model entries in a VPK: `<dir>/<name>/<name>.vmdl_c` under a
/// `models/heroes*` path (the body model convention, skipping LODs/backups/props).
/// Set `MORPHIC_DIAG_VPK`; skipped otherwise. Cheap (path listing only, no decode).
#[test]
fn list_heroes() {
    let Ok(vpk_path) = std::env::var("MORPHIC_DIAG_VPK") else {
        eprintln!("MORPHIC_DIAG_VPK not set; skipping");
        return;
    };
    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut hits: Vec<String> = vpk
        .file_paths()
        .filter(|p| p.ends_with(".vmdl_c") && p.contains("/heroes"))
        .filter(|p| {
            let stem = p
                .rsplit('/')
                .next()
                .unwrap_or(p)
                .trim_end_matches(".vmdl_c");
            let parent = p.rsplit('/').nth(1).unwrap_or("");
            stem == parent
        })
        .cloned()
        .collect();
    hits.sort();
    hits.dedup();
    for h in &hits {
        eprintln!("HERO {h}");
    }
    eprintln!("HEROCOUNT {}", hits.len());
}

/// Lists EVERY `.vmdl_c` under `/heroes` (minus obvious non-body parts), for
/// finding a hero's real/current body model (which may be in a `_vN` dir, e.g.
/// `hornet_v3/hornet.vmdl_c`). Set `MORPHIC_DIAG_VPK`; optional `MORPHIC_DIAG_GREP`
/// filters by substring. Cheap (path listing only).
#[test]
fn list_hero_vmdl_all() {
    let Ok(vpk_path) = std::env::var("MORPHIC_DIAG_VPK") else {
        eprintln!("MORPHIC_DIAG_VPK not set; skipping");
        return;
    };
    let needle = std::env::var("MORPHIC_DIAG_GREP")
        .unwrap_or_default()
        .to_lowercase();
    let skip = [
        "_lod",
        "lod0",
        "lod1",
        "lod2",
        "lod3",
        "backup",
        "/clips/",
        "abilities",
        "particle",
        "/materials/",
        "_dbg",
        "destruction",
    ];
    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut hits: Vec<String> = vpk
        .file_paths()
        .filter(|p| p.ends_with(".vmdl_c") && p.contains("/heroes"))
        .filter(|p| {
            let lc = p.to_lowercase();
            !skip.iter().any(|s| lc.contains(s)) && (needle.is_empty() || lc.contains(&needle))
        })
        .cloned()
        .collect();
    hits.sort();
    hits.dedup();
    for h in &hits {
        eprintln!("VMDL {h}");
    }
    eprintln!("VMDLCOUNT {}", hits.len());
}

/// Lists the animation clips a model carries. Set `MORPHIC_DIAG_VPK`
/// (+ optional `MORPHIC_DIAG_ENTRY`); skipped otherwise. Diagnostic for picking a
/// `--pose` clip.
#[test]
fn list_clips() {
    let Ok(vpk_path) = std::env::var("MORPHIC_DIAG_VPK") else {
        eprintln!("MORPHIC_DIAG_VPK not set; skipping");
        return;
    };
    let entry = std::env::var("MORPHIC_DIAG_ENTRY")
        .unwrap_or_else(|_| "models/heroes_staging/hornet_v3/hornet.vmdl_c".to_string());
    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut vf = vpk.get_file(&entry).expect("entry");
    let bytes = vf.read_all().expect("read");
    let model = morphic::model::decode(&bytes).expect("decode");
    eprintln!("CLIPS {} ({} total):", entry, model.animations.len());
    for c in &model.animations {
        eprintln!("  {} ({} frames)", c.name, c.frame_count);
    }
}

#[test]
fn decode_hornet_local() {
    let Ok(vpk_path) = std::env::var("MORPHIC_MODEL_VPK") else {
        eprintln!("MORPHIC_MODEL_VPK not set; skipping local model decode");
        return;
    };
    let entry = std::env::var("MORPHIC_MODEL_ENTRY")
        .unwrap_or_else(|_| "models/heroes_staging/hornet_v3/hornet.vmdl_c".to_string());

    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut vf = vpk.get_file(&entry).expect("locate entry");
    let bytes = vf.read_all().expect("read entry");

    let model = morphic::model::decode(&bytes).expect("decode model");
    let g = load_golden();

    // Skeleton.
    assert_eq!(model.skeleton.bones.len(), g.bone_count, "joint count");
    assert_eq!(
        model.skeleton.sorted_bone_names(),
        g.bone_names,
        "bone names"
    );

    // Totals.
    assert_eq!(model.total_vertices(), g.unique_vertices, "unique vertices");
    assert_eq!(model.gltf_vertex_total(), g.gltf_vertices, "gltf vertices");
    assert_eq!(model.total_indices(), g.total_indices, "total indices");
    assert_eq!(model.materials(), g.materials, "materials");
    assert_eq!(model.materials().len(), g.material_count, "material count");

    // Bounds over decoded positions.
    let bounds = model.position_bounds().expect("position bounds");
    approx3(bounds.min, g.bbox_min, "bbox min");
    approx3(bounds.max, g.bbox_max, "bbox max");

    // Per-mesh primitives + skin attributes.
    assert_eq!(model.meshes.len(), g.meshes.len(), "mesh count");
    for (m, gm) in model.meshes.iter().zip(&g.meshes) {
        assert_eq!(m.name, gm.name, "mesh name");
        assert_eq!(m.mesh_index, gm.mesh_index, "mesh index");
        assert_eq!(
            m.primitives.len(),
            gm.primitives.len(),
            "{}: prim count",
            gm.name
        );

        for (p, gp) in m.primitives.iter().zip(&gm.primitives) {
            assert_eq!(p.vertex_buffer, gp.vertex_buffer, "{}: prim vbuf", gm.name);
            assert_eq!(
                p.vertex_count, gp.vertex_count,
                "{}: prim vert count",
                gm.name
            );
            assert_eq!(
                p.indices.len(),
                gp.index_count,
                "{}: prim index count",
                gm.name
            );
            assert_eq!(p.material, gp.material, "{}: prim material", gm.name);

            // Every index addresses a real vertex in the buffer it draws from.
            let vb = &m.vertex_buffers[p.vertex_buffer];
            assert!(
                p.indices.iter().all(|&i| (i as usize) < vb.element_count),
                "{}: index out of range",
                gm.name
            );
        }

        // Skinned buffers carry one joint set per vertex, positioned and normaled.
        for vb in &m.vertex_buffers {
            assert_eq!(
                vb.positions.len(),
                vb.element_count,
                "{}: positions",
                gm.name
            );
            assert_eq!(vb.normals.len(), vb.element_count, "{}: normals", gm.name);
            if !vb.joints.is_empty() {
                assert_eq!(vb.joints.len(), vb.element_count, "{}: joints", gm.name);
                let max_joint = vb.joints.iter().flatten().copied().max().unwrap_or(0);
                assert!(
                    (max_joint as usize) < model.skeleton.bones.len(),
                    "{}: joint index exceeds skeleton",
                    gm.name
                );
            }
        }
    }

    eprintln!(
        "hornet OK: {} bones, {} meshes, {} unique verts ({} gltf), {} indices, {} materials",
        model.skeleton.bones.len(),
        model.meshes.len(),
        model.total_vertices(),
        model.gltf_vertex_total(),
        model.total_indices(),
        model.materials().len(),
    );

    assert_glb_roundtrip(&model);
}

/// Tier-0 displacement edit, end to end on the real hornet model: enumerate
/// vertex targets, translate one buffer's positions, splice it back, and confirm
/// the edited `.vmdl_c` (a) re-decodes, (b) shows exactly that buffer's positions
/// shifted by the delta, and (c) leaves every other attribute and buffer
/// byte-identical. Gated on `MORPHIC_MODEL_VPK`; run with the local Deadlock pak.
#[test]
fn displacement_edit_round_trips_local() {
    let Ok(vpk_path) = std::env::var("MORPHIC_MODEL_VPK") else {
        eprintln!("MORPHIC_MODEL_VPK not set; skipping local displacement edit");
        return;
    };
    let entry = std::env::var("MORPHIC_MODEL_ENTRY")
        .unwrap_or_else(|_| "models/heroes_staging/hornet_v3/hornet.vmdl_c".to_string());

    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut vf = vpk.get_file(&entry).expect("locate entry");
    let bytes = vf.read_all().expect("read entry");

    // Pick the first displacement-editable vertex buffer.
    let targets = morphic::model::vertex_targets(&bytes).expect("targets");
    let target = targets
        .iter()
        .find(|t| t.editable)
        .expect("at least one editable vertex buffer");
    eprintln!(
        "editing buffer: mesh={} block={} verts={} stride={}",
        target.mesh_name, target.block_index, target.vertex_count, target.stride
    );

    // Translate the whole buffer by a fixed delta.
    let delta = [5.0f32, -3.0, 2.0];
    let orig_pos = morphic::model::read_vertex_positions(&bytes, target.block_index).expect("read");
    assert_eq!(orig_pos.len(), target.vertex_count, "position count");
    let moved: Vec<[f32; 3]> = orig_pos
        .iter()
        .map(|p| [p[0] + delta[0], p[1] + delta[1], p[2] + delta[2]])
        .collect();

    let edited = morphic::model::replace_vertex_positions(&bytes, target.block_index, &moved)
        .expect("splice");

    // (a) The edited buffer's positions read back shifted by exactly the delta.
    let new_pos =
        morphic::model::read_vertex_positions(&edited, target.block_index).expect("read edited");
    assert_eq!(new_pos.len(), orig_pos.len(), "edited position count");
    for (o, n) in orig_pos.iter().zip(&new_pos) {
        for k in 0..3 {
            assert!(
                (n[k] - (o[k] + delta[k])).abs() <= 1e-3,
                "position not shifted by delta: {o:?} -> {n:?}"
            );
        }
    }

    // (b)+(c) Full re-decode: same structure; only the edited buffer's positions
    // changed, everything else (normals/uv/joints/weights/indices) byte-identical.
    let before = morphic::model::decode(&bytes).expect("decode original");
    let after = morphic::model::decode(&edited).expect("decode edited");
    assert_eq!(
        before.meshes.len(),
        after.meshes.len(),
        "mesh count preserved"
    );
    assert_eq!(
        before.total_indices(),
        after.total_indices(),
        "index total preserved"
    );

    let mut changed_buffers = 0usize;
    for (mb, ma) in before.meshes.iter().zip(&after.meshes) {
        assert_eq!(mb.name, ma.name, "mesh name");
        assert_eq!(
            mb.vertex_buffers.len(),
            ma.vertex_buffers.len(),
            "buffer count"
        );
        for (vb, va) in mb.vertex_buffers.iter().zip(&ma.vertex_buffers) {
            // Non-position attributes are always preserved.
            assert_eq!(vb.normals, va.normals, "{}: normals", mb.name);
            assert_eq!(vb.tangents, va.tangents, "{}: tangents", mb.name);
            assert_eq!(vb.texcoords, va.texcoords, "{}: texcoords", mb.name);
            assert_eq!(vb.joints, va.joints, "{}: joints", mb.name);
            assert_eq!(vb.weights, va.weights, "{}: weights", mb.name);
            if vb.positions != va.positions {
                changed_buffers += 1;
            }
        }
        // Primitive indices are unchanged everywhere.
        for (pb, pa) in mb.primitives.iter().zip(&ma.primitives) {
            assert_eq!(pb.indices, pa.indices, "{}: indices", mb.name);
        }
    }
    assert_eq!(
        changed_buffers, 1,
        "exactly one vertex buffer's positions changed"
    );
    eprintln!("displacement edit OK: 1 buffer moved, all other attributes preserved");
}

/// Blender-reshape round-trip plumbing on the real model, WITHOUT Blender: export
/// a buffer to an edit `.glb`, feed the UNEDITED glb straight back through the
/// importer, and confirm the spliced model's positions are unchanged (an identity
/// round-trip through the `_ORIGID` carrier + glb reader + splice). Gated on
/// `MORPHIC_MODEL_VPK`.
#[test]
fn edit_glb_identity_round_trips_local() {
    let Ok(vpk_path) = std::env::var("MORPHIC_MODEL_VPK") else {
        eprintln!("MORPHIC_MODEL_VPK not set; skipping local edit-glb round-trip");
        return;
    };
    let entry = std::env::var("MORPHIC_MODEL_ENTRY")
        .unwrap_or_else(|_| "models/heroes_staging/hornet_v3/hornet.vmdl_c".to_string());

    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut vf = vpk.get_file(&entry).expect("entry");
    let bytes = vf.read_all().expect("read");

    let target = morphic::model::vertex_targets(&bytes)
        .expect("targets")
        .into_iter()
        .find(|t| t.editable)
        .expect("an editable buffer");
    eprintln!(
        "edit-glb round-trip: mesh={} block={} verts={}",
        target.mesh_name, target.block_index, target.vertex_count
    );

    let glb = morphic::model::export_buffer_for_edit(&bytes, target.block_index).expect("export");
    let edited = morphic::model::apply_edited_glb(&bytes, target.block_index, &glb).expect("apply");

    // Identity edit: the edited buffer's positions match the original within float
    // round-trip tolerance; every other attribute is byte-identical.
    let before = morphic::model::decode(&bytes).expect("decode original");
    let after = morphic::model::decode(&edited).expect("decode edited");
    let mut compared = 0usize;
    for (mb, ma) in before.meshes.iter().zip(&after.meshes) {
        for (vb, va) in mb.vertex_buffers.iter().zip(&ma.vertex_buffers) {
            assert_eq!(vb.normals, va.normals, "{}: normals", mb.name);
            assert_eq!(vb.texcoords, va.texcoords, "{}: texcoords", mb.name);
            assert_eq!(vb.joints, va.joints, "{}: joints", mb.name);
            assert_eq!(vb.weights, va.weights, "{}: weights", mb.name);
            for (p0, p1) in vb.positions.iter().zip(&va.positions) {
                for k in 0..3 {
                    assert!(
                        (p0[k] - p1[k]).abs() <= 1e-3,
                        "{}: position drifted: {p0:?} vs {p1:?}",
                        mb.name
                    );
                }
                compared += 1;
            }
        }
    }
    assert!(compared > 0, "compared some vertices");
    eprintln!("edit-glb identity round-trip OK: {compared} vertices stable");
}

/// Tier-1a draw-call removal, end to end on the real hornet model: neutralize the
/// `vindicta_dress` material's draw call(s) by zeroing their `m_nIndexCount` in a
/// byte-faithful (uncompressed, structure-preserving) `MDAT` re-wrap, splice, and
/// confirm (a) the edited `.vmdl_c` still decodes, (b) every dress draw call now
/// renders zero indices, (c) every other primitive's indices are unchanged and the
/// total drops by exactly the dress's index count, and (d) no vertex buffer was
/// touched. The draw call is neutralized in place, not deleted, because the engine
/// rejects a lossy KV3 re-encode (confirmed in-game; see `docs/handoff-model-edit.md`).
/// Gated on `MORPHIC_MODEL_VPK`.
#[test]
fn remove_material_round_trips_local() {
    let Ok(vpk_path) = std::env::var("MORPHIC_MODEL_VPK") else {
        eprintln!("MORPHIC_MODEL_VPK not set; skipping local material removal");
        return;
    };
    let entry = std::env::var("MORPHIC_MODEL_ENTRY")
        .unwrap_or_else(|_| "models/heroes_staging/hornet_v3/hornet.vmdl_c".to_string());

    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut vf = vpk.get_file(&entry).expect("entry");
    let bytes = vf.read_all().expect("read");

    let needle =
        std::env::var("MORPHIC_REMOVE_MATERIAL").unwrap_or_else(|_| "vindicta_dress".to_string());

    let before = morphic::model::decode(&bytes).expect("decode original");
    let dress_prims_before = before
        .meshes
        .iter()
        .flat_map(|m| &m.primitives)
        .filter(|p| {
            p.material
                .to_ascii_lowercase()
                .contains(&needle.to_ascii_lowercase())
        })
        .count();
    assert!(
        dress_prims_before > 0,
        "model has no {needle:?} draw call to remove (materials: {:?})",
        before.materials()
    );

    let (edited, removed) =
        morphic::model::remove_draw_calls_by_material(&bytes, &needle).expect("remove");
    assert!(!removed.is_empty(), "removal reported nothing");
    assert!(
        removed.iter().all(|r| r
            .material
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())),
        "removed a non-matching material: {removed:?}"
    );
    eprintln!("removed {} draw call(s) for {needle:?}", removed.len());

    let after = morphic::model::decode(&edited).expect("decode edited");
    let needle_lc = needle.to_ascii_lowercase();

    // (b) The dress draw call(s) are still present (neutralized, not deleted) but
    // now render zero indices.
    let dress_prims_after: Vec<&morphic::model::Primitive> = after
        .meshes
        .iter()
        .flat_map(|m| &m.primitives)
        .filter(|p| p.material.to_ascii_lowercase().contains(&needle_lc))
        .collect();
    assert_eq!(
        dress_prims_after.len(),
        dress_prims_before,
        "dress draw calls remain in place (neutralized, not deleted)"
    );
    assert!(
        dress_prims_after.iter().all(|p| p.indices.is_empty()),
        "every dress draw call now renders zero indices"
    );

    // (c) The full material set is unchanged (the dress is still referenced), and
    // exactly the dress's indices left the rendered total.
    let mut before_mats = before.materials();
    before_mats.sort();
    let mut after_mats = after.materials();
    after_mats.sort();
    assert_eq!(after_mats, before_mats, "material set unchanged");
    assert_eq!(
        after.total_indices() + dress_index_count(&before, &needle),
        before.total_indices(),
        "only the dress's indices left the rendered set"
    );

    // (d) Mesh/vertex-buffer geometry is byte-identical: the edit touches MDAT only.
    assert_eq!(before.meshes.len(), after.meshes.len(), "mesh count");
    for (mb, ma) in before.meshes.iter().zip(&after.meshes) {
        assert_eq!(mb.name, ma.name, "mesh name");
        assert_eq!(
            mb.vertex_buffers.len(),
            ma.vertex_buffers.len(),
            "{}: buffer count",
            mb.name
        );
        for (vb, va) in mb.vertex_buffers.iter().zip(&ma.vertex_buffers) {
            assert_eq!(vb.positions, va.positions, "{}: positions", mb.name);
            assert_eq!(vb.normals, va.normals, "{}: normals", mb.name);
            assert_eq!(vb.joints, va.joints, "{}: joints", mb.name);
            assert_eq!(vb.weights, va.weights, "{}: weights", mb.name);
        }
    }
    eprintln!(
        "material removal OK: {} draw call(s) gone, {} -> {} indices, geometry untouched",
        removed.len(),
        before.total_indices(),
        after.total_indices()
    );
}

/// T1d-d replace-in-place, end to end on the real hornet model: re-read the gun
/// mesh from a textured-glb export and splice it back over the gun part. This is
/// the same-geometry case (the strongest skinning round-trip): the new buffer is
/// localized to the gun's bone palette (T1d-c), assembled to the gun's exact field
/// set (T1d-b), re-encoded, and the CTRL registry + MDAT draw call patched. Asserts
/// (a) the edited `.vmdl_c` decodes, (b) the gun primitive carries the new counts,
/// (c) the gun's position bounds are preserved, (d) skinning round-trips (the
/// weighted influence's model bone is recovered through the forward remap), and
/// (e) every other part (body, `ghost_glow`) is byte-identical. Gated on
/// `MORPHIC_MODEL_VPK` + `MORPHIC_EDIT_GLB` (a `to_glb_textured` export).
#[test]
fn replace_gun_from_glb_round_trips_local() {
    let (Ok(vpk_path), Ok(glb_path)) = (
        std::env::var("MORPHIC_MODEL_VPK"),
        std::env::var("MORPHIC_EDIT_GLB"),
    ) else {
        eprintln!("MORPHIC_MODEL_VPK / MORPHIC_EDIT_GLB not set; skipping replace-gun round-trip");
        return;
    };
    let entry = std::env::var("MORPHIC_MODEL_ENTRY")
        .unwrap_or_else(|_| "models/heroes_staging/hornet_v3/hornet.vmdl_c".to_string());

    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut vf = vpk.get_file(&entry).expect("entry");
    let bytes = vf.read_all().expect("read");
    let glb = std::fs::read(&glb_path).expect("read glb");

    let (vb, indices) = morphic::model::read_edited_mesh(&glb, Some("gun")).expect("read gun mesh");
    eprintln!(
        "new gun mesh: {} verts, {} indices (joints={}, weights={})",
        vb.element_count,
        indices.len(),
        vb.joints.len(),
        vb.weights.len()
    );

    let before = morphic::model::decode(&bytes).expect("decode original");
    let (edited, report) =
        morphic::model::replace_mesh_part(&bytes, "gun", &vb, &indices).expect("replace gun");
    eprintln!(
        "replaced gun: {} -> {} verts, {} -> {} idx, stride {}, idx width {}",
        report.old_vertex_count,
        report.new_vertex_count,
        report.old_index_count,
        report.new_index_count,
        report.stride,
        report.index_size
    );

    let after = morphic::model::decode(&edited).expect("decode edited model");

    // (b) The gun primitive now renders the new buffer.
    let gun_after = after
        .meshes
        .iter()
        .find(|m| m.name == "gun")
        .expect("gun part present");
    assert_eq!(gun_after.primitives.len(), 1, "gun has one draw call");
    assert_eq!(
        gun_after.primitives[0].indices.len(),
        indices.len(),
        "gun index count"
    );
    assert_eq!(
        gun_after.vertex_buffers[0].element_count, vb.element_count,
        "gun vertex count"
    );

    // (c) The gun's geometry is preserved (same-mesh replace): bounds match closely.
    let gun_before = before.meshes.iter().find(|m| m.name == "gun").unwrap();
    let bb = |m: &morphic::model::MeshPart| {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &m.vertex_buffers[0].positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    };
    let (lo0, hi0) = bb(gun_before);
    let (lo1, hi1) = bb(gun_after);
    approx3(lo0, lo1, "gun bbox min");
    approx3(hi0, hi1, "gun bbox max");

    // (d) Skinning round-trips: for each vertex, the highest-weight influence's
    // model bone (decoded through the forward remap) matches the input glb joint.
    let ja = &gun_after.vertex_buffers[0].joints;
    assert_eq!(ja.len(), vb.joints.len(), "joint rows");
    let mut checked = 0usize;
    for (after_j, (in_j, in_w)) in ja.iter().zip(vb.joints.iter().zip(&vb.weights)) {
        // The lane the glb marked as the dominant influence.
        let lane = (0..4).max_by(|&a, &b| in_w[a].total_cmp(&in_w[b])).unwrap();
        if in_w[lane] > 0.0 {
            assert_eq!(
                after_j[lane], in_j[lane],
                "dominant influence bone preserved through remap round-trip"
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "checked some skin influences");

    // (e) The surgery is local: every other part is byte-identical.
    for (mb, ma) in before.meshes.iter().zip(&after.meshes) {
        if mb.name == "gun" {
            continue;
        }
        assert_eq!(mb.name, ma.name, "mesh order");
        for (vbb, vba) in mb.vertex_buffers.iter().zip(&ma.vertex_buffers) {
            assert_eq!(vbb.positions, vba.positions, "{}: positions", mb.name);
            assert_eq!(vbb.normals, vba.normals, "{}: normals", mb.name);
            assert_eq!(vbb.joints, vba.joints, "{}: joints", mb.name);
        }
        for (pb, pa) in mb.primitives.iter().zip(&ma.primitives) {
            assert_eq!(pb.indices, pa.indices, "{}: indices", mb.name);
        }
    }
    eprintln!("replace-gun round-trip OK: gun re-encoded + skinned, other parts untouched");
}

/// T1d-d's headline guarantee: a replacement part of a **different** vertex/index
/// count loads. Builds a tiny synthetic triangle (3 verts / 3 indices) rigidly
/// bound to model bone 0 (present in every palette as local 0), splices it over
/// the gun (11750 v / 76329 idx), and asserts the edited model decodes with the
/// gun now reduced to 3 verts / 3 indices, while body + `ghost_glow` are byte
/// identical. This is the offline analog of the in-game "different count renders"
/// gate. Gated on `MORPHIC_MODEL_VPK`.
#[test]
fn replace_gun_with_different_count_local() {
    let Ok(vpk_path) = std::env::var("MORPHIC_MODEL_VPK") else {
        eprintln!("MORPHIC_MODEL_VPK not set; skipping different-count replace");
        return;
    };
    let entry = std::env::var("MORPHIC_MODEL_ENTRY")
        .unwrap_or_else(|_| "models/heroes_staging/hornet_v3/hornet.vmdl_c".to_string());

    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut vf = vpk.get_file(&entry).expect("entry");
    let bytes = vf.read_all().expect("read");

    // A small triangle near the model origin, rigidly skinned to model bone 0.
    let vb = morphic::model::VertexBuffer {
        element_count: 3,
        positions: vec![[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 4.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        texcoords: vec![vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]],
        joints: vec![[0, 0, 0, 0]; 3],
        weights: vec![[1.0, 0.0, 0.0, 0.0]; 3],
        ..Default::default()
    };
    let indices = vec![0u32, 1, 2];

    let before = morphic::model::decode(&bytes).expect("decode original");
    let (edited, report) =
        morphic::model::replace_mesh_part(&bytes, "gun", &vb, &indices).expect("replace");
    assert_eq!(report.new_vertex_count, 3);
    assert_eq!(report.new_index_count, 3);
    assert_ne!(
        report.old_vertex_count, report.new_vertex_count,
        "count changed"
    );

    let after = morphic::model::decode(&edited).expect("decode edited (different count)");
    let gun = after.meshes.iter().find(|m| m.name == "gun").expect("gun");
    assert_eq!(
        gun.vertex_buffers[0].element_count, 3,
        "gun shrunk to 3 verts"
    );
    assert_eq!(gun.primitives[0].indices.len(), 3, "gun draws one triangle");

    // Body + ghost_glow untouched.
    for (mb, ma) in before.meshes.iter().zip(&after.meshes) {
        if mb.name == "gun" {
            continue;
        }
        for (vbb, vba) in mb.vertex_buffers.iter().zip(&ma.vertex_buffers) {
            assert_eq!(vbb.positions, vba.positions, "{}: positions", mb.name);
        }
    }
    eprintln!(
        "different-count replace OK: gun {} -> 3 verts, other parts intact",
        report.old_vertex_count
    );
}

/// Vertex-color recolor round-trip on a real color-bearing model (Paige's ult
/// horse/knight), end to end: pick a `COLOR`-carrying vertex buffer and recolor
/// it with (a) an identity transform, confirming the edited `.vmdl_c` re-decodes
/// with its colors AND positions byte-identical (the recolor is lossless and
/// never touches geometry), then (b) a channel-swap transform, confirming only
/// the colors changed and positions are still identical. Exercises whichever of
/// the meshopt / uncompressed buffer paths the chosen entry uses. Gated on
/// `MORPHIC_MODEL_VPK`; entry via `MORPHIC_RECOLOR_ENTRY` (default the particle
/// horse/knight, whose color lives in a meshopt buffer, so the re-encode path is
/// exercised by default).
#[test]
fn recolor_vertex_colors_round_trips_local() {
    let Ok(vpk_path) = std::env::var("MORPHIC_MODEL_VPK") else {
        eprintln!("MORPHIC_MODEL_VPK not set; skipping local vertex-color recolor");
        return;
    };
    let entry = std::env::var("MORPHIC_RECOLOR_ENTRY")
        .unwrap_or_else(|_| "models/particle/bookworm_horse_knight.vmdl_c".to_string());

    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let Ok(mut vf) = vpk.get_file(&entry) else {
        eprintln!("entry {entry} not in {vpk_path}; skipping vertex-color recolor");
        return;
    };
    let bytes = vf.read_all().expect("read entry");

    let Some(target) = morphic::model::vertex_targets(&bytes)
        .expect("targets")
        .into_iter()
        .find(|t| t.has_color)
    else {
        eprintln!("{entry} has no color-bearing buffer; skipping");
        return;
    };
    eprintln!(
        "recolor: mesh={} block={} verts={} meshopt={}",
        target.mesh_name, target.block_index, target.vertex_count, target.meshopt
    );

    let orig_colors = morphic::model::read_vertex_colors(&bytes, target.block_index)
        .expect("read colors")
        .expect("buffer carries COLOR");
    // The recolored buffer may or may not expose a float POSITION; capture it
    // when present so we can prove geometry is untouched on either path.
    let orig_pos = morphic::model::read_vertex_positions(&bytes, target.block_index).ok();

    // (a) Identity recolor: colors AND positions byte-identical after re-encode.
    let (ident, lanes) =
        morphic::model::recolor_vertex_buffer(&bytes, target.block_index, |c| c).expect("identity");
    assert!(lanes >= 1, "at least one COLOR lane recolored");
    morphic::model::decode(&ident).expect("identity recolor re-decodes");

    // A meshopt color buffer is converted to uncompressed (re-encoding meshopt is
    // not engine-compatible), so the recolored buffer must read back uncompressed;
    // an already-uncompressed buffer stays uncompressed (in-place byte patch).
    let after_target = morphic::model::vertex_targets(&ident)
        .expect("targets")
        .into_iter()
        .find(|t| t.block_index == target.block_index)
        .expect("buffer still present");
    assert!(
        !after_target.meshopt,
        "recolored color buffer must be uncompressed (was meshopt={})",
        target.meshopt
    );
    let ident_colors = morphic::model::read_vertex_colors(&ident, target.block_index)
        .expect("read")
        .expect("COLOR");
    assert_eq!(ident_colors, orig_colors, "identity recolor is lossless");
    if let Some(p0) = &orig_pos {
        let p1 = morphic::model::read_vertex_positions(&ident, target.block_index).expect("pos");
        assert_eq!(&p1, p0, "identity recolor leaves positions byte-identical");
    }

    // (b) Channel-swap recolor (R<->B): obvious, deterministic change; positions
    // unchanged.
    let (swapped, _) = morphic::model::recolor_vertex_buffer(&bytes, target.block_index, |c| {
        [c[2], c[1], c[0], c[3]]
    })
    .expect("swap");
    morphic::model::decode(&swapped).expect("swap recolor re-decodes");
    let new_colors = morphic::model::read_vertex_colors(&swapped, target.block_index)
        .expect("read")
        .expect("COLOR");
    assert_eq!(
        new_colors.len(),
        orig_colors.len(),
        "vertex count preserved"
    );
    for (o, n) in orig_colors.iter().zip(&new_colors) {
        assert!((n[0] - o[2]).abs() <= 1.0 / 255.0, "R <- B");
        assert!((n[1] - o[1]).abs() <= 1.0 / 255.0, "G unchanged");
        assert!((n[2] - o[0]).abs() <= 1.0 / 255.0, "B <- R");
        assert!((n[3] - o[3]).abs() <= 1.0 / 255.0, "A unchanged");
    }
    if let Some(p0) = &orig_pos {
        let p1 = morphic::model::read_vertex_positions(&swapped, target.block_index).expect("pos");
        assert_eq!(&p1, p0, "swap recolor leaves positions byte-identical");
    }
    eprintln!(
        "vertex-color recolor OK: {} verts, identity lossless + channel-swap applied, geometry untouched",
        orig_colors.len()
    );
}

/// Total LOD0 indices belonging to draw calls whose material matches `needle`.
fn dress_index_count(model: &morphic::model::Model, needle: &str) -> usize {
    let lc = needle.to_ascii_lowercase();
    model
        .meshes
        .iter()
        .flat_map(|m| &m.primitives)
        .filter(|p| p.material.to_ascii_lowercase().contains(&lc))
        .map(|p| p.indices.len())
        .sum()
}

/// Mirrors the GLB writer's shell rule: only the inverted-hull toon-outline /
/// jitter border is dropped. Additive `*_glow` overlays are KEPT now (the viewer
/// composites them additively), so they are no longer shells.
fn is_shell(s: &str) -> bool {
    let lc = s.to_ascii_lowercase();
    lc.contains("outline") || lc.contains("jitter")
}

/// M5a: write the `.glb`, re-read it with the `gltf` crate (which validates the
/// container + accessors), and confirm the structure survived. The file is left
/// at `MORPHIC_GLB_OUT` (default /tmp/hornet.glb) for a maintainer to open in a
/// glTF viewer / Grimoire's three.js and confirm it skins + retargets clips.
fn assert_glb_roundtrip(model: &morphic::model::Model) {
    let glb = morphic::model::to_glb(model).expect("write glb");
    let out = std::env::var("MORPHIC_GLB_OUT").unwrap_or_else(|_| "/tmp/hornet.glb".to_string());
    std::fs::write(&out, &glb).expect("write glb file");
    eprintln!("wrote {} ({} bytes)", out, glb.len());

    let gltf = gltf::Gltf::from_slice(&glb).unwrap_or_else(|e| panic!("re-read glb: {e:?}"));
    assert!(gltf.blob.is_some(), "glb carries its binary blob");
    let doc = &gltf.document;

    // The GLB writer drops parts that are entirely inverted-hull toon-outline /
    // jitter shells, so the glTF carries the model's renderable parts (including
    // the kept additive `*_glow` overlays), not every part. Mirror that rule and
    // check the surviving meshes are exactly those parts (by name).
    let mut expected_meshes: Vec<&str> = model
        .meshes
        .iter()
        .filter(|p| !is_shell(&p.name) && p.primitives.iter().any(|pr| !is_shell(&pr.material)))
        .map(|p| p.name.as_str())
        .collect();
    expected_meshes.sort_unstable();
    let mut glb_mesh_names: Vec<&str> = doc.meshes().filter_map(|m| m.name()).collect();
    glb_mesh_names.sort_unstable();
    assert_eq!(
        glb_mesh_names, expected_meshes,
        "glb keeps exactly the renderable (non-shell) parts"
    );

    let skin = doc.skins().next().expect("glb has a skin");
    assert_eq!(
        skin.joints().count(),
        model.skeleton.bones.len(),
        "glb joint count"
    );

    // Bone names survive (the retarget key).
    let mut glb_bone_names: Vec<String> = skin
        .joints()
        .filter_map(|j| j.name().map(str::to_owned))
        .collect();
    glb_bone_names.sort();
    assert_eq!(
        glb_bone_names,
        model.skeleton.sorted_bone_names(),
        "glb bone names"
    );

    // Every primitive has POSITION + JOINTS_0 and an index accessor.
    let mut prim_total = 0;
    for mesh in doc.meshes() {
        for prim in mesh.primitives() {
            prim_total += 1;
            assert!(
                prim.get(&gltf::Semantic::Positions).is_some(),
                "prim has POSITION"
            );
            assert!(prim.indices().is_some(), "prim has indices");
            assert!(
                prim.get(&gltf::Semantic::Joints(0)).is_some(),
                "skinned prim has JOINTS_0"
            );
        }
    }
    // body (5) + gun (1); the `ghost_glow` shell part (1 prim) is dropped.
    assert_eq!(prim_total, 6, "hornet LOD0 primitive count");
    eprintln!(
        "glb re-read OK: {} meshes, {prim_total} primitives, valid skin",
        glb_mesh_names.len()
    );
}
