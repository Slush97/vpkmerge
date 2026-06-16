//! Build a Deadlock music/SFX addon VPK from a pack manifest.
//!
//! Generic, manifest-driven builder for the `music-packs/*` scaffolds
//! (title-fight, pokemon, ...). For each manifest entry it:
//!   1. trims/loops the local source audio to the target length and applies
//!      fade in/out (ffmpeg),
//!   2. mints a `.vsnd_c` from the trimmed MP3 using a donor music container
//!      (`morphic::encode_vsnd_c`),
//!   3. retargets the named Deadlock soundevent(s) at the new `.vsnd` path
//!      (`vpkmerge_core::SoundEvents`),
//! then packs every minted clip plus the edited soundevent files into one addon
//! VPK (`vpkmerge_core::pack`). Same shape as `bake_silver_ult.rs`, fed by a
//! manifest instead of hardcoded paths.
//!
//! Usage:
//!   cargo run --release --example build_music_pack -- \
//!     <manifest.json> <base_pak01_dir.vpk> <out_dir.vpk> [--donor ENTRY] [--dry-run]
//!
//! Manifest entries may live under `entries`, `music_entries`, and/or
//! `sfx_entries`. Each entry needs `local_audio`, `target_vsnd`,
//! `target_vsnd_c`, an `edit` block (`target_seconds`, optional `loop`,
//! `fade_in_seconds`, `fade_out_seconds`), and one or more events via
//! `deadlock_event` (string) or `deadlock_events` (array). Entries may set
//! `deadlock_field` for layered events; it defaults to `vsnd_files`. The
//! soundevent file is taken from each entry's `soundevent_entry`, else the
//! manifest's `default_soundevent`, else `soundevents/music.vsndevts_c`.
//!
//! Entries whose `local_audio` file is missing are skipped with a warning, so a
//! partially-populated pack still builds.

#![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;

/// A stock streamed-MP3 music container (`CVoiceContainerDefault`, `m_nFormat =
/// MP3`): the right donor shape for `encode_vsnd_c`. Overridable with `--donor`.
const DEFAULT_DONOR: &str = "sounds/music/music_menu_lp.vsnd_c";
/// Where the title-fight manifest's events live when no `soundevent_entry` is given.
const DEFAULT_SOUNDEVENT: &str = "soundevents/music.vsndevts_c";

struct Entry {
    role: String,
    local_audio: PathBuf,
    target_vsnd: String,
    target_vsnd_c: String,
    soundevent: String,
    field: String,
    events: Vec<String>,
    target_secs: f64,
    start_secs: f64,
    loop_clip: bool,
    fade_in: f64,
    fade_out: f64,
    event_fields: BTreeMap<String, f64>,
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let manifest_path = PathBuf::from(args.next().context(
        "usage: build_music_pack <manifest.json> <base.vpk> <out_dir.vpk> [--donor ENTRY] [--dry-run]",
    )?);
    let base = args.next().context("missing base pak01_dir.vpk")?;
    let out = args.next().context("missing out_dir.vpk")?;

