use morphic::{decode_at, DecodeOptions, ImageData, TextureFlags};

// Minimal synthetic RGBA8 resource. Each mip/slice/face has its own solid tag.
fn texture(flags: TextureFlags, depth: u16) -> Vec<u8> {
    let mut bytes = vec![0u8; 80];
    bytes[4..6].copy_from_slice(&12u16.to_le_bytes());
    bytes[6..8].copy_from_slice(&1u16.to_le_bytes());
    bytes[8..12].copy_from_slice(&8u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&1u32.to_le_bytes());
    bytes[16..20].copy_from_slice(b"DATA");
    bytes[20..24].copy_from_slice(&12u32.to_le_bytes());
    bytes[24..28].copy_from_slice(&48u32.to_le_bytes());
    bytes[32..34].copy_from_slice(&1u16.to_le_bytes());
    bytes[34..36].copy_from_slice(&flags.bits().to_le_bytes());
    bytes[52..54].copy_from_slice(&8u16.to_le_bytes());
    bytes[54..56].copy_from_slice(&8u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&depth.to_le_bytes());
    bytes[58] = 4; // RGBA8888
    bytes[59] = 4; // mip count
    let faces = if flags.contains(TextureFlags::CUBE_TEXTURE) {
        6
    } else {
        1
    };
    for mip in (0u8..4).rev() {
        let side = 8usize >> mip;
        let slices = if flags.contains(TextureFlags::VOLUME_TEXTURE) {
            (depth >> mip).max(1)
        } else {
            depth
        };
        for slice in 0..slices {
            for face in 0..faces {
                let tag = 10 + mip * 40 + u8::try_from(slice).unwrap() * 6 + face;
                for _ in 0..side * side {
                    bytes.extend_from_slice(&[tag, tag + 1, tag + 2, 255]);
                }
            }
        }
    }
    let size = u32::try_from(bytes.len()).unwrap();
    bytes[0..4].copy_from_slice(&size.to_le_bytes());
    bytes
}

#[test]
fn decodes_every_slice_and_face_at_every_mip() {
    for flags in [
        TextureFlags::TEXTURE_ARRAY,
        TextureFlags::TEXTURE_ARRAY | TextureFlags::CUBE_TEXTURE,
        TextureFlags::VOLUME_TEXTURE,
    ] {
        let bytes = texture(flags, 4);
        let faces = if flags.contains(TextureFlags::CUBE_TEXTURE) {
            6
        } else {
            1
        };
        for mip in 0u8..4 {
            let slices = if flags.contains(TextureFlags::VOLUME_TEXTURE) {
                (4u16 >> mip).max(1)
            } else {
                4
            };
            for slice in 0..slices {
                for face in 0..faces {
                    let image = decode_at(&bytes, &DecodeOptions { mip, slice, face }).unwrap();
                    let ImageData::Rgba8(pixels) = image.data else {
                        panic!("expected RGBA8")
                    };
                    let tag = 10 + mip * 40 + u8::try_from(slice).unwrap() * 6 + face;
                    assert!(
                        pixels
                            .chunks_exact(4)
                            .all(|p| p == [tag, tag + 1, tag + 2, 255]),
                        "{flags:?} mip {mip} slice {slice} face {face}"
                    );
                }
            }
            assert!(decode_at(
                &bytes,
                &DecodeOptions {
                    mip,
                    slice: slices,
                    face: 0
                }
            )
            .is_err());
        }
    }
}
