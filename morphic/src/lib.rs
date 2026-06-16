//! Pure-Rust Source 2 `.vtex_c` texture decoder.
//!
//! The decoder is built up format-by-format; the supported list lives in
//! [`TextureFormat`]. Unimplemented formats return [`DecodeError::Unimplemented`]
//! rather than panicking, so callers can probe with [`inspect`] before
//! committing to a decode.
//!
//! ```no_run
//! let bytes = std::fs::read("hero_diffuse.vtex_c").unwrap();
//! let info = morphic::inspect(&bytes).unwrap();
//! println!("{:?} {}x{}", info.format, info.width, info.height);
//! ```

use std::path::Path;

mod edit;
mod error;
pub mod kv3;
pub mod material;
mod meshopt;
pub mod model;
mod resource;
pub mod sound;
mod texture;
pub mod vfx_expr;

pub use edit::{replace_face0_mip0, replace_face_mip, replace_face_mip_chain, replace_mip_chain};
pub use error::{DecodeError, EncodeError};
pub use material::{compile_pbr_vmat, encode_pbr_vmat_c, PbrVmatParams};
pub use sound::{encode_vsnd_c, VsndParams};
pub use texture::{
    decode::decode_image,
    encode::encode_image,
    encode_vtex_png_rgba8888, encode_vtex_png_rgba8888_from_png,
    format::{TextureFlags, TextureFormat},
    parse_texture_header, Image, ImageData, TextureInfo,
};

/// Decode options for [`decode_at`]. Defaults select mip 0, slice 0, face 0.
#[derive(Clone, Copy, Debug, Default)]
pub struct DecodeOptions {
    pub mip: u8,
    pub slice: u16,
    pub face: u8,
}

/// Cheap header read: parses the resource container and the texture binary
/// header without touching pixel data.
pub fn inspect(bytes: &[u8]) -> Result<TextureInfo, DecodeError> {
    let resource = resource::Resource::parse(bytes)?;
    let data = resource.data_block()?;
    parse_texture_header(data)
}

/// Decode the top mip of the first slice/face. Convenience entry point.
pub fn decode(bytes: &[u8]) -> Result<Image, DecodeError> {
    decode_at(bytes, &DecodeOptions::default())
}

/// Decode a specific mip/slice/face.
pub fn decode_at(bytes: &[u8], opts: &DecodeOptions) -> Result<Image, DecodeError> {
    let resource = resource::Resource::parse(bytes)?;
    let data = resource.data_block()?;
    let info = parse_texture_header(data)?;
    let pixels = texture::pixel_data(&resource, &info, *opts)?;
    decode_image(&info, pixels, opts)
}

/// Open a VPK and decode the given entry. Convenience for the GUI preview path.
pub fn decode_from_vpk<P: AsRef<Path>>(_vpk: P, _entry: &str) -> Result<Image, DecodeError> {
    Err(DecodeError::Unimplemented(TextureFormat::Unknown))
}

/// Decode the binary KV3 `DATA` block of a Source 2 resource file (e.g.
/// `.vsndevts_c`) into a [`kv3::Value`] tree.
pub fn decode_kv3_resource(file_bytes: &[u8]) -> Result<kv3::Value, DecodeError> {
    let resource = resource::Resource::parse(file_bytes)?;
    let data = resource.data_block()?;
    kv3::decode(data)
}

/// The raw bytes of a resource's KV3 `DATA` block (as stored, possibly
/// LZ4-compressed inside the KV3 framing).
///
/// # Errors
/// Fails when the container does not parse or has no `DATA` block.
pub fn kv3_resource_data_block(file_bytes: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(file_bytes)?;
    Ok(resource.data_block()?.to_vec())
}

/// Whether a resource's KV3 `DATA` block carries a binary-blob section
/// (`countBlocks > 0`). A blobbed block must not be re-emitted uncompressed
/// (the engine misreads the blob framing), so callers use this to choose between
/// the byte-faithful in-place patch and a full re-encode.
pub fn kv3_resource_has_blobs(file_bytes: &[u8]) -> Result<bool, DecodeError> {
    let resource = resource::Resource::parse(file_bytes)?;
    let data = resource.data_block()?;
    // countBlocks is the i32 at block offset 56 for KV3 v2..=5.
    if data.len() < 60 {
        return Ok(false);
    }
    Ok(i32::from_le_bytes([data[56], data[57], data[58], data[59]]) != 0)
}

/// Re-encode `value` into the `DATA` block of `original`, keeping the original
/// KV3 format GUID and every non-DATA block (e.g. `RED2`) byte-for-byte. The new
/// `DATA` is uncompressed KV3 v4. Returns a complete, loadable resource file.
pub fn encode_kv3_resource(original: &[u8], value: &kv3::Value) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let data = resource.data_block()?;
    let format = kv3::Format::from_payload(data)?;
    let new_data = kv3::encode(value, &format);
    resource.rebuild_with_data(&new_data)
}

/// Patch integer scalar fields of a resource's KV3 `DATA` block in place by
/// path, preserving every other byte: value flags, auxiliary-buffer typed-array
/// tags, and the v5 framing the engine's particle/model loaders require.
///
/// This is the byte-faithful alternative to [`encode_kv3_resource`], which
/// rebuilds the `DATA` from a [`kv3::Value`] tree and so drops flags and typed
/// tags (fine for soundevents, fatal for particles/models). Use this to retint a
/// particle's `m_ConstantColor` / gradient `m_Color` channels without
/// invalidating its resource references. Edits and their path/width contract are
/// exactly [`kv3::set_scalars`]'s. Returns a complete, loadable resource file.
pub fn patch_kv3_resource_scalars(
    original: &[u8],
    edits: &[(Vec<kv3::Seg>, i64)],
) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let data = resource.data_block()?;
    let new_data = kv3::set_scalars(data, edits)?;
    resource.rebuild_with_data(&new_data)
}

