use morphic::{decode_stored_mips, ImageData};

fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

// Synthetic resources keep enumeration tests independent of game files.
#[allow(clippy::cast_possible_truncation)]
fn texture(width: u16, height: u16, levels: u8, compressed: bool) -> Vec<u8> {
    let extra = if compressed {
        24 + usize::from(levels) * 4
    } else {
        0
    };
    let data_size = 40 + extra;
    let mut bytes = vec![0; 28 + data_size];
    put16(&mut bytes, 4, 12);
    put16(&mut bytes, 6, 1);
    put32(&mut bytes, 8, 8);
    put32(&mut bytes, 12, 1);
    bytes[16..20].copy_from_slice(b"DATA");
    put32(&mut bytes, 20, 8);
    put32(&mut bytes, 24, data_size as u32);
    let header = &mut bytes[28..];
    put16(header, 0, 1);
    put16(header, 20, width);
    put16(header, 22, height);
    put16(header, 24, 1);
    header[26] = 4;
    header[27] = levels;
    if compressed {
        put32(header, 32, 8);
        put32(header, 36, 1);
        put32(header, 40, 4);
        put32(header, 44, 8);
        put32(header, 48, (12 + usize::from(levels) * 4) as u32);
        put32(header, 52, 1);
        put32(header, 56, 8);
        put32(header, 60, u32::from(levels));
    }
    for mip in (0..levels).rev() {
        let count = usize::from((width >> mip).max(1)) * usize::from((height >> mip).max(1));
        let pixels = [mip, 47, 123, 13].repeat(count);
        let packed = lz4_flex::block::compress(&pixels);
        let stored = if compressed && packed.len() < pixels.len() {
            packed
        } else {
            pixels
        };
        if compressed {
            put32(
                &mut bytes,
                28 + 64 + usize::from(mip) * 4,
                stored.len() as u32,
            );
        }
        bytes.extend_from_slice(&stored);
    }
    put32(&mut bytes, 0, (28 + data_size) as u32);
    bytes
}

#[test]
fn enumerates_only_stored_levels_and_ignores_trailing_bytes() {
    for compressed in [false, true] {
        let mut bytes = texture(8, 4, 3, compressed);
        bytes.extend_from_slice(b"unrelated trailing data");
        let levels = decode_stored_mips(&bytes).unwrap();
        assert_eq!(levels.len(), 3); // Partial chain stops at 2x1, not 1x1.
        for (mip, image) in levels.iter().enumerate() {
            assert_eq!((image.width, image.height), (8 >> mip, (4 >> mip).max(1)));
            let ImageData::Rgba8(rgba) = &image.data else {
                panic!("RGBA8");
            };
            assert!(rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| *p == [u8::try_from(mip).unwrap(), 47, 123, 13]));
        }
    }
}

#[test]
fn no_lod_does_not_synthesize_levels() {
    let mut bytes = texture(8, 4, 1, false);
    put16(&mut bytes, 30, 8);
    assert_eq!(decode_stored_mips(&bytes).unwrap().len(), 1);
}

#[test]
fn bc3_alpha_rounding_and_sub_block_mip_tails_match_vrf() {
    check_bc3_alpha(255, 0, [255, 0, 219, 182, 146, 109, 73, 36]);
    check_bc3_alpha(0, 255, [0, 255, 51, 102, 153, 204, 0, 255]);
}

