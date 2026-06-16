//! Compiled material (`.vmat_c`) parsing, ported from VRF
//! `ResourceTypes/Material.cs` (`Material.Read`). A material's `DATA` block is a
//! KV3 tree of shader name + named parameter tables; the texture parameters name
//! the `.vtex` paths the model's textures live at.
//!
//! morphic stays pure (bytes in): this produces a [`Material`] and a best-effort
//! [`PbrSlots`] name mapping. Resolving those texture paths across VPKs,
//! decoding them (via [`crate::decode`]), and packing them into glTF PBR images
//! happens in the orchestration + GLB-writer layers (M5/M6), which is where
//! VRF's shader-aware channel splitting / ORM packing also belongs. The v1 goal
//! is base color + normal, with the rest exposed "as available".

// KV3 stores material float/vector params as f64-widened f32; narrowing back is
// exact for real material data.
#![allow(clippy::cast_possible_truncation)]

use std::collections::BTreeMap;

use crate::error::DecodeError;
use crate::kv3::{self, Format, Value};
use crate::resource::Resource;
use crate::resource::{build_resource_with_tail, BLOCK_TYPE_DATA};

const RESOURCE_VERSION: u16 = 1;
const BLOCK_TYPE_INSG: [u8; 4] = *b"INSG";
const MATERIAL_KV3_FORMAT: Format = Format([
    0x7c, 0x16, 0x12, 0x74, 0xe9, 0x06, 0x98, 0x46, 0xaf, 0xf2, 0xe6, 0x3e, 0xb5, 0x90, 0x37, 0xe7,
]);

const DEFAULT_AO: &str = "materials/default/default_ao_tga_559f1ac6.vtex";
const DEFAULT_NORMAL_ROUGHNESS: &str = "materials/default/default_normal_tga_7be61377.vtex";
const DEFAULT_MASK: &str = "materials/default/default_mask_tga_344101f8.vtex";
const DEFAULT_BLACK_MASK: &str = "materials/default/default_black_mask_tga_e7be3cc.vtex";
const DEFAULT_TINT_RIM_MASK: &str = "materials/default/default_mask_tga_8d0774e6.vtex";

/// A decoded compiled material.
#[derive(Debug, Clone, Default)]
pub struct Material {
    /// `m_materialName` (e.g. `models/.../vindicta_headv2.vmat`).
    pub name: String,
    /// `m_shaderName` (e.g. `pbr.vfx`).
    pub shader_name: String,
    /// Texture slot -> `.vtex` path (`m_textureParams`).
    pub texture_params: BTreeMap<String, String>,
    /// `m_intParams` (shader feature flags like `F_TRANSLUCENT`, plus others).
    pub int_params: BTreeMap<String, i64>,
    /// `m_floatParams`.
    pub float_params: BTreeMap<String, f32>,
    /// `m_vectorParams` (each a 4-component vector).
    pub vector_params: BTreeMap<String, [f32; 4]>,
}

/// Parameters for the constrained `pbr.vfx` material writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PbrVmatParams {
    /// Source-relative material name, including `.vmat`.
    pub material_name: String,
    /// Source-relative color texture path, including `.vtex`.
    pub color_texture: String,
    /// Representative color texture width recorded as material metadata.
    pub representative_width: u16,
    /// Representative color texture height recorded as material metadata.
    pub representative_height: u16,
}

/// Build a complete `.vmat_c` resource for the generated soul-container PBR
/// subset.
///
/// The `DATA` KV3 payload mirrors the tiny CSDK output shape for generated
/// source VMATs: `pbr.vfx`, the NPR/self-illum/status flags, the color texture
/// plus CSDK default mask slots, tint params, and representative texture
/// dimensions. The resource also carries the static `INSG` shader input
/// signature observed across the soul-container PBR oracle corpus.
pub fn encode_pbr_vmat_c(params: &PbrVmatParams) -> Result<Vec<u8>, DecodeError> {
    let value = pbr_vmat_value(params);
    let data = kv3::encode(&value, &MATERIAL_KV3_FORMAT);
    let insg_value = pbr_input_signature_value();
    let insg = kv3::encode(&insg_value, &MATERIAL_KV3_FORMAT);
    build_resource_with_tail(
        &[
            (BLOCK_TYPE_DATA, data.as_slice()),
            (BLOCK_TYPE_INSG, insg.as_slice()),
        ],
        &[],
        RESOURCE_VERSION,
    )
}

/// Index of a `m_textureParams` entry by its `m_name`, for byte-faithful patching.
fn texture_param_index(data: &Value, slot: &str) -> Option<usize> {
    data.get("m_textureParams")?
        .as_array()?
        .iter()
        .position(|p| p.get("m_name").and_then(Value::as_str) == Some(slot))
}

