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
pub mod resource;
pub mod sound;
mod texture;
pub mod vfx_expr;

pub use edit::{
    replace_face0_mip0, replace_face_mip, replace_face_mip_chain, replace_mip_chain,
    resize_mip_chain,
};
pub use error::{DecodeError, EncodeError};
pub use material::{compile_pbr_vmat, encode_pbr_vmat_c, PbrVmatParams};
pub use sound::{
    encode_vsnd_c, encode_vsnd_pcm16_c, extract_vsnd_audio, extract_vsnd_mp3, VsndAudio, VsndParams,
};
pub use texture::{
    crop_to_actual,
    decode::decode_image,
    decode_stored_mips,
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
    let mut info = parse_texture_header(data)?;
    info.ycocg = texture::detect_ycocg(&resource);
    Ok(info)
}

/// Decode the top mip of the first slice/face. Convenience entry point.
pub fn decode(bytes: &[u8]) -> Result<Image, DecodeError> {
    decode_at(bytes, &DecodeOptions::default())
}

/// Decode a specific mip/slice/face.
pub fn decode_at(bytes: &[u8], opts: &DecodeOptions) -> Result<Image, DecodeError> {
    let resource = resource::Resource::parse(bytes)?;
    let data = resource.data_block()?;
    let mut info = parse_texture_header(data)?;
    info.ycocg = texture::detect_ycocg(&resource);
    let pixels = texture::pixel_data(&resource, &info, *opts)?;
    decode_image(&info, pixels, opts)
}

/// Decode a specific mip/slice/face of an R16 texture to raw u16 texels,
/// row-major, top-left origin, at `max(1, dim >> mip)`. Errors with
/// `Unimplemented` for any other format.
pub fn decode_r16_at(bytes: &[u8], opts: &DecodeOptions) -> Result<Vec<u16>, DecodeError> {
    let resource = resource::Resource::parse(bytes)?;
    let data = resource.data_block()?;
    let info = parse_texture_header(data)?;
    let pixels = texture::pixel_data(&resource, &info, *opts)?;
    texture::decode::decode_r16_texels(&info, pixels, opts.mip)
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

/// Rebuild a resource container with its raw `DATA` block replaced, preserving
/// every non-DATA block byte-for-byte.
pub fn replace_resource_data_block(
    original: &[u8],
    new_data: &[u8],
) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    resource.rebuild_with_data(new_data)
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

/// Re-encode `value` into the `DATA` block of `original` as **KV3 v5**, taking
/// every node's wire shape (typed-array framing, numeric widths, value flags,
/// string-table order) from the original `DATA` wherever the paths line up
/// (see [`kv3::encode_v5_like`]). Every non-DATA block is kept byte-for-byte.
///
/// Unlike [`encode_kv3_resource`] (uncompressed v4, typed framing and flags
/// dropped), this keeps what the engine binds natively, so it is the full
/// re-encode for model and animation data. Blob-bearing blocks stay LZ4 with
/// Valve's per-blob framing.
pub fn encode_kv3_resource_like(
    original: &[u8],
    value: &kv3::Value,
) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let template = kv3::decode_lossless(resource.data_block()?)?;
    let new_data = kv3::encode_v5_like(value, &template)?;
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
/// complete, loadable resource file. Both v4 and v5 append, and a blobbed v5
/// block stays LZ4-compressed; a blobbed v4 block is refused.
pub fn patch_kv3_resource_strings_adding(
    original: &[u8],
    edits: &[(Vec<kv3::Seg>, String)],
) -> Result<Vec<u8>, DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let data = resource.data_block()?;
    let new_data = kv3::set_strings_adding(data, edits)?;
    resource.rebuild_with_data(&new_data)
}

