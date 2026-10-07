//! M4 validation: parse the committed `.vmat_c` fixture and diff it against the
//! oracle golden (`morphic-oracle material-meta`, wrapping `ValveResourceFormat`).
//! Covers the shader name, every parameter table, and the name-based PBR slot
//! mapping morphic exposes for the GLB writer.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

use super::{parse, AlphaMode};

#[derive(Deserialize)]
struct Golden {
    name: String,
    shader_name: String,
    texture_params: BTreeMap<String, String>,
    int_params: BTreeMap<String, i64>,
    float_params: BTreeMap<String, f32>,
    vector_params: BTreeMap<String, [f32; 4]>,
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/material")
}

#[test]
fn vindicta_headv2_matches_golden() {
    let dir = fixture_dir();
    let bytes = std::fs::read(dir.join("vindicta_headv2.vmat_c")).expect("read vmat_c fixture");
    let golden: Golden = serde_json::from_str(
        &std::fs::read_to_string(dir.join("vindicta_headv2.material.json")).expect("read golden"),
    )
    .expect("parse golden");

    let mat = parse(&bytes).expect("parse material");

    assert_eq!(mat.name, golden.name, "material name");
    assert_eq!(mat.shader_name, golden.shader_name, "shader name");
    assert_eq!(mat.texture_params, golden.texture_params, "texture params");
    assert_eq!(mat.int_params, golden.int_params, "int params");

    assert_eq!(
        mat.float_params.keys().collect::<Vec<_>>(),
        golden.float_params.keys().collect::<Vec<_>>(),
        "float param names"
    );
    for (k, v) in &mat.float_params {
        let g = golden.float_params[k];
        assert!((v - g).abs() < 1e-5, "float param {k}: {v} vs {g}");
    }

    assert_eq!(
        mat.vector_params.keys().collect::<Vec<_>>(),
        golden.vector_params.keys().collect::<Vec<_>>(),
        "vector param names"
    );
    for (k, v) in &mat.vector_params {
        let g = golden.vector_params[k];
        for i in 0..4 {
            assert!(
                (v[i] - g[i]).abs() < 1e-5,
                "vector param {k}[{i}]: {} vs {}",
                v[i],
                g[i]
            );
        }
    }
}

#[test]
fn pbr_slots_map_by_name() {
    let bytes = std::fs::read(fixture_dir().join("vindicta_headv2.vmat_c")).expect("read fixture");
    let mat = parse(&bytes).expect("parse material");
    let pbr = mat.pbr();

    // pbr.vfx names: g_tColor / g_tNormalRoughness / g_tAmbientOcclusion /
    // g_tSelfIllumMask. No standalone roughness/metalness texture here.
    assert_eq!(pbr.base_color, mat.texture("g_tColor"), "base color slot");
    assert_eq!(pbr.normal, mat.texture("g_tNormalRoughness"), "normal slot");
    assert_eq!(
        pbr.occlusion,
        mat.texture("g_tAmbientOcclusion"),
        "occlusion slot"
    );
    assert_eq!(
        pbr.emissive,
        mat.texture("g_tSelfIllumMask"),
        "emissive slot"
    );
    assert_eq!(pbr.roughness, None, "no standalone roughness");
    assert_eq!(pbr.metalness, None, "no standalone metalness");

    assert!(pbr.base_color.is_some_and(|p| p.contains(".vtex")));

    // No F_TRANSLUCENT / F_ALPHA_TEST on this material.
    assert_eq!(mat.alpha_mode(), AlphaMode::Opaque, "alpha mode");
    assert!(mat.uses_vertex_color(), "skin material uses vertex color");
}

#[test]
fn decodes_dynamic_params_with_failure_capture() {
    use super::Material;
    use crate::kv3::Value;

    fn obj(pairs: Vec<(&str, Value)>) -> Value {
        Value::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    // A real, compilable expression -> bytecode -> the m_value blob the engine
    // stores. Mirrors the CLAUDE.md example proven byte-identical to Valve's.
    let src = "$ent_health<.4?float3(1,.1,.1):float3(1,1,1)";
    let good = crate::vfx_expr::compile(src).expect("compile sample expr");

    let data = obj(vec![
        ("m_materialName", Value::String("test".into())),
        ("m_shaderName", Value::String("pbr.vfx".into())),
        (
            "m_renderAttributesUsed",
            Value::Array(vec![Value::String("$ent_health".into())]),
        ),
        (
            "m_dynamicParams",
            Value::Array(vec![
                obj(vec![
                    ("m_name", Value::String("g_vColorTint1".into())),
                    ("m_value", Value::Binary(good.bytecode.clone())),
                ]),
                // empty blob -> decompile fails cleanly; the failure must be
                // captured, never panicked or masqueraded as a static value.
                obj(vec![
                    ("m_name", Value::String("g_flBroken".into())),
                    ("m_value", Value::Binary(Vec::new())),
                ]),
            ]),
        ),
    ]);

    let mat = Material::from_data(&data).expect("from_data");
    assert_eq!(mat.render_attributes_used, vec!["$ent_health".to_owned()]);

    let ok = &mat.dynamic_params["g_vColorTint1"];
    assert!(ok.decompiled, "good expr decompiles");
    assert!(ok.error.is_none());
    assert_eq!(ok.byte_len, good.bytecode.len());
    assert!(
        ok.attributes.contains(&"$ent_health".to_owned()),
        "attrs from source"
    );
    // decompiled source recompiles to the same bytecode (the codec contract).
    assert_eq!(
        crate::vfx_expr::compile(&ok.source)
            .expect("recompile")
            .bytecode,
        good.bytecode,
        "round-trip"
    );

    let bad = &mat.dynamic_params["g_flBroken"];
    assert!(!bad.decompiled, "failure is reported, not guessed");
    assert!(bad.error.is_some());
    assert_eq!(bad.source, "");
    assert_eq!(bad.byte_len, 0);
    assert!(!bad.hash.is_empty(), "blob hash present even on failure");
}
