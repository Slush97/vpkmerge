//! Swap one Deadlock sound clip with user-supplied MP3 audio.
//!
//! Deadlock VO / ability / weapon clips ship as `.vsnd_c` containers: a `CTRL`
//! KeyValues3 block describing the audio (`m_nRate`, `m_nChannels`, duration,
//! loop points, envelope) followed by the raw MP3 stream appended at the tail.
//! [`morphic::encode_vsnd_c`] mints a new `.vsnd_c` by reusing an existing clip as
//! a *donor* template and substituting fresh MP3 bytes + the matching audio
//! params, exactly the way [`crate::icon`] reuses a texture as a template. Packing
//! the result at the donor's own entry path overrides the clip in place: the
//! soundevent keeps pointing at the same path, the bytes there are now the user's.
//!
//! This is the Foundry sound-swap backbone (drop an MP3 on a hero's sound -> mint
//! -> pack an addon VPK -> install as a managed local mod). The mint technique is
//! in-game-proven (the music-pack / custom-`.vsnd_c` pipeline). v1 takes **MP3**
//! input: the audio params (rate / channels / duration) are parsed from the MP3
//! frame headers in pure Rust, so the tool stays a dependency-free standalone
//! binary (no ffmpeg at runtime). Transcoding other formats to MP3 is a caller
//! concern (a later enhancement can add it here behind a feature).

use anyhow::{bail, Context, Result};
use morphic::VsndParams;

/// Parse the audio parameters needed to mint a `.vsnd_c` straight from an MP3
/// byte stream: sample rate, channel count, and the total sample count / duration
/// (derived by walking the frame headers, so it is correct for both CBR and VBR).
///
/// `looped` selects whether the minted resource loops (a one-shot VO / ability
/// cast is `false`; music is `true`); it is carried through to [`VsndParams`].
///
/// # Errors
/// Fails if the bytes carry no decodable MPEG audio frame (not an MP3).
pub fn parse_mp3_params(mp3: &[u8], looped: bool) -> Result<VsndParams> {
    let start = skip_id3v2(mp3);
    let mut cursor = find_first_frame(mp3, start)
        .context("input is not MP3 (no MPEG audio frame sync found)")?;

    // Read the first frame for the stream-wide rate + channel count (constant
    // across an MP3), then walk every frame to total the sample count.
    let first = FrameHeader::parse(&mp3[cursor..])
        .context("input is not MP3 (first frame header is invalid)")?;
    let rate = first.sample_rate;
    let channels = first.channels;

    let mut total_samples: u64 = 0;
    while let Some(frame) = mp3.get(cursor..).and_then(FrameHeader::parse) {
        total_samples += u64::from(frame.samples_per_frame);
        let len = frame.frame_len();
        if len == 0 {
            break;
        }
        cursor += len;
    }

    if total_samples == 0 {
        bail!("input is not MP3 (no audio frames decoded)");
    }

    let sample_count = u32::try_from(total_samples).unwrap_or(u32::MAX);
    let duration = f64::from(sample_count) / f64::from(rate);

    Ok(VsndParams {
        rate,
        channels,
        sample_count,
        duration,
        looped,
    })
}

/// Whether a donor `.vsnd_c` clip is authored to loop (reads its
/// `m_vSound.m_nLoopStart`; `-1` = one-shot). A swap can inherit this so a
/// `..._loop` / music clip stays looping and a VO line stays one-shot, instead of
/// asking the caller to know.
///
/// # Errors
/// Fails if `donor` is not a readable `.vsnd_c` (no `CTRL` / `m_vSound`).
pub fn donor_is_looped(donor: &[u8]) -> Result<bool> {
    // Reached via the public `sound` module path rather than a top-level re-export
    // so this commit does not touch morphic/src/lib.rs.
    morphic::sound::vsnd_looped(donor)
        .map_err(|e| anyhow::anyhow!("reading donor loop flag failed: {e}"))
}

/// Mint a replacement `.vsnd_c` from `donor` (a template clip, typically the very
/// clip being overridden, read from the pak) and `mp3` (the user's audio). The
/// returned bytes pack back at the donor's entry path to override the clip in
/// place.
///
/// # Errors
/// Fails if `mp3` is not MP3, or if `donor` is not a mintable `.vsnd_c`
/// (`CVoiceContainerDefault` MP3 shape: a `CTRL` block + appended MP3).
pub fn mint_swapped_clip(donor: &[u8], mp3: &[u8], looped: bool) -> Result<Vec<u8>> {
    let params = parse_mp3_params(mp3, looped)?;
    morphic::encode_vsnd_c(donor, mp3, &params)
        .map_err(|e| anyhow::anyhow!("minting .vsnd_c from the donor failed: {e}"))
}

/// Skip a leading ID3v2 tag if present, returning the offset of the first byte
/// after it (or 0 when there is no tag). The tag size is a 28-bit syncsafe int.
fn skip_id3v2(data: &[u8]) -> usize {
    if data.len() < 10 || &data[0..3] != b"ID3" {
        return 0;
    }
    // Bytes 6..10 are a syncsafe size (7 bits per byte) of the tag body.
    let size = (u32::from(data[6]) << 21)
        | (u32::from(data[7]) << 14)
        | (u32::from(data[8]) << 7)
        | u32::from(data[9]);
    10 + size as usize
}

