use base64::Engine;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Cursor;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

#[derive(Serialize)]
struct ModInfo {
    path: String,
    name: String,
    file_count: usize,
    size_bytes: u64,
    file_paths: Vec<String>,
}

impl From<vpkmerge_core::ModInfo> for ModInfo {
    fn from(m: vpkmerge_core::ModInfo) -> Self {
        ModInfo {
            path: m.path.to_string_lossy().into_owned(),
            name: m.name,
            file_count: m.file_count,
            size_bytes: m.size_bytes,
            file_paths: m.file_paths,
        }
    }
}

#[derive(Serialize)]
struct Conflict {
    path: String,
    owner_indices: Vec<usize>,
}

impl From<vpkmerge_core::Conflict> for Conflict {
    fn from(c: vpkmerge_core::Conflict) -> Self {
        Conflict {
            path: c.path,
            owner_indices: c.owner_indices,
        }
    }
}

#[derive(Serialize)]
struct MergeReport {
    total_entries: usize,
    overridden: usize,
    inputs: usize,
    output_path: String,
}

impl From<vpkmerge_core::MergeReport> for MergeReport {
    fn from(r: vpkmerge_core::MergeReport) -> Self {
        MergeReport {
            total_entries: r.total_entries,
            overridden: r.overridden_paths,
            inputs: r.inputs,
            output_path: r.output_path.to_string_lossy().into_owned(),
        }
    }
}

