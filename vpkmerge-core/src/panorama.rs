//! Utilities for turning HUD/Panorama-heavy VPKs into a readable workspace.
//!
//! Source 2 Panorama JavaScript and CSS resources store their original source in
//! the resource `DATA` block. This module extracts that exact byte payload rather
//! than relying on a lossy decompiler pass.

mod layout;

use anyhow::{bail, Context, Result};
#[cfg(test)]
use layout::{compile_panorama_layout_xml, print_panorama_layout_xml};
use layout::{decompile_panorama_layout_xml, rebuild_panorama_layout_xml_resource};
#[cfg(test)]
use morphic::kv3::Value;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::path::{Component, Path, PathBuf};

const TEXTURE_PREVIEW_MAX_EDGE: u32 = 2048;

#[derive(Clone, Debug, Default)]
pub struct PanoramaDumpOptions {
    /// Dump every VPK entry. If false, only entries under `panorama/` are dumped.
    pub include_all: bool,
    /// Also copy the exact compiled/raw entry bytes under `_raw/<entry>`.
    pub include_raw: bool,
    /// Optional extra path prefixes. If non-empty, entries must match at least
    /// one prefix after `include_all` / `panorama/` filtering.
    pub prefixes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PanoramaDumpEntry {
    pub entry: String,
    pub mode: PanoramaDumpMode,
    pub raw_path: Option<String>,
    pub source_path: Option<String>,
    pub raw_size: usize,
    pub source_size: Option<usize>,
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PanoramaDumpMode {
    DataSource,
    LayoutXml,
    TexturePng,
    SoundMp3,
    SoundWav,
    SoundeventsJson,
    AssetCopy,
    Text,
    RawOnly,
    Skipped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PanoramaDumpReport {
    pub vpk: String,
    pub out_dir: String,
    pub entries_total: usize,
    pub entries_dumped: usize,
    pub data_sources: usize,
    pub layout_xml: usize,
    pub texture_pngs: usize,
    pub sound_mp3s: usize,
    pub sound_wavs: usize,
    pub soundevent_json: usize,
    pub asset_copies: usize,
    pub text_files: usize,
    pub raw_only: usize,
    pub raw_copies: usize,
    pub manifest_path: String,
    pub entries: Vec<PanoramaDumpEntry>,
}

#[derive(Clone, Debug, Default)]
pub struct PanoramaBuildOptions {
    /// Permit changed sidecars that cannot yet be compiled by falling back to
    /// their raw compiled resources. This is mainly useful for smoke-packing an
    /// unchanged dump; edited XML/PNG/WAV sidecars are rejected by default.
    pub allow_stale_raw: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PanoramaBuildReport {
    pub workspace: String,
    pub output_vpk: String,
    pub entries_packed: usize,
    pub data_rebuilt: usize,
    pub layouts_rebuilt: usize,
    pub textures_rebuilt: usize,
    pub sounds_rebuilt: usize,
    pub soundevents_rebuilt: usize,
    pub raw_preserved: usize,
    pub direct_copies: usize,
    pub stale_raw_allowed: usize,
    pub unsupported_changed: Vec<PanoramaBuildUnsupported>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PanoramaBuildUnsupported {
    pub entry: String,
    pub source_path: Option<String>,
    pub reason: String,
}

#[derive(Clone, Copy)]
struct ResourceBlock {
    kind: [u8; 4],
    offset: usize,
    size: usize,
}

#[allow(clippy::too_many_lines)]
pub fn dump_panorama_workspace<P: AsRef<Path>, O: AsRef<Path>>(
    vpk_path: P,
    out_dir: O,
    options: &PanoramaDumpOptions,
) -> Result<PanoramaDumpReport> {
    let vpk_path = vpk_path.as_ref();
    let out_dir = out_dir.as_ref();
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    cleanup_previous_dump(out_dir)?;

    let info = crate::inspect(vpk_path)?;
    let mut entries = Vec::new();
    let mut data_sources = 0usize;
    let mut layout_xml = 0usize;
    let mut texture_pngs = 0usize;
    let mut sound_mp3s = 0usize;
    let mut sound_wavs = 0usize;
    let mut soundevent_json = 0usize;
    let mut asset_copies = 0usize;
    let mut text_files = 0usize;
    let mut raw_only = 0usize;
    let mut raw_copies = 0usize;

    for entry in &info.file_paths {
        if !should_dump_entry(entry, options) {
            entries.push(PanoramaDumpEntry {
                entry: entry.clone(),
                mode: PanoramaDumpMode::Skipped,
                raw_path: None,
                source_path: None,
                raw_size: 0,
                source_size: None,
                note: Some("filtered out".to_string()),
            });
            continue;
        }

        let bytes = crate::read_vpk_entry(vpk_path, entry)
            .with_context(|| format!("reading {entry:?} from {}", vpk_path.display()))?;
        let mut raw_path = None;
        if options.include_raw {
            let raw_dest = safe_entry_path(&out_dir.join("_raw"), entry)?;
            write_file(&raw_dest, &bytes)?;
            raw_path = Some(relative_slash(out_dir, &raw_dest)?);
            raw_copies += 1;
        }

        let (mode, source_path, source_size, note) = if entry.ends_with(".vxml_c") {
            match decompile_panorama_layout_xml(&bytes) {
                Ok(xml) => {
                    let source_entry = compiled_extension_path(entry, ".vxml_c", ".vxml")?;
                    let source_dest = safe_entry_path(out_dir, &source_entry)?;
                    write_file(&source_dest, xml.as_bytes())?;
                    layout_xml += 1;
                    (
                        PanoramaDumpMode::LayoutXml,
                        Some(relative_slash(out_dir, &source_dest)?),
                        Some(xml.len()),
                        Some("reconstructed XML from LaCo layout block".to_string()),
                    )
                }
                Err(err) => {
                    raw_only += 1;
                    (
                        PanoramaDumpMode::RawOnly,
                        None,
                        None,
                        Some(format!(
                            "could not reconstruct XML from LaCo block: {err:#}"
                        )),
                    )
                }
            }
        } else if let Some(source_entry) = compiled_panorama_source_path(entry) {
            match resource_data_block(&bytes) {
                Ok(data) => {
                    let (source_data, source_note) = readable_panorama_data(entry, data);
                    let source_dest = safe_entry_path(out_dir, &source_entry)?;
                    write_file(&source_dest, source_data)?;
                    data_sources += 1;
                    (
                        PanoramaDumpMode::DataSource,
                        Some(relative_slash(out_dir, &source_dest)?),
                        Some(source_data.len()),
                        source_note,
                    )
                }
                Err(err) => {
                    raw_only += 1;
                    (
                        PanoramaDumpMode::RawOnly,
                        None,
                        None,
                        Some(format!("could not extract DATA block: {err:#}")),
                    )
                }
            }
        } else if entry.ends_with(".vtex_c") {
            match crate::thumbnail_png(&bytes, TEXTURE_PREVIEW_MAX_EDGE) {
                Ok(thumbnail) => {
                    let source_entry = compiled_extension_path(entry, ".vtex_c", ".png")?;
                    let source_dest = safe_entry_path(out_dir, &source_entry)?;
                    write_file(&source_dest, &thumbnail.png)?;
                    texture_pngs += 1;
                    let note = if thumbnail.source_width > thumbnail.width
                        || thumbnail.source_height > thumbnail.height
                    {
                        Some(format!(
                            "decoded PNG preview capped at {}px; source {}x{} {}",
                            TEXTURE_PREVIEW_MAX_EDGE,
                            thumbnail.source_width,
                            thumbnail.source_height,
                            thumbnail.format
                        ))
                    } else {
                        Some(format!(
                            "decoded PNG preview; source {}x{} {}",
                            thumbnail.source_width, thumbnail.source_height, thumbnail.format
                        ))
                    };
                    (
                        PanoramaDumpMode::TexturePng,
                        Some(relative_slash(out_dir, &source_dest)?),
                        Some(thumbnail.png.len()),
                        note,
                    )
                }
                Err(err) => {
                    raw_only += 1;
                    (
                        PanoramaDumpMode::RawOnly,
                        None,
                        None,
                        Some(format!("could not decode VTEX preview PNG: {err:#}")),
                    )
                }
            }
        } else if entry.ends_with(".vsnd_c") {
            match morphic::extract_vsnd_audio(&bytes) {
                Ok(morphic::VsndAudio::Mp3(mp3)) => {
                    let source_entry = compiled_extension_path(entry, ".vsnd_c", ".mp3")?;
                    let source_dest = safe_entry_path(out_dir, &source_entry)?;
                    write_file(&source_dest, &mp3)?;
                    sound_mp3s += 1;
                    (
                        PanoramaDumpMode::SoundMp3,
                        Some(relative_slash(out_dir, &source_dest)?),
                        Some(mp3.len()),
                        Some("extracted embedded MP3 stream".to_string()),
                    )
                }
                Ok(morphic::VsndAudio::WavPcm16 {
                    wav,
                    rate,
                    channels,
                    sample_count,
                }) => {
                    let source_entry = compiled_extension_path(entry, ".vsnd_c", ".wav")?;
                    let source_dest = safe_entry_path(out_dir, &source_entry)?;
                    write_file(&source_dest, &wav)?;
                    sound_wavs += 1;
                    (
                        PanoramaDumpMode::SoundWav,
                        Some(relative_slash(out_dir, &source_dest)?),
                        Some(wav.len()),
                        Some(format!(
                            "decoded PCM16 WAV: {rate} Hz, {channels} channel(s), {sample_count} samples"
                        )),
                    )
                }
                Err(err) => {
                    raw_only += 1;
                    (
                        PanoramaDumpMode::RawOnly,
                        None,
                        None,
                        Some(format!("could not extract supported VSND audio: {err:#}")),
                    )
                }
            }
        } else if entry.ends_with(".vsndevts_c") {
            match crate::SoundEvents::from_bytes(bytes.clone()) {
                Ok(soundevents) => {
                    let source_entry =
                        compiled_extension_path(entry, ".vsndevts_c", ".vsndevts.json")?;
                    let source_dest = safe_entry_path(out_dir, &source_entry)?;
                    let json = serde_json::to_vec_pretty(&soundevents.to_json())
                        .context("serializing soundevents JSON")?;
                    write_file(&source_dest, &json)?;
                    soundevent_json += 1;
                    (
                        PanoramaDumpMode::SoundeventsJson,
                        Some(relative_slash(out_dir, &source_dest)?),
                        Some(json.len()),
                        Some(format!(
                            "decoded {} soundevent entries to JSON",
                            soundevents.event_names().len()
                        )),
                    )
                }
                Err(err) => {
                    raw_only += 1;
                    (
                        PanoramaDumpMode::RawOnly,
                        None,
                        None,
                        Some(format!("could not decode soundevents KV3: {err:#}")),
                    )
                }
            }
        } else if is_copyable_asset_entry(entry) {
            let asset_dest = safe_entry_path(out_dir, entry)?;
            write_file(&asset_dest, &bytes)?;
            asset_copies += 1;
            (
                PanoramaDumpMode::AssetCopy,
                Some(relative_slash(out_dir, &asset_dest)?),
                Some(bytes.len()),
                Some("copied binary asset verbatim".to_string()),
            )
        } else if is_plain_text_entry(entry) {
            if std::str::from_utf8(&bytes).is_ok() {
                let text_dest = safe_entry_path(out_dir, entry)?;
                write_file(&text_dest, &bytes)?;
                text_files += 1;
                (
                    PanoramaDumpMode::Text,
                    Some(relative_slash(out_dir, &text_dest)?),
                    Some(bytes.len()),
                    None,
                )
            } else {
                raw_only += 1;
                (
                    PanoramaDumpMode::RawOnly,
                    None,
                    None,
                    Some("not valid UTF-8; raw copy only".to_string()),
                )
            }
        } else {
            raw_only += 1;
            (PanoramaDumpMode::RawOnly, None, None, None)
        };

        entries.push(PanoramaDumpEntry {
            entry: entry.clone(),
            mode,
            raw_path,
            source_path,
            raw_size: bytes.len(),
            source_size,
            note,
        });
    }

    let manifest_path = out_dir.join("_manifest.json");
    let report = PanoramaDumpReport {
        vpk: vpk_path.display().to_string(),
        out_dir: out_dir.display().to_string(),
        entries_total: info.file_paths.len(),
        entries_dumped: entries
            .iter()
            .filter(|entry| entry.mode != PanoramaDumpMode::Skipped)
            .count(),
        data_sources,
        layout_xml,
        texture_pngs,
        sound_mp3s,
        sound_wavs,
        soundevent_json,
        asset_copies,
        text_files,
        raw_only,
        raw_copies,
        manifest_path: manifest_path.display().to_string(),
        entries,
    };
    let json = serde_json::to_vec_pretty(&report).context("serializing panorama dump manifest")?;
    write_file(&manifest_path, &json)?;
    Ok(report)
}

#[allow(clippy::too_many_lines)]
pub fn build_panorama_workspace<P: AsRef<Path>, O: AsRef<Path>>(
    workspace: P,
    output_vpk: O,
    options: &PanoramaBuildOptions,
) -> Result<PanoramaBuildReport> {
    let workspace = workspace.as_ref();
    let output_vpk = output_vpk.as_ref();
    let manifest_path = workspace.join("_manifest.json");
    let manifest_bytes = std::fs::read(&manifest_path)
        .with_context(|| format!("reading manifest {}", manifest_path.display()))?;
    let manifest: PanoramaDumpReport = serde_json::from_slice(&manifest_bytes)
        .with_context(|| format!("parsing manifest {}", manifest_path.display()))?;

    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();
    let mut data_rebuilt = 0usize;
    let mut layouts_rebuilt = 0usize;
    let mut textures_rebuilt = 0usize;
    let mut sounds_rebuilt = 0usize;
    let mut soundevents_rebuilt = 0usize;
    let mut raw_preserved = 0usize;
    let mut direct_copies = 0usize;
    let mut stale_raw_allowed = 0usize;
    let mut unsupported_changed = Vec::new();

    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.mode != PanoramaDumpMode::Skipped)
    {
        match entry.mode {
            PanoramaDumpMode::DataSource => {
                let raw = read_workspace_file(workspace, entry.raw_path.as_deref(), "raw_path")?;
                let source =
                    read_workspace_file(workspace, entry.source_path.as_deref(), "source_path")?;
                if data_source_changed(&entry.entry, &raw, &source)? {
                    let rebuilt = rebuild_panorama_data_source(&entry.entry, &raw, &source)
                        .with_context(|| format!("rebuilding {}", entry.entry))?;
                    packed.push((entry.entry.clone(), rebuilt));
                    data_rebuilt += 1;
                } else {
                    packed.push((entry.entry.clone(), raw));
                    raw_preserved += 1;
                }
            }
            PanoramaDumpMode::AssetCopy | PanoramaDumpMode::Text => {
                let source =
                    read_workspace_file(workspace, entry.source_path.as_deref(), "source_path")?;
                packed.push((entry.entry.clone(), source));
                direct_copies += 1;
            }
            PanoramaDumpMode::RawOnly => {
                let raw = read_workspace_file(workspace, entry.raw_path.as_deref(), "raw_path")?;
                packed.push((entry.entry.clone(), raw));
                raw_preserved += 1;
            }
            PanoramaDumpMode::TexturePng
            | PanoramaDumpMode::SoundMp3
            | PanoramaDumpMode::SoundWav
            | PanoramaDumpMode::SoundeventsJson => {
                let raw = read_workspace_file(workspace, entry.raw_path.as_deref(), "raw_path")?;
                if sidecar_changed(entry, &raw, workspace)? {
                    let source = read_workspace_file(
                        workspace,
                        entry.source_path.as_deref(),
                        "source_path",
                    )?;
                    match rebuild_changed_sidecar(entry, &raw, &source) {
                        Ok(rebuilt) => {
                            match entry.mode {
                                PanoramaDumpMode::TexturePng => textures_rebuilt += 1,
                                PanoramaDumpMode::SoundMp3 | PanoramaDumpMode::SoundWav => {
                                    sounds_rebuilt += 1;
                                }
                                PanoramaDumpMode::SoundeventsJson => soundevents_rebuilt += 1,
                                _ => {}
                            }
                            packed.push((entry.entry.clone(), rebuilt));
                        }
                        Err(err) if options.allow_stale_raw => {
                            unsupported_changed.push(unsupported_build_change(entry, &err));
                            stale_raw_allowed += 1;
                            packed.push((entry.entry.clone(), raw));
                            raw_preserved += 1;
                        }
                        Err(err) => {
                            return Err(err).with_context(|| {
                                format!("rebuilding changed sidecar {}", entry.entry)
                            });
                        }
                    }
                } else {
                    packed.push((entry.entry.clone(), raw));
                    raw_preserved += 1;
                }
            }
            PanoramaDumpMode::LayoutXml => {
                let raw = read_workspace_file(workspace, entry.raw_path.as_deref(), "raw_path")?;
                if sidecar_changed(entry, &raw, workspace)? {
                    let source = read_workspace_file(
                        workspace,
                        entry.source_path.as_deref(),
                        "source_path",
                    )?;
                    match rebuild_panorama_layout_xml_resource(&raw, &source) {
                        Ok(rebuilt) => {
                            packed.push((entry.entry.clone(), rebuilt));
                            layouts_rebuilt += 1;
                        }
                        Err(err) if options.allow_stale_raw => {
                            unsupported_changed.push(unsupported_build_change(entry, &err));
                            stale_raw_allowed += 1;
                            packed.push((entry.entry.clone(), raw));
                            raw_preserved += 1;
                        }
                        Err(err) => {
                            return Err(err).with_context(|| {
                                format!("rebuilding changed layout {}", entry.entry)
                            });
                        }
                    }
                } else {
                    packed.push((entry.entry.clone(), raw));
                    raw_preserved += 1;
                }
            }
            PanoramaDumpMode::Skipped => {}
        }
    }

    packed.sort_by(|a, b| a.0.cmp(&b.0));
    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    crate::pack(&refs, output_vpk)?;

    Ok(PanoramaBuildReport {
        workspace: workspace.display().to_string(),
        output_vpk: output_vpk.display().to_string(),
        entries_packed: refs.len(),
        data_rebuilt,
        layouts_rebuilt,
        textures_rebuilt,
        sounds_rebuilt,
        soundevents_rebuilt,
        raw_preserved,
        direct_copies,
        stale_raw_allowed,
        unsupported_changed,
    })
}

fn should_dump_entry(entry: &str, options: &PanoramaDumpOptions) -> bool {
    if !options.include_all && !entry.starts_with("panorama/") {
        return false;
    }
    if options.prefixes.is_empty() {
        return true;
    }
    options
        .prefixes
        .iter()
        .any(|prefix| entry.starts_with(prefix))
}

fn compiled_panorama_source_path(entry: &str) -> Option<String> {
    entry
        .strip_suffix(".vjs_c")
        .map(|base| format!("{base}.vjs"))
        .or_else(|| {
            entry
                .strip_suffix(".vcss_c")
                .map(|base| format!("{base}.vcss"))
        })
        .or_else(|| {
            entry
                .strip_suffix(".vsvg_c")
                .map(|base| format!("{base}.vsvg"))
        })
}

fn readable_panorama_data<'a>(entry: &str, data: &'a [u8]) -> (&'a [u8], Option<String>) {
    if entry.ends_with(".vcss_c") || entry.ends_with(".vsvg_c") {
        if let Some(source) = strip_panorama_data_header(data) {
            let kind = if entry.ends_with(".vsvg_c") {
                "VSVG"
            } else {
                "VCSS"
            };
            return (
                source,
                Some(format!("stripped {kind} Panorama DATA header")),
            );
        }
    }
    (data, None)
}

fn strip_panorama_data_header(data: &[u8]) -> Option<&[u8]> {
    let offset = panorama_data_source_offset(data)?;
    let source = data.get(offset..)?;
    is_probably_text_source(source).then_some(source)
}

fn panorama_data_source_offset(data: &[u8]) -> Option<usize> {
    if data.len() < 6 {
        return None;
    }
    let count = u16::from_le_bytes(data.get(4..6)?.try_into().ok()?) as usize;
    let mut offset = 6usize;
    for _ in 0..count {
        let rel_end = data.get(offset..)?.iter().position(|b| *b == 0)?;
        offset = offset.checked_add(rel_end)?.checked_add(1)?;
        offset = offset.checked_add(8)?;
        if offset > data.len() {
            return None;
        }
    }
    Some(offset)
}

fn is_probably_text_source(bytes: &[u8]) -> bool {
    let trimmed = bytes
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .take(16)
        .collect::<Vec<_>>();
    trimmed.starts_with(b"<")
        || trimmed.starts_with(b"@")
        || trimmed.starts_with(b".")
        || trimmed.starts_with(b"#")
        || trimmed.starts_with(b"/")
}

fn compiled_extension_path(
    entry: &str,
    compiled_suffix: &str,
    source_suffix: &str,
) -> Result<String> {
    let base = entry
        .strip_suffix(compiled_suffix)
        .with_context(|| format!("{entry:?} does not end with {compiled_suffix:?}"))?;
    Ok(format!("{base}{source_suffix}"))
}

fn is_plain_text_entry(entry: &str) -> bool {
    entry_extension_matches(entry, &["txt", "cfg", "json", "kv", "res"])
}

fn is_copyable_asset_entry(entry: &str) -> bool {
    entry_extension_matches(entry, &["ttf", "otf", "woff", "woff2"])
}

fn entry_extension_matches(entry: &str, extensions: &[&str]) -> bool {
    Path::new(entry)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

fn cleanup_previous_dump(out_dir: &Path) -> Result<()> {
    let manifest_path = out_dir.join("_manifest.json");
    if !manifest_path.exists() {
        return Ok(());
    }
    let bytes = std::fs::read(&manifest_path)
        .with_context(|| format!("reading previous manifest {}", manifest_path.display()))?;
    let manifest: JsonValue = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing previous manifest {}", manifest_path.display()))?;
    if let Some(entries) = manifest.get("entries").and_then(JsonValue::as_array) {
        for entry in entries {
            for key in ["raw_path", "source_path"] {
                let Some(path) = entry.get(key).and_then(JsonValue::as_str) else {
                    continue;
                };
                let target = safe_entry_path(out_dir, path)?;
                if target.is_file() {
                    std::fs::remove_file(&target).with_context(|| {
                        format!("removing stale dump file {}", target.display())
                    })?;
                }
            }
        }
    }
    std::fs::remove_file(&manifest_path)
        .with_context(|| format!("removing previous manifest {}", manifest_path.display()))?;
    Ok(())
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn safe_entry_path(root: &Path, entry: &str) -> Result<PathBuf> {
    let mut out = root.to_path_buf();
    for part in entry.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            bail!("unsafe VPK entry path {entry:?}");
        }
        let component_path = Path::new(part);
        if component_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            bail!("unsafe VPK entry path {entry:?}");
        }
        out.push(part);
    }
    Ok(out)
}

fn relative_slash(root: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(root)
        .with_context(|| format!("{} is not under {}", path.display(), root.display()))?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn resource_data_block(resource: &[u8]) -> Result<&[u8]> {
    resource_block(resource, *b"DATA")
}

fn resource_block(resource: &[u8], kind: [u8; 4]) -> Result<&[u8]> {
    let blocks = read_resource_blocks(resource)?;
    let block = blocks
        .iter()
        .find(|block| block.kind == kind)
        .with_context(|| format!("resource has no {} block", String::from_utf8_lossy(&kind)))?;
    Ok(&resource[block.offset..block.offset + block.size])
}

fn read_resource_blocks(resource: &[u8]) -> Result<Vec<ResourceBlock>> {
    if resource.len() < 16 {
        bail!("resource is too small");
    }
    let header_version = read_u16(resource, 4)?;
    if header_version != 12 {
        bail!("unexpected resource header version {header_version}");
    }
    let table = 8usize
        .checked_add(read_u32(resource, 8)? as usize)
        .context("resource block table offset overflow")?;
    let count = read_u32(resource, 12)? as usize;
    let mut blocks = Vec::with_capacity(count);

    for index in 0..count {
        let entry_offset = table
            .checked_add(index * 12)
            .context("resource block entry offset overflow")?;
        let kind: [u8; 4] = resource
            .get(entry_offset..entry_offset + 4)
            .with_context(|| format!("resource block {index} kind out of range"))?
            .try_into()?;
        let rel = read_u32(resource, entry_offset + 4)? as usize;
        let size = read_u32(resource, entry_offset + 8)? as usize;
        let offset = entry_offset
            .checked_add(4)
            .and_then(|value| value.checked_add(rel))
            .context("resource block payload offset overflow")?;
        resource
            .get(offset..offset + size)
            .with_context(|| format!("resource block {index} payload out of range"))?;
        blocks.push(ResourceBlock { kind, offset, size });
    }

    Ok(blocks)
}

fn replace_resource_block(resource: &[u8], kind: [u8; 4], payload: &[u8]) -> Result<Vec<u8>> {
    let parsed = morphic::resource::Resource::parse(resource)
        .map_err(|err| anyhow::anyhow!("parsing resource container failed: {err}"))?;
    let index = parsed
        .blocks()
        .iter()
        .position(|block| block.kind == kind)
        .with_context(|| format!("resource has no {} block", String::from_utf8_lossy(&kind)))?;
    parsed
        .rebuild_with_block(index, payload)
        .map_err(|err| anyhow::anyhow!("rebuilding resource block failed: {err}"))
}

fn read_workspace_file(workspace: &Path, path: Option<&str>, field: &str) -> Result<Vec<u8>> {
    let path = path.with_context(|| format!("manifest entry missing {field}"))?;
    let path = safe_entry_path(workspace, path)?;
    std::fs::read(&path).with_context(|| format!("reading {}", path.display()))
}

fn rebuild_panorama_data_source(entry: &str, raw: &[u8], source: &[u8]) -> Result<Vec<u8>> {
    let old_data = resource_data_block(raw)?;
    let new_data = if entry.ends_with(".vcss_c") || entry.ends_with(".vsvg_c") {
        rebuild_named_panorama_data(old_data, source)
            .with_context(|| format!("rebuilding Panorama DATA header for {entry}"))?
    } else {
        source.to_vec()
    };
    morphic::replace_resource_data_block(raw, &new_data)
        .with_context(|| format!("replacing DATA block for {entry}"))
}

fn data_source_changed(entry: &str, raw: &[u8], source: &[u8]) -> Result<bool> {
    let old_data = resource_data_block(raw)?;
    let (dumped_source, _) = readable_panorama_data(entry, old_data);
    Ok(source != dumped_source)
}

fn rebuild_changed_sidecar(
    entry: &PanoramaDumpEntry,
    raw: &[u8],
    source: &[u8],
) -> Result<Vec<u8>> {
    match entry.mode {
        PanoramaDumpMode::TexturePng => {
            ensure_png_matches_texture(raw, source)?;
            crate::build_icon_from_template(raw, source)
                .context("encoding edited PNG using original VTEX as template")
        }
        PanoramaDumpMode::SoundMp3 => {
            let looped = crate::donor_is_looped(raw).unwrap_or(false);
            crate::mint_swapped_clip(raw, source, looped)
        }
        PanoramaDumpMode::SoundWav => morphic::encode_vsnd_pcm16_c(raw, source)
            .map_err(|err| anyhow::anyhow!("encoding edited WAV as VSND failed: {err}")),
        PanoramaDumpMode::SoundeventsJson => {
            let json: JsonValue =
                serde_json::from_slice(source).context("parsing edited soundevents JSON")?;
            let mut soundevents = crate::SoundEvents::from_bytes(raw.to_vec())?;
            soundevents.replace_from_json(json);
            soundevents.encode()
        }
        _ => bail!("entry {} is not an encodable sidecar", entry.entry),
    }
}

fn ensure_png_matches_texture(template_vtex: &[u8], png: &[u8]) -> Result<()> {
    let texture = morphic::inspect(template_vtex).context("reading original VTEX dimensions")?;
    let reader = image::ImageReader::new(std::io::Cursor::new(png))
        .with_guessed_format()
        .context("guessing edited PNG format")?;
    let (width, height) = reader
        .into_dimensions()
        .context("reading edited PNG dimensions")?;
    if width != u32::from(texture.width) || height != u32::from(texture.height) {
        bail!(
            "edited PNG is {}x{}, but original texture is {}x{}; refusing to rebuild from a resized/capped preview",
            width,
            height,
            texture.width,
            texture.height
        );
    }
    Ok(())
}

// The leading CRC of a Panorama DATA block is CRC-32/ISO-HDLC over the source.
// Verified against shipped Valve `.vcss_c`/`.vsvg_c` with no referenced names:
// `crc32_ieee(source)` reproduces the stored value byte-for-byte. When the source
// had `url(...)`/`@import` references, the compiler hoists them into the name
// table ahead of `source_offset` and computes the CRC over the *original*
// pre-hoist text, which is unrecoverable from the stripped DATA payload; for
// those, this recomputed value is a best-effort that won't match. That is
// harmless because the engine cannot be validating this CRC against the payload
// (it would reject Valve's own referenced files): it is a build-time staleness
// marker, not a runtime gate. We always preserve the original name table by
// copying `old_data[..source_offset]` and only overwrite the 4-byte CRC.
fn rebuild_named_panorama_data(old_data: &[u8], source: &[u8]) -> Result<Vec<u8>> {
    let source_offset = panorama_data_source_offset(old_data)
        .context("could not locate Panorama DATA source offset")?;
    let mut data = Vec::with_capacity(source_offset + source.len());
    data.extend_from_slice(&old_data[..source_offset]);
    data[0..4].copy_from_slice(&crc32_ieee(source).to_le_bytes());
    data.extend_from_slice(source);
    Ok(data)
}

fn sidecar_changed(entry: &PanoramaDumpEntry, raw: &[u8], workspace: &Path) -> Result<bool> {
    let Some(source_path) = entry.source_path.as_deref() else {
        return Ok(false);
    };
    let current = read_workspace_file(workspace, Some(source_path), "source_path")?;
    let regenerated = match entry.mode {
        PanoramaDumpMode::LayoutXml => decompile_panorama_layout_xml(raw)?.into_bytes(),
        PanoramaDumpMode::TexturePng => crate::thumbnail_png(raw, TEXTURE_PREVIEW_MAX_EDGE)?.png,
        PanoramaDumpMode::SoundMp3 => match morphic::extract_vsnd_audio(raw)? {
            morphic::VsndAudio::Mp3(mp3) => mp3,
            morphic::VsndAudio::WavPcm16 { .. } => bail!("expected MP3-backed VSND"),
        },
        PanoramaDumpMode::SoundWav => match morphic::extract_vsnd_audio(raw)? {
            morphic::VsndAudio::WavPcm16 { wav, .. } => wav,
            morphic::VsndAudio::Mp3(_) => bail!("expected PCM16-backed VSND"),
        },
        PanoramaDumpMode::SoundeventsJson => {
            let soundevents = crate::SoundEvents::from_bytes(raw.to_vec())?;
            serde_json::to_vec_pretty(&soundevents.to_json())?
        }
        _ => return Ok(false),
    };
    Ok(current != regenerated)
}

fn unsupported_build_change(
    entry: &PanoramaDumpEntry,
    err: &anyhow::Error,
) -> PanoramaBuildUnsupported {
    PanoramaBuildUnsupported {
        entry: entry.entry.clone(),
        source_path: entry.source_path.clone(),
        reason: err.to_string(),
    }
}

fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .with_context(|| format!("u16 read out of range at {offset}"))?
            .try_into()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .with_context(|| format!("u32 read out of range at {offset}"))?
            .try_into()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_resource(blocks: &[([u8; 4], &[u8])]) -> Vec<u8> {
        let table_len = blocks.len() * 12;
        let mut cursor = (16 + table_len + 15) & !15;
        let mut offsets = Vec::with_capacity(blocks.len());
        for (_, payload) in blocks {
            offsets.push(cursor);
            cursor = (cursor + payload.len() + 15) & !15;
        }

        let mut out = vec![0u8; cursor];
        out[0..4].copy_from_slice(
            &u32::try_from(cursor)
                .expect("test resource size fits u32")
                .to_le_bytes(),
        );
        out[4..6].copy_from_slice(&12u16.to_le_bytes());
        out[6..8].copy_from_slice(&1u16.to_le_bytes());
        out[8..12].copy_from_slice(&8u32.to_le_bytes());
        out[12..16].copy_from_slice(
            &u32::try_from(blocks.len())
                .expect("test resource block count fits u32")
                .to_le_bytes(),
        );

        for (i, ((kind, payload), offset)) in blocks.iter().zip(&offsets).enumerate() {
            let entry = 16 + i * 12;
            out[entry..entry + 4].copy_from_slice(kind);
            let offset_field = entry + 4;
            out[offset_field..offset_field + 4].copy_from_slice(
                &u32::try_from(*offset - offset_field)
                    .expect("test resource block offset fits u32")
                    .to_le_bytes(),
            );
            out[offset_field + 4..offset_field + 8].copy_from_slice(
                &u32::try_from(payload.len())
                    .expect("test resource payload size fits u32")
                    .to_le_bytes(),
            );
            out[*offset..*offset + payload.len()].copy_from_slice(payload);
        }
        out
    }

    fn named_panorama_data(source: &[u8]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&crc32_ieee(source).to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(source);
        data
    }

    #[test]
    fn maps_compiled_panorama_source_paths() {
        assert_eq!(
            compiled_panorama_source_path("panorama/scripts/foo.vjs_c").as_deref(),
            Some("panorama/scripts/foo.vjs")
        );
        assert_eq!(
            compiled_panorama_source_path("panorama/styles/foo.vcss_c").as_deref(),
            Some("panorama/styles/foo.vcss")
        );
        assert_eq!(
            compiled_panorama_source_path("panorama/layout/foo.vxml_c"),
            None
        );
    }

    #[test]
    fn strips_vcss_data_prefix_for_readable_source() {
        let data = [1, 2, 3, 4, 0, 0, b'@', b'i'];
        let (source, note) = readable_panorama_data("panorama/styles/foo.vcss_c", &data);
        assert_eq!(source, b"@i");
        assert!(note.is_some());

        let (source, note) = readable_panorama_data("panorama/scripts/foo.vjs_c", &data);
        assert_eq!(source, &data);
        assert!(note.is_none());
    }

    #[test]
    fn rejects_unsafe_entry_paths() {
        assert!(safe_entry_path(Path::new("out"), "panorama/scripts/foo.vjs").is_ok());
        assert!(safe_entry_path(Path::new("out"), "../x").is_err());
        assert!(safe_entry_path(Path::new("out"), "panorama//x").is_err());
    }

    #[test]
    fn rebuilds_named_panorama_data_with_updated_crc() -> Result<()> {
        let old = named_panorama_data(b".old { color: red; }");
        let new = rebuild_named_panorama_data(&old, b".new { color: green; }")?;

        assert_eq!(
            u32::from_le_bytes(new[0..4].try_into()?),
            crc32_ieee(b".new { color: green; }")
        );
        assert_eq!(&new[4..6], &0u16.to_le_bytes());
        assert_eq!(
            strip_panorama_data_header(&new),
            Some(&b".new { color: green; }"[..])
        );
        Ok(())
    }

    #[test]
    fn rebuilds_vcss_resource_data_source() -> Result<()> {
        let raw = build_resource(&[(
            *b"DATA",
            named_panorama_data(b".old { color: red; }").as_slice(),
        )]);
        let rebuilt = rebuild_panorama_data_source(
            "panorama/styles/example.vcss_c",
            &raw,
            b".new { color: green; }",
        )?;
        let data = resource_data_block(&rebuilt)?;

        assert_eq!(
            strip_panorama_data_header(data),
            Some(&b".new { color: green; }"[..])
        );
        assert_eq!(
            u32::from_le_bytes(data[0..4].try_into()?),
            crc32_ieee(b".new { color: green; }")
        );
        Ok(())
    }

    #[test]
    fn build_workspace_rebuilds_vjs_and_packs_vpk() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let workspace = tmp.path().join("workspace");
        let raw_path = workspace.join("_raw/panorama/scripts/example.vjs_c");
        let source_path = workspace.join("panorama/scripts/example.vjs");
        std::fs::create_dir_all(raw_path.parent().unwrap())?;
        std::fs::create_dir_all(source_path.parent().unwrap())?;

        let raw = build_resource(&[(*b"DATA", b"$.Msg('old');".as_slice())]);
        std::fs::write(&raw_path, raw)?;
        std::fs::write(&source_path, b"$.Msg('new');")?;

        let manifest = PanoramaDumpReport {
            vpk: "input.vpk".to_string(),
            out_dir: workspace.display().to_string(),
            entries_total: 1,
            entries_dumped: 1,
            data_sources: 1,
            layout_xml: 0,
            texture_pngs: 0,
            sound_mp3s: 0,
            sound_wavs: 0,
            soundevent_json: 0,
            asset_copies: 0,
            text_files: 0,
            raw_only: 0,
            raw_copies: 1,
            manifest_path: workspace.join("_manifest.json").display().to_string(),
            entries: vec![PanoramaDumpEntry {
                entry: "panorama/scripts/example.vjs_c".to_string(),
                mode: PanoramaDumpMode::DataSource,
                raw_path: Some("_raw/panorama/scripts/example.vjs_c".to_string()),
                source_path: Some("panorama/scripts/example.vjs".to_string()),
                raw_size: 1,
                source_size: Some(1),
                note: None,
            }],
        };
        std::fs::write(
            workspace.join("_manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;

        let out_vpk = tmp.path().join("rebuilt_dir.vpk");
        let report =
            build_panorama_workspace(&workspace, &out_vpk, &PanoramaBuildOptions::default())?;
        assert_eq!(report.data_rebuilt, 1);

        let packed = crate::read_vpk_entry(&out_vpk, "panorama/scripts/example.vjs_c")?;
        assert_eq!(resource_data_block(&packed)?, b"$.Msg('new');");
        Ok(())
    }

    #[test]
    fn build_workspace_preserves_unchanged_data_source_raw() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let workspace = tmp.path().join("workspace");
        let raw_path = workspace.join("_raw/panorama/styles/example.vcss_c");
        let source_path = workspace.join("panorama/styles/example.vcss");
        std::fs::create_dir_all(raw_path.parent().unwrap())?;
        std::fs::create_dir_all(source_path.parent().unwrap())?;

        let raw = build_resource(&[(
            *b"DATA",
            named_panorama_data(b".same { color: green; }").as_slice(),
        )]);
        std::fs::write(&raw_path, &raw)?;
        std::fs::write(&source_path, b".same { color: green; }")?;

        let manifest = PanoramaDumpReport {
            vpk: "input.vpk".to_string(),
            out_dir: workspace.display().to_string(),
            entries_total: 1,
            entries_dumped: 1,
            data_sources: 1,
            layout_xml: 0,
            texture_pngs: 0,
            sound_mp3s: 0,
            sound_wavs: 0,
            soundevent_json: 0,
            asset_copies: 0,
            text_files: 0,
            raw_only: 0,
            raw_copies: 1,
            manifest_path: workspace.join("_manifest.json").display().to_string(),
            entries: vec![PanoramaDumpEntry {
                entry: "panorama/styles/example.vcss_c".to_string(),
                mode: PanoramaDumpMode::DataSource,
                raw_path: Some("_raw/panorama/styles/example.vcss_c".to_string()),
                source_path: Some("panorama/styles/example.vcss".to_string()),
                raw_size: raw.len(),
                source_size: Some(".same { color: green; }".len()),
                note: Some("stripped VCSS Panorama DATA header".to_string()),
            }],
        };
        std::fs::write(
            workspace.join("_manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;

        let out_vpk = tmp.path().join("rebuilt_dir.vpk");
        let report =
            build_panorama_workspace(&workspace, &out_vpk, &PanoramaBuildOptions::default())?;
        assert_eq!(report.data_rebuilt, 0);
        assert_eq!(report.raw_preserved, 1);

        let packed = crate::read_vpk_entry(&out_vpk, "panorama/styles/example.vcss_c")?;
        assert_eq!(packed, raw);
        Ok(())
    }

    #[test]
    fn build_workspace_rebuilds_texture_png() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let workspace = tmp.path().join("workspace");
        let raw_path = workspace.join("_raw/panorama/images/example.vtex_c");
        let source_path = workspace.join("panorama/images/example.png");
        std::fs::create_dir_all(raw_path.parent().unwrap())?;
        std::fs::create_dir_all(source_path.parent().unwrap())?;

        let raw = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../morphic/fixtures/rgba8/minimap_circle.vtex_c"
        ))?;
        let info = morphic::inspect(&raw)?;
        let image = image::RgbaImage::from_pixel(
            u32::from(info.width),
            u32::from(info.height),
            image::Rgba([250, 10, 20, 255]),
        );
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image).write_to(&mut png, image::ImageFormat::Png)?;

        std::fs::write(&raw_path, raw)?;
        std::fs::write(&source_path, png.into_inner())?;

        write_single_entry_manifest(
            &workspace,
            "panorama/images/example.vtex_c",
            PanoramaDumpMode::TexturePng,
            "_raw/panorama/images/example.vtex_c",
            "panorama/images/example.png",
        )?;

        let out_vpk = tmp.path().join("rebuilt_texture_dir.vpk");
        let report =
            build_panorama_workspace(&workspace, &out_vpk, &PanoramaBuildOptions::default())?;
        assert_eq!(report.textures_rebuilt, 1);
        let packed = crate::read_vpk_entry(&out_vpk, "panorama/images/example.vtex_c")?;
        let thumb = crate::thumbnail_png(&packed, TEXTURE_PREVIEW_MAX_EDGE)?;
        assert_eq!(thumb.source_width, u32::from(info.width));
        assert_eq!(thumb.source_height, u32::from(info.height));
        Ok(())
    }

