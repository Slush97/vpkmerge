//! Hue-recolor a Source 2 texture (`.vtex_c`) in place.
//!
//! Built for the Deadlock ability-VFX recolor (see
//! `grimoire/docs/ability-vfx-recolor.md`). Most ability effects recolor by
//! patching the particle `.vpcf_c` color params, but a few render with a
//! texture that carries baked chroma (the ult dragon's color map, a hero's
//! projectile self-illum). Those need their own hue shift: the particle param
//! only multiplies over the texture, so the new hue comes out muddy.
//!
//! The transform decodes the texture's top mip, sets every pixel's hue to a
//! target and scales its saturation/brightness (see [`Recolor`]), then re-encodes
//! the full mip chain in the texture's own format via
//! [`morphic::replace_mip_chain`].
//! Packing the result at the base entry path overrides the texture in place,
//! with no `.vmat_c` edit (sidestepping Source 2's content-hashed texture
//! rename).
//!
//! Hue is *set* (absolute), not rotated, to match the particle recolor: a hue
//! slider lands the dragon, the projectile, and the particle params all on the
//! same color. Neutral pixels (saturation 0: white highlights, black shadows)
//! stay neutral, since their chroma is zero regardless of hue.

use anyhow::{Context, Result};
use morphic::{Image, ImageData, TextureFormat};

/// A recolor target: the absolute hue to set, plus saturation and brightness
/// scales applied on top of each source color.
///
/// Hue alone can't express a color like "light blue": pale/pastel is the
/// saturation + brightness axes, not hue. So a target carries both scales:
///
/// - `saturation == 1.0` keeps each source pixel's own saturation (the original
///   hue-only behavior). `> 1.0` boosts it, so the pale, low-saturation areas a
///   hue-only recolor leaves looking "drowned out" read as the picked color;
///   `< 1.0` mutes it toward a pastel.
/// - `value == 1.0` keeps each source pixel's own brightness. `> 1.0` lightens,
///   `< 1.0` darkens (a deep/ink color).
///
/// Both are *scales*, not absolutes, so the light-to-dark gradient and the
/// highlight/shadow structure survive (a flat retint would lose them). A neutral
/// pixel (saturation 0) stays neutral at any hue or saturation scale, since
/// scaling zero chroma is still zero.
///
/// Shared by all three recolor mechanisms (texture, model vertex colors, particle
/// params) so one target lands them on the same color.
#[derive(Debug, Clone, Copy)]
pub struct Recolor {
    /// Absolute target hue in degrees (taken mod 360).
    pub hue: f64,
    /// Saturation multiplier (clamped to a valid range per pixel). 1.0 keeps the
    /// source saturation.
    pub saturation: f64,
    /// Brightness (HSV value) multiplier (clamped per pixel). 1.0 keeps the
    /// source brightness.
    pub value: f64,
}

impl Recolor {
    /// A hue-only recolor (source saturation + brightness unchanged): the
    /// original behavior.
    #[must_use]
    pub fn hue(hue: f64) -> Self {
        Self {
            hue,
            saturation: 1.0,
            value: 1.0,
        }
    }

    /// A hue plus saturation- and brightness-scale recolor.
    #[must_use]
    pub fn new(hue: f64, saturation: f64, value: f64) -> Self {
        Self {
            hue,
            saturation,
            value,
        }
    }
}

/// At-a-glance shape of a `.vtex_c`, for a human-readable recolor summary
/// (the CLI doesn't depend on `morphic`, so it can't read `TextureInfo`).
#[derive(Debug, Clone)]
pub struct TextureSummary {
    /// Pixel format name (e.g. `Bc7`, `Dxt5`), matching VRF's `VTexFormat`.
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub mip_count: u8,
}

/// Parse a `.vtex_c` header into a [`TextureSummary`] without decoding pixels.
pub fn inspect_texture(vtex_bytes: &[u8]) -> Result<TextureSummary> {
    let info = morphic::inspect(vtex_bytes).context("reading texture header")?;
    Ok(TextureSummary {
        format: format!("{:?}", info.format),
        width: u32::from(info.width),
        height: u32::from(info.height),
        mip_count: info.mip_count,
    })
}