/// Find the offset of the first valid MPEG audio frame at or after `from`.
fn find_first_frame(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 4 <= data.len() {
        if data[i] == 0xFF && FrameHeader::parse(&data[i..]).is_some() {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// A decoded MPEG audio frame header (the fields we need to size the frame and
/// total samples). Layer III only is required for Deadlock clips, but the bitrate
/// / rate tables cover Layers I-III for robustness.
struct FrameHeader {
    sample_rate: u32,
    channels: u32,
    samples_per_frame: u32,
    bitrate_bps: u32,
    padding: u32,
    /// `144` for MPEG1, `72` for MPEG2 / 2.5 (= samples_per_frame / 8) for Layer
    /// III; Layers I/II differ but Deadlock clips are Layer III.
    coef: u32,
    layer1: bool,
}

impl FrameHeader {
    fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < 4 {
            return None;
        }
        // Sync: 11 set bits.
        if b[0] != 0xFF || (b[1] & 0xE0) != 0xE0 {
            return None;
        }
        let version = (b[1] >> 3) & 0x03; // 00=2.5, 10=2, 11=1 (01 reserved)
        let layer = (b[1] >> 1) & 0x03; // 01=III, 10=II, 11=I (00 reserved)
        if version == 0b01 || layer == 0b00 {
            return None;
        }
        let bitrate_idx = ((b[2] >> 4) & 0x0F) as usize;
        let rate_idx = ((b[2] >> 2) & 0x03) as usize;
        let padding = u32::from((b[2] >> 1) & 0x01);
        let chan_mode = (b[3] >> 6) & 0x03;
        if bitrate_idx == 0 || bitrate_idx == 0x0F || rate_idx == 0x03 {
            return None; // free-format / bad values
        }

        let is_v1 = version == 0b11;
        let layer3 = layer == 0b01;
        let layer1 = layer == 0b11;

        // Sample rate by version + index.
        const RATES_V1: [u32; 3] = [44100, 48000, 32000];
        const RATES_V2: [u32; 3] = [22050, 24000, 16000];
        const RATES_V25: [u32; 3] = [11025, 12000, 8000];
        let sample_rate = match version {
            0b11 => RATES_V1[rate_idx],
            0b10 => RATES_V2[rate_idx],
            _ => RATES_V25[rate_idx],
        };

        // Bitrate (kbps) by version + layer + index. Layer III tables (the
        // Deadlock case) are exact; Layer I/II reuse the V1-L1/L2 + V2 tables.
        const BR_V1_L3: [u32; 15] = [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ];
        const BR_V1_L1: [u32; 15] = [
            0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
        ];
        const BR_V2_L3: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
        const BR_V2_L1: [u32; 15] = [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
        ];
        // Layer II V1 has its own table, but Deadlock clips are Layer III; we
        // approximate any Layer II as its same-version Layer III/I table.
        let bitrate_kbps = match (is_v1, layer1) {
            (true, true) => BR_V1_L1[bitrate_idx],
            (true, false) => BR_V1_L3[bitrate_idx],
            (false, true) => BR_V2_L1[bitrate_idx],
            (false, false) => BR_V2_L3[bitrate_idx],
        };
        if bitrate_kbps == 0 {
            return None;
        }

        // Samples per frame + the byte-length coefficient (= spf / 8 for II/III):
        // Layer I = 384/12; MPEG2/2.5 Layer III = 576/72; everything else (MPEG1
        // any layer, MPEG2 Layer II) = 1152/144.
        let (samples_per_frame, coef) = if layer1 {
            (384, 12)
        } else if layer3 && !is_v1 {
            (576, 72)
        } else {
            (1152, 144)
        };

        let channels = if chan_mode == 0b11 { 1 } else { 2 };

        Some(FrameHeader {
            sample_rate,
            channels,
            samples_per_frame,
            bitrate_bps: bitrate_kbps * 1000,
            padding,
            coef,
            layer1,
        })
    }

    /// Frame length in bytes (Layer I rounds in 4-byte slots; II/III in bytes).
    fn frame_len(&self) -> usize {
        let n = self.coef * self.bitrate_bps / self.sample_rate;
        let len = if self.layer1 {
            (n + self.padding) * 4
        } else {
            n + self.padding
        };
        len as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a CBR MPEG1 Layer III frame (128 kbps, 44100 Hz, stereo) padded to
    /// its computed length with zeros. Header: FF FB 90 00.
    fn mpeg1_l3_frame() -> Vec<u8> {
        // frame_len = 144 * 128000 / 44100 = 417 (no padding).
        let mut f = vec![0xFF, 0xFB, 0x90, 0x00];
        f.resize(417, 0);
        f
    }

    #[test]
    fn parses_cbr_mpeg1_layer3() {
        let mut mp3 = Vec::new();
        for _ in 0..3 {
            mp3.extend_from_slice(&mpeg1_l3_frame());
        }
        let p = parse_mp3_params(&mp3, false).expect("parse");
        assert_eq!(p.rate, 44100);
        assert_eq!(p.channels, 2);
        // 3 frames * 1152 samples each.
        assert_eq!(p.sample_count, 3 * 1152);
        assert!((p.duration - (3.0 * 1152.0 / 44100.0)).abs() < 1e-6);
        assert!(!p.looped);
    }

    #[test]
    fn skips_id3v2_then_parses() {
        let mut mp3 = vec![b'I', b'D', b'3', 4, 0, 0, 0, 0, 0, 5]; // 10-byte header, body size 5
        mp3.extend_from_slice(&[0u8; 5]); // tag body
        mp3.extend_from_slice(&mpeg1_l3_frame());
        let p = parse_mp3_params(&mp3, true).expect("parse");
        assert_eq!(p.rate, 44100);
        assert_eq!(p.sample_count, 1152);
        assert!(p.looped);
    }

    #[test]
    fn rejects_non_mp3() {
        assert!(parse_mp3_params(b"this is not audio at all, no sync here", false).is_err());
    }
}
