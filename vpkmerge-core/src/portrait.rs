//! Hero portrait / card extraction from Deadlock skin and icon VPKs.
//!
//! Deadlock skins and icon packs ship hero portrait art under
//! `panorama/images/heroes/<codename>_<variant>_(psd|png).vtex_c`. This module
//! pulls those textures out of a VPK and decodes them to PNG, so the desktop
//! client can offer a "pick your hero card" selector independent of the active
//! skin.
//!
//! The Deadlock-specific part (filename -> hero codename + variant) lives here;
//! the actual `.vtex_c` decoding is delegated to the [`morphic`] crate, which
//! handles every image format these textures use (uncompressed `BGRA8888` /
//! `RGBA8888`, embedded `PNG`, and the `BCn` block formats). Formats morphic
//! can't decode are reported with a reason rather than failing the batch, so
//! callers fall back to another art source (e.g. the `GameBanana` thumbnail).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use morphic::TextureFormat;

/// VPK path prefix every hero portrait/card texture lives under.
const PORTRAIT_PREFIX: &str = "panorama/images/heroes";

/// Which portrait asset a texture represents, parsed from its filename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortraitVariant {
    /// Minimap icon (`_mm`), tiny.
    Minimap,
    /// Small square icon (`_sm`).
    Small,
    /// Full hero card (`_card`) - the natural "cover" for the locker.
    Card,
    /// Low-HP card state (`_card_critical`).
    CardCritical,
    /// Taunt/gloat card state (`_card_gloat`).
    CardGloat,
    /// Tall vertical portrait (`_vertical`).
    Vertical,
    /// Under the portrait prefix but an unrecognized suffix.
    Other,
}

impl PortraitVariant {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimap => "minimap",
            Self::Small => "small",
            Self::Card => "card",
            Self::CardCritical => "card_critical",
            Self::CardGloat => "card_gloat",
            Self::Vertical => "vertical",
            Self::Other => "other",
        }
    }
}

/// One portrait texture found in a VPK, decoded if its format is supported.
#[derive(Debug, Clone)]
pub struct PortraitInfo {
    /// Entry path inside the VPK.
    pub source_path: String,
    /// Hero codename parsed from the filename (e.g. "hornet" = Vindicta).
    pub hero_codename: String,
    pub variant: PortraitVariant,
    pub width: u32,
    pub height: u32,
    /// VTEX image-format name (e.g. `BGRA8888`, `PNG_RGBA8888`), or `unknown`
    /// if the header could not be parsed.
    pub format_name: &'static str,
    /// Where the decoded PNG was written, or `None` if the texture could not
    /// be parsed or its format is unsupported.
    pub output_path: Option<PathBuf>,
    /// Why the texture was not decoded (set iff `output_path` is `None`).
    pub skipped_reason: Option<String>,
}

/// Split a portrait filename into `(hero_codename, variant)`. Variant suffixes
/// are checked longest-first so `_card_critical` wins over `_card`.
fn parse_variant_codename(source_path: &str) -> (String, PortraitVariant) {
    let file = source_path.rsplit('/').next().unwrap_or(source_path);
    let stem = file.strip_suffix(".vtex_c").unwrap_or(file);
    // Strip the source-format marker the toolchain appends (`_psd` / `_png`).
    let stem = stem
        .strip_suffix("_psd")
        .or_else(|| stem.strip_suffix("_png"))
        .unwrap_or(stem);

    let candidates = [
        ("_card_critical", PortraitVariant::CardCritical),
        ("_card_gloat", PortraitVariant::CardGloat),
        ("_card", PortraitVariant::Card),
        ("_vertical", PortraitVariant::Vertical),
        ("_mm", PortraitVariant::Minimap),
        ("_sm", PortraitVariant::Small),
    ];
    for (suffix, variant) in candidates {
        if let Some(codename) = stem.strip_suffix(suffix) {
            return (codename.to_string(), variant);
        }
    }
    (stem.to_string(), PortraitVariant::Other)
}

