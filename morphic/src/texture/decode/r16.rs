use byteorder::{ByteOrder, LittleEndian};

use crate::error::DecodeError;
use crate::texture::TextureInfo;

/// R16: one little-endian u16 UNORM per texel, row-major, top-left origin
/// (VRF `DecodeR16`). Values come back raw, without normalization, so no
/// precision is lost.
pub fn decode_r16(info: &TextureInfo, pixels: &[u8]) -> Result<Vec<u16>, DecodeError> {
    let count = usize::from(info.width) * usize::from(info.height);
    let needed = count * 2;
    if pixels.len() < needed {
        return Err(DecodeError::Truncated {
            offset: 0,
            needed,
            had: pixels.len(),
        });
    }
    let mut out = vec![0u16; count];
    LittleEndian::read_u16_into(&pixels[..needed], &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::format::{TextureFlags, TextureFormat};

    fn info(w: u16, h: u16) -> TextureInfo {
        TextureInfo {
            format: TextureFormat::R16,
            width: w,
            height: h,
            actual_width: w,
            actual_height: h,
            depth: 1,
            mip_count: 1,
            flags: TextureFlags::empty(),
            reflectivity: [0.0; 4],
            ycocg: false,
        }
    }

    #[test]
    fn r16_reads_little_endian_texels_raw() {
        let pixels = [0x00, 0x00, 0xff, 0xff, 0x34, 0x12, 0x01, 0x80, 0xee];
        let texels = decode_r16(&info(2, 2), &pixels).unwrap();
        assert_eq!(texels, [0, 65535, 0x1234, 0x8001]);
        assert!(matches!(
            decode_r16(&info(2, 2), &pixels[..7]),
            Err(DecodeError::Truncated { needed: 8, .. })
        ));
    }
}
