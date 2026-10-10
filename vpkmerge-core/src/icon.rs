//! Build a custom Deadlock icon / hero-card texture from a user PNG.
//!
//! Deadlock hero card art ships as `.vtex_c` under
//! `panorama/images/heroes/<codename>_<variant>_(psd or png).vtex_c`, each variant
//! at its own fixed dimensions (minimap, small, card, `card_critical`, `card_gloat`,
//! vertical). To let a user drop in their own art without an encoder that has to
//! reproduce the exact header/format the game expects, we reuse an existing
//! texture as a *template*: take the base game's variant `.vtex_c`, decode the
//! user PNG, resize it to the template's mip-0 dimensions, and splice it into the
//! template's mip chain via [`morphic::replace_mip_chain`] (the same in-place
//! mechanism the ability-VFX recolor uses, see `recolor.rs`).
//!
//! Packing the result at the template's own entry path overrides the base
//! texture in place: no `.vmat_c` edit, format and header preserved, so the game
//! loads it exactly as it would the original.

use std::path::Path;

use anyhow::{Context, Result};
use morphic::{Image, ImageData};

use crate::recolor::{inspect_texture, TextureSummary};

/// Decode `png_bytes`, resize to `width` x `height`, and wrap as a morphic
/// RGBA8 [`Image`] (row-major, top-left origin) ready for mip splicing.
pub fn png_to_rgba8_image(png_bytes: &[u8], width: u32, height: u32) -> Result<Image> {
    let decoded = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)
        .context("decoding PNG (input must be a valid PNG)")?
        .to_rgba8();
    // Lanczos3 keeps card art crisp when down/upscaling to the template dims.
    let resized = image::imageops::resize(
        &decoded,
        width,
        height,
        image::imageops::FilterType::Lanczos3,
    );
    Ok(Image {
        width,
        height,
        data: ImageData::Rgba8(resized.into_raw()),
    })
}

/// One texture replaced by [`build_icon_addon`] or [`build_texture_addon`].
pub struct IconReplacement {
    /// Entry path the rebuilt texture was packed at.
    pub entry: String,
    /// The template's own format and dimensions, which the PNG was fitted to.
    pub template: TextureSummary,
    /// Something the caller should know about the result, e.g. that a tiny
    /// placeholder template kept only the PNG's average color.
    pub note: Option<String>,
}

/// Where a replaced texture's alpha channel comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaSource {
    /// The PNG's alpha (opaque when it has none). Right for UI art, where alpha
    /// is transparency.
    Png,
    /// The template's alpha, kept as is. Right for material textures, where
    /// alpha is a data channel (a mask) rather than transparency.
    Original,
}

/// One texture for [`build_texture_addon`].
pub struct TextureReplacement<'a> {
    pub entry: &'a str,
    pub png: &'a [u8],
    /// White-on-black PNG of any size: the new image lands where it is white,
    /// the original stays where it is black, blended in between.
    pub mask: Option<&'a [u8]>,
    pub alpha: AlphaSource,
}

/// Like [`build_icon_addon`], with an optional mask and a chosen alpha source per
/// texture, for painting material textures as well as UI art.
pub fn build_texture_addon(
    template_vpk: impl AsRef<Path>,
    replacements: &[TextureReplacement],
    out: impl AsRef<Path>,
) -> Result<Vec<IconReplacement>> {
    let template_vpk = template_vpk.as_ref();
    let mut built: Vec<(String, Vec<u8>)> = Vec::with_capacity(replacements.len());
    let mut report = Vec::with_capacity(replacements.len());
    for r in replacements {
        let template = crate::read_vpk_entry(template_vpk, r.entry).with_context(|| {
            format!(
                "reading template {} from {}",
                r.entry,
                template_vpk.display()
            )
        })?;
        let summary = inspect_texture(&template)
            .with_context(|| format!("{} is not a readable .vtex_c", r.entry))?;
        let vtex = build_texture_from_template(&template, r.png, r.mask, r.alpha)
            .with_context(|| format!("building {}", r.entry))?;
        let info = morphic::inspect(&template)?;
        let note = (info.actual_width <= 16 && info.actual_height <= 16).then(|| {
            format!(
                "{} is a {}x{} placeholder: the PNG was averaged down to that size, so \
                 only its overall color survives. A painted image needs a full-size texture.",
                r.entry, info.actual_width, info.actual_height
            )
        });
        built.push((r.entry.to_owned(), vtex));
        report.push(IconReplacement {
            entry: r.entry.to_owned(),
            template: summary,
            note,
        });
    }
    let refs: Vec<(&str, &[u8])> = built
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    crate::pack(&refs, out)?;
    Ok(report)
}

