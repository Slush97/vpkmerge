//! Bake custom urn (Idol) objective music: mint four songs into the four
//! shipped urn music stems and pack them at their existing entry paths.
//!
//! Deadlock's urn music is two adaptive loop events, each with two stems:
//!   Music.Idol.Pickup.Lp  (carry / "running it"):  close  + distant
//!   Music.Idol.Timer.Lp   (deposit timer):         your-team + opponent
//! The soundevents already reference these `.vsnd` paths, so overriding the
//! four compiled `.vsnd_c` in place re-skins the music with no soundevents edit.
//!
//! Each stem's own shipped clip is its donor (encode_vsnd_c patches the donor
//! CTRL with the new MP3 + duration). Clips are looped (matches the originals)
//! and kept full-length so the whole track is available during a long urn run.
//!
//! Usage:
//!   cargo run --release --example bake_urn_music -- <base_pak01_dir.vpk> <wav_dir> <out_dir.vpk>
//! where <wav_dir> holds run.wav, fight.wav, dropoff.wav, contested.wav.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// (stem `.vsnd_c` entry path, source wav basename, human label).
const STEMS: &[(&str, &str, &str)] = &[
    (
        "sounds/music/music_idol_carry_lp_141bpm.vsnd_c",
        "run",
        "carry close (Run)",
    ),
    (
        "sounds/music/music_idol_carry_distant_lp_141bpm.vsnd_c",
        "fight",
        "carry distant (Fight)",
    ),
    (
        "sounds/music/music_idol_timer_lp_team_160bpm.vsnd_c",
        "dropoff",
        "deposit timer, team (Drop off)",
    ),
    (
        "sounds/music/music_idol_timer_lp_opponent_160bpm.vsnd_c",
        "contested",
        "deposit timer, opponent (Fight Contested)",
    ),
];

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let base = a
        .next()
        .expect("usage: bake_urn_music <base_pak01_dir.vpk> <wav_dir> <out_dir.vpk>");
    let wav_dir = a.next().expect("wav_dir");
    let out = a.next().expect("out_dir.vpk");

    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();
    for (entry_c, stem, label) in STEMS {
        let wav = PathBuf::from(&wav_dir).join(format!("{stem}.wav"));
        anyhow::ensure!(wav.exists(), "missing {}", wav.display());

        // Donor = the stem's own shipped clip, so the CTRL envelope/format is
        // whatever the engine already expects for this music slot.
        let donor = vpkmerge_core::read_vpk_entry(&base, entry_c)
            .with_context(|| format!("reading donor {entry_c}"))?;

        let (rate, channels) = ffprobe_stream(&wav)?;
        let duration = ffprobe_duration(&wav)?;
        let mp3 = wav_to_mp3(&wav, rate, channels)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let sample_count = (duration * f64::from(rate)).round() as u32;
        let params = morphic::VsndParams {
            rate,
            channels,
            sample_count,
            duration,
            looped: true,
        };
        let vsnd = morphic::encode_vsnd_c(&donor, &mp3, &params)?;
        println!(
            "  {label:<40} <- {stem}.wav ({duration:6.1}s, {rate} Hz, {channels} ch, {} KB)",
            vsnd.len() / 1024
        );
        packed.push(((*entry_c).to_owned(), vsnd));
    }

    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &out)?;
    println!("\nbaked {} urn music stems -> {out}", refs.len());
    Ok(())
}

fn wav_to_mp3(wav: &Path, rate: u32, channels: u32) -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!(
        "bake_urn_{}.mp3",
        wav.file_stem().unwrap().to_string_lossy()
    ));
    let status = Command::new("ffmpeg")
        .args(["-loglevel", "error", "-y", "-i"])
        .arg(wav)
        .args([
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
