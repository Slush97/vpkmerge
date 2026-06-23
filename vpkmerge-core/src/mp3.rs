//! Pure-Rust MP3 stream editing for the sound-import pipeline: **trim** (cut a
//! clip to a time window) and **gain** (loudness-match an import to the sound it
//! replaces). Both operate on the raw MPEG audio frames without decoding to PCM
//! or re-encoding, so the tool stays a dependency-free standalone binary (no
//! ffmpeg / no MP3 encoder), the same constraint [`crate::soundswap`] mints under.
//!
//! - **Trim** slices whole frames between the frame boundaries nearest the
//!   requested in/out points (~26 ms granularity for a 44.1 kHz Layer III clip,
//!   well inside the precision a sound swap needs). It is a byte-range copy of the
//!   selected frames, so the audio is bit-identical inside the window.
//! - **Gain** is the lossless mp3gain technique: every Layer III granule's
//!   `global_gain` field (the per-granule quantizer scale) is shifted by a
//!   constant number of steps, where one step is `~1.505 dB`. No samples are
//!   touched, so it is reversible and introduces no transcode loss; the only cost
//!   is `1.505 dB` quantization of the requested gain and saturation at the
//!   field's `0..=255` range. CRC-protected frames have their checksum recomputed.
//!
//! The frame header parser ([`FrameHeader`]) and the ID3 / sync helpers are
//! shared with [`crate::soundswap::parse_mp3_params`].

use anyhow::{bail, Context, Result};

/// One step of MP3 `global_gain` scales the requantized values by `2^0.25`, i.e.
/// `20 * log10(2^0.25) ~= 1.505 dB`. The mp3gain step size.
const GAIN_STEP_DB: f64 = 1.505_149_978_319_906;

// MPEG audio sample-rate tables, indexed by the header's rate index, per version.
const RATES_V1: [u32; 3] = [44100, 48000, 32000];
const RATES_V2: [u32; 3] = [22050, 24000, 16000];
const RATES_V25: [u32; 3] = [11025, 12000, 8000];

// Bitrate (kbps) tables indexed by the header's bitrate index. Layer III tables
// (the Deadlock case) are exact; Layer I/II reuse the V1-L1 / V2 tables.
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

/// Bit offset of `global_gain` within a Layer III granule-channel side-info
/// block: after `part2_3_length` (12 bits) and `big_values` (9 bits).
const GLOBAL_GAIN_BIT: usize = 21;

/// Trim an MP3 to the `[start_ms, end_ms)` window by copying the whole frames
/// that fall in it. The cut snaps to frame boundaries (~26 ms for a 44.1 kHz
/// Layer III clip), which is far finer than a sound swap needs and keeps the kept
/// audio bit-identical (no re-encode).
///
/// The returned stream starts at the first frame whose audio reaches past
/// `start_ms` and ends at the first frame that begins at or after `end_ms`. Any
/// leading `ID3v2` tag is dropped (the audio params are re-derived from the frames
/// by [`crate::soundswap::parse_mp3_params`] downstream, so the tag is dead
/// weight).
///
/// # Errors
/// Fails if `end_ms <= start_ms`, if the input carries no MPEG audio frame (not
/// an MP3), or if the window selects no audio.
pub fn trim_mp3(data: &[u8], start_ms: u32, end_ms: u32) -> Result<Vec<u8>> {
    if end_ms <= start_ms {
        bail!("trim end ({end_ms} ms) must be after trim start ({start_ms} ms)");
    }
    let lo_ms = f64::from(start_ms);
    let hi_ms = f64::from(end_ms);

    let begin =
        find_first_frame(data, skip_id3v2(data)).context("input is not MP3 (no frame sync)")?;
    let rate = f64::from(
        FrameHeader::parse(&data[begin..])
            .context("input is not MP3 (bad first frame header)")?
            .sample_rate,
    );
    let ms_per_sample = 1000.0 / rate;

    let mut cum_samples = 0f64;
    let mut cursor = begin;
    let mut out_start: Option<usize> = None;
    let mut out_end = data.len();
    let mut last_frame_end = begin;
    let mut stopped_early = false;

    while let Some(frame) = data.get(cursor..).and_then(FrameHeader::parse) {
        let len = frame.frame_len();
        if len == 0 {
            break;
        }
        let frame_start_ms = cum_samples * ms_per_sample;
        cum_samples += f64::from(frame.samples_per_frame);
        let frame_end_ms = cum_samples * ms_per_sample;

        // First frame whose audio extends past the in-point starts the cut.
        if out_start.is_none() && frame_end_ms > lo_ms {
            out_start = Some(cursor);
        }
        // First frame that begins at/after the out-point ends it (exclusive).
        if frame_start_ms >= hi_ms {
            out_end = cursor;
            stopped_early = true;
            break;
        }
        last_frame_end = cursor + len;
        cursor += len;
    }
    if !stopped_early {
        out_end = last_frame_end;
    }

    let s = out_start.unwrap_or(begin);
    if out_end <= s {
        bail!("trim window {start_ms}..{end_ms} ms selects no audio");
    }
    Ok(data[s..out_end].to_vec())
}