    #[test]
    fn build_workspace_rebuilds_soundevents_json() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let workspace = tmp.path().join("workspace");
        let raw_path = workspace.join("_raw/soundevents/example.vsndevts_c");
        let source_path = workspace.join("soundevents/example.vsndevts.json");
        std::fs::create_dir_all(raw_path.parent().unwrap())?;
        std::fs::create_dir_all(source_path.parent().unwrap())?;

        let raw = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../morphic/fixtures/kv3/gigawatt.vsndevts_c"
        ))?;
        let mut soundevents = crate::SoundEvents::from_bytes(raw.clone())?;
        let mut json = soundevents.to_json();
        json["Seven.Wpn.Fire"]["volume"] = serde_json::json!(-4.25);

        std::fs::write(&raw_path, raw)?;
        std::fs::write(&source_path, serde_json::to_vec_pretty(&json)?)?;

        write_single_entry_manifest(
            &workspace,
            "soundevents/example.vsndevts_c",
            PanoramaDumpMode::SoundeventsJson,
            "_raw/soundevents/example.vsndevts_c",
            "soundevents/example.vsndevts.json",
        )?;

        let out_vpk = tmp.path().join("rebuilt_soundevents_dir.vpk");
        let report =
            build_panorama_workspace(&workspace, &out_vpk, &PanoramaBuildOptions::default())?;
        assert_eq!(report.soundevents_rebuilt, 1);
        let packed = crate::read_vpk_entry(&out_vpk, "soundevents/example.vsndevts_c")?;
        soundevents = crate::SoundEvents::from_bytes(packed)?;
        let fire = soundevents.root.get("Seven.Wpn.Fire").unwrap();
        assert_eq!(fire.get("volume").and_then(Value::as_f64), Some(-4.25));
        Ok(())
    }

    #[test]
    fn compiles_panorama_layout_xml_to_printable_laco() -> Result<()> {
        let xml = br#"<root>
	<styles>
		<include src="s2r://panorama/styles/example.vcss_c" />
	</styles>
	<snippets>
		<snippet name="Demo">
			<Panel id="PanelA" class="foo" />
		</snippet>
	</snippets>
</root>
"#;
        let compiled = compile_panorama_layout_xml(xml)?;
        let printed = print_panorama_layout_xml(&compiled)?;
        assert!(printed.contains("<include src=\"s2r://panorama/styles/example.vcss_c\" />"));
        assert!(printed.contains("<snippet name=\"Demo\">"));
        assert!(printed.contains("<Panel id=\"PanelA\" class=\"foo\" />"));
        Ok(())
    }

    #[test]
    fn build_workspace_rebuilds_layout_xml() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let workspace = tmp.path().join("workspace");
        let raw_path = workspace.join("_raw/panorama/layout/example.vxml_c");
        let source_path = workspace.join("panorama/layout/example.vxml");
        std::fs::create_dir_all(raw_path.parent().unwrap())?;
        std::fs::create_dir_all(source_path.parent().unwrap())?;

        let original_xml = br#"<root>
	<styles>
		<include src="s2r://panorama/styles/original.vcss_c" />
	</styles>
