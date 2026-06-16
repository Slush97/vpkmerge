//! Build the local Title Fight music-kit addon from
//! `music-packs/title-fight/manifest.json`.
//!
//! Usage:
//!   cargo run --release -p vpkmerge-core --example build_title_fight_pack -- \
//!     <base pak01_dir.vpk> <pack dir> <out_dir.vpk>
//!
//! The pack dir must contain local source audio matching manifest `local_audio`
//! paths. This example does not download audio.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::Value;

const MUSIC_EVENTS_ENTRY: &str = "soundevents/music.vsndevts_c";
const FADE_DEFAULT_SECS: f64 = 0.25;

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let base = PathBuf::from(
        a.next()
            .expect("usage: build_title_fight_pack <base pak01_dir.vpk> <pack dir> <out_dir.vpk>"),
    );
    let pack_dir = PathBuf::from(a.next().expect("pack dir"));
    let out = PathBuf::from(a.next().expect("out_dir.vpk"));

    let manifest_path = pack_dir.join("manifest.json");
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(&manifest_path)
            .with_context(|| format!("reading {}", manifest_path.display()))?,
    )
    .with_context(|| format!("parsing {}", manifest_path.display()))?;

    let entries = manifest
        .get("entries")
        .and_then(Value::as_array)
        .context("manifest.entries array")?;

    let mut soundevents = vpkmerge_core::SoundEvents::from_vpk(&base, MUSIC_EVENTS_ENTRY)
        .with_context(|| format!("loading {MUSIC_EVENTS_ENTRY}"))?;
    let source_events_json = soundevents.to_json();

    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();
    let mut randomized_fields: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut field_durations: BTreeMap<(String, String), f64> = BTreeMap::new();
    for entry in entries {
        let event = str_field(entry, "deadlock_event")?;
        let field = str_field_opt(entry, "deadlock_field").unwrap_or("vsnd_files");
        let local_audio = pack_dir.join(str_field(entry, "local_audio")?);
        let target_vsnd = str_field(entry, "target_vsnd")?;
        let target_vsnd_c = str_field(entry, "target_vsnd_c")?;
        let edit = entry.get("edit").context("entry.edit")?;
        let target_seconds = num_field(edit, "target_seconds")?;
        let loop_clip = edit.get("loop").and_then(Value::as_bool).unwrap_or(false);
        let fade_out = num_field_opt(edit, "fade_out_seconds").unwrap_or(FADE_DEFAULT_SECS);
        let fade_in = num_field_opt(edit, "fade_in_seconds").unwrap_or(0.0);

        let donor_vsnd = source_events_json
            .get(event)
            .and_then(|v| v.get(field))
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .with_context(|| format!("finding original {field} donor for {event}"))?;
        let donor_entry = format!("{donor_vsnd}_c");
        let donor = vpkmerge_core::read_vpk_entry(&base, &donor_entry)
            .with_context(|| format!("reading donor {donor_entry} for {event}"))?;

        let (rate, channels) = ffprobe_stream(&local_audio)
            .with_context(|| format!("probing {}", local_audio.display()))?;
        let source_duration = ffprobe_duration(&local_audio)
            .with_context(|| format!("probing duration {}", local_audio.display()))?;
        let duration = source_duration.min(target_seconds);
        let mp3 = wav_to_mp3(&local_audio, rate, channels, duration, fade_in, fade_out)
            .with_context(|| format!("encoding mp3 for {}", local_audio.display()))?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let sample_count = (duration * f64::from(rate)).round() as u32;
        let params = morphic::VsndParams {
            rate,
            channels,
            sample_count,
            duration,
            looped: loop_clip,
        };
        let vsnd = morphic::encode_vsnd_c(&donor, &mp3, &params)
            .with_context(|| format!("minting {target_vsnd_c}"))?;

        println!(
            "{event:<38} {field:<25} {:>6.1}s {} -> {target_vsnd_c}",
            duration,
            local_audio.display()
        );
        packed.push((target_vsnd_c.to_owned(), vsnd));
        let field_key = (event.to_owned(), field.to_owned());
        randomized_fields
            .entry(field_key.clone())
            .or_default()
            .push(target_vsnd.to_owned());
        field_durations
            .entry(field_key)
            .and_modify(|max_duration| *max_duration = max_duration.max(duration))
            .or_insert(duration);
    }

    for ((event, field), paths) in &randomized_fields {
        anyhow::ensure!(
            soundevents.set_string_array_field(event, field, paths),
            "event {event} not found in {MUSIC_EVENTS_ENTRY}"
        );
        if let Some(duration) = field_durations.get(&(event.clone(), field.clone())) {
            let _ = soundevents.set_event_field(event, "vsnd_duration", *duration);
        }
        if paths.len() > 1 {
            println!("randomized {event:<38} {field:<25} {} choices", paths.len());
        }
    }

    packed.push((MUSIC_EVENTS_ENTRY.to_owned(), soundevents.encode()?));
    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &out).with_context(|| format!("packing {}", out.display()))?;
    println!("\nwrote {} with {} entries", out.display(), refs.len());
    Ok(())
}

fn str_field<'a>(obj: &'a Value, key: &str) -> Result<&'a str> {
    obj.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string field {key}"))
}

fn str_field_opt<'a>(obj: &'a Value, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(Value::as_str)
}

fn num_field(obj: &Value, key: &str) -> Result<f64> {
    obj.get(key)
        .and_then(Value::as_f64)
        .with_context(|| format!("missing numeric field {key}"))
}

fn num_field_opt(obj: &Value, key: &str) -> Option<f64> {
    obj.get(key).and_then(Value::as_f64)
}

fn wav_to_mp3(
    wav: &Path,
    rate: u32,
    channels: u32,
    duration: f64,
    fade_in: f64,
    fade_out: f64,
) -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!(
        "title_fight_pack_{}_{}.mp3",
        std::process::id(),
        sanitize(wav.file_stem().and_then(|s| s.to_str()).unwrap_or("clip"))
    ));
    let fade_start = (duration - fade_out).max(0.0);
    let filter = if fade_in > 0.0 {
        format!("afade=t=in:st=0:d={fade_in},afade=t=out:st={fade_start}:d={fade_out}")
    } else {
        format!("afade=t=out:st={fade_start}:d={fade_out}")
    };
    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-i"])
        .arg(wav)
        .args([
            "-t",
            &format!("{duration}"),
            "-af",
            &filter,
            "-ar",
            &rate.to_string(),
            "-ac",
            &channels.to_string(),
            "-codec:a",
            "libmp3lame",
            "-q:a",
            "4",
        ])
        .arg(&tmp)
        .status()?;
    anyhow::ensure!(status.success(), "ffmpeg failed on {}", wav.display());
    let bytes = std::fs::read(&tmp)?;
    let _ = std::fs::remove_file(&tmp);
    Ok(bytes)
}

fn ffprobe_stream(path: &Path) -> Result<(u32, u32)> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=sample_rate,channels",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()?;
    anyhow::ensure!(out.status.success(), "ffprobe stream failed");
    let s = String::from_utf8_lossy(&out.stdout);
    let mut it = s.split_whitespace();
    let rate: u32 = it.next().context("sample_rate")?.parse()?;
    let channels: u32 = it.next().context("channels")?.parse()?;
    Ok((rate, channels))
}

fn ffprobe_duration(path: &Path) -> Result<f64> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()?;
    anyhow::ensure!(out.status.success(), "ffprobe duration failed");
    Ok(String::from_utf8_lossy(&out.stdout).trim().parse()?)
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
