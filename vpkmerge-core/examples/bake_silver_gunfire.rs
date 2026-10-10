//! Bake Silver's 60s-western gunfire + ability-1 cast sting into the existing
//! Psychedelic Kit. Mints the prepared clips to `.vsnd_c`, rewires the rifle-fire
//! and Slamfire-cast events (weighting common shots by repetition so a rare
//! flourish lands ~2% of the time), and merges the result over the Kit so every
//! other custom sound is preserved.
//!
//! Usage:
//!   cargo run --release --example bake_silver_gunfire -- <base_pak01_dir.vpk> <kit_dir.vpk> <clip_dir> <out_dir.vpk>

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use vpkmerge_core::{merge, MergeOptions};

const SND_ENTRY: &str = "soundevents/hero/werewolf.vsndevts_c";
const DONOR_ENTRY: &str = "sounds/weapons/werewolf/werewolf_wpn_rifle_fire_01.vsnd_c";
const GUN_DIR: &str = "sounds/custom/silver_psych/gun";

/// (event, weighted list of clip stems). A stem repeated N times is N/total
/// likely to play (the engine picks a uniformly random array index).
fn wiring() -> Vec<(&'static str, Vec<&'static str>)> {
    let rep = |stem: &'static str, n: usize| std::iter::repeat(stem).take(n);
    vec![
        // Normal fire: 3 western takes share ~98%, ricochet ~2% (1/49).
        (
            "Werewolf.Wpn.Rifle.Fire.Main",
            rep("western_a", 20)
                .chain(rep("western_b", 15))
                .chain(rep("western_c", 13))
                .chain(rep("ricochet", 1))
                .collect(),
        ),
        // First shot of a burst: bigger accent, occasional normal take.
        (
            "Werewolf.Wpn.Rifle.Fire.First",
            rep("first", 3)
                .chain(rep("western_a", 1))
                .chain(rep("western_b", 1))
                .collect(),
        ),
        // Slamfire unload: tight takes, fast + chaotic so the ricochet can hit ~5%.
        (
            "Werewolf.Wpn.Rifle.Fire.Slamfire",
            rep("slam_a", 10)
                .chain(rep("slam_b", 9))
                .chain(rep("ricochet", 1))
                .collect(),
        ),
        // Ability-1 activation sting (was vanilla).
        ("Werewolf.Slamfire.Cast.Delay", vec!["cast"]),
    ]
}

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let base = a
        .next()
        .context("usage: bake_silver_gunfire <base> <kit> <clip_dir> <out>")?;
    let kit = a.next().context("kit vpk")?;
    let clip_dir = a.next().context("clip_dir")?;
    let out = a.next().context("out vpk")?;

    let donor = vpkmerge_core::read_vpk_entry(&base, DONOR_ENTRY)
        .with_context(|| format!("reading donor {DONOR_ENTRY}"))?;

    // Unique stems referenced by the wiring -> one minted clip each.
    let wiring = wiring();
    let mut stems: Vec<&str> = wiring.iter().flat_map(|(_, v)| v.iter().copied()).collect();
    stems.sort_unstable();
    stems.dedup();

    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();
    for stem in &stems {
        let wav = PathBuf::from(&clip_dir).join(format!("{stem}.wav"));
        anyhow::ensure!(wav.exists(), "missing clip {}", wav.display());
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
            looped: false,
        };
        let vsnd = morphic::encode_vsnd_c(&donor, &mp3, &params)?;
        let entry = format!("{GUN_DIR}/{stem}.vsnd_c");
        println!(
            "  mint {stem:<10} {duration:.2}s {rate}Hz {channels}ch -> {} KB",
            vsnd.len() / 1024
        );
        packed.push((entry, vsnd));
    }

    // Rewire the events on the Kit's soundevents (keeps every other event intact).
    let mut snd = vpkmerge_core::SoundEvents::from_vpk(&kit, SND_ENTRY)
        .with_context(|| format!("loading {SND_ENTRY} from {kit}"))?;
    for (event, list) in &wiring {
        let paths: Vec<String> = list.iter().map(|s| format!("{GUN_DIR}/{s}.vsnd")).collect();
        anyhow::ensure!(snd.set_vsnd_files(event, &paths), "event {event} not found");
        let rares = list.iter().filter(|s| **s == "ricochet").count();
        println!(
            "  wire {event}: {} entries ({} ricochet)",
            list.len(),
            rares
        );
    }
    packed.push((SND_ENTRY.to_owned(), snd.encode()?));

    // Pack new clips + edited soundevents, then merge over the Kit (LastWins).
    let addon = std::env::temp_dir().join("silver_gun_addon_dir.vpk");
    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, addon.to_str().unwrap())?;
    merge(
        &[kit.clone(), addon.to_string_lossy().into_owned()],
        &out,
        &MergeOptions::default(),
    )?;
    println!(
        "\nbaked {} new clips + rewired {} events -> {out}",
        stems.len(),
        wiring.len()
    );
    Ok(())
}

fn wav_to_mp3(wav: &std::path::Path, rate: u32, channels: u32) -> Result<Vec<u8>> {
    let tmp = std::env::temp_dir().join(format!("bake_gun_{}.mp3", std::process::id()));
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
            "2",
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
    Ok((
        it.next().context("rate")?.parse()?,
        it.next().context("channels")?.parse()?,
    ))
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
