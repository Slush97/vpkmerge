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
mod resource;
mod texture;

pub use edit::{replace_face0_mip0, replace_face_mip, replace_face_mip_chain, replace_mip_chain};
pub use error::{DecodeError, EncodeError};
pub use texture::{
    decode::decode_image,
    encode::encode_image,
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