#[tauri::command]
async fn pick_vpk_files(app: AppHandle) -> Vec<String> {
    app.dialog()
        .file()
        .add_filter("VPK files", &["vpk"])
        .blocking_pick_files()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[tauri::command]
async fn pick_output_path(app: AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .add_filter("VPK file", &["vpk"])
        .set_file_name("merged_dir.vpk")
        .blocking_save_file()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
async fn add_mod(path: String) -> Result<ModInfo, String> {
    vpkmerge_core::inspect(&path)
        .map(Into::into)
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn detect_conflicts(ordered_paths: Vec<String>) -> Result<Vec<Conflict>, String> {
    vpkmerge_core::detect_conflicts(&ordered_paths)
        .map(|cs| cs.into_iter().map(Into::into).collect())
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn merge_vpks(
    ordered_paths: Vec<String>,
    output_path: String,
    policy: Option<String>,
    overrides: Option<HashMap<String, usize>>,
) -> Result<MergeReport, String> {
    let collision_policy = match policy.as_deref() {
        None | Some("last_wins") => vpkmerge_core::CollisionPolicy::LastWins,
        Some("first_wins") => vpkmerge_core::CollisionPolicy::FirstWins,
        Some("strict") => vpkmerge_core::CollisionPolicy::Error,
        Some(other) => return Err(format!("unknown policy: {other}")),
    };
    vpkmerge_core::merge(
        &ordered_paths,
        &output_path,
        &vpkmerge_core::MergeOptions {
            collision_policy,
            overrides: overrides.unwrap_or_default(),
        },
    )
    .map(Into::into)
    .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn path_exists(path: String) -> bool {
    std::path::Path::new(&path).exists()
}

/// Best-effort resolution of the local Deadlock `pak01_dir.vpk`. Checks the
/// standard Steam install roots on Linux and Windows. Returns `None` if no
/// candidate exists; the frontend then falls back to asking the user.
#[tauri::command]
async fn default_deadlock_vpk_path() -> Option<String> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if cfg!(target_os = "linux") {
        if let Ok(home) = std::env::var("HOME") {
            candidates.push(
                format!("{home}/.steam/steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk")
                    .into(),
            );
            candidates.push(
                format!(
                    "{home}/.local/share/Steam/steamapps/common/Deadlock/game/citadel/pak01_dir.vpk"
                )
                .into(),
            );
        }
    } else if cfg!(target_os = "windows") {
        candidates.push(
            "C:\\Program Files (x86)\\Steam\\steamapps\\common\\Deadlock\\game\\citadel\\pak01_dir.vpk"
                .into(),
        );
        candidates.push(
            "C:\\Program Files\\Steam\\steamapps\\common\\Deadlock\\game\\citadel\\pak01_dir.vpk"
                .into(),
        );
    }
    candidates
        .into_iter()
        .find(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
}

#[derive(Serialize)]
struct TexturePreview {
    /// `data:image/png;base64,...` URL, ready to drop into `<img :src>`.
    data_url: String,
    /// Displayed (post-downscale) dimensions.
    width: u32,
    height: u32,
    /// Original texture dimensions before any downscale.
    orig_width: u32,
    orig_height: u32,
    /// `VTexFormat` name (e.g. "BC7", "RGBA8888").
    format: String,
    /// True if the texture has the `CUBE_TEXTURE` flag; callers may then
    /// re-invoke with `face` in `1..=5` to see other faces. Face ordering
    /// is `[+X, -X, +Y, -Y, +Z, -Z]`; default (no face arg) is `+X`.
    is_cubemap: bool,
    /// Number of mip levels the source texture has. Callers may pass `mip`
    /// in `0..mip_count` to see lower-detail versions.
    mip_count: u8,
}

#[tauri::command]
async fn preview_texture(
    vpk_path: String,
    entry: String,
    max_dim: Option<u32>,
    face: Option<u8>,
    mip: Option<u8>,
) -> Result<TexturePreview, String> {
    let cap = max_dim.unwrap_or(256).max(16);
    let face = face.unwrap_or(0);
    let mip = mip.unwrap_or(0);
    let vpk = valve_pak::open(&vpk_path).map_err(|e| format!("open vpk: {e}"))?;
    let mut vf = vpk
        .get_file(&entry)
        .map_err(|e| format!("entry not found: {e}"))?;
    let bytes = vf.read_all().map_err(|e| format!("read entry: {e}"))?;

    let info = morphic::inspect(&bytes).map_err(|e| format!("inspect: {e}"))?;
    let is_cubemap = info.flags.contains(morphic::TextureFlags::CUBE_TEXTURE);
    let mip_count = info.mip_count;
    let img = match morphic::decode_at(
        &bytes,
        &morphic::DecodeOptions {
            mip,
            slice: 0,
            face,
        },
    ) {
        Ok(img) => img,
        Err(morphic::DecodeError::Unimplemented(fmt)) => {
            return Err(format!("preview not supported for format: {}", fmt.name()));
        }
        Err(e) => return Err(format!("decode: {e}")),
    };

    let raw_rgba = match img.data {
        morphic::ImageData::Rgba8(buf) => buf,
        morphic::ImageData::Rgba16F(buf) => tonemap_rgba_f16_to_u8(&buf),
    };
    let buffer: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> =
        image::ImageBuffer::from_raw(img.width, img.height, raw_rgba)
            .ok_or_else(|| "decoded buffer size mismatch".to_string())?;

    let dyn_img = image::DynamicImage::ImageRgba8(buffer);
    let (orig_w, orig_h) = (img.width, img.height);
    let downscaled = if orig_w > cap || orig_h > cap {
        dyn_img.resize(cap, cap, image::imageops::FilterType::Triangle)
    } else {
        dyn_img
    };

    let (w, h) = (downscaled.width(), downscaled.height());
    let mut png_bytes: Vec<u8> = Vec::new();
    downscaled
        .write_to(&mut Cursor::new(&mut png_bytes), image::ImageFormat::Png)
        .map_err(|e| format!("png encode: {e}"))?;

    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Ok(TexturePreview {
        data_url: format!("data:image/png;base64,{b64}"),
        width: w,
        height: h,
        orig_width: orig_w,
        orig_height: orig_h,
        format: info.format.name().to_string(),
        is_cubemap,
        mip_count,
    })
}

// Reinhard tone-map + sRGB-ish 1/2.2 gamma so HDR (BC6H, future RGBA16F)
// textures look like reasonable LDR previews in the conflict modal. Negative
// values are clamped to 0; alpha is a direct clamp to [0, 1]. This matches
// the spirit of what VRF's tone-mapped PNG output produces for the same
// fixtures, though it's not bit-exact with VRF (which is fine, this is
// purely for human preview).
fn tonemap_rgba_f16_to_u8(buf: &[half::f16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len());
    for px in buf.chunks_exact(4) {
        for c in &px[..3] {
            let v = c.to_f32().max(0.0);
            let tonemapped = v / (1.0 + v);
            let gamma = tonemapped.powf(1.0 / 2.2);
            out.push(float_to_u8(gamma));
        }
        out.push(float_to_u8(px[3].to_f32()));
    }
    out
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn float_to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(r: f32, g: f32, b: f32, a: f32) -> [half::f16; 4] {
        [
            half::f16::from_f32(r),
            half::f16::from_f32(g),
            half::f16::from_f32(b),
            half::f16::from_f32(a),
        ]
    }

    #[test]
    fn tonemap_black_stays_black() {
        let out = tonemap_rgba_f16_to_u8(&rgba(0.0, 0.0, 0.0, 1.0));
        assert_eq!(out, vec![0, 0, 0, 255]);
    }

    #[test]
    fn tonemap_alpha_is_direct_clamp() {
        // Color channels tone-map (so 0.0 -> 0), alpha passes straight through.
        let out = tonemap_rgba_f16_to_u8(&rgba(0.0, 0.0, 0.0, 0.5));
        assert_eq!(out[3], 128); // round(0.5 * 255) = 128
        let saturated = tonemap_rgba_f16_to_u8(&rgba(0.0, 0.0, 0.0, 4.0));
        assert_eq!(saturated[3], 255);
        let neg_alpha = tonemap_rgba_f16_to_u8(&rgba(0.0, 0.0, 0.0, -1.0));
        assert_eq!(neg_alpha[3], 0);
    }

    #[test]
    fn tonemap_compresses_bright_values_below_white() {
        // Even a "100x brighter than mid-grey" input must not produce pure
        // white, that's the whole point of Reinhard: 100 / (1 + 100) ≈ 0.99,
        // then gamma 1/2.2 ≈ 0.995, * 255 ≈ 253.
        let out = tonemap_rgba_f16_to_u8(&rgba(100.0, 100.0, 100.0, 1.0));
        assert!(
            out[0] >= 250 && out[0] < 255,
            "bright but not saturated, got {}",
            out[0]
        );
        assert_eq!(out[3], 255);
    }

    #[test]
    fn tonemap_negative_color_clamps_to_zero() {
        let out = tonemap_rgba_f16_to_u8(&rgba(-1.0, -0.5, -10.0, 1.0));
        assert_eq!(&out[..3], &[0, 0, 0]);
    }

    #[test]
    fn tonemap_mid_grey_is_reasonable() {
        // 0.5 -> 0.5/1.5 = 0.333... -> ^(1/2.2) ≈ 0.605 -> *255 ≈ 154
        let out = tonemap_rgba_f16_to_u8(&rgba(0.5, 0.5, 0.5, 1.0));
        for c in &out[..3] {
            assert!(*c >= 150 && *c <= 158, "mid-grey ~154, got {c}");
        }
    }
}

#[tauri::command]
async fn save_text_file(
    app: AppHandle,
    default_name: String,
    content: String,
) -> Result<Option<String>, String> {
    let Some(path) = app
        .dialog()
        .file()
        .set_file_name(&default_name)
        .blocking_save_file()
        .and_then(|p| p.into_path().ok())
    else {
        return Ok(None);
    };
    std::fs::write(&path, content.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn reveal_in_folder(path: String) -> Result<(), String> {
    use std::process::Command;
    let result = if cfg!(target_os = "linux") {
        let p = std::path::Path::new(&path);
        let target = if p.is_file() {
            p.parent().unwrap_or(p)
        } else {
            p
        };
        Command::new("xdg-open").arg(target).spawn()
    } else if cfg!(target_os = "windows") {
        Command::new("explorer")
            .arg(format!("/select,{path}"))
            .spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").args(["-R", &path]).spawn()
    } else {
        return Err("Unsupported OS".into());
    };
    result.map(|_| ()).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            pick_vpk_files,
            pick_output_path,
            add_mod,
            detect_conflicts,
            merge_vpks,
            path_exists,
            reveal_in_folder,
            preview_texture,
            save_text_file,
            default_deadlock_vpk_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