/// Decode the top mip of a `.vtex_c` and apply `recolor` to every pixel (set hue,
/// scale saturation/brightness), returning the recolored image *without*
/// re-encoding.
///
/// This is the fast path for a live UI preview: it skips the lossy `BCn`
/// re-encode that [`recolor_texture_hue`] does, so a hue slider can repaint
/// without recompressing the whole mip chain every tick.
///
/// Only LDR (8-bit) textures are supported; the Deadlock color maps this
/// targets are all LDR. An HDR (f16) texture returns an error rather than a
/// silently-wrong result, since HSV on linear f16 is not the same transform.
pub fn recolor_texture_image(vtex_bytes: &[u8], recolor: Recolor) -> Result<Image> {
    let mut image = morphic::decode(vtex_bytes).context("decoding texture top mip")?;
    shift_hue_in_place(&mut image, recolor)?;
    Ok(image)
}

/// Produce a new `.vtex_c` recolored to `hue_deg`: decode the top mip, set its
/// hue, then re-encode the full mip chain in the texture's own format.
///
/// The returned bytes are a complete, loadable resource. Pack them at the
/// source texture's entry path and the addon overrides the base texture in
/// place. Re-encoding a `BCn` texture is lossy (the pixels are recompressed),
/// which is fine for a recolor: the chroma is what we changed on purpose.
pub fn recolor_texture_hue(vtex_bytes: &[u8], recolor: Recolor) -> Result<Vec<u8>> {
    let recolored = recolor_texture_image(vtex_bytes, recolor)?;
    morphic::replace_mip_chain(vtex_bytes, &recolored)
        .context("re-encoding recolored texture mip chain")
}

/// Recolor the top mip to `hue_deg` and encode it as a PNG, for an
/// eyeball/preview before committing the (lossy, slower) full re-encode. This
/// is the design-intent color (the recolored pixels straight off the decode),
/// not the post-`BCn`-recompression result.
pub fn recolor_texture_preview_png(vtex_bytes: &[u8], recolor: Recolor) -> Result<Vec<u8>> {
    let image = recolor_texture_image(vtex_bytes, recolor)?;
    morphic::encode_image(&image, TextureFormat::PngRgba8888)
        .context("encoding recolor preview PNG")
}

/// What a [`recolor_model_vertex_colors`] pass touched, for a human-readable
/// summary.
#[derive(Debug, Clone, Default)]
pub struct ModelRecolorStats {
    /// Vertex buffers that had at least one `COLOR` lane rewritten.
    pub buffers_recolored: usize,
    /// Total `COLOR` attributes rewritten across all buffers (usually one per
    /// buffer, but a buffer can carry more).
    pub color_lanes: usize,
    /// Vertices across the recolored buffers (the count whose tint changed).
    pub vertices: usize,
}

/// Recolor every baked per-vertex `COLOR` of a model's mesh buffers per
/// `recolor` (set hue, scale saturation/brightness), and return the new
/// `.vmdl_c` bytes.
///
/// This is the third color mechanism behind the Deadlock ability-VFX recolor:
/// some effects (Paige's ult horse/knight) bake their green into the mesh's
/// per-vertex color, which neither the particle `.vpcf_c` param edit nor the
/// `.vtex_c` recolor reaches (a material tint only multiplies, so it cannot turn
/// green into purple). The transform reuses the texture/particle [`set_hue`], so
/// the same hue value lands the model, the textures, and the particles on one
/// color. Neutral vertices (saturation 0) stay neutral.
///
/// Only the `COLOR` lane of each affected vertex buffer is rewritten and
/// re-encoded; positions, normals, UVs, and skin weights are byte-preserved.
pub fn recolor_model_vertex_colors(
    vmdl_bytes: &[u8],
    recolor: Recolor,
) -> Result<(Vec<u8>, ModelRecolorStats)> {
    let targets =
        morphic::model::vertex_targets(vmdl_bytes).context("reading model vertex buffers")?;
    let color_blocks: Vec<(usize, usize)> = targets
        .iter()
        .filter(|t| t.has_color)
        .map(|t| (t.block_index, t.vertex_count))
        .collect();

    // The same display-space color set as the texture/particle recolor: vertex
    // colors are 8-bit unorm, so round-tripping each channel through the u8
    // `set_color` is lossless and keeps all three recolor paths on one transform.
    let transform = |c: [f32; 4]| -> [f32; 4] {
        let rgb = [
            channel(f64::from(c[0])),
            channel(f64::from(c[1])),
            channel(f64::from(c[2])),
        ];
        let [r, g, b] = set_color(rgb, recolor.hue, recolor.saturation, recolor.value);
        [
            f32::from(r) / 255.0,
            f32::from(g) / 255.0,
            f32::from(b) / 255.0,
            c[3], // alpha preserved, as in the texture path
        ]
    };

    let mut bytes = vmdl_bytes.to_vec();
    let mut stats = ModelRecolorStats::default();
    for (block, vertex_count) in &color_blocks {
        let (new_bytes, lanes) =
            morphic::model::recolor_vertex_buffer(&bytes, *block, transform)
                .with_context(|| format!("recoloring vertex buffer at block {block}"))?;
        bytes = new_bytes;
        if lanes > 0 {
            stats.buffers_recolored += 1;
            stats.color_lanes += lanes;
            stats.vertices += vertex_count;
        }
    }
    Ok((bytes, stats))
}