    let mut donor_entry = DEFAULT_DONOR.to_owned();
    let mut dry_run = false;
    let rest: Vec<String> = args.collect();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--donor" => {
                donor_entry = rest.get(i + 1).context("--donor needs a value")?.clone();
                i += 2;
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            other => bail!("unknown arg: {other}"),
        }
    }

    let manifest_dir = manifest_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let manifest: Value = serde_json::from_slice(&std::fs::read(&manifest_path)?)
        .with_context(|| format!("parsing {}", manifest_path.display()))?;

    let default_se = manifest
        .get("default_soundevent")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_SOUNDEVENT)
        .to_owned();

    // Entries can live under any of these top-level arrays (title-fight uses
    // `entries`; pokemon splits `music_entries` + `sfx_entries`).
    let mut entries = Vec::new();
    for key in ["entries", "music_entries", "sfx_entries"] {
        if let Some(arr) = manifest.get(key).and_then(Value::as_array) {
            for e in arr {
                entries.push(parse_entry(e, &manifest_dir, &default_se)?);
            }
        }
    }
    ensure!(
        !entries.is_empty(),
        "manifest has no entries / music_entries / sfx_entries"
    );

    if dry_run {
        println!(
            "plan: {} manifest clip(s); donor = {donor_entry}",
            entries.len()
        );
        for e in &entries {
            println!(
                "  {:<28} {} -> {}  [{} event(s), field={}, start={:.1}s, {:.1}s, loop={}, fade {:.2}/{:.2}] @ {}{}",
                e.role,
                e.local_audio.display(),
                e.target_vsnd_c,
                e.events.len(),
                e.field,
                e.start_secs,
                e.target_secs,
                e.loop_clip,
                e.fade_in,
                e.fade_out,
                e.soundevent,
                if e.local_audio.exists() { "" } else { " [missing audio]" },
            );
        }
        return Ok(());
    }

    let (present, missing): (Vec<Entry>, Vec<Entry>) =
        entries.into_iter().partition(|e| e.local_audio.exists());
    for m in &missing {
        eprintln!("[skip] {} (missing {})", m.role, m.local_audio.display());
    }
    ensure!(
        !present.is_empty(),
        "no entries have local audio under {} (populate source_audio first)",
        manifest_dir.display()
    );

    println!(
        "building {} clip(s){}; donor = {donor_entry}",
        present.len(),
        if missing.is_empty() {
            String::new()
        } else {
            format!(" ({} skipped, no audio)", missing.len())
        }
    );

    let donor = vpkmerge_core::read_vpk_entry(&base, &donor_entry)
        .with_context(|| format!("reading donor {donor_entry} from {base}"))?;

    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();
    // soundevent file -> (event, field) -> new .vsnd paths. Duplicate manifest
    // entries for the same event/field become engine-side random choices.
    let mut retargets: BTreeMap<String, BTreeMap<(String, String), Vec<String>>> = BTreeMap::new();
    let mut event_durations: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    let mut event_fields: BTreeMap<String, BTreeMap<String, BTreeMap<String, f64>>> =
        BTreeMap::new();

    for e in &present {
        let (rate, channels) = ffprobe_stream(&e.local_audio)?;
        let src_dur = ffprobe_duration(&e.local_audio)?;
        // Loop clips fill out to the full target; one-shots cap at the source.
        let duration = if e.loop_clip {
            e.target_secs
        } else {
            let remaining = (src_dur - e.start_secs).max(0.0);
            remaining.min(e.target_secs)
        };
        ensure!(
            duration > 0.0,
            "{} starts at {:.2}s but {} is only {:.2}s long",
            e.role,
            e.start_secs,
            e.local_audio.display(),
            src_dur
        );
        let mp3 = prep_audio(e, rate, channels, duration)?;
        let sample_count = (duration * f64::from(rate)).round() as u32;
        let params = morphic::VsndParams {
            rate,
            channels,
            sample_count,
            duration,
            looped: e.loop_clip,
        };
        let vsnd = morphic::encode_vsnd_c(&donor, &mp3, &params)
            .with_context(|| format!("minting {}", e.target_vsnd_c))?;
        println!(
            "  [{:<20}] {} -> {} ({:.1}s, {} Hz, {} ch, {} KB)",
            e.role,
            e.local_audio.file_name().unwrap().to_string_lossy(),
            e.target_vsnd_c,
            duration,
            rate,
            channels,
            vsnd.len() / 1024
        );
        packed.push((e.target_vsnd_c.clone(), vsnd));
        for ev in &e.events {
            retargets
                .entry(e.soundevent.clone())
                .or_default()
                .entry((ev.clone(), e.field.clone()))
                .or_default()
                .push(e.target_vsnd.clone());
            event_durations
                .entry(e.soundevent.clone())
                .or_default()
                .entry(ev.clone())
                .and_modify(|max_duration| *max_duration = max_duration.max(duration))
                .or_insert(duration);
            for (field, value) in &e.event_fields {
                event_fields
                    .entry(e.soundevent.clone())
                    .or_default()
                    .entry(ev.clone())
                    .or_default()
                    .insert(field.clone(), *value);
            }
        }
    }

    // Edit each soundevent file once, applying all of its retargets.
    for (se_entry, edits) in &retargets {
        let mut snd = vpkmerge_core::SoundEvents::from_vpk(&base, se_entry)
            .with_context(|| format!("loading {se_entry} from {base}"))?;
        for ((event, field), paths) in edits {
            ensure!(
                snd.set_string_array_field(event, field, paths),
                "event {event} not found in {se_entry}"
            );
            if paths.len() > 1 {
                println!(
                    "  [random] {se_entry}: {event}.{field} = {} choices",
                    paths.len()
                );
            }
        }
        if let Some(durations) = event_durations.get(se_entry) {
            for (event, duration) in durations {
                let _ = snd.set_event_field(event, "vsnd_duration", *duration);
            }
        }
        if let Some(overrides) = event_fields.get(se_entry) {
            for (event, fields) in overrides {
                for (field, value) in fields {
                    let _ = snd.set_event_field(event, field, *value);
                }
            }
        }
        let bytes = snd
            .encode()
            .with_context(|| format!("re-encoding {se_entry}"))?;
        println!(
            "  [soundevent] {se_entry}: {} event field(s) retargeted",
            edits.len()
        );
        packed.push((se_entry.clone(), bytes));
    }

    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &out)?;
    println!(
        "\npacked {} clip(s) + {} soundevent file(s) -> {} ({} entries)",
        present.len(),
        retargets.len(),
        out,
        refs.len()
    );
    Ok(())
}

