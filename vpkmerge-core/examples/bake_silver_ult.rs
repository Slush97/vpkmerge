//! Bake a Silver (Werewolf) ult sound randomizer: mint N custom clips to
//! `.vsnd_c`, point the ult **cast** event at all of them (engine randomizes per
//! play), and pack everything into one addon VPK.
//!
//! Clips are trimmed to TARGET_SECS (default ~6, matching Valve's 5.99s original
//! cast sound). This is deliberate and load-bearing: the mod replaces the sound on
//! every client, so a long replacement lets you hear *enemy* Silvers ult across the
//! map, which gets mods taken down from GameBanana (>10s). Short + on the cast event
//! (positional falloff) matches the original and stays publishable. We can't make a
//! long version safe: who hears the ult sound is server-side, not moddable.
//!
//! Usage:
//!   cargo run --release --example bake_silver_ult -- <base_pak01_dir.vpk> <wav_dir> <out_dir.vpk> [target_secs]

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};

const SND_ENTRY: &str = "soundevents/hero/werewolf.vsndevts_c";
// The cast (transform) event: a short, positional one-shot, like Valve's original.
const ULT_EVENT: &str = "Werewolf.Lycan.Curse.Cast";
/// Default clip length cap, in seconds. ~6 matches the original cast sound and
/// stays under GameBanana's ~10s enemy-audibility takedown threshold.
const DEFAULT_TARGET_SECS: f64 = 6.0;
/// Tail fade so the trimmed clip ends cleanly instead of hard-cutting.
const FADE_SECS: f64 = 0.5;
const DONOR_ENTRY: &str =
    "sounds/abilities/werewolf/a4_lycan_curse/werewolf_lycan_curse_cast.vsnd_c";
// New clips live beside the original. The soundevent lists the `.vsnd` form; the
// packed resource is the `.vsnd_c` form (the engine resolves one to the other).
const CLIP_DIR: &str = "sounds/abilities/werewolf/a4_lycan_curse";
const CLIP_STEM: &str = "silver_ult_rand";

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let base = a
        .next()
        .expect("usage: bake_silver_ult <base.vpk> <wav_dir> <out_dir.vpk> [target_secs]");
    let wav_dir = a.next().expect("wav_dir");
    let out = a.next().expect("out_dir.vpk");
    let target_secs: f64 = a.next().map_or(Ok(DEFAULT_TARGET_SECS), |s| s.parse())?;
    // Optional 5th arg picks the event the clips go on:
    //   "cast" (default) -> Werewolf.Lycan.Curse.Cast (short, GameBanana-safe)
    //   "loop"           -> Werewolf.Lycan.Curse.Ready.Warning (ambient loop, tied
    //                       to the form's real duration; pair with a large cap)
    let event = match a.next().as_deref() {
        Some("loop") => "Werewolf.Lycan.Curse.Ready.Warning",
        _ => ULT_EVENT,
    };
    println!("target event: {event}  |  clip length cap: {target_secs:.1}s (fade {FADE_SECS:.1}s)");

    // Collect WAVs in sorted order.
    let mut wavs: Vec<PathBuf> = std::fs::read_dir(&wav_dir)?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wav")))
        .collect();
    wavs.sort();
    anyhow::ensure!(!wavs.is_empty(), "no .wav files in {wav_dir}");
    println!("found {} clips", wavs.len());

    // Donor: the ult's own shipped clip.
    let donor = vpkmerge_core::read_vpk_entry(&base, DONOR_ENTRY)
        .with_context(|| format!("reading donor {DONOR_ENTRY}"))?;

    // Mint each clip; remember its `.vsnd` path for the soundevent.
    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();
    let mut vsnd_paths: Vec<String> = Vec::new();
    for (i, wav) in wavs.iter().enumerate() {
        let n = i + 1;
        let (rate, channels) = ffprobe_stream(wav)?;
        // Cap the clip at the target length; leave already-shorter clips alone.
        let duration = ffprobe_duration(wav)?.min(target_secs);
        let mp3 = wav_to_mp3(wav, rate, channels, duration)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let sample_count = (duration * f64::from(rate)).round() as u32;
        let params = morphic::VsndParams {
            rate,
            channels,
            sample_count,
            duration,
            looped: event == "Werewolf.Lycan.Curse.Ready.Warning",
        };
        let vsnd = morphic::encode_vsnd_c(&donor, &mp3, &params)?;

        let entry_c = format!("{CLIP_DIR}/{CLIP_STEM}_{n:02}.vsnd_c");
        let entry_src = format!("{CLIP_DIR}/{CLIP_STEM}_{n:02}.vsnd");
        println!(
            "  [{n:02}] {} -> {} ({:.1}s, {} Hz, {} ch, {} KB)",
            wav.file_name().unwrap().to_string_lossy(),
            entry_c,
            duration,
            rate,
            channels,
            vsnd.len() / 1024
        );
        packed.push((entry_c, vsnd));
        vsnd_paths.push(entry_src);
    }

    // Point the chosen event at all the new clips.
    let mut snd = vpkmerge_core::SoundEvents::from_vpk(&base, SND_ENTRY)
        .with_context(|| format!("loading {SND_ENTRY} from {base}"))?;
    anyhow::ensure!(
        snd.set_vsnd_files(event, &vsnd_paths),
        "event {event} not found in {SND_ENTRY}"
    );
    let snd_bytes = snd.encode()?;
    packed.push((SND_ENTRY.to_owned(), snd_bytes));

    // Pack the clips + the edited soundevents into one addon VPK.
    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &out)?;
    println!(
        "\nbaked {} clips + soundevents -> {} ({} entries)",
        vsnd_paths.len(),
        out,
        refs.len()
    );
    Ok(())
}

fn wav_to_mp3(wav: &std::path::Path, rate: u32, channels: u32, duration: f64) -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join("bake_silver_clip.mp3");
    // Cut to `duration` and fade the last FADE_SECS to silence so the clip ends
    // cleanly. `-t` bounds the output length; the vsnd metadata is computed from the
    // same `duration`, so they stay consistent.
    let fade_start = (duration - FADE_SECS).max(0.0);
    let afade = format!("afade=t=out:st={fade_start}:d={FADE_SECS}");
    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-i"])
        .arg(wav)
        .args([
            "-t",
            &format!("{duration}"),
            "-af",
            &afade,
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
    Ok(std::fs::read(&tmp)?)
}

fn ffprobe_stream(path: &std::path::Path) -> Result<(u32, u32)> {
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

fn ffprobe_duration(path: &std::path::Path) -> Result<f64> {
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