</root>
"#;
        let original_laco = compile_panorama_layout_xml(original_xml)?;
        let format = morphic::kv3::Format([1; 16]);
        let raw = build_resource(&[(
            *b"LaCo",
            morphic::kv3::encode(&original_laco, &format).as_slice(),
        )]);
        std::fs::write(&raw_path, raw)?;

        let edited_xml = br#"<root>
	<styles>
		<include src="s2r://panorama/styles/edited.vcss_c" />
	</styles>
</root>
"#;
        std::fs::write(&source_path, edited_xml)?;

        write_single_entry_manifest(
            &workspace,
            "panorama/layout/example.vxml_c",
            PanoramaDumpMode::LayoutXml,
            "_raw/panorama/layout/example.vxml_c",
            "panorama/layout/example.vxml",
        )?;

        let out_vpk = tmp.path().join("rebuilt_layout_dir.vpk");
        let report =
            build_panorama_workspace(&workspace, &out_vpk, &PanoramaBuildOptions::default())?;
        assert_eq!(report.layouts_rebuilt, 1);
        let packed = crate::read_vpk_entry(&out_vpk, "panorama/layout/example.vxml_c")?;
        let printed = decompile_panorama_layout_xml(&packed)?;
        assert!(printed.contains("panorama/styles/edited.vcss_c"));
        Ok(())
    }

    fn write_single_entry_manifest(
        workspace: &Path,
        entry: &str,
        mode: PanoramaDumpMode,
        raw_path: &str,
        source_path: &str,
    ) -> Result<()> {
        let manifest = PanoramaDumpReport {
            vpk: "input.vpk".to_string(),
            out_dir: workspace.display().to_string(),
            entries_total: 1,
            entries_dumped: 1,
            data_sources: usize::from(mode == PanoramaDumpMode::DataSource),
            layout_xml: usize::from(mode == PanoramaDumpMode::LayoutXml),
            texture_pngs: usize::from(mode == PanoramaDumpMode::TexturePng),
            sound_mp3s: usize::from(mode == PanoramaDumpMode::SoundMp3),
            sound_wavs: usize::from(mode == PanoramaDumpMode::SoundWav),
            soundevent_json: usize::from(mode == PanoramaDumpMode::SoundeventsJson),
            asset_copies: usize::from(mode == PanoramaDumpMode::AssetCopy),
            text_files: usize::from(mode == PanoramaDumpMode::Text),
            raw_only: usize::from(mode == PanoramaDumpMode::RawOnly),
            raw_copies: 1,
            manifest_path: workspace.join("_manifest.json").display().to_string(),
            entries: vec![PanoramaDumpEntry {
                entry: entry.to_string(),
                mode,
                raw_path: Some(raw_path.to_string()),
                source_path: Some(source_path.to_string()),
                raw_size: 1,
                source_size: Some(1),
                note: None,
            }],
        };
        std::fs::write(
            workspace.join("_manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        Ok(())
    }
}