/// Set every RGB pixel's hue to `hue_deg`, leaving alpha untouched. Operates on
/// the image's stored 8-bit channels (the same display-space values the
/// particle `Color32` recolor edits), so the two paths stay consistent.
fn shift_hue_in_place(image: &mut Image, recolor: Recolor) -> Result<()> {
    match &mut image.data {
        ImageData::Rgba8(buf) => {
            for px in buf.as_chunks_mut::<4>().0 {
                let [r, g, b] = set_color(
                    [px[0], px[1], px[2]],
                    recolor.hue,
                    recolor.saturation,
                    recolor.value,
                );
                px[0] = r;
                px[1] = g;
                px[2] = b;
                // px[3] (alpha) is intentionally preserved.
            }
            Ok(())
        }
        ImageData::Rgba16F(_) => anyhow::bail!(
            "hue recolor supports LDR (8-bit) textures only, but this one is HDR (f16); \
             Deadlock color maps are LDR, so this usually means the wrong entry path"
        ),
    }
}

/// One pixel: set its hue to `hue_deg` (taken mod 360), scale its saturation by
/// `sat_scale`, and scale its brightness (HSV value) by `val_scale` (both clamped
/// to a valid range). `sat_scale == val_scale == 1.0` reproduces [`set_hue`];
/// raising saturation lifts pale areas toward the target color, raising value
/// lightens. A neutral pixel (saturation 0) stays neutral at any hue or saturation
/// scale, since scaling zero chroma is still zero. Shared across all three recolor
/// mechanisms (texture, model vertex colors, and particle params via
/// [`crate::hero_recolor`]) so one target lands them on a single color.
#[allow(clippy::many_single_char_names)]
pub(crate) fn set_color(rgb: [u8; 3], hue_deg: f64, sat_scale: f64, val_scale: f64) -> [u8; 3] {
    let (_, s, v) = rgb_to_hsv(
        f64::from(rgb[0]) / 255.0,
        f64::from(rgb[1]) / 255.0,
        f64::from(rgb[2]) / 255.0,
    );
    let s2 = (s * sat_scale).clamp(0.0, 1.0);
    let v2 = (v * val_scale).clamp(0.0, 1.0);
    let (r, g, b) = hsv_to_rgb(hue_deg.rem_euclid(360.0), s2, v2);
    [channel(r), channel(g), channel(b)]
}

/// The absolute brand color to **stamp** onto a material tint, as linear 0..1
/// RGB: the picked hue at `recolor`'s saturation and value, here read as
/// **absolutes** (clamped to 0..1), not the per-pixel scales [`set_color`] uses.
///
/// A material's `g_vColorTint` / `g_vSelfIllumTint` is a single flat effect color,
/// and the reference recolor mods stamp one brand color on every tint, including
/// neutral white ones. A saturation-preserving recolor can't colorize a white
/// tint (zero chroma scales to zero), so the material path stamps an absolute hue
/// instead. The default `Recolor::hue(h)` (saturation = value = 1.0) stamps a
/// vivid, fully-saturated hue; dial saturation/value down for a paler stamp.
#[allow(clippy::many_single_char_names)]
pub(crate) fn stamp_rgb(recolor: Recolor) -> [f64; 3] {
    let s = recolor.saturation.clamp(0.0, 1.0);
    let v = recolor.value.clamp(0.0, 1.0);
    let (r, g, b) = hsv_to_rgb(recolor.hue.rem_euclid(360.0), s, v);
    [r, g, b]
}