/// Compile a `pbr.vfx` `.vmat_c` by byte-faithfully patching a compiled donor's
/// `DATA` block with a new `m_materialName` plus texture-slot paths, preserving
/// the donor's v5 KV3 layout and every non-`DATA` block (`RERL`/`RED2`/`INSG`).
///
/// This is the engine-accepted material path, and it is why it differs from
/// [`encode_pbr_vmat_c`]: a full v4 re-encode of material `DATA` renders the red
/// error shader in game; only byte-faithful v5 `DATA` is accepted (proven in game
/// by the soul-container probes). `RERL` is a precache hint, not a render binding,
/// so the donor's reference blocks ride along unchanged while textures resolve by
/// path from the patched `DATA` -- which is why a donor whose `RERL` names absent
/// textures still renders correctly.
///
/// `slot_paths` maps texture-parameter names (`g_tColor`, `g_tNormalRoughness`,
/// ...) to the `.vtex` path each should point at. Every named slot must already
/// exist in the donor (the compiler-shaped donor declares the full `pbr.vfx`
/// slot set); a missing slot is an error rather than a silent no-op. The result
/// is self-gated: it re-decodes and asserts every edit took.
pub fn compile_pbr_vmat(
    donor: &[u8],
    material_name: &str,
    slot_paths: &[(&str, &str)],
) -> Result<Vec<u8>, DecodeError> {
    let data = kv3::decode(Resource::parse(donor)?.data_block()?)?;
    let mut edits: Vec<(Vec<kv3::Seg>, String)> = vec![(
        vec![kv3::Seg::Key("m_materialName".to_string())],
        material_name.to_string(),
    )];
    for (slot, path) in slot_paths {
        let idx = texture_param_index(&data, slot).ok_or_else(|| {
            DecodeError::Material(format!("donor material has no texture slot `{slot}`"))
        })?;
        edits.push((
            vec![
                kv3::Seg::Key("m_textureParams".to_string()),
                kv3::Seg::Index(idx),
                kv3::Seg::Key("m_pValue".to_string()),
            ],
            (*path).to_string(),
        ));
    }
    let patched = crate::patch_kv3_resource_strings_adding(donor, &edits)?;
    // Self-gate: re-decode and assert every edit landed, so a future donor whose
    // DATA layout shifts fails loudly instead of shipping a silently-unpatched
    // material to the engine.
    let check = parse(&patched)?;
    if check.name != material_name {
        return Err(DecodeError::Material(
            "m_materialName patch did not take".to_string(),
        ));
    }
    for (slot, path) in slot_paths {
        if check.texture_params.get(*slot).map(String::as_str) != Some(*path) {
            return Err(DecodeError::Material(format!(
                "texture slot `{slot}` patch did not take"
            )));
        }
    }
    Ok(patched)
}

/// glTF alpha handling derived from the material's feature flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaMode {
    Opaque,
    Mask,
    Blend,
}

/// Best-effort PBR texture slots, resolved from the material's texture
/// parameter names. Source paths only (no decode). The mapping is name-based
/// (not shader-aware like VRF), which covers the common Deadlock hero shaders;
/// `normal` may be a combined normal+roughness texture (`g_tNormalRoughness`),
/// which the GLB writer splits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PbrSlots<'a> {
    pub base_color: Option<&'a str>,
    pub normal: Option<&'a str>,
    pub occlusion: Option<&'a str>,
    pub emissive: Option<&'a str>,
    pub roughness: Option<&'a str>,
    pub metalness: Option<&'a str>,
}

/// Parses a compiled `.vmat_c` resource into a [`Material`].
pub fn parse(bytes: &[u8]) -> Result<Material, DecodeError> {
    let resource = Resource::parse(bytes)?;
    let data = kv3::decode(resource.data_block()?)?;
    Material::from_data(&data)
}

