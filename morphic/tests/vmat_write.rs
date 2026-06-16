use morphic::{
    compile_pbr_vmat, decode_kv3_resource, encode_pbr_vmat_c, kv3::Value, material, PbrVmatParams,
};

/// A committed clean compiled `pbr.vfx` donor (psyduck `tooncuerpoo.vmat_c`):
/// RERL + RED2 + DATA + INSG, no dynamic-expression blob.
const DONOR: &[u8] = include_bytes!("../fixtures/soul/soul_material_donor.vmat_c");

#[test]
fn compiles_pbr_vmat_from_donor_template() {
    let name = "models/props_gameplay/soul_container/materials/piplup.vmat";
    let color = "models/props_gameplay/soul_container/materials/piplup_color.vtex";
    let normal = "materials/default/default_normal_tga_7be61377.vtex";

    let bytes = compile_pbr_vmat(
        DONOR,
        name,
        &[("g_tColor", color), ("g_tNormalRoughness", normal)],
    )
    .expect("compile vmat from donor");

    // The patched material carries our content...
    let mat = material::parse(&bytes).expect("parse compiled vmat");
    assert_eq!(mat.name, name);
    assert_eq!(mat.shader_name, "pbr.vfx");
    assert_eq!(mat.texture("g_tColor"), Some(color));
    assert_eq!(mat.texture("g_tNormalRoughness"), Some(normal));

    // ...while every non-DATA block rides along byte-faithfully. This is the
    // engine-accepted path (a v4 re-encode renders the red error shader and would
    // not preserve RERL/RED2/INSG): the donor's four blocks must all survive.
    assert_eq!(
        block_kinds(DONOR),
        vec![*b"RERL", *b"RED2", *b"DATA", *b"INSG"]
    );
    let mut kinds = block_kinds(&bytes);
    kinds.sort_unstable();
    let mut expected = vec![*b"RERL", *b"RED2", *b"DATA", *b"INSG"];
    expected.sort_unstable();
    assert_eq!(kinds, expected, "all donor blocks preserved");
    assert_eq!(
        block_payload(&bytes, *b"RERL"),
        block_payload(DONOR, *b"RERL"),
        "RERL preserved verbatim (precache hint rides along)"
    );
    assert_eq!(
        block_payload(&bytes, *b"INSG"),
        block_payload(DONOR, *b"INSG"),
        "INSG preserved verbatim"
    );
}

#[test]
fn compile_pbr_vmat_rejects_unknown_slot() {
    let err = compile_pbr_vmat(DONOR, "x.vmat", &[("g_tNotARealSlot", "y.vtex")])
        .expect_err("unknown slot must error");
    assert!(
        err.to_string().contains("g_tNotARealSlot"),
        "error names the missing slot: {err}"
    );
}

#[test]
fn writes_generated_pbr_vmat() {
    let bytes = encode_pbr_vmat_c(&PbrVmatParams {
        material_name: "models/props_gameplay/soul_container/materials/piplup.vmat".to_string(),
        color_texture:
            "models/props_gameplay/soul_container/materials/piplup_color_png_deadbeef.vtex"
                .to_string(),
        representative_width: 2,
        representative_height: 2,
    })
    .expect("encode vmat");

    let mat = material::parse(&bytes).expect("parse generated vmat");
    assert_eq!(
        mat.name,
        "models/props_gameplay/soul_container/materials/piplup.vmat"
    );
    assert_eq!(mat.shader_name, "pbr.vfx");

    assert_eq!(mat.int_params.get("F_SELF_ILLUM"), Some(&1));
    assert_eq!(mat.int_params.get("F_USE_NPR_LIGHTING"), Some(&1));
    assert_eq!(mat.int_params.get("F_USE_STATUS_EFFECTS_PROXY"), Some(&1));
    assert_eq!(mat.int_params.get("g_bMaskColorTint1"), Some(&1));
    assert_eq!(mat.int_params.get("g_bMaskVertexColorTint1"), Some(&1));
    assert_eq!(mat.int_params.get("g_nTextureColorTintMode1"), Some(&0));
    assert_eq!(mat.float_params.len(), 0);
    assert_eq!(
        mat.vector_params.get("g_vColorTint1"),
        Some(&[1.0, 1.0, 1.0, 0.0])
    );

    assert_eq!(
        mat.texture("g_tColor"),
        Some("models/props_gameplay/soul_container/materials/piplup_color_png_deadbeef.vtex")
    );
    assert_eq!(
        mat.texture("g_tAmbientOcclusion"),
        Some("materials/default/default_ao_tga_559f1ac6.vtex")
    );
    assert_eq!(
        mat.texture("g_tNormalRoughness"),
        Some("materials/default/default_normal_tga_7be61377.vtex")
    );
    assert_eq!(
        mat.texture("g_tSelfIllumMask"),
        Some("materials/default/default_mask_tga_344101f8.vtex")
    );
    assert!(mat.uses_vertex_color());
    assert_eq!(mat.alpha_mode(), material::AlphaMode::Opaque);

    let data = decode_kv3_resource(&bytes).expect("decode generated vmat KV3");
    let attrs = data
        .get("m_intAttributes")
        .and_then(Value::as_array)
        .expect("m_intAttributes");
    assert_eq!(named_int(attrs, "RepresentativeTextureWidth"), Some(2));
    assert_eq!(named_int(attrs, "RepresentativeTextureHeight"), Some(2));

    assert_eq!(block_kinds(&bytes), vec![*b"DATA", *b"INSG"]);
    let insg = morphic::kv3::decode(block_payload(&bytes, *b"INSG").expect("INSG payload"))
        .expect("decode INSG");
    assert_eq!(
        insg.get("m_elems")
            .and_then(Value::as_array)
            .map(<[Value]>::len),
        Some(8)
    );
    assert_eq!(
        insg.get("m_depth_elems")
            .and_then(Value::as_array)
            .map(<[Value]>::len),
        Some(4)
    );
}

fn named_int(values: &[Value], name: &str) -> Option<i64> {
    values.iter().find_map(|value| {
        let this_name = value.get("m_name").and_then(Value::as_str)?;
        (this_name == name).then(|| value.get("m_nValue").and_then(Value::as_int))?
    })
}

fn block_kinds(bytes: &[u8]) -> Vec<[u8; 4]> {
    let table = 8 + u32le(bytes, 8) as usize;
    let count = u32le(bytes, 12) as usize;
    (0..count)
        .map(|i| {
            let row = table + i * 12;
            bytes[row..row + 4].try_into().expect("block kind")
        })
        .collect()
}

fn block_payload(bytes: &[u8], kind: [u8; 4]) -> Option<&[u8]> {
    let table = 8 + u32le(bytes, 8) as usize;
    let count = u32le(bytes, 12) as usize;
    for i in 0..count {
        let row = table + i * 12;
        if bytes.get(row..row + 4)? != kind.as_slice() {
            continue;
        }
        let offset = row + 4 + u32le(bytes, row + 4) as usize;
        let size = u32le(bytes, row + 8) as usize;
        return bytes.get(offset..offset + size);
    }
    None
}

fn u32le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("u32"))
}