/// Apply a constant `gain_db` to an MP3 losslessly by shifting every Layer III
/// granule's `global_gain` (the mp3gain technique). Positive boosts, negative
/// attenuates; `0` (or a gain that rounds to zero steps) returns the input
/// unchanged. The applied gain is quantized to the nearest `~1.505 dB` step and
/// saturates where a frame's field would leave `0..=255` (so an extreme boost on
/// an already-hot clip cannot push past the field's ceiling). No PCM is decoded.
///
/// # Errors
/// Fails if the input carries no MPEG audio frame (not an MP3).
pub fn apply_mp3_gain(data: &[u8], gain_db: f64) -> Result<Vec<u8>> {
    let steps = gain_db_to_steps(gain_db);
    if steps == 0 {
        return Ok(data.to_vec());
    }
    let mut out = data.to_vec();
    let begin =
        find_first_frame(&out, skip_id3v2(&out)).context("input is not MP3 (no frame sync)")?;

    let mut cursor = begin;
    while let Some(frame) = out.get(cursor..).and_then(FrameHeader::parse) {
        let len = frame.frame_len();
        if len == 0 || cursor + len > out.len() {
            break;
        }
        adjust_frame_global_gain(&mut out[cursor..cursor + len], &frame, steps);
        cursor += len;
    }
    Ok(out)
}

/// Convert a decibel gain to the nearest whole `global_gain` step (`~1.505 dB`).
/// Clamped to a range that comfortably covers the field's `0..=255` span so the
/// `f64 -> i32` conversion cannot overflow.
#[allow(clippy::cast_possible_truncation)]
fn gain_db_to_steps(gain_db: f64) -> i32 {
    if !gain_db.is_finite() || gain_db.abs() < 1e-6 {
        return 0;
    }
    (gain_db / GAIN_STEP_DB).round().clamp(-512.0, 512.0) as i32
}

/// Shift every Layer III `global_gain` field in one frame by `steps`, clamping to
/// `0..=255`, then recompute the frame CRC if the frame is CRC-protected. Non
/// Layer III frames (not produced for Deadlock clips) are left untouched.
fn adjust_frame_global_gain(frame: &mut [u8], header: &FrameHeader, steps: i32) {
    if !header.layer3 {
        return;
    }
    let nch = header.channels as usize;
    let si_offset = 4 + if header.crc_protected { 2 } else { 0 };

    // Layer III side-info layout is constant-width per granule-channel (the
    // window-switching and normal branches are both 22 bits), so each block is a
    // fixed size and `global_gain` sits at a fixed bit offset (21) inside it.
    let (header_bits, block_bits, blocks, si_len) = if header.is_v1 {
        let hb = 9 + if nch == 1 { 5 } else { 3 } + 4 * nch;
        (hb, 59usize, 2 * nch, if nch == 1 { 17 } else { 32 })
    } else {
        let hb = 8 + if nch == 1 { 1 } else { 2 };
        (hb, 63usize, nch, if nch == 1 { 9 } else { 17 })
    };

    if frame.len() < si_offset + si_len {
        return;
    }
    {
        let si = &mut frame[si_offset..si_offset + si_len];
        for blk in 0..blocks {
            let bitpos = header_bits + blk * block_bits + GLOBAL_GAIN_BIT;
            // `global_gain` is an 8-bit field (0..=255), so it fits an i32 for the
            // shift and converts back without loss after clamping.
            let g = i32::try_from(read_bits(si, bitpos, 8)).unwrap_or(0);
            let ng = u32::try_from((g + steps).clamp(0, 255)).unwrap_or(0);
            write_bits(si, bitpos, 8, ng);
        }
    }
    if header.crc_protected {
        let crc = mpeg_crc16(frame, si_offset, si_len);
        frame[4] = (crc >> 8) as u8;
        frame[5] = (crc & 0xFF) as u8;
    }
}

/// Read an `n`-bit big-endian field at `bitpos` (bits from the slice start).
fn read_bits(data: &[u8], bitpos: usize, n: usize) -> u32 {
    let mut v = 0u32;
    for i in 0..n {
        let bp = bitpos + i;
        let bit = (data[bp / 8] >> (7 - (bp % 8))) & 1;
        v = (v << 1) | u32::from(bit);
    }
    v
}