fn pbr_vmat_value(params: &PbrVmatParams) -> Value {
    Value::Object(vec![
        string_pair("m_materialName", &params.material_name),
        string_pair("m_shaderName", "pbr.vfx"),
        (
            "m_intParams".to_string(),
            Value::Array(vec![
                named_int("F_SELF_ILLUM", 1),
                named_int("F_USE_NPR_LIGHTING", 1),
                named_int("F_USE_STATUS_EFFECTS_PROXY", 1),
                named_int("g_bMaskColorTint1", 1),
                named_int("g_bMaskVertexColorTint1", 1),
                named_int("g_flSelfIllumAlbedoFactor1", 1),
                named_int("g_flSelfIllumScale1", 0),
                named_int("g_fVertexColorStrength1", 1),
                named_int("g_nTextureColorTintMode1", 0),
            ]),
        ),
        ("m_floatParams".to_string(), Value::Array(Vec::new())),
        (
            "m_vectorParams".to_string(),
            Value::Array(vec![named_vec4("g_vColorTint1", [1.0, 1.0, 1.0, 0.0])]),
        ),
        (
            "m_textureParams".to_string(),
            Value::Array(vec![
                named_string("g_tAmbientOcclusion", DEFAULT_AO),
                named_string("g_tColor", &params.color_texture),
                named_string("g_tNormalRoughness", DEFAULT_NORMAL_ROUGHNESS),
                named_string("g_tNprOutlineMask", DEFAULT_MASK),
                named_string("g_tNprTransmissiveColor", DEFAULT_BLACK_MASK),
                named_string("g_tSelfIllumMask", DEFAULT_MASK),
                named_string("g_tTintMaskRimLightMask", DEFAULT_TINT_RIM_MASK),
            ]),
        ),
        ("m_dynamicParams".to_string(), Value::Array(Vec::new())),
        (
            "m_dynamicTextureParams".to_string(),
            Value::Array(Vec::new()),
        ),
        (
            "m_intAttributes".to_string(),
            Value::Array(vec![
                named_int(
                    "RepresentativeTextureHeight",
                    i64::from(params.representative_height),
                ),
                named_int(
                    "RepresentativeTextureWidth",
                    i64::from(params.representative_width),
                ),
            ]),
        ),
        ("m_floatAttributes".to_string(), Value::Array(Vec::new())),
        ("m_vectorAttributes".to_string(), Value::Array(Vec::new())),
        ("m_textureAttributes".to_string(), Value::Array(Vec::new())),
        ("m_stringAttributes".to_string(), Value::Array(Vec::new())),
        (
            "m_renderAttributesUsed".to_string(),
            Value::Array(Vec::new()),
        ),
    ])
}

fn pbr_input_signature_value() -> Value {
    Value::Object(vec![
        (
            "m_elems".to_string(),
            Value::Array(vec![
                input_sig_elem("vBlendIndices", "BlendIndices", "BLENDINDICES", 0),
                input_sig_elem("nPackedFrame", "CompressedTangentFrame", "NORMAL", 0),
                input_sig_elem("vNormalOs", "Normal", "NORMAL", 0),
                input_sig_elem("vPositionOs", "PosXyz", "POSITION", 0),
                input_sig_elem("nVertexID", "None", "SV_VertexID", 0),
                input_sig_elem("vTangentUOs_flTangentVSign", "TangentU_SignV", "TANGENT", 0),
                input_sig_elem("vTexCoord", "LowPrecisionUv", "TEXCOORD", 0),
                input_sig_elem("nInstanceIdx", "InstanceTransformUv", "TEXCOORD", 13),
            ]),
        ),
        (
            "m_depth_elems".to_string(),
            Value::Array(vec![
                input_sig_elem("vBlendIndices", "BlendIndices", "BLENDINDICES", 0),
                input_sig_elem("vPositionOs", "PosXyz", "POSITION", 0),
                input_sig_elem("nVertexID", "None", "SV_VertexID", 0),
                input_sig_elem("nInstanceIdx", "InstanceTransformUv", "TEXCOORD", 13),
            ]),
        ),
    ])
}

fn input_sig_elem(name: &str, semantic: &str, d3d_semantic: &str, d3d_index: i64) -> Value {
    Value::Object(vec![
        string_pair("m_pName", name),
        string_pair("m_pSemantic", semantic),
        string_pair("m_pD3DSemanticName", d3d_semantic),
        int_pair("m_nD3DSemanticIndex", d3d_index),
    ])
}

fn named_int(name: &str, value: i64) -> Value {
    Value::Object(vec![
        string_pair("m_name", name),
        int_pair("m_nValue", value),
    ])
}

fn named_string(name: &str, value: &str) -> Value {
    Value::Object(vec![
        string_pair("m_name", name),
        string_pair("m_pValue", value),
    ])
}

fn named_vec4(name: &str, value: [f32; 4]) -> Value {
    Value::Object(vec![
        string_pair("m_name", name),
        (
            "m_value".to_string(),
            Value::Array(
                value
                    .into_iter()
                    .map(|component| Value::Double(f64::from(component)))
                    .collect(),
            ),
        ),
    ])
}

fn string_pair(key: &str, value: &str) -> (String, Value) {
    (key.to_string(), Value::String(value.to_string()))
}

fn int_pair(key: &str, value: i64) -> (String, Value) {
    (key.to_string(), Value::Int(value))
}