fn check_bc3_alpha(a: u8, b: u8, expected: [u8; 8]) {
    let mut bytes = texture(8, 8, 4, false);
    bytes.truncate(68);
    bytes[54] = 2;
    let mut block = [0u8; 16];
    block[0] = a;
    block[1] = b;
    block[8..10].copy_from_slice(&u16::MAX.to_le_bytes());
    let indices = (0..16).fold(0u64, |bits, pixel| bits | ((pixel % 8) << (pixel * 3)));
    block[2..8].copy_from_slice(&indices.to_le_bytes()[..6]);
    for count in [1, 1, 1, 4] {
        bytes.extend_from_slice(&block.repeat(count));
    }
    let levels = decode_stored_mips(&bytes).unwrap();
    assert_eq!(
        levels
            .iter()
            .map(|i| (i.width, i.height))
            .collect::<Vec<_>>(),
        [(8, 8), (4, 4), (2, 2), (1, 1)]
    );
    let ImageData::Rgba8(rgba) = &levels[1].data else {
        panic!("RGBA8");
    };
    let alpha: Vec<_> = rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
    assert_eq!(alpha, expected.repeat(2));
    assert!(rgba
        .as_chunks::<4>()
        .0
        .iter()
        .all(|p| p[..3] == [255, 255, 255]));
}

#[test]
fn bc1_three_color_mode_keeps_vrf_opaque_black() {
    let mut bytes = texture(4, 4, 1, false);
    bytes.truncate(68);
    bytes[54] = 1;
    // Black and white endpoints, all pixels use three-color mode index 3.
    bytes.extend_from_slice(&[0, 0, 255, 255, 255, 255, 255, 255]);
    let levels = decode_stored_mips(&bytes).unwrap();
    let ImageData::Rgba8(rgba) = &levels[0].data else {
        panic!("RGBA8");
    };
    assert_eq!(rgba, &[0, 0, 0, 255].repeat(16));
    // The existing preview retains its punch-through alpha behavior.
    let preview = morphic::decode(&bytes).unwrap();
    let ImageData::Rgba8(rgba) = preview.data else {
        panic!("RGBA8");
    };
    assert_eq!(rgba, [0, 0, 0, 0].repeat(16));
}

#[test]
fn inline_png_preserves_unpremultiplied_channels() {
    let image = morphic::Image {
        width: 2,
        height: 1,
        data: ImageData::Rgba8(vec![217, 9, 43, 0, 128, 254, 37, 13]),
    };
    let bytes = morphic::encode_vtex_png_rgba8888(&image, morphic::TextureFlags::NO_LOD).unwrap();
    let levels = decode_stored_mips(&bytes).unwrap();
    assert_eq!(levels.len(), 1);
    let ImageData::Rgba8(rgba) = &levels[0].data else {
        panic!("RGBA8");
    };
    assert_eq!(rgba, &[217, 9, 43, 0, 128, 254, 37, 13]);
}

#[test]
fn crops_display_rectangle_at_each_mip() {
    let original = texture(8, 4, 3, false);
    let mut bytes = original[..68].to_vec();
    bytes.extend_from_slice(&[0; 18]);
    bytes.extend_from_slice(&original[68..]);
    put32(&mut bytes, 24, 58);
    put32(&mut bytes, 60, 8);
    put32(&mut bytes, 64, 1);
    put32(&mut bytes, 68, 3);
    put32(&mut bytes, 72, 8);
    put32(&mut bytes, 76, 6);
    put16(&mut bytes, 82, 6);
    put16(&mut bytes, 84, 3);
    let levels = decode_stored_mips(&bytes).unwrap();
    assert_eq!(
        levels
            .iter()
            .map(|i| (i.width, i.height))
            .collect::<Vec<_>>(),
        [(6, 3), (3, 1), (1, 1)]
    );
    put16(&mut bytes, 82, 1);
    put16(&mut bytes, 84, 1);
    let levels = decode_stored_mips(&bytes).unwrap();
    assert_eq!((levels[0].width, levels[0].height), (8, 1));
}

#[test]
fn rejects_truncated_mips_and_invalid_compression_tables() {
    let mut bytes = texture(8, 4, 3, true);
    bytes.pop();
    assert!(decode_stored_mips(&bytes).is_err());
    let mut bytes = texture(8, 4, 3, true);
    put32(&mut bytes, 88, 2);
    assert!(decode_stored_mips(&bytes).is_err());
    let mut bytes = texture(8, 4, 3, false);
    bytes[55] = 0;
    assert!(decode_stored_mips(&bytes).is_err());
}