/// Write an `n`-bit big-endian `val` at `bitpos` (bits from the slice start).
fn write_bits(data: &mut [u8], bitpos: usize, n: usize, val: u32) {
    for i in 0..n {
        let bp = bitpos + i;
        let bit = ((val >> (n - 1 - i)) & 1) as u8;
        let mask = 1u8 << (7 - (bp % 8));
        if bit == 1 {
            data[bp / 8] |= mask;
        } else {
            data[bp / 8] &= !mask;
        }
    }
}

/// The MPEG audio frame CRC-16 (poly `0x8005`, init `0xFFFF`): computed over the
/// two protected header bytes (`frame[2..4]`) and the side-info bytes, the field
/// itself excluded. `si_offset` is where the side info begins (`6` when CRC is
/// present); `si_len` is the side-info byte count.
fn mpeg_crc16(frame: &[u8], si_offset: usize, si_len: usize) -> u16 {
    let mut crc = 0xFFFFu16;
    let mut update = |byte: u8| {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    };
    update(frame[2]);
    update(frame[3]);
    for &b in &frame[si_offset..si_offset + si_len] {
        update(b);
    }
    crc
}

/// Skip a leading `ID3v2` tag if present, returning the offset of the first byte
/// after it (or 0 when there is no tag). The tag size is a 28-bit syncsafe int.
pub(crate) fn skip_id3v2(data: &[u8]) -> usize {
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
pub(crate) fn find_first_frame(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 4 <= data.len() {
        if data[i] == 0xFF && FrameHeader::parse(&data[i..]).is_some() {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// A decoded MPEG audio frame header (the fields needed to size the frame, total
/// samples, and locate the Layer III side info). Layer III only is required for
/// Deadlock clips, but the bitrate / rate tables cover Layers I-III for
/// robustness.
#[allow(clippy::struct_excessive_bools)] // distinct MPEG header flags, not state
pub(crate) struct FrameHeader {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u32,
    pub(crate) samples_per_frame: u32,
    bitrate_bps: u32,
    padding: u32,
    /// `144` for MPEG1, `72` for MPEG2 / 2.5 (= `samples_per_frame` / 8) for Layer
    /// III; Layers I/II differ but Deadlock clips are Layer III.
    coef: u32,
    layer1: bool,
    /// MPEG version 1 (vs MPEG2 / 2.5), which sets the side-info width.
    is_v1: bool,
    /// Layer III (the only layer with the `global_gain` side info we edit).
    layer3: bool,
    /// CRC protection present (a 16-bit checksum follows the 4-byte header).
    crc_protected: bool,
}

impl FrameHeader {
    pub(crate) fn parse(b: &[u8]) -> Option<Self> {
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
        let crc_protected = (b[1] & 0x01) == 0; // protection bit: 0 = CRC present
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
        let sample_rate = match version {
            0b11 => RATES_V1[rate_idx],
            0b10 => RATES_V2[rate_idx],
            _ => RATES_V25[rate_idx],
        };

        // Bitrate (kbps) by version + layer + index. Layer II V1 has its own
        // table, but Deadlock clips are Layer III; we approximate any Layer II as
        // its same-version Layer III/I table.
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
            is_v1,
            layer3,
            crc_protected,
        })
    }

    /// Frame length in bytes (Layer I rounds in 4-byte slots; II/III in bytes).
    pub(crate) fn frame_len(&self) -> usize {
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
    use crate::soundswap::parse_mp3_params;

    /// Build a CBR MPEG1 Layer III frame (128 kbps, 44100 Hz, stereo, no CRC)
    /// whose side-info bytes are zero. Header: FF FB 90 00. Length 417.
    fn mpeg1_l3_frame() -> Vec<u8> {
        let mut f = vec![0xFF, 0xFB, 0x90, 0x00];
        f.resize(417, 0);
        f
    }

    fn stream(frames: usize) -> Vec<u8> {
        let mut mp3 = Vec::new();
        for _ in 0..frames {
            mp3.extend_from_slice(&mpeg1_l3_frame());
        }
        mp3
    }

    #[test]
    fn trim_keeps_frames_in_window() {
        // ~522 ms total. Ask for ~100..300 ms: snaps to frames 4..12 (4*26=104, stop at >=300).
        let mp3 = stream(20);
        let out = trim_mp3(&mp3, 100, 300).expect("trim");
        let p = parse_mp3_params(&out, false).expect("parse trimmed");
        let frames = out.len() / 417;
        // One frame is 1152/44100 Hz ~= 26.12 ms. The in-point 100 ms first lands inside
        // frame 3 (its end 4*26.12 = 104.5 > 100); the out-point 300 ms is the
        // start of frame 12 (12*26.12 = 313 >= 300). So frames 3..12 = 9 frames.
        assert_eq!(frames, 9);
        assert_eq!(p.sample_count, 9 * 1152);
    }

    #[test]
    fn trim_to_end_clamps() {
        let mp3 = stream(10);
        // End beyond the stream keeps everything from the in-point on.
        let out = trim_mp3(&mp3, 0, 100_000).expect("trim");
        assert_eq!(out.len(), mp3.len());
    }

    #[test]
    fn trim_rejects_inverted_window() {
        let mp3 = stream(5);
        assert!(trim_mp3(&mp3, 200, 100).is_err());
        assert!(trim_mp3(&mp3, 100, 100).is_err());
    }

    #[test]
    fn gain_zero_is_identity() {
        let mp3 = stream(3);
        assert_eq!(apply_mp3_gain(&mp3, 0.0).unwrap(), mp3);
        // A gain smaller than half a step rounds to zero -> unchanged.
        assert_eq!(apply_mp3_gain(&mp3, 0.5).unwrap(), mp3);
    }

    /// Read the first granule's `global_gain` from a zero-side-info MPEG1 stereo
    /// frame (`header_bits` 20, block offset 21 -> bit 41), 8 bits.
    fn first_global_gain(frame: &[u8]) -> u32 {
        read_bits(&frame[4..], 41, 8)
    }

    #[test]
    fn gain_shifts_global_gain_by_steps() {
        let mut frame = mpeg1_l3_frame();
        // Seed a mid-range global_gain so we can shift both ways without clamping.
        // First granule-channel global_gain is at bit 41 of the side info.
        write_bits(&mut frame[4..], 41, 8, 100);
        for blk in 1..4 {
            write_bits(&mut frame[4..], 20 + blk * 59 + 21, 8, 100);
        }
        assert_eq!(first_global_gain(&frame), 100);

        // +3.01 dB ~= 2 steps.
        let up = apply_mp3_gain(&frame, 2.0 * GAIN_STEP_DB).unwrap();
        assert_eq!(first_global_gain(&up), 102);

        // Round-trip: up then down returns to the seed.
        let back = apply_mp3_gain(&up, -2.0 * GAIN_STEP_DB).unwrap();
        assert_eq!(first_global_gain(&back), 100);
        assert_eq!(back, frame);
    }

    #[test]
    fn gain_clamps_at_ceiling_and_floor() {
        let mut frame = mpeg1_l3_frame();
        for blk in 0..4 {
            write_bits(&mut frame[4..], 20 + blk * 59 + 21, 8, 254);
        }
        let up = apply_mp3_gain(&frame, 50.0).unwrap(); // far past the ceiling
        assert_eq!(first_global_gain(&up), 255);

        let mut quiet = mpeg1_l3_frame();
        for blk in 0..4 {
            write_bits(&mut quiet[4..], 20 + blk * 59 + 21, 8, 1);
        }
        let down = apply_mp3_gain(&quiet, -50.0).unwrap();
        assert_eq!(first_global_gain(&down), 0);
    }

    #[test]
    fn gain_keeps_frames_parseable() {
        let mp3 = stream(5);
        let out = apply_mp3_gain(&mp3, 6.0).unwrap();
        let p = parse_mp3_params(&out, false).expect("still parses");
        assert_eq!(p.sample_count, 5 * 1152);
        assert_eq!(out.len(), mp3.len()); // gain never changes byte length
    }

    #[test]
    fn crc16_recomputed_for_protected_frame() {
        // Flip the protection bit on (FB -> FA: low bit 0 = CRC present) and make
        // room for the 2 CRC bytes. The frame length formula is unchanged, so we
        // keep 417 bytes; side info is now 32 bytes at offset 6.
        let mut frame = vec![0xFF, 0xFA, 0x90, 0x00];
        frame.resize(417, 0);
        let header = FrameHeader::parse(&frame).expect("header");
        assert!(header.crc_protected);

        let out = apply_mp3_gain(&frame, 3.0).unwrap();
        // The stored CRC must match a fresh recompute over the modified frame.
        let expect = mpeg_crc16(&out, 6, 32);
        let stored = (u16::from(out[4]) << 8) | u16::from(out[5]);
        assert_eq!(stored, expect);
        // And the frame still parses as a header.
        assert!(FrameHeader::parse(&out).is_some());
    }
}
