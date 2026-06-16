use morphic::{
    decode, encode_image, encode_vtex_png_rgba8888, encode_vtex_png_rgba8888_from_png, inspect,
    Image, ImageData, TextureFlags, TextureFormat,
};

fn sample_image() -> Image {
    Image {
        width: 2,
        height: 2,
        data: ImageData::Rgba8(vec![
            255, 0, 0, 255, //
            0, 255, 0, 255, //
            0, 0, 255, 255, //
            255, 255, 255, 128,
        ]),
    }
}

fn assert_same_rgba8(a: &Image, b: &Image) {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let ImageData::Rgba8(a_pixels) = &a.data else {
        panic!("left image is not RGBA8");
    };
    let ImageData::Rgba8(b_pixels) = &b.data else {
        panic!("right image is not RGBA8");
    };
    assert_eq!(a_pixels, b_pixels);
}

#[test]
fn writes_png_rgba8888_vtex_from_image() {
    let image = sample_image();
    let vtex = encode_vtex_png_rgba8888(&image, TextureFlags::empty()).expect("write vtex");

    let info = inspect(&vtex).expect("inspect generated vtex");
    assert_eq!(info.format, TextureFormat::PngRgba8888);
    assert_eq!((info.width, info.height), (2, 2));
    assert_eq!(info.depth, 1);
    assert_eq!(info.mip_count, 1);
    assert_eq!(info.flags, TextureFlags::empty());

    let decoded = decode(&vtex).expect("decode generated vtex");
    assert_same_rgba8(&image, &decoded);
}

#[test]
fn writes_png_rgba8888_vtex_from_existing_png_bytes() {
    let image = sample_image();
    let png = encode_image(&image, TextureFormat::PngRgba8888).expect("encode source png");
    let vtex = encode_vtex_png_rgba8888_from_png(&png, TextureFlags::NO_LOD).expect("write vtex");

    assert!(
        vtex.ends_with(&png),
        "source PNG should be copied into the VTEX payload unchanged"
    );

    let info = inspect(&vtex).expect("inspect generated vtex");
    assert_eq!(info.format, TextureFormat::PngRgba8888);
    assert_eq!((info.width, info.height), (2, 2));
    assert!(info.flags.contains(TextureFlags::NO_LOD));

    let decoded = decode(&vtex).expect("decode generated vtex");
    assert_same_rgba8(&image, &decoded);
}