/// One pixel: set its hue to `hue_deg`, keeping saturation and value. Equivalent
/// to [`set_color`] with saturation and value scales of `1.0`. Kept as the
/// hue-only convenience the recolor work was first proven against (the tests that
/// pin the documented in-game colors call it); production paths go through
/// [`set_color`] via [`Recolor`].
#[allow(dead_code)]
pub(crate) fn set_hue(rgb: [u8; 3], hue_deg: f64) -> [u8; 3] {
    set_color(rgb, hue_deg, 1.0, 1.0)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn channel(x: f64) -> u8 {
    (x * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Standard RGB (0..1) -> HSV (hue in degrees, s/v in 0..1).
// `max` is by construction exactly one of `r`/`g`/`b`, so the `==` branch picks
// are exact, not approximate; this is the canonical RGB->HSV form.
#[allow(clippy::float_cmp, clippy::many_single_char_names)]
fn rgb_to_hsv(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

/// Standard HSV (hue in degrees, s/v in 0..1) -> RGB (0..1).
#[allow(clippy::many_single_char_names)]
fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (f64, f64, f64) {
    let c = v * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    #[allow(clippy::cast_possible_truncation)]
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (r1 + m, g1 + m, b1 + m)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The one committed fixture with real chroma (a yellow/orange flare);
    // the dev gradients and icons all decode to grayscale.
    const DXT1_CHROMA: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../morphic/fixtures/dxt1/yellowflare.vtex_c"
    );
    const BC7_COLOR: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../morphic/fixtures/bc7/gradient_dev_02_color_psd_73660177.vtex_c"
    );
    const BC6H_HDR: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../morphic/fixtures/bc6h/sky_l4d_c1_2_hdr_cube_pfm_b562e1cd.vtex_c"
    );

    /// The documented particle example: a fully-saturated green at hue 280
    /// lands on the same purple the in-game-verified recolor produced.
    #[test]
    fn set_hue_matches_documented_example() {
        assert_eq!(set_hue([0, 255, 148], 280.0), [170, 0, 255]);
    }

    /// Hue is taken mod 360, so 280 and 640 are the same color.
    #[test]
    fn hue_wraps_modulo_360() {
        assert_eq!(set_hue([0, 255, 148], 280.0), set_hue([0, 255, 148], 640.0));
    }

    /// A neutral pixel (saturation 0) is unchanged by any hue: white stays
    /// white, mid-gray stays mid-gray.
    #[test]
    fn neutral_pixels_unchanged() {
        for gray in [0u8, 64, 128, 200, 255] {
            assert_eq!(set_hue([gray, gray, gray], 200.0), [gray, gray, gray]);
        }
    }

    /// A saturation scale > 1 lifts a pale, low-saturation pixel toward the
    /// target color (more chroma); a value scale adjusts brightness; the hue
    /// still lands on target; and a neutral pixel stays neutral no matter the
    /// saturation scale.
    #[test]
    fn saturation_and_value_scales_keep_neutral_and_hue() {
        // A pale, washed pixel: low saturation, high value.
        let pale = [200u8, 170, 210];
        let (_, s0, v0) = rgb_to_hsv(
            f64::from(pale[0]) / 255.0,
            f64::from(pale[1]) / 255.0,
            f64::from(pale[2]) / 255.0,
        );
        // Saturation up, brightness held: more chroma, same value, on-target hue.
        let boosted = set_color(pale, 280.0, 2.0, 1.0);
        let (hb, sb, vb) = rgb_to_hsv(
            f64::from(boosted[0]) / 255.0,
            f64::from(boosted[1]) / 255.0,
            f64::from(boosted[2]) / 255.0,
        );
        assert!(sb > s0, "saturation should rise: {s0} -> {sb}");
        assert!((vb - v0).abs() <= 0.02, "value should hold: {v0} -> {vb}");
        let dh = (hb - 280.0)
            .rem_euclid(360.0)
            .min((280.0 - hb).rem_euclid(360.0));
        assert!(dh <= 4.0, "hue {hb} not near target 280 (d={dh})");

        // Brightness down darkens (value drops) without touching saturation.
        let darker = set_color(pale, 280.0, 1.0, 0.5);
        let (_, sd, vd) = rgb_to_hsv(
            f64::from(darker[0]) / 255.0,
            f64::from(darker[1]) / 255.0,
            f64::from(darker[2]) / 255.0,
        );
        assert!(vd < v0, "value should drop: {v0} -> {vd}");
        assert!(
            (sd - s0).abs() <= 0.02,
            "saturation should hold: {s0} -> {sd}"
        );

        // Unit scales are exactly the hue-only behavior.
        assert_eq!(set_color(pale, 280.0, 1.0, 1.0), set_hue(pale, 280.0));

        // Neutral pixels carry no chroma, so any saturation scale leaves the hue
        // untouched (value can still scale, but a gray stays gray under x1).
        for gray in [0u8, 128, 255] {
            assert_eq!(
                set_color([gray, gray, gray], 200.0, 3.0, 1.0),
                [gray, gray, gray]
            );
        }
    }

    /// Recoloring the decoded image sets hue to target while preserving each
    /// pixel's saturation and value (checked on the strongly-chromatic pixels,
    /// where hue is well-defined).
    #[test]
    fn recolor_image_sets_hue_keeps_sat_and_value() {
        let bytes = std::fs::read(DXT1_CHROMA).expect("fixture present");
        let original = morphic::decode(&bytes).expect("decode original");
        let target = 200.0;
        let recolored = recolor_texture_image(&bytes, Recolor::hue(target)).expect("recolor");

        assert_eq!(
            (recolored.width, recolored.height),
            (original.width, original.height)
        );
        let (ImageData::Rgba8(orig), ImageData::Rgba8(new)) = (&original.data, &recolored.data)
        else {
            panic!("expected Rgba8 fixture");
        };
        assert_eq!(orig.len(), new.len());

        // Assert on the single highest-chroma opaque pixel: the most channel
        // separation gives the best-defined hue, so the check is robust to u8
        // quantization without depending on the fixture's pixel population.
        let best = orig
            .as_chunks::<4>()
            .0
            .iter()
            .zip(new.as_chunks::<4>().0)
            .filter(|(o, _)| o[3] >= 16)
            .max_by_key(|(o, _)| o[0].max(o[1]).max(o[2]) - o[0].min(o[1]).min(o[2]))
            .expect("fixture has opaque pixels");
        let (o, n) = best;
        let chroma = o[0].max(o[1]).max(o[2]) - o[0].min(o[1]).min(o[2]);
        assert!(chroma > 32, "fixture's most chromatic pixel is near-gray");

        let (_, so, vo) = rgb_to_hsv(
            f64::from(o[0]) / 255.0,
            f64::from(o[1]) / 255.0,
            f64::from(o[2]) / 255.0,
        );
        let (hn, sn, vn) = rgb_to_hsv(
            f64::from(n[0]) / 255.0,
            f64::from(n[1]) / 255.0,
            f64::from(n[2]) / 255.0,
        );
        let dh = (hn - target)
            .rem_euclid(360.0)
            .min((target - hn).rem_euclid(360.0));
        assert!(dh <= 4.0, "hue {hn} not near target {target} (d={dh})");
        assert!((sn - so).abs() <= 0.03, "saturation drifted: {so} -> {sn}");
        assert!((vn - vo).abs() <= 0.03, "value drifted: {vo} -> {vn}");
        assert_eq!(o[3], n[3], "alpha changed");
    }

    /// The full path yields a loadable `.vtex_c`: same dims, decodes cleanly
    /// after the lossy re-encode.
    #[test]
    fn recolor_hue_round_trips_to_loadable_vtex() {
        let bytes = std::fs::read(BC7_COLOR).expect("fixture present");
        let info = morphic::inspect(&bytes).expect("inspect original");
        let out = recolor_texture_hue(&bytes, Recolor::hue(120.0)).expect("recolor to vtex");
        let out_info = morphic::inspect(&out).expect("inspect recolored");
        assert_eq!((out_info.width, out_info.height), (info.width, info.height));
        assert_eq!(
            out_info.mip_count, info.mip_count,
            "mip chain length changed"
        );
        let decoded = morphic::decode(&out).expect("recolored decodes");
        assert_eq!(
            (decoded.width, decoded.height),
            (u32::from(info.width), u32::from(info.height))
        );
    }

    /// HDR textures are refused with a clear message, not silently mangled.
    #[test]
    fn hdr_texture_is_rejected() {
        let bytes = std::fs::read(BC6H_HDR).expect("fixture present");
        let err =
            recolor_texture_hue(&bytes, Recolor::hue(90.0)).expect_err("HDR must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("LDR"), "unexpected error: {msg}");
    }
}