fn parse_entry(e: &Value, dir: &Path, default_se: &str) -> Result<Entry> {
    let role = e
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_owned();
    let str_field = |k: &str| -> Result<String> {
        Ok(e.get(k)
            .and_then(Value::as_str)
            .with_context(|| format!("entry {role} missing {k}"))?
            .to_owned())
    };
    let local_audio = dir.join(str_field("local_audio")?);
    let target_vsnd = str_field("target_vsnd")?;
    let target_vsnd_c = str_field("target_vsnd_c")?;
    let soundevent = e
        .get("soundevent_entry")
        .and_then(Value::as_str)
        .unwrap_or(default_se)
        .to_owned();
    let field = e
        .get("deadlock_field")
        .and_then(Value::as_str)
        .unwrap_or("vsnd_files")
        .to_owned();

    let mut events = Vec::new();
    if let Some(s) = e.get("deadlock_event").and_then(Value::as_str) {
        events.push(s.to_owned());
    }
    if let Some(arr) = e.get("deadlock_events").and_then(Value::as_array) {
        events.extend(arr.iter().filter_map(|v| v.as_str().map(str::to_owned)));
    }
    ensure!(
        !events.is_empty(),
        "entry {role} has no deadlock_event / deadlock_events"
    );

    let edit = e.get("edit").cloned().unwrap_or(Value::Null);
    let target_secs = edit
        .get("target_seconds")
        .and_then(Value::as_f64)
        .with_context(|| format!("entry {role} needs edit.target_seconds"))?;
    let start_secs = edit
        .get("start_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let loop_clip = edit.get("loop").and_then(Value::as_bool).unwrap_or(false);
    let fade_in = edit
        .get("fade_in_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let fade_out = edit
        .get("fade_out_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let event_fields = e
        .get("soundevent_fields")
        .and_then(Value::as_object)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|(key, value)| value.as_f64().map(|n| (key.clone(), n)))
                .collect()
        })
        .unwrap_or_default();

    Ok(Entry {
        role,
        local_audio,
        target_vsnd,
        target_vsnd_c,
        soundevent,
        field,
        events,
        target_secs,
        start_secs,
        loop_clip,
        fade_in,
        fade_out,
        event_fields,
    })
}

/// Trim (and optionally loop) the source to `duration`, fade in/out, emit MP3.
fn prep_audio(e: &Entry, rate: u32, channels: u32, duration: f64) -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join("build_music_pack_clip.mp3");

    let mut filters: Vec<String> = Vec::new();
    if e.fade_in > 0.0 {
        filters.push(format!("afade=t=in:st=0:d={}", e.fade_in));
    }
    if e.fade_out > 0.0 {
        let st = (duration - e.fade_out).max(0.0);
        filters.push(format!("afade=t=out:st={st}:d={}", e.fade_out));
    }
    let af = filters.join(",");

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-loglevel", "error", "-y"]);
    if e.loop_clip {
        // Repeat the input so a short clip can fill the whole target window.
        cmd.args(["-stream_loop", "-1"]);
    }
    if e.start_secs > 0.0 {
        cmd.args(["-ss", &format!("{}", e.start_secs)]);
    }
    cmd.arg("-i").arg(&e.local_audio);
    cmd.args(["-t", &format!("{duration}")]);
    if !af.is_empty() {
        cmd.args(["-af", &af]);
    }
    cmd.args([
        "-ar",
        &rate.to_string(),
        "-ac",
        &channels.to_string(),
        "-codec:a",
        "libmp3lame",
        "-q:a",
        "4",
    ]);
    cmd.arg(&tmp);

    let status = cmd.status()?;
    ensure!(
        status.success(),
        "ffmpeg failed on {}",
        e.local_audio.display()
    );
    Ok(std::fs::read(&tmp)?)
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
    ensure!(
        out.status.success(),
        "ffprobe stream failed on {}",
        path.display()
    );
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
    ensure!(
        out.status.success(),
        "ffprobe duration failed on {}",
        path.display()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().parse()?)
}
