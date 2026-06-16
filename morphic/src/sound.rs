//! Mint a Source 2 `.vsnd_c` from an MP3 payload by patching a donor container.
//!
//! A compiled sound resource is laid out as
//! `[header][RED2][empty DATA][CTRL][appended MP3]`. The `CTRL` block is a
//! `CVoiceContainerDefault` KV3 describing the clip (sample rate, channels,
//! sample count, duration, streamed-data size); the raw MP3 stream is appended
//! after the resource structure, starting at the byte offset the header records
//! as its file size.
//!
//! Valve's `resourcecompiler` produces these from a WAV. We don't have it on
//! Linux, but we don't need to *compress* anything: the audio is plain MP3, which
//! `ffmpeg`/`lame` emit. So to forge a new clip we reuse a stock clip as a donor,
//! keep its `RED2`, format GUID, and envelope byte-faithful, rewrite only the five
//! fields that depend on the new audio, and swap the appended MP3 stream. The same
//! "patch a container, don't recompile" approach the model recolor uses for
//! meshopt buffers.

use crate::error::DecodeError;
use crate::kv3::{self, Value};
use crate::resource::Resource;

const BLOCK_CTRL: [u8; 4] = *b"CTRL";

/// The audio parameters of the replacement clip. `streaming_size` is taken from
/// the MP3 byte length, so it is not part of this struct.
#[derive(Debug, Clone, Copy)]
pub struct VsndParams {
    /// Sample rate in Hz (e.g. 44100).
    pub rate: u32,
    /// Channel count (1 = mono, 2 = stereo).
    pub channels: u32,
    /// Total PCM sample count (`duration_seconds * rate`, rounded).
    pub sample_count: u32,
    /// Clip duration in seconds.
    pub duration: f64,
    /// Whether the sound resource itself should loop.
    pub looped: bool,
}

/// Forge a `.vsnd_c` by reusing `donor` as a template and substituting `mp3` as
/// the streamed audio. `donor` must be an MP3 `CVoiceContainerDefault` clip (the
/// common Deadlock VO / ability-cast shape: a `CTRL` block plus an appended MP3).
///
/// The donor's `RED2` dependency info, KV3 format GUID, loop points, and envelope
/// curve are preserved; only `m_nRate`, `m_nChannels`, `m_nSampleCount`,
/// `m_flDuration`, and `m_nStreamingSize` are rewritten. Returns a complete,
/// loadable resource file.
///
/// # Errors
/// Fails if the donor does not parse as a resource, lacks a `CTRL` block, or that
/// block is not the expected `m_vSound` KV3 shape.
pub fn encode_vsnd_c(
    donor: &[u8],
    mp3: &[u8],
    params: &VsndParams,
) -> Result<Vec<u8>, DecodeError> {
    let resource = Resource::parse(donor)?;
    let ctrl_idx = resource
        .blocks()
        .iter()
        .position(|b| b.kind == BLOCK_CTRL)
        .ok_or(DecodeError::BadResource("vsnd_c has no CTRL block"))?;
    let ctrl_bytes = resource
        .find_block(BLOCK_CTRL)
        .ok_or(DecodeError::BadResource("vsnd_c CTRL block out of range"))?;

    let format = kv3::Format::from_payload(ctrl_bytes)?;
    let mut root = kv3::decode(ctrl_bytes)?;

    let sound = root
        .get_mut("m_vSound")
        .ok_or(DecodeError::BadResource("vsnd_c CTRL has no m_vSound"))?;
    set_value(sound, "m_nRate", Value::Int(i64::from(params.rate)));
    set_value(sound, "m_nChannels", Value::Int(i64::from(params.channels)));
    set_value(
        sound,
        "m_nSampleCount",
        Value::UInt(u64::from(params.sample_count)),
    );
    let streaming_size = u64::try_from(mp3.len()).unwrap_or(u64::MAX);
    set_value(sound, "m_nStreamingSize", Value::UInt(streaming_size));
    set_value(sound, "m_flDuration", Value::Double(params.duration));
    set_value(
        sound,
        "m_nLoopStart",
        Value::Int(if params.looped { 0 } else { -1 }),
    );
    set_value(sound, "m_nLoopEnd", Value::Int(0));

    // The envelope analyzer's spline is an amplitude-vs-time curve whose x axis is
    // in seconds, sized to the donor clip. It cannot carry over to a different
    // duration (a 0.8s donor envelope on a 20s clip would describe nothing past
    // the first second), so regenerate a flat full-length curve: constant
    // amplitude across [0, duration]. Harmless if the engine treats the curve as
    // analysis metadata, correct if it applies it to playback.
    regenerate_flat_envelope(&mut root, params.duration);

    let new_ctrl = kv3::encode(&root, &format);
    let mut out = resource.rebuild_with_block(ctrl_idx, &new_ctrl)?;
    out.extend_from_slice(mp3);
    Ok(out)
}

/// Overwrite `key` on an object if present, leaving its absence to surface later
/// as a shape error rather than silently inserting a field the engine ignores.
fn set_value(obj: &mut Value, key: &str, value: Value) {
    if let Some(slot) = obj.get_mut(key) {
        *slot = value;
    }
}

/// Replace the envelope analyzer curve with a flat two-point curve (`y = 1` at
/// `x = 0` and `x = duration`). No-op if the analyzer/curve is absent.
fn regenerate_flat_envelope(root: &mut Value, duration: f64) {
    let Some(curve) = root
        .get_mut("m_pEnvelopeAnalyzer")
        .and_then(|a| a.get_mut("m_curve"))
    else {
        return;
    };
    let Some(spline) = curve.get_mut("m_spline") else {
        return;
    };
    *spline = Value::Array(vec![spline_point(0.0, 1.0), spline_point(duration, 1.0)]);
    if let Some(tangents) = curve.get_mut("m_tangents") {
        *tangents = Value::Array(vec![linear_tangent(), linear_tangent()]);
    }
    if let Some(domain_maxs) = curve.get_mut("m_vDomainMaxs") {
        *domain_maxs = Value::Array(vec![Value::Double(duration), Value::Double(1.0)]);
    }
}

/// One envelope spline knot: position `x` (seconds), amplitude `y`, flat slopes.
fn spline_point(x: f64, y: f64) -> Value {
    Value::Object(vec![
        ("x".to_owned(), Value::Double(x)),
        ("y".to_owned(), Value::Double(y)),
        ("m_flSlopeIncoming".to_owned(), Value::Double(0.0)),
        ("m_flSlopeOutgoing".to_owned(), Value::Double(0.0)),
    ])
}

fn linear_tangent() -> Value {
    Value::Object(vec![
        (
            "m_nIncomingTangent".to_owned(),
            Value::String("CURVE_TANGENT_LINEAR".to_owned()),
        ),
        (
            "m_nOutgoingTangent".to_owned(),
            Value::String("CURVE_TANGENT_LINEAR".to_owned()),
        ),
    ])
}
