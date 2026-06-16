//! Forge a `.vsnd_c` from a WAV using ffmpeg (WAV -> MP3) + a donor container.
//!
//! Usage: cargo run --release --example mint_vsnd -- <in.wav> <donor.vsnd_c> <out.vsnd_c>
//!
//! Proves the pure-Rust sound pipeline end to end: encode the audio to MP3, mint
//! a resource via `morphic::encode_vsnd_c`, then re-parse the result and assert
//! the metadata + appended MP3 survived.

use std::process::Command;

use anyhow::Context;

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let wav = a
        .next()
        .expect("usage: mint_vsnd <in.wav> <donor.vsnd_c> <out.vsnd_c>");
    let donor_path = a.next().expect("donor.vsnd_c");
    let out_path = a.next().expect("out.vsnd_c");

    // 1. WAV -> MP3, preserving the source's rate/channels.
    let mp3_path = format!("{out_path}.mp3");
    let (rate, channels) = ffprobe_stream(&wav)?;
    let status = Command::new("ffmpeg")
        .args([
            "-loglevel",
            "error",
            "-y",
            "-i",
            &wav,
            "-ar",
            &rate.to_string(),
            "-ac",
            &channels.to_string(),
            "-codec:a",
            "libmp3lame",
            "-q:a",
            "4",
            &mp3_path,
        ])
        .status()?;
    anyhow::ensure!(status.success(), "ffmpeg WAV->MP3 failed");
    let mp3 = std::fs::read(&mp3_path)?;

    // 2. Duration -> sample count.
    let duration = ffprobe_duration(&wav)?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let sample_count = (duration * f64::from(rate)).round() as u32;
    println!(
        "audio: {:.3}s, {} Hz, {} ch, mp3 {} bytes, {} samples",
        duration,
        rate,
        channels,
        mp3.len(),
        sample_count
    );

    // 3. Mint the resource from the donor.
    let donor = std::fs::read(&donor_path)?;
    let params = morphic::VsndParams {
        rate,
        channels,
        sample_count,
        duration,
        looped: false,
    };
    let vsnd = morphic::encode_vsnd_c(&donor, &mp3, &params)?;
    std::fs::write(&out_path, &vsnd)?;
    println!("wrote {} ({} bytes)", out_path, vsnd.len());

    // 4. Round-trip validation: re-parse and confirm the swap took.
    validate(&vsnd, &mp3, rate, sample_count)?;
    println!("OK: round-trip validated");
    Ok(())
}

fn ffprobe_stream(path: &str) -> anyhow::Result<(u32, u32)> {
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
            path,
        ])
        .output()?;
    anyhow::ensure!(out.status.success(), "ffprobe stream failed");
    let s = String::from_utf8_lossy(&out.stdout);
    let mut it = s.split_whitespace();
    let rate: u32 = it.next().context("sample_rate")?.parse()?;
    let channels: u32 = it.next().context("channels")?.parse()?;
    Ok((rate, channels))
}

fn ffprobe_duration(path: &str) -> anyhow::Result<f64> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()?;
    anyhow::ensure!(out.status.success(), "ffprobe failed");
    Ok(String::from_utf8_lossy(&out.stdout).trim().parse()?)
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn validate(vsnd: &[u8], mp3: &[u8], rate: u32, sample_count: u32) -> anyhow::Result<()> {
    let file_size = u32le(vsnd, 0) as usize;
    // The appended MP3 must sit exactly at file_size and match byte-for-byte.
    anyhow::ensure!(
        vsnd.len() == file_size + mp3.len(),
        "tail size mismatch: file_size={file_size} + mp3={} != total {}",
        mp3.len(),
        vsnd.len()
    );
    anyhow::ensure!(&vsnd[file_size..] == mp3, "appended MP3 bytes differ");

    // Decode the CTRL metadata back and check the rewritten fields.
    let block_offset = u32le(vsnd, 8) as usize;
    let block_count = u32le(vsnd, 12) as usize;
    let mut cur = 8 + block_offset;
    let mut found = false;
    for _ in 0..block_count {
        let kind = &vsnd[cur..cur + 4];
        let abs = cur + 4 + u32le(vsnd, cur + 4) as usize;
        let size = u32le(vsnd, cur + 8) as usize;
        if kind == b"CTRL" && size > 0 {
            let kv = morphic::kv3::decode(&vsnd[abs..abs + size])?;
            let snd = kv.get("m_vSound").expect("m_vSound");
            assert_eq!(
                snd.get("m_nRate").and_then(morphic::kv3::Value::as_int),
                Some(i64::from(rate))
            );
            assert_eq!(
                snd.get("m_nSampleCount")
                    .and_then(morphic::kv3::Value::as_uint),
                Some(u64::from(sample_count))
            );
            assert_eq!(
                snd.get("m_nStreamingSize")
                    .and_then(morphic::kv3::Value::as_uint),
                Some(mp3.len() as u64)
            );
            assert_eq!(
                snd.get("m_nFormat").and_then(morphic::kv3::Value::as_str),
                Some("MP3")
            );
            found = true;
        }
        cur += 12;
    }
    anyhow::ensure!(found, "no CTRL block in minted file");
    Ok(())
}