impl Material {
    /// Builds a [`Material`] from a parsed `.vmat_c` `DATA` KV3 tree.
    pub fn from_data(data: &Value) -> Result<Material, DecodeError> {
        let mut mat = Material {
            name: string_field(data, "m_materialName"),
            shader_name: string_field(data, "m_shaderName"),
            ..Material::default()
        };

        for kv in array(data, "m_textureParams") {
            if let (Some(name), Some(value)) = (name_of(kv), str_of(kv, "m_pValue")) {
                mat.texture_params.insert(name, value.to_owned());
            }
        }
        for kv in array(data, "m_intParams") {
            if let (Some(name), Some(v)) = (name_of(kv), kv.get("m_nValue").and_then(Value::as_int))
            {
                mat.int_params.insert(name, v);
            }
        }
        for kv in array(data, "m_floatParams") {
            if let (Some(name), Some(v)) =
                (name_of(kv), kv.get("m_flValue").and_then(Value::as_f64))
            {
                mat.float_params.insert(name, v as f32);
            }
        }
        for kv in array(data, "m_vectorParams") {
            if let (Some(name), Some(v)) = (name_of(kv), kv.get("m_value").and_then(vec4)) {
                mat.vector_params.insert(name, v);
            }
        }

        Ok(mat)
    }

    /// The `.vtex` path bound to a texture slot, if present.
    #[must_use]
    pub fn texture(&self, slot: &str) -> Option<&str> {
        self.texture_params.get(slot).map(String::as_str)
    }

    /// Best-effort PBR slot mapping by texture-parameter name.
    #[must_use]
    pub fn pbr(&self) -> PbrSlots<'_> {
        PbrSlots {
            base_color: self.first_texture(&["g_tColor", "g_tColorA", "g_tBaseColor", "g_tAlbedo"]),
            normal: self.first_texture(&["g_tNormal", "g_tNormalRoughness"]),
            occlusion: self.first_texture(&["g_tAmbientOcclusion", "g_tOcclusion"]),
            emissive: self.first_texture(&["g_tSelfIllumMask", "g_tEmissive", "g_tSelfIllum"]),
            roughness: self.first_texture(&["g_tRoughness"]),
            metalness: self.first_texture(&["g_tMetalness", "g_tMetalnessMask"]),
        }
    }

    /// glTF alpha mode from `F_TRANSLUCENT` / `F_ALPHA_TEST` (and `*_glass.vfx`),
    /// matching VRF's `GenerateGLTFMaterialFromRenderMaterial`.
    #[must_use]
    pub fn alpha_mode(&self) -> AlphaMode {
        let translucent = self.int_params.get("F_TRANSLUCENT").copied().unwrap_or(0) > 0
            || self.shader_name.ends_with("_glass.vfx");
        if translucent {
            AlphaMode::Blend
        } else if self.int_params.get("F_ALPHA_TEST").copied().unwrap_or(0) > 0 {
            AlphaMode::Mask
        } else {
            AlphaMode::Opaque
        }
    }

    /// Alpha cutoff for `AlphaMode::Mask` materials (`g_flAlphaTestReference`).
    #[must_use]
    pub fn alpha_cutoff(&self) -> Option<f32> {
        self.float_params.get("g_flAlphaTestReference").copied()
    }

    /// True when this material's shader params say to multiply/use mesh vertex
    /// `COLOR` data. Deadlock uses this path for skin tone and some colored
    /// ability meshes whose albedo texture is neutral.
    #[must_use]
    pub fn uses_vertex_color(&self) -> bool {
        self.int_params.get("F_VERTEX_COLOR").copied().unwrap_or(0) > 0
            || self
                .int_params
                .get("F_PAINT_VERTEX_COLORS")
                .copied()
                .unwrap_or(0)
                > 0
            || self
                .int_params
                .get("g_bMaskVertexColorTint1")
                .copied()
                .unwrap_or(0)
                > 0
            || self
                .int_params
                .get("g_bApplyTintToVertexColors")
                .copied()
                .unwrap_or(0)
                > 0
    }

    fn first_texture(&self, slots: &[&str]) -> Option<&str> {
        slots.iter().find_map(|s| self.texture(s))
    }
}

// --- KV3 field helpers ---

fn string_field(data: &Value, key: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn array<'a>(data: &'a Value, key: &str) -> &'a [Value] {
    data.get(key).and_then(Value::as_array).unwrap_or(&[])
}

fn name_of(kv: &Value) -> Option<String> {
    kv.get("m_name").and_then(Value::as_str).map(str::to_owned)
}

fn str_of<'a>(kv: &'a Value, key: &str) -> Option<&'a str> {
    kv.get(key).and_then(Value::as_str)
}

fn vec4(v: &Value) -> Option<[f32; 4]> {
    let a = v.as_array()?;
    if a.len() < 4 {
        return None;
    }
    Some([
        a[0].as_f64()? as f32,
        a[1].as_f64()? as f32,
        a[2].as_f64()? as f32,
        a[3].as_f64()? as f32,
    ])
}

#[cfg(test)]
mod tests;