/// Build a new `.vtex_c` from `template_vtex` with `png_bytes` as its image,
/// optionally blended in through `mask`, keeping the template's format, header
/// and mip count.
///
/// A PNG at the template's stored (padded) size is used as is; any other size
/// is resized to the real image size and placed top-left, where the engine
/// samples a non-power-of-two texture, with its edge texels repeated into the
/// padding.
pub fn build_texture_from_template(
    template_vtex: &[u8],
    png_bytes: &[u8],
    mask: Option<&[u8]>,
    alpha: AlphaSource,
) -> Result<Vec<u8>> {
    let info = morphic::inspect(template_vtex).context("reading template .vtex_c header")?;
    if matches!(
        info.format,
        morphic::TextureFormat::Bc6h | morphic::TextureFormat::Rgba16161616F
    ) {
        anyhow::bail!(
            "template is an HDR ({:?}) texture; PNG import supports 8-bit formats only",
            info.format
        );
    }
    anyhow::ensure!(
        !info.ycocg,
        "template is a YCoCg-encoded DXT5, which can be decoded but not re-encoded yet"
    );
    let (sw, sh) = (u32::from(info.width), u32::from(info.height));
    let png = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)
        .context("decoding PNG (input must be a valid PNG)")?
        .to_rgba8();
    let (fw, fh) = if png.dimensions() == (sw, sh) {
        (sw, sh)
    } else {
        (u32::from(info.actual_width), u32::from(info.actual_height))
    };
    let fitted = if png.dimensions() == (fw, fh) {
        png
    } else {
        image::imageops::resize(&png, fw, fh, image::imageops::FilterType::Lanczos3)
    };
    let mask = mask
        .map(|m| -> Result<image::GrayImage> {
            let m = image::load_from_memory(m)
                .context("decoding mask PNG")?
                .to_luma8();
            Ok(if m.dimensions() == (fw, fh) {
                m
            } else {
                image::imageops::resize(&m, fw, fh, image::imageops::FilterType::Triangle)
            })
        })
        .transpose()?;

    let mut canvas = if mask.is_some() || alpha == AlphaSource::Original {
        let decoded = morphic::decode(template_vtex).context("decoding the template")?;
        let morphic::ImageData::Rgba8(px) = decoded.data else {
            anyhow::bail!("template did not decode to 8-bit pixels");
        };
        image::RgbaImage::from_raw(sw, sh, px).context("template buffer size mismatch")?
    } else {
        image::RgbaImage::new(sw, sh)
    };
    for (x, y, out) in canvas.enumerate_pixels_mut() {
        let (px, py) = (x.min(fw - 1), y.min(fh - 1));
        let new = fitted.get_pixel(px, py).0;
        let m = mask
            .as_ref()
            .map_or(255, |m| u16::from(m.get_pixel(px, py).0[0]));
        let blend = |old: u8, new: u8| {
            u8::try_from((u16::from(old) * (255 - m) + u16::from(new) * m + 127) / 255)
                .unwrap_or(u8::MAX)
        };
        let old = out.0;
        out.0 = [
            blend(old[0], new[0]),
            blend(old[1], new[1]),
            blend(old[2], new[2]),
            match alpha {
                AlphaSource::Png => blend(old[3], new[3]),
                AlphaSource::Original => old[3],
            },
        ];
    }
    let image = morphic::Image {
        width: sw,
        height: sh,
        data: morphic::ImageData::Rgba8(canvas.into_raw()),
    };
    morphic::replace_mip_chain(template_vtex, &image)
        .context("splicing the PNG into the template's mip chain")
}

/// Build an addon VPK at `out` that replaces each `(entry, png_bytes)` texture:
/// every entry is read from `template_vpk` as its own template, rebuilt around
/// the PNG via [`build_icon_from_template`], and packed at the same entry path
/// so it overrides the base art in place.
///
/// # Errors
/// Fails if an entry is missing from `template_vpk` or is not a supported
/// `.vtex_c`, or if a PNG does not decode.
pub fn build_icon_addon(
    template_vpk: impl AsRef<Path>,
    replacements: &[(&str, &[u8])],
    out: impl AsRef<Path>,
) -> Result<Vec<IconReplacement>> {
    let template_vpk = template_vpk.as_ref();
    let mut built: Vec<(String, Vec<u8>)> = Vec::with_capacity(replacements.len());
    let mut report = Vec::with_capacity(replacements.len());
    for (entry, png) in replacements {
        let template = crate::read_vpk_entry(template_vpk, entry)
            .with_context(|| format!("reading template {entry} from {}", template_vpk.display()))?;
        let summary = inspect_texture(&template)
            .with_context(|| format!("{entry} is not a readable .vtex_c"))?;
        let vtex = build_icon_from_template(&template, png)
            .with_context(|| format!("building {entry}"))?;
        built.push(((*entry).to_owned(), vtex));
        report.push(IconReplacement {
            entry: (*entry).to_owned(),
            template: summary,
            note: None,
        });
    }
    let refs: Vec<(&str, &[u8])> = built
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    crate::pack(&refs, out)?;
    Ok(report)
}

/// Build a new `.vtex_c` by replacing `template_vtex`'s image with `png_bytes`
/// (resized to the template's dimensions), preserving the template's format,
/// header, and mip count. The returned bytes pack back at the template's entry
/// path to override the base texture in place.
pub fn build_icon_from_template(template_vtex: &[u8], png_bytes: &[u8]) -> Result<Vec<u8>> {
    let info = morphic::inspect(template_vtex).context("reading template .vtex_c header")?;
    // BCn / 8-bit card formats decode to RGBA8; HDR (Rgba16F) templates are not
    // a hero-card art path and would need float pixels, so reject them clearly.
    if matches!(
        info.format,
        morphic::TextureFormat::Bc6h | morphic::TextureFormat::Rgba16161616F
    ) {
        anyhow::bail!(
            "template is an HDR ({:?}) texture; custom PNG import supports 8-bit card formats only",
            info.format
        );
    }
    let image = png_to_rgba8_image(png_bytes, u32::from(info.width), u32::from(info.height))?;
    morphic::replace_mip_chain(template_vtex, &image)
        .context("splicing the PNG into the template's mip chain")
}
