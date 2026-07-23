//! Regression test for the YCoCg icon/recolor replace fix.
//!
//! Replacing a YCoCg-DXT5 texture (item art, some icons) used to splice PLAIN
//! RGB under the texture's retained `YCoCg Conversion` flag, so the engine's
//! inverse transform (mirrored by `decode`) garbled the colors. `replace_mip_chain`
//! now forward-transforms the pixels (`encode_ycocg`) when the template is YCoCg.
//!
//! This exercises the full path against the repo's known YCoCg fixture: splice a
//! solid crimson image in, decode it back (the decoder applies the inverse YCoCg
//! transform because the flag is still set), and assert the color survives. With
//! the old code this comes back as garbage; with the fix it comes back crimson.

// Prose-heavy test docs mention `YCoCg` repeatedly; the acronym trips the
// pedantic doc_markdown lint without adding clarity as inline code.
#![allow(clippy::doc_markdown)]

use morphic::{decode, inspect, replace_mip_chain, Image, ImageData, TextureFormat};

const FIXTURE: &[u8] = include_bytes!("../fixtures/dxt5/radiant_regeneration_psd_ycocg.vtex_c");

/// Strong-chroma test color: a YCoCg error shifts it dramatically.
const CRIMSON: [u8; 4] = [220, 30, 40, 255];

#[test]
fn ycocg_template_replace_keeps_colors() {
    let info = inspect(FIXTURE).expect("inspect fixture");
    assert_eq!(info.format, TextureFormat::Dxt5, "fixture must be DXT5");
    assert!(
        info.ycocg,
        "fixture must be YCoCg (else this proves nothing)"
    );

    // Solid crimson at the template's STORED dims (replace_mip_chain requires
    // new_mip0 to match info.width/height; padding region is filled too).
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    let crimson = Image {
        width: w,
        height: h,
        data: ImageData::Rgba8(
            CRIMSON
                .iter()
                .copied()
                .cycle()
                .take((w * h * 4) as usize)
                .collect(),
        ),
    };

    let out = replace_mip_chain(FIXTURE, &crimson).expect("splice crimson into YCoCg template");
    let decoded = decode(&out).expect("decode the spliced result");
    let ImageData::Rgba8(px) = &decoded.data else {
        panic!("expected rgba8");
    };

    // Check the actual (non-padded) region: every pixel should read back crimson
    // within DXT5 tolerance. A YCoCg-flag/plain-RGB mismatch would blow this out
    // (channels off by tens to >100), so a tight bound is a real discriminator.
    let (aw, ah) = (
        u32::from(info.actual_width) as usize,
        u32::from(info.actual_height) as usize,
    );
    let stride = decoded.width as usize;
    let (mut max_err, mut worst) = (0i32, [0u8; 3]);
    for y in 0..ah {
        for x in 0..aw {
            let i = (y * stride + x) * 4;
            for c in 0..3 {
                let d = (i32::from(px[i + c]) - i32::from(CRIMSON[c])).abs();
                if d > max_err {
                    max_err = d;
                    worst = [px[i], px[i + 1], px[i + 2]];
                }
            }
        }
    }
    assert!(
        max_err <= 12,
        "decoded color drifted too far from crimson (max channel err {max_err}, worst pixel {worst:?}); \
         a large error means the YCoCg forward transform is wrong or not applied"
    );
}