/// Redirect resource references of a KV3 resource (`.vpcf_c`, `.vmat_c`, ...) from
/// old names to new ones, keeping its `RERL` precache list consistent. Returns the
/// rebuilt file and the number of KV3 string fields redirected.
///
/// `map` holds `(old, new)` resource names exactly as KV3 and `RERL` store them:
/// lowercase and uncompiled (`particles/x.vpcf`, `materials/y.vmat`,
/// `materials/z.vtex`). `old` matches ASCII case-insensitively; `new` is
/// lowercased, as Valve's compiler stores every name. Every `DATA` string equal to
/// an `old` is redirected through [`kv3::set_strings_adding`], so the edit is
/// byte-faithful (v4, v5 and blobbed v5 alike; a blobbed v4 block is refused) and
/// the string keeps its resource flag. An `old` that no `DATA` string names is an
/// error ([`DecodeError::MissingReference`]), never a silent no-op.
///
/// `RERL` then follows the redirect: an `old` no `DATA` string still names has its
/// entry rewritten to `new` (id and name), one that is still named elsewhere keeps
/// its entry and `new` is appended, and a `new` the list already holds is never
/// duplicated. The list stays sorted by name, which is how Valve emits it. A
/// resource without a `RERL` block gets the `DATA` redirect only. For a model,
/// whose `RERL` also covers references held in blocks other than `DATA`, an
/// entry only those blocks still use would be rewritten too.
///
/// `RED2` (and legacy `REDI`) is left as is. It is compiler edit info, not a load
/// binding: `.vpcf_c` files carry an empty `m_ChildResourceList` whatever they
/// reference, and `.vmat_c` files mirror their `RERL` there (sorted, one entry
/// per use), an order a redirect would break without a re-encode. The engine does
/// not read it (materials built on a donor's stale `RED2` render in game).
///
/// Every byte outside the redirected string ids, the grown string table, the
/// `RERL` block and the container offsets is preserved.
pub fn repoint_resource_refs(
    original: &[u8],
    map: &[(String, String)],
) -> Result<(Vec<u8>, usize), DecodeError> {
    let resource = resource::Resource::parse(original)?;
    let data = resource.data_block()?;
    let mut fields = Vec::new();
    string_fields(&kv3::decode(data)?, &mut Vec::new(), &mut fields);

    let pairs: Vec<(String, String)> = map
        .iter()
        .map(|(old, new)| (old.to_ascii_lowercase(), new.to_ascii_lowercase()))
        .collect();
    let mut after: Vec<&str> = fields.iter().map(|(_, value)| value.as_str()).collect();
    let mut edits = Vec::new();
    for (old, new) in &pairs {
        let before = edits.len();
        for (i, (path, value)) in fields.iter().enumerate() {
            if value.eq_ignore_ascii_case(old) {
                after[i] = new;
                edits.push((path.clone(), new.clone()));
            }
        }
        if edits.len() == before {
            return Err(DecodeError::MissingReference(old.clone()));
        }
    }
    let new_data = kv3::set_strings_adding(data, &edits)?;

    let blocks = resource.blocks();
    let data_idx = blocks
        .iter()
        .position(|b| b.kind == resource::BLOCK_TYPE_DATA)
        .ok_or(DecodeError::MissingDataBlock)?;
    let mut replacements: Vec<(usize, &[u8])> = vec![(data_idx, &new_data)];
    let rerl_idx = blocks.iter().position(|b| b.kind == *b"RERL");
    let new_rerl;
    if let Some(idx) = rerl_idx {
        let block = resource
            .get_block_by_index(idx)
            .ok_or(DecodeError::BadResource("RERL block out of range"))?;
        let mut refs = resource::read_rerl(block)?;
        for (old, new) in &pairs {
            let still_named = after.iter().any(|value| value.eq_ignore_ascii_case(old));
            let old_at = refs.iter().position(|(_, n)| n.eq_ignore_ascii_case(old));
            let has_new = refs.iter().any(|(_, n)| n.eq_ignore_ascii_case(new));
            match (old_at, has_new, still_named) {
                (Some(i), false, false) => refs[i] = (resource::resource_id(new), new.clone()),
                (Some(i), true, false) => {
                    refs.remove(i);
                }
                (_, false, _) => refs.push((resource::resource_id(new), new.clone())),
                (_, true, _) => {}
            }
        }
        refs.sort_by(|a, b| a.1.cmp(&b.1));
        new_rerl = resource::write_rerl(&refs);
        replacements.push((idx, &new_rerl));
    }
    Ok((resource.rebuild_with_blocks(&replacements)?, edits.len()))
}

fn string_fields(
    value: &kv3::Value,
    path: &mut Vec<kv3::Seg>,
    out: &mut Vec<(Vec<kv3::Seg>, String)>,
) {
    match value {
        kv3::Value::String(s) => out.push((path.clone(), s.clone())),
        kv3::Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                path.push(kv3::Seg::Index(i));
                string_fields(item, path, out);
                path.pop();
            }
        }
        kv3::Value::Object(members) => {
            for (key, member) in members {
                path.push(kv3::Seg::Key(key.clone()));
                string_fields(member, path, out);
                path.pop();
            }
        }
        _ => {}
    }
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