/// Decode one extracted `.vtex_c` payload to a PNG at `out`, building the
/// result record. Never returns `Err`: parse/decode failures are captured as
/// `skipped_reason` so one bad texture doesn't abort the batch.
fn decode_one(
    source_path: String,
    hero_codename: String,
    variant: PortraitVariant,
    bytes: &[u8],
    out: PathBuf,
) -> PortraitInfo {
    let mut info = PortraitInfo {
        source_path,
        hero_codename,
        variant,
        width: 0,
        height: 0,
        format_name: "unknown",
        output_path: None,
        skipped_reason: None,
    };

    let header = match morphic::inspect(bytes) {
        Ok(h) => h,
        Err(e) => {
            info.skipped_reason = Some(format!("could not parse texture header: {e}"));
            return info;
        }
    };
    info.width = u32::from(header.width);
    info.height = u32::from(header.height);
    info.format_name = header.format.name();

    let image = match morphic::decode(bytes) {
        Ok(img) => img,
        Err(e) => {
            info.skipped_reason = Some(format!("decode failed: {e}"));
            return info;
        }
    };
    let png = match morphic::encode_image(&image, TextureFormat::PngRgba8888) {
        Ok(png) => png,
        Err(e) => {
            info.skipped_reason = Some(format!("png encode failed: {e}"));
            return info;
        }
    };
    if let Err(e) = std::fs::write(&out, &png) {
        info.skipped_reason = Some(format!("writing {}: {e}", out.display()));
        return info;
    }
    info.output_path = Some(out);
    info
}

/// Extract and decode every hero portrait under `panorama/images/heroes/` in
/// `vpk_path`, writing PNGs into `out_dir`. When `hero_filter` is set, only
/// portraits whose parsed codename matches are processed (so multi-hero icon
/// packs do not dump all of their heroes).
///
/// Returns one [`PortraitInfo`] per portrait texture found, including ones
/// whose format could not be decoded (with `skipped_reason` populated).
pub fn extract_portraits(
    vpk_path: impl AsRef<Path>,
    hero_filter: Option<&str>,
    out_dir: impl AsRef<Path>,
) -> Result<Vec<PortraitInfo>> {
    let vpk_path = vpk_path.as_ref();
    let out_dir = out_dir.as_ref();
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("creating output directory {}", out_dir.display()))?;

    let vpk =
        valve_pak::open(vpk_path).with_context(|| format!("opening {}", vpk_path.display()))?;

    let paths: Vec<String> = vpk
        .file_paths()
        .filter(|p| p.starts_with(PORTRAIT_PREFIX))
        .cloned()
        .collect();

    let mut results = Vec::new();
    for path in paths {
        let (codename, variant) = parse_variant_codename(&path);
        if let Some(f) = hero_filter {
            if codename != f {
                continue;
            }
        }

        let mut vf = vpk
            .get_file(&path)
            .with_context(|| format!("locating {path}"))?;
        let bytes = vf.read_all().with_context(|| format!("reading {path}"))?;

        let stem = path
            .rsplit('/')
            .next()
            .and_then(|f| f.strip_suffix(".vtex_c"))
            .unwrap_or("portrait");
        let out = out_dir.join(format!("{stem}.png"));

        results.push(decode_one(path, codename, variant, &bytes, out));
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_variant_and_codename() {
        let cases = [
            (
                "panorama/images/heroes/hornet_card_psd.vtex_c",
                "hornet",
                PortraitVariant::Card,
            ),
            (
                "panorama/images/heroes/hornet_card_critical_psd.vtex_c",
                "hornet",
                PortraitVariant::CardCritical,
            ),
            (
                "panorama/images/heroes/hornet_card_gloat_psd.vtex_c",
                "hornet",
                PortraitVariant::CardGloat,
            ),
            (
                "panorama/images/heroes/hornet_vertical_psd.vtex_c",
                "hornet",
                PortraitVariant::Vertical,
            ),
            (
                "panorama/images/heroes/vampirebat_mm_psd.vtex_c",
                "vampirebat",
                PortraitVariant::Minimap,
            ),
            (
                "panorama/images/heroes/hornet_sm_png.vtex_c",
                "hornet",
                PortraitVariant::Small,
            ),
        ];
        for (path, codename, variant) in cases {
            let (c, v) = parse_variant_codename(path);
            assert_eq!(c, codename, "codename for {path}");
            assert_eq!(v, variant, "variant for {path}");
        }
    }

    #[test]
    fn decode_one_records_reason_on_garbage() {
        let info = decode_one(
            "panorama/images/heroes/hornet_card_psd.vtex_c".to_string(),
            "hornet".to_string(),
            PortraitVariant::Card,
            b"not a real vtex file",
            PathBuf::from("/tmp/should-not-be-written.png"),
        );
        assert!(info.output_path.is_none());
        assert!(info.skipped_reason.is_some());
    }
}