/// Patch `DOUBLE` (f64) fields of a resource's KV3 `DATA` block in place by path,
/// preserving every other byte. The double sibling of [`patch_kv3_resource_scalars`],
/// built to retint a material's `g_vColorTint` RGBA vector in a `.vmat_c` without a
/// lossy re-encode. Edits and their path contract are exactly [`kv3::set_doubles`]'s.
pub fn patch_kv3_resource_doubles(
    original: &[u8],
    edits: &[(Vec<kv3::Seg>, f64)],
) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let data = resource.data_block()?;
    let new_data = kv3::set_doubles(data, edits)?;
    resource.rebuild_with_data(&new_data)
}

/// Patch `FLOAT` (f32) fields of a resource's KV3 `DATA` block in place by path,
/// preserving every other byte. This is the particle-safe path for probing
/// existing brightness/radius/lifetime-style controls without a lossy full
/// particle re-encode.
pub fn patch_kv3_resource_floats(
    original: &[u8],
    edits: &[(Vec<kv3::Seg>, f32)],
) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let data = resource.data_block()?;
    let new_data = kv3::set_floats(data, edits)?;
    resource.rebuild_with_data(&new_data)
}

/// Replace a binary-blob value in a resource's KV3 `DATA` block in place,
/// preserving every other byte. `old` is the current blob bytes (located by exact
/// content, so it must occur once) and `new` must be the same length. Built to
/// write a re-encoded `m_compressedPoseData` stream back into a `.vnmclip_c`, the
/// blob sibling of [`patch_kv3_resource_floats`]; see [`kv3::set_blob`].
pub fn patch_kv3_resource_blob(
    original: &[u8],
    old: &[u8],
    new: &[u8],
) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let data = resource.data_block()?;
    let new_data = kv3::set_blob(data, old, new)?;
    resource.rebuild_with_data(&new_data)
}

/// Replace the sole binary blob of a resource's KV3 `DATA` block with `new` of any
/// length up to one LZ4 frame (16 KB). The length-changing sibling of
/// [`patch_kv3_resource_blob`], for re-encoding a `.vnmclip_c` whose pose stream
/// grew or shrank (a changed animated-channel set). See [`kv3::set_sole_blob`].
pub fn patch_kv3_resource_sole_blob(original: &[u8], new: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let data = resource.data_block()?;
    let new_data = kv3::set_sole_blob(data, new)?;
    resource.rebuild_with_data(&new_data)
}

/// Patch `BOOLEAN` fields of a resource's KV3 `DATA` block in place by path. The
/// bool sibling of [`patch_kv3_resource_scalars`]; used to flip a track's
/// `m_bIsRotationStatic` when re-encoding an NM clip that animates a
/// previously-static bone. Edits and their path contract are [`kv3::set_bools`]'s.
pub fn patch_kv3_resource_bools(
    original: &[u8],
    edits: &[(Vec<kv3::Seg>, bool)],
) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let data = resource.data_block()?;
    let new_data = kv3::set_bools(data, edits)?;
    resource.rebuild_with_data(&new_data)
}

/// Patch `STRING` fields of a resource's KV3 `DATA` block in place by path by
/// redirecting the field to an already-interned string table value. This does not
/// add strings or change the KV3 structure, so it is suitable for conservative
/// enum probes such as existing particle input modes/types.
pub fn patch_kv3_resource_strings(
    original: &[u8],
    edits: &[(Vec<kv3::Seg>, String)],
) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let data = resource.data_block()?;
    let new_data = kv3::set_strings(data, edits)?;
    resource.rebuild_with_data(&new_data)
}

/// Patch `STRING` fields of a resource's KV3 `DATA` block by path, **adding** any
/// target string not already in the string table (unlike
/// [`patch_kv3_resource_strings`], which can only redirect to an interned value).
///
/// This is the structural-edit primitive that unlocks animated VFX: it lets a
/// gradient's `m_FloatInterp/m_nType` be set to `PF_TYPE_COLLECTION_AGE` and
/// `m_nInputMode` to `PF_INPUT_MODE_LOOPED` even on particles whose string table
/// lacks those enums, so the recolored spectrum cycles over time. The string table
/// is grown byte-faithfully ([`kv3::set_strings_adding`]); every other byte is
/// preserved, so the engine's particle loader accepts the result. Returns a
/// complete, loadable resource file. v5 only for the append; a v4 block succeeds
/// only when every target is already interned.
pub fn patch_kv3_resource_strings_adding(
    original: &[u8],
    edits: &[(Vec<kv3::Seg>, String)],
) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let data = resource.data_block()?;
    let new_data = kv3::set_strings_adding(data, edits)?;
    resource.rebuild_with_data(&new_data)
}

/// Insert one element into a KV3 array inside a resource's `DATA` block,
/// byte-faithfully preserving the existing typed lanes and adding any strings the
/// inserted subtree needs.
///
/// This is the structural primitive used for particle operator insertion: it
/// appends missing key/value strings to the KV3 string table, serializes only the
/// new element, splices those bytes into the existing b1/b2/b4/b8/type/object
/// streams at the walked array cursor, bumps the array length/header counts, and
/// rebuilds the resource with the resized `DATA` block. It deliberately avoids a
/// full KV3 re-encode, which is lossy for compiled particles.
pub fn patch_kv3_resource_array_insert(
    original: &[u8],
    array_path: &[kv3::Seg],
    index: usize,
    value: &kv3::Value,
) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let data = resource.data_block()?;
    let new_data = kv3::insert_array_element_adding(data, array_path, index, value)?;
    resource.rebuild_with_data(&new_data)
}
