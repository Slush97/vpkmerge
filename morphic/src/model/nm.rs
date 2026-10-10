//! Decode the newer Source 2 "NM" (motion-matching) animation resources: a
//! `.vnmskel_c` skeleton and a *static* single-frame `.vnmclip_c` pose.
//!
//! WIP Deadlock heroes (Apollo `fencer`, Billy `punkgoat`, Celeste `unicorn`,
//! Mina `vampirebat`, Paige `bookworm`, Rem `familiar`) ship their menu/idle
//! pose this way: loose files under `models/heroes_wip/<h>/clips/*.vnmclip_c`
//! plus one `<h>.vnmskel_c`, referenced through an animation graph. They embed
//! no `ANIM`/`AGRP`, so [`super::animation::decode_all`] finds nothing and the
//! hero would only ever bind-pose. The card-pose clips (`ui_hero_select`,
//! `<h>_hero_pose`, ...) are all **single-frame and fully static**: every
//! per-bone track stores a constant transform and `m_compressedPoseData` is
//! empty, so the pose reconstructs from `m_trackCompressionSettings` alone, with
//! no quantized-stream decode and no graph traversal.
//!
//! Only static tracks are decoded. A non-static track (none observed in any
//! menu/idle clip) is reported as `None` for that bone, which the baker treats
//! as "keep bind" rather than rendering a wrong transform; animated NM clips
//! would need the `m_compressedPoseData` dequantizer, which is out of scope.
//! Recon + the full layout writeup: `docs/handoff-nm-loose-clip-pose.md`.

// KV3 widens f32 transform components to f64 and stores frame counts as wider
// integers; narrowing them back is exact for real model data (the same contract
// `skeleton.rs` relies on). The pose codec also rounds f32 magnitudes back into
// u16 quantization slots (sign already non-negative by construction).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::collections::HashMap;

use crate::error::DecodeError;
use crate::kv3::{Seg, Value};

use super::math::{Quat, Vec3};
use super::pose::LocalPose;
use super::skeleton::Skeleton;
use super::{BoneTrack, Clip, Model};

/// A decoded NM skeleton. Only the bone *names*, in track order (the clip's
/// track `i` is bone `i`), are needed to map an NM pose onto a model skeleton by
/// name; parent indices and the reference pose are not required for posing.
#[derive(Debug, Clone)]
pub struct NmSkeleton {
    pub bone_names: Vec<String>,
}

/// A decoded static NM clip: one constant parent-space transform per skeleton
/// bone (in bone order), or `None` for a non-static (unsupported) track. Also
/// carries the `.vnmskel` resource path the clip references, so the caller can
/// resolve the matching skeleton.
#[derive(Debug, Clone)]
pub struct NmPose {
    /// The `m_skeleton` reference (uncompiled, e.g. `models/.../h.vnmskel`).
    pub skeleton_ref: String,
    /// `m_nNumFrames` (1 for every menu/idle pose seen).
    pub frame_count: u32,
    /// Per-bone constant local transform; `None` where the track is non-static.
    pub bones: Vec<Option<LocalPose>>,
}

impl NmPose {
    /// Number of bones that decoded to a constant transform.
    #[must_use]
    pub fn static_bone_count(&self) -> usize {
        self.bones.iter().filter(|b| b.is_some()).count()
    }
}

/// Decodes a `.vnmskel_c` into its ordered bone names.
pub fn decode_nm_skeleton(bytes: &[u8]) -> Result<NmSkeleton, DecodeError> {
    nm_skeleton_from_value(&crate::decode_kv3_resource(bytes)?)
}

/// Decodes a static `.vnmclip_c` into one constant local transform per bone.
pub fn decode_nm_pose(bytes: &[u8]) -> Result<NmPose, DecodeError> {
    nm_pose_from_value(&crate::decode_kv3_resource(bytes)?)
}

/// [`decode_nm_skeleton`] on an already-decoded `DATA` KV3 tree (split out so the
/// KV3 interpretation is unit-testable without a resource fixture).
fn nm_skeleton_from_value(data: &Value) -> Result<NmSkeleton, DecodeError> {
    let ids = data
        .get("m_boneIDs")
        .and_then(Value::as_array)
        .ok_or(DecodeError::Model("vnmskel missing m_boneIDs"))?;
    let bone_names = ids
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or(DecodeError::Model("vnmskel bone id not a string"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(NmSkeleton { bone_names })
}

/// [`decode_nm_pose`] on an already-decoded `DATA` KV3 tree.
fn nm_pose_from_value(data: &Value) -> Result<NmPose, DecodeError> {
    let skeleton_ref = data
        .get("m_skeleton")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let frame_count = data
        .get("m_nNumFrames")
        .and_then(|v| {
            v.as_uint()
                .or_else(|| v.as_int().and_then(|n| u64::try_from(n).ok()))
        })
        .unwrap_or(1) as u32;

    let tracks = data
        .get("m_trackCompressionSettings")
        .and_then(Value::as_array)
        .ok_or(DecodeError::Model(
            "vnmclip missing m_trackCompressionSettings",
        ))?;

    let mut bones = Vec::with_capacity(tracks.len());
    for t in tracks {
        bones.push(decode_track(t));
    }
    Ok(NmPose {
        skeleton_ref,
        frame_count,
        bones,
    })
}

/// One track -> a constant local pose, or `None` if any channel is non-static
/// (its value lives in the compressed stream we do not decode). For a static
/// channel the constant is: rotation = `m_constantRotation`; translation =
/// `(m_translationRange{X,Y,Z}).m_flRangeStart`; scale = `m_scaleRange.m_flRangeStart`.
fn decode_track(t: &Value) -> Option<LocalPose> {
    let is_static = |k: &str| t.get(k).and_then(Value::as_bool).unwrap_or(false);
    if !(is_static("m_bIsRotationStatic")
        && is_static("m_bIsTranslationStatic")
        && is_static("m_bIsScaleStatic"))
    {
        return None;
    }
    let rotation = read_quat(t.get("m_constantRotation")?)?;
    Some(LocalPose {
        translation: Vec3 {
            x: range_start(t, "m_translationRangeX")?,
            y: range_start(t, "m_translationRangeY")?,
            z: range_start(t, "m_translationRangeZ")?,
        },
        rotation,
        scale: range_start(t, "m_scaleRange")?,
    })
}

/// `m_flRangeStart` of a track's quantization-range object (the constant value
/// when that channel is static).
fn range_start(track: &Value, key: &str) -> Option<f32> {
    track
        .get(key)
        .and_then(|o| o.get("m_flRangeStart"))
        .and_then(Value::as_f64)
        .map(|d| d as f32)
}

fn read_quat(v: &Value) -> Option<Quat> {
    let a = v.as_array()?;
    if a.len() < 4 {
        return None;
    }
    Some(Quat {
        x: a[0].as_f64()? as f32,
        y: a[1].as_f64()? as f32,
        z: a[2].as_f64()? as f32,
        w: a[3].as_f64()? as f32,
    })
}

/// Bakes an NM static pose onto `model`'s mesh: zips `skeleton`'s bone names with
/// `pose`'s per-bone transforms (track `i` is bone `i`), maps them onto the model
/// skeleton by name, and folds the skinning into the vertices, returning a static
/// posed [`Model`] (no skeleton/skin/clips), exactly like [`super::bake_pose`].
///
/// The NM skeleton's bones are a by-name subset of the model's mesh skeleton
/// (the model's extra cloth/twist/helper bones are not driven by the clip and
/// keep their bind pose). Errors if the track and bone-name counts disagree.
pub fn bake_nm_pose(
    model: &Model,
    skeleton: &NmSkeleton,
    pose: &NmPose,
) -> Result<Model, DecodeError> {
    if pose.bones.len() != skeleton.bone_names.len() {
        return Err(DecodeError::Model(
            "vnmclip track count != vnmskel bone count",
        ));
    }
    let mut by_name: HashMap<String, LocalPose> = HashMap::with_capacity(pose.bones.len());
    for (name, bp) in skeleton.bone_names.iter().zip(pose.bones.iter()) {
        if let Some(lp) = bp {
            by_name.insert(name.clone(), *lp);
        }
    }
    Ok(super::pose::bake_pose_named(model, &by_name))
}

// ===========================================================================
// Quantized-pose codec (animated NM clips)
// ===========================================================================
//
// A faithful port of VRF `ModelAnimation2/AnimationClip` (`ReadFrame`,
// `DecodeQuaternion`, `DecodeTranslation`, `DecodeFloat`). An animated
// `.vnmclip_c` stores its non-constant channels in `m_compressedPoseData`, a
// flat little-endian `u16` stream, with `m_compressedPoseOffsets[frame]` giving
// the `u16` index at which that frame begins. Within a frame the per-bone tracks
// appear in `m_trackCompressionSettings` order; a track contributes, in order,
// 3 words for an animated rotation, 3 for an animated translation, and 1 for an
// animated scale (a *static* channel contributes nothing, its constant lives in
// the track settings). Decode dequantizes each word against the track's range;
// encode is the exact inverse, so a clip's stream round-trips byte-for-byte.

/// One channel's `[start, start+length]` quantization window (KV3
/// `m_flRangeStart`/`m_flRangeLength`). A static channel's constant is
/// `start`; an animated channel maps a `u16` linearly across the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuantRange {
    pub start: f32,
    pub length: f32,
}

/// One bone's `m_trackCompressionSettings` entry: the per-channel ranges, the
/// constant rotation (used when rotation is static), and which channels are
/// static (i.e. absent from the compressed stream).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackSettings {
    pub translation_range: [QuantRange; 3],
    pub scale_range: QuantRange,
    pub constant_rotation: Quat,
    pub rotation_static: bool,
    pub translation_static: bool,
    pub scale_static: bool,
}

/// One bone's decoded animation: the settings it came from, plus a per-frame
/// sample vector for each *animated* channel (`None` when that channel is static,
/// in which case its constant is in `settings`). Each present vector has exactly
/// `NmClip::frame_count` entries.
#[derive(Debug, Clone, PartialEq)]
pub struct NmTrack {
    pub settings: TrackSettings,
    pub rotations: Option<Vec<Quat>>,
    pub translations: Option<Vec<Vec3>>,
    pub scales: Option<Vec<f32>>,
}

/// A fully decoded NM animation clip: the referenced skeleton, frame count, the
/// additive flag, the per-bone tracks, and the original quantized stream
/// (`compressed_pose_data` + `compressed_pose_offsets`) kept verbatim so a caller
/// can verify a re-encode is byte-faithful or splice in only the frames it edits.
#[derive(Debug, Clone, PartialEq)]
pub struct NmClip {
    pub skeleton_ref: String,
    pub frame_count: u32,
    /// `m_flDuration` in seconds (0 for a single-frame pose). Sets the glTF
    /// playback rate when converting to an animation.
    pub duration: f32,
    pub additive: bool,
    pub tracks: Vec<NmTrack>,
    pub compressed_pose_data: Vec<u8>,
    pub compressed_pose_offsets: Vec<u32>,
}

impl NmClip {
    /// Playback frames per second: `(frame_count - 1) / duration` (frame 0 at
    /// t=0, the last frame at t=duration). Falls back to 30 fps when the duration
    /// is missing or the clip is a single frame.
    #[must_use]
    pub fn fps(&self) -> f32 {
        if self.duration > 0.0 && self.frame_count > 1 {
            (self.frame_count - 1) as f32 / self.duration
        } else {
            30.0
        }
    }
}

// Quaternion "smallest three" packing: each stored component sits in
// [-1/sqrt2, 1/sqrt2], 15-bit quantized; the dropped (largest) component is
// reconstructed from unit length, its index carried in the two flag bits.
const QUAT_MIN: f32 = -std::f32::consts::FRAC_1_SQRT_2;
const QUAT_RANGE: f32 = std::f32::consts::SQRT_2; // (1/sqrt2) - (-1/sqrt2)
const QUAT_QUANT_MAX: f32 = 0x7FFF as f32;

/// Decodes a `.vnmclip_c` (static or animated) into per-bone tracks. The static
/// constants come from `m_trackCompressionSettings`; the animated channels are
/// dequantized from `m_compressedPoseData`. A fully static clip decodes with
/// every track's channel vectors `None` (its pose is the per-track constants).
pub fn decode_nm_clip(bytes: &[u8]) -> Result<NmClip, DecodeError> {
    nm_clip_from_value(&crate::decode_kv3_resource(bytes)?)
}

fn nm_clip_from_value(data: &Value) -> Result<NmClip, DecodeError> {
    let skeleton_ref = data
        .get("m_skeleton")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let additive = data
        .get("m_bIsAdditive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let frame_count = data
        .get("m_nNumFrames")
        .and_then(|v| {
            v.as_uint()
                .or_else(|| v.as_int().and_then(|n| u64::try_from(n).ok()))
        })
        .unwrap_or(1) as u32;
    let duration = data
        .get("m_flDuration")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32;

    let settings: Vec<TrackSettings> = data
        .get("m_trackCompressionSettings")
        .and_then(Value::as_array)
        .ok_or(DecodeError::Model(
            "vnmclip missing m_trackCompressionSettings",
        ))?
        .iter()
        .map(parse_track_settings)
        .collect::<Option<Vec<_>>>()
        .ok_or(DecodeError::Model("vnmclip track settings malformed"))?;

    let compressed_pose_data = match data.get("m_compressedPoseData") {
        Some(Value::Binary(b)) => b.clone(),
        _ => Vec::new(),
    };
    let compressed_pose_offsets = data
        .get("m_compressedPoseOffsets")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_uint().map(|u| u as u32))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let tracks = decode_frames(
        &settings,
        &compressed_pose_data,
        &compressed_pose_offsets,
        frame_count,
    )?;

    Ok(NmClip {
        skeleton_ref,
        frame_count,
        duration,
        additive,
        tracks,
        compressed_pose_data,
        compressed_pose_offsets,
    })
}

/// Converts a decoded NM clip into a glTF-ready [`Clip`] driving `model`'s
/// skeleton, so [`super::to_glb`] emits a playable animated GLB. NM track `i` maps
/// to `skel.bone_names[i]`, resolved against the model skeleton by name (NM bones
/// are a by-name subset of the mesh skeleton); tracks whose bone is absent are
/// dropped. Every channel is emitted with one sample per frame: an animated
/// channel uses its decoded samples, a static channel holds its constant across
/// all frames, so the clip reproduces the authored motion (and pose) exactly. The
/// frame rate comes from [`NmClip::fps`].
#[must_use]
pub fn nm_clip_to_clip(clip: &NmClip, skel: &NmSkeleton, model: &Skeleton, name: &str) -> Clip {
    let frames = clip.frame_count as usize;
    let mut tracks = Vec::new();
    for (i, t) in clip.tracks.iter().enumerate() {
        let Some(bone_name) = skel.bone_names.get(i) else {
            continue;
        };
        let Some(bone) = model.bones.iter().position(|b| &b.name == bone_name) else {
            continue;
        };
        let s = &t.settings;
        let translations = t.translations.clone().unwrap_or_else(|| {
            vec![
                Vec3 {
                    x: s.translation_range[0].start,
                    y: s.translation_range[1].start,
                    z: s.translation_range[2].start,
                };
                frames
            ]
        });
        let rotations = t
            .rotations
            .clone()
            .unwrap_or_else(|| vec![s.constant_rotation; frames]);
        let scales = t
            .scales
            .clone()
            .unwrap_or_else(|| vec![s.scale_range.start; frames]);
        tracks.push(BoneTrack {
            bone,
            translations: Some(translations),
            rotations: Some(rotations),
            scales: Some(scales),
        });
    }
    Clip {
        name: name.to_owned(),
        fps: clip.fps(),
        frame_count: clip.frame_count as usize,
        looping: true,
        tracks,
    }
}

fn parse_track_settings(t: &Value) -> Option<TrackSettings> {
    let range = |key: &str| -> Option<QuantRange> {
        let o = t.get(key)?;
        Some(QuantRange {
            start: o.get("m_flRangeStart").and_then(Value::as_f64)? as f32,
            length: o.get("m_flRangeLength").and_then(Value::as_f64)? as f32,
        })
    };
    let is = |k: &str| t.get(k).and_then(Value::as_bool).unwrap_or(false);
    Some(TrackSettings {
        translation_range: [
            range("m_translationRangeX")?,
            range("m_translationRangeY")?,
            range("m_translationRangeZ")?,
        ],
        scale_range: range("m_scaleRange")?,
        constant_rotation: read_quat(t.get("m_constantRotation")?)?,
        rotation_static: is("m_bIsRotationStatic"),
        translation_static: is("m_bIsTranslationStatic"),
        scale_static: is("m_bIsScaleStatic"),
    })
}

/// Dequantizes a quantized pose stream against a set of track settings, the same
/// way [`decode_nm_clip`] does internally. Exposed so a caller can prove a
/// decode -> encode -> decode round-trip is pose-identical: feed back the
/// `(data, offsets)` from [`encode_compressed_pose`] and the clip's per-track
/// [`TrackSettings`].
pub fn decode_pose_stream(
    settings: &[TrackSettings],
    data: &[u8],
    offsets: &[u32],
    frame_count: u32,
) -> Result<Vec<NmTrack>, DecodeError> {
    decode_frames(settings, data, offsets, frame_count)
}

/// Dequantizes every frame into per-bone channel vectors. Mirrors VRF
/// `ReadFrame`: per frame, start at the frame's `u16` offset and walk the tracks
/// in order, consuming 3/3/1 words for each animated rotation/translation/scale.
fn decode_frames(
    settings: &[TrackSettings],
    data: &[u8],
    offsets: &[u32],
    frame_count: u32,
) -> Result<Vec<NmTrack>, DecodeError> {
    let frame_count = frame_count as usize;
    // Allocate a sample vector for each animated channel, `None` for static ones.
    let mut tracks: Vec<NmTrack> = settings
        .iter()
        .map(|s| NmTrack {
            settings: *s,
            rotations: (!s.rotation_static).then(|| Vec::with_capacity(frame_count)),
            translations: (!s.translation_static).then(|| Vec::with_capacity(frame_count)),
            scales: (!s.scale_static).then(|| Vec::with_capacity(frame_count)),
        })
        .collect();

    let any_animated = settings
        .iter()
        .any(|s| !(s.rotation_static && s.translation_static && s.scale_static));
    if !any_animated || frame_count == 0 {
        return Ok(tracks);
    }
    if offsets.len() < frame_count {
        return Err(DecodeError::Model(
            "vnmclip has fewer pose offsets than frames",
        ));
    }

    for &frame_start in &offsets[..frame_count] {
        let mut word = frame_start as usize;
        let mut read = |n: usize| -> Result<&[u8], DecodeError> {
            let start = word * 2;
            let end = start + n * 2;
            let slice = data
                .get(start..end)
                .ok_or(DecodeError::Model("vnmclip pose stream truncated"))?;
            word += n;
            Ok(slice)
        };
        for tr in &mut tracks {
            if let Some(rots) = &mut tr.rotations {
                rots.push(decode_quaternion(read(3)?));
            }
            if let Some(trans) = &mut tr.translations {
                let b = read(3)?;
                let r = &tr.settings.translation_range;
                trans.push(Vec3 {
                    x: decode_float(u16le(b, 0), r[0]),
                    y: decode_float(u16le(b, 1), r[1]),
                    z: decode_float(u16le(b, 2), r[2]),
                });
            }
            if let Some(scales) = &mut tr.scales {
                let b = read(1)?;
                scales.push(decode_float(u16le(b, 0), tr.settings.scale_range));
            }
        }
    }
    Ok(tracks)
}

/// Re-encodes an edited NM clip back into `original`'s resource, byte-faithfully,
/// supporting a **changed animated-rotation channel set** at a fixed frame count.
/// This is the first step toward authoring clips (e.g. importing a Blender
/// animation that keyframes bones the slot left static): rotation tracks may be
/// added (a static bone becomes animated) and re-posed; the re-encoded pose stream
/// is spliced in with [`crate::patch_kv3_resource_sole_blob`] (it may now be a
/// different length), the per-frame offsets are rewritten, and each newly-animated
/// bone's `m_bIsRotationStatic` is flipped to false.
///
/// Constrained to what is safe without touching quantization ranges or the
/// document's array shapes:
/// - `clip.frame_count` must equal the original's (a frame-count change would
///   resize `m_compressedPoseOffsets`, a separate capability);
/// - the **translation** and **scale** animated-channel sets must match the
///   original's (adding those needs new `m_flRange*` values, which may be tagless
///   `0`/`1` constants the in-place patcher cannot rewrite);
/// - a rotation channel may be **added** or kept, but not **removed** (removing
///   would need a fresh `m_constantRotation`).
///
/// Translation/scale values are quantized against the original ranges (authored
/// values outside a bone's range saturate). Errors if any constraint is violated.
pub fn reencode_nm_clip(original: &[u8], clip: &NmClip) -> Result<Vec<u8>, DecodeError> {
    let orig = decode_nm_clip(original)?;
    if clip.frame_count != orig.frame_count {
        return Err(DecodeError::Model(
            "reencode: frame-count change not supported",
        ));
    }
    if clip.tracks.len() != orig.tracks.len() {
        return Err(DecodeError::Model("reencode: track count mismatch"));
    }
    for (a, b) in orig.tracks.iter().zip(&clip.tracks) {
        if a.translations.is_some() != b.translations.is_some() {
            return Err(DecodeError::Model(
                "reencode: translation channel-set change not supported",
            ));
        }
        if a.scales.is_some() != b.scales.is_some() {
            return Err(DecodeError::Model(
                "reencode: scale channel-set change not supported",
            ));
        }
        if a.rotations.is_some() && b.rotations.is_none() {
            return Err(DecodeError::Model(
                "reencode: removing a rotation channel not supported",
            ));
        }
    }

    let (blob, offsets) = encode_compressed_pose(clip);

    // 1. Splice the (possibly longer) pose stream in.
    let mut out = crate::patch_kv3_resource_sole_blob(original, &blob)?;

    // 2. Rewrite the per-frame offsets (same count, new values).
    if offsets != orig.compressed_pose_offsets {
        let edits: Vec<(Vec<Seg>, i64)> = offsets
            .iter()
            .enumerate()
            .map(|(f, &o)| {
                (
                    vec![Seg::Key("m_compressedPoseOffsets".into()), Seg::Index(f)],
                    i64::from(o),
                )
            })
            .collect();
        out = crate::patch_kv3_resource_scalars(&out, &edits)?;
    }

    // 3. Flip m_bIsRotationStatic = false for each newly-animated rotation track.
    let bool_edits: Vec<(Vec<Seg>, bool)> = orig
        .tracks
        .iter()
        .zip(&clip.tracks)
        .enumerate()
        .filter(|(_, (a, b))| a.rotations.is_none() && b.rotations.is_some())
        .map(|(i, _)| {
            (
                vec![
                    Seg::Key("m_trackCompressionSettings".into()),
                    Seg::Index(i),
                    Seg::Key("m_bIsRotationStatic".into()),
                ],
                false,
            )
        })
        .collect();
    if !bool_edits.is_empty() {
        out = crate::patch_kv3_resource_bools(&out, &bool_edits)?;
    }
    Ok(out)
}

/// Re-encodes an edited NM clip by **rebuilding the whole `DATA` block** from its
/// KV3 value tree (KV3 v5 via [`crate::encode_kv3_resource_like`], keeping the
/// original's typed-array framing and flags), rather than patching the v5
/// container in place like [`reencode_nm_clip`]. This is the
/// general path the Blender importer needs: it handles **arbitrary** edits that the
/// in-place patcher cannot, because each becomes a plain value-tree edit:
/// - a **frame-count change** (the `m_compressedPoseOffsets` array is rebuilt and
///   `m_nNumFrames` set);
/// - **adding any channel**, including translation/scale on a previously-static
///   bone (the quantization ranges are recomputed from the channel data and written
///   as real values -- no tagless-constant problem, since the writer re-tags
///   everything).
///
/// Per-channel quantization ranges are derived from each animated channel's min/max
/// over the clip's frames; static channels keep their constant. Every per-channel
/// vector must have `clip.frame_count` entries (the caller resamples to the target
/// frame count). An earlier version emitted uncompressed v4 with plain arrays; the
/// engine loaded that but never animated it, so the output is now v5 shaped like
/// Valve's (in-game confirmed for rotation edits, a frame-count change, and added
/// translation channels). Prefer [`reencode_nm_clip`]
/// for in-place rotation edits at a fixed frame count (byte-faithful, smaller, and
/// already in-game confirmed); use this when the channel set or frame count changes.
pub fn reencode_nm_clip_full(original: &[u8], clip: &NmClip) -> Result<Vec<u8>, DecodeError> {
    // 1. Recompute quantization ranges from the (possibly new) channel data, so the
    //    quantizer maps each animated value across its actual [min, max].
    let mut work = clip.clone();
    for t in &mut work.tracks {
        if let Some(trans) = &t.translations {
            for (axis, range) in t.settings.translation_range.iter_mut().enumerate() {
                *range = range_of(trans.iter().map(|v| [v.x, v.y, v.z][axis]));
            }
        }
        if let Some(scales) = &t.scales {
            t.settings.scale_range = range_of(scales.iter().copied());
        }
    }

    // 2. Quantize with the recomputed ranges.
    let (blob, offsets) = encode_compressed_pose(&work);

    // 3. Mutate the decoded DATA tree, then re-encode it whole.
    let mut tree = crate::decode_kv3_resource(original)?;
    let old_frames = tree.get("m_nNumFrames").and_then(Value::as_uint);
    if old_frames != Some(u64::from(work.frame_count)) {
        retime_root_motion(&mut tree, old_frames, work.frame_count)?;
    }
    set_value(
        &mut tree,
        "m_nNumFrames",
        Value::Int(i64::from(work.frame_count)),
    );
    set_value(&mut tree, "m_compressedPoseData", Value::Binary(blob));
    set_value(
        &mut tree,
        "m_compressedPoseOffsets",
        Value::Array(
            offsets
                .into_iter()
                .map(|o| Value::UInt(u64::from(o)))
                .collect(),
        ),
    );
    if let Some(Value::Array(tracks)) = tree.get_mut("m_trackCompressionSettings") {
        for (i, slot) in tracks.iter_mut().enumerate() {
            if let Some(t) = work.tracks.get(i) {
                write_track_settings(slot, t);
            }
        }
    }
    crate::encode_kv3_resource_like(original, &tree)
}

/// Keep `m_rootMotion` in step with a frame-count change: its own frame count
/// follows the clip's. A clip with per-frame root-motion transforms is refused
/// (their layout is not modelled, so they cannot be resampled safely); the
/// usual single identity transform needs no change.
fn retime_root_motion(tree: &mut Value, old: Option<u64>, frames: u32) -> Result<(), DecodeError> {
    let Some(rm) = tree.get_mut("m_rootMotion") else {
        return Ok(());
    };
    let per_frame = rm
        .get("m_transforms")
        .and_then(Value::as_array)
        .is_some_and(|t| t.len() > 1);
    if per_frame {
        return Err(DecodeError::Kv3(
            "frame-count change on a clip with per-frame root motion is not supported",
        ));
    }
    if rm.get("m_nNumFrames").and_then(Value::as_uint) == old {
        set_value(rm, "m_nNumFrames", Value::Int(i64::from(frames)));
    }
    Ok(())
}

/// `[min, max-min]` over an iterator of channel samples; a constant channel yields
/// length 0 (which dequantizes back to the constant).
fn range_of(samples: impl Iterator<Item = f32>) -> QuantRange {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for v in samples {
        min = min.min(v);
        max = max.max(v);
    }
    if min > max {
        return QuantRange {
            start: 0.0,
            length: 0.0,
        }; // empty (no frames)
    }
    QuantRange {
        start: min,
        length: max - min,
    }
}

/// Overwrite one named child of an object value (no-op if the key is absent).
fn set_value(obj: &mut Value, key: &str, v: Value) {
    if let Some(slot) = obj.get_mut(key) {
        *slot = v;
    }
}

/// Write a [`NmTrack`]'s settings (static flags from channel presence, ranges, and
/// constant rotation) into its `m_trackCompressionSettings` object in the tree.
fn write_track_settings(track_obj: &mut Value, t: &NmTrack) {
    set_value(
        track_obj,
        "m_bIsRotationStatic",
        Value::Bool(t.rotations.is_none()),
    );
    set_value(
        track_obj,
        "m_bIsTranslationStatic",
        Value::Bool(t.translations.is_none()),
    );
    set_value(
        track_obj,
        "m_bIsScaleStatic",
        Value::Bool(t.scales.is_none()),
    );
    let r = &t.settings.translation_range;
    for (key, range) in [
        ("m_translationRangeX", r[0]),
        ("m_translationRangeY", r[1]),
        ("m_translationRangeZ", r[2]),
        ("m_scaleRange", t.settings.scale_range),
    ] {
        if let Some(obj) = track_obj.get_mut(key) {
            set_value(obj, "m_flRangeStart", Value::Double(f64::from(range.start)));
            set_value(
                obj,
                "m_flRangeLength",
                Value::Double(f64::from(range.length)),
            );
        }
    }
    if let Some(Value::Array(a)) = track_obj.get_mut("m_constantRotation") {
        let q = t.settings.constant_rotation;
        for (slot, v) in a.iter_mut().zip([q.x, q.y, q.z, q.w]) {
            *slot = Value::Double(f64::from(v));
        }
    }
}

/// Re-quantizes the decoded tracks back into `(m_compressedPoseData,
/// m_compressedPoseOffsets)`. The exact inverse of [`decode_frames`]; on a clip
/// decoded straight from a file the result is byte-identical to the original
/// stream, and round-trips (decode -> encode -> decode) reproduce poses exactly.
#[must_use]
pub fn encode_compressed_pose(clip: &NmClip) -> (Vec<u8>, Vec<u32>) {
    let frame_count = clip.frame_count as usize;
    let mut words: Vec<u16> = Vec::new();
    let mut offsets: Vec<u32> = Vec::with_capacity(frame_count);

    for f in 0..frame_count {
        offsets.push(words.len() as u32);
        for tr in &clip.tracks {
            if let Some(rots) = &tr.rotations {
                if let Some(q) = rots.get(f) {
                    words.extend_from_slice(&encode_quaternion(*q));
                }
            }
            if let Some(trans) = &tr.translations {
                if let Some(t) = trans.get(f) {
                    let r = &tr.settings.translation_range;
                    words.push(encode_float(t.x, r[0]));
                    words.push(encode_float(t.y, r[1]));
                    words.push(encode_float(t.z, r[2]));
                }
            }
            if let Some(scales) = &tr.scales {
                if let Some(s) = scales.get(f) {
                    words.push(encode_float(*s, tr.settings.scale_range));
                }
            }
        }
    }

    let mut bytes = Vec::with_capacity(words.len() * 2);
    for w in words {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    (bytes, offsets)
}

fn u16le(b: &[u8], word: usize) -> u16 {
    u16::from_le_bytes([b[word * 2], b[word * 2 + 1]])
}

/// Dequantizes one channel value: `start + (u16 / 65535) * length` (VRF
/// `DecodeFloat`).
fn decode_float(u: u16, range: QuantRange) -> f32 {
    (f32::from(u) / f32::from(u16::MAX)).mul_add(range.length, range.start)
}

/// The inverse of [`decode_float`]: nearest `u16` whose dequantization is closest
/// to `v`. A zero-length range (degenerate, never animated in practice) encodes
/// to 0.
fn encode_float(v: f32, range: QuantRange) -> u16 {
    if range.length == 0.0 {
        return 0;
    }
    let norm = (v - range.start) / range.length;
    (norm * f32::from(u16::MAX))
        .round()
        .clamp(0.0, f32::from(u16::MAX)) as u16
}

/// Decodes Source 2's NM 3-word "smallest three" quaternion, a port of VRF
/// `DecodeQuaternion`: words 0/1 carry a 15-bit magnitude plus one flag bit
/// (bit 15), word 2 a 15-bit magnitude; the two flag bits name which component
/// was dropped, and it is rebuilt from the unit-length constraint.
fn decode_quaternion(b: &[u8]) -> Quat {
    let d0 = u16le(b, 0);
    let d1 = u16le(b, 1);
    let d2 = u16le(b, 2);
    let mul = QUAT_RANGE / QUAT_QUANT_MAX;
    let v0 = f32::from(d0 & 0x7FFF).mul_add(mul, QUAT_MIN);
    let v1 = f32::from(d1 & 0x7FFF).mul_add(mul, QUAT_MIN);
    let v2 = f32::from(d2).mul_add(mul, QUAT_MIN);
    let sum = v0.mul_add(v0, v1.mul_add(v1, v2 * v2));
    let v3 = (1.0 - sum).max(0.0).sqrt();
    let largest = ((d0 >> 14) & 0x0002) | (d1 >> 15);
    match largest {
        0 => Quat {
            x: v3,
            y: v0,
            z: v1,
            w: v2,
        },
        1 => Quat {
            x: v0,
            y: v3,
            z: v1,
            w: v2,
        },
        2 => Quat {
            x: v0,
            y: v1,
            z: v3,
            w: v2,
        },
        _ => Quat {
            x: v0,
            y: v1,
            z: v2,
            w: v3,
        },
    }
}

/// The inverse of [`decode_quaternion`]: drop the largest-magnitude component
/// (recoverable from unit length), store the other three 15-bit quantized with
/// the dropped index in the flag bits, sign-flipping so the dropped component is
/// non-negative (q and -q are the same rotation, and decode always rebuilds it
/// non-negative).
fn encode_quaternion(q: Quat) -> [u16; 3] {
    let comps = [q.x, q.y, q.z, q.w];
    let mut largest = 0usize;
    for i in 1..4 {
        if comps[i].abs() > comps[largest].abs() {
            largest = i;
        }
    }
    let sign = if comps[largest] < 0.0 { -1.0 } else { 1.0 };
    let mut stored = [0f32; 3];
    let mut k = 0;
    for (i, &c) in comps.iter().enumerate() {
        if i != largest {
            stored[k] = c * sign;
            k += 1;
        }
    }
    let q15 = |v: f32| -> u16 {
        let n = (v - QUAT_MIN) / (QUAT_RANGE / QUAT_QUANT_MAX);
        n.round().clamp(0.0, QUAT_QUANT_MAX) as u16
    };
    let mut d0 = q15(stored[0]);
    let mut d1 = q15(stored[1]);
    let d2 = q15(stored[2]);
    d0 |= ((largest as u16 >> 1) & 1) << 15;
    d1 |= (largest as u16 & 1) << 15;
    [d0, d1, d2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(pairs: Vec<(&str, Value)>) -> Value {
        Value::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }
    fn range(start: f64) -> Value {
        obj(vec![
            ("m_flRangeStart", Value::Double(start)),
            ("m_flRangeLength", Value::Double(0.1)),
        ])
    }
    /// A fully-static track: translation/scale from range starts, constant quat.
    fn static_track(tx: f64, ty: f64, tz: f64, s: f64, q: [f64; 4]) -> Value {
        obj(vec![
            ("m_translationRangeX", range(tx)),
            ("m_translationRangeY", range(ty)),
            ("m_translationRangeZ", range(tz)),
            ("m_scaleRange", range(s)),
            (
                "m_constantRotation",
                Value::Array(q.iter().map(|c| Value::Double(*c)).collect()),
            ),
            ("m_bIsRotationStatic", Value::Bool(true)),
            ("m_bIsTranslationStatic", Value::Bool(true)),
            ("m_bIsScaleStatic", Value::Bool(true)),
        ])
    }
    /// A track with an animated rotation: its constant is in the (here absent)
    /// compressed stream, so it must decode to `None`.
    fn animated_track() -> Value {
        let mut t = static_track(0.0, 0.0, 0.0, 1.0, [0.0, 0.0, 0.0, 1.0]);
        if let Value::Object(o) = &mut t {
            for (k, v) in o.iter_mut() {
                if k == "m_bIsRotationStatic" {
                    *v = Value::Bool(false);
                }
            }
        }
        t
    }

    #[test]
    fn skeleton_reads_bone_ids_in_order() {
        let data = obj(vec![(
            "m_boneIDs",
            Value::Array(vec![
                Value::String("root".into()),
                Value::String("spine".into()),
            ]),
        )]);
        let skel = nm_skeleton_from_value(&data).expect("skeleton");
        assert_eq!(skel.bone_names, vec!["root", "spine"]);
    }

    #[test]
    fn pose_decodes_static_track_and_skips_animated() {
        let data = obj(vec![
            ("m_skeleton", Value::String("models/h/h.vnmskel".into())),
            ("m_nNumFrames", Value::Int(1)),
            (
                "m_trackCompressionSettings",
                Value::Array(vec![
                    static_track(5.0, -2.0, 77.0, 1.0, [0.5, 0.5, 0.5, 0.5]),
                    animated_track(),
                ]),
            ),
        ]);
        let pose = nm_pose_from_value(&data).expect("pose");
        assert_eq!(pose.skeleton_ref, "models/h/h.vnmskel");
        assert_eq!(pose.bones.len(), 2);
        assert_eq!(pose.static_bone_count(), 1, "animated track -> None");

        let lp = pose.bones[0].expect("static bone decoded");
        // translation = the range starts; scale = scale range start; rot = constant.
        assert!((lp.translation.x - 5.0).abs() < 1e-5);
        assert!((lp.translation.y + 2.0).abs() < 1e-5);
        assert!((lp.translation.z - 77.0).abs() < 1e-5);
        assert!((lp.scale - 1.0).abs() < 1e-5);
        assert!((lp.rotation.w - 0.5).abs() < 1e-5);
        assert!(pose.bones[1].is_none(), "animated rotation -> None");
    }

    #[test]
    fn pose_requires_track_settings() {
        let data = obj(vec![("m_nNumFrames", Value::Int(1))]);
        assert!(nm_pose_from_value(&data).is_err());
    }

    // ---- pose codec ------------------------------------------------------

    fn approx_quat(a: Quat, b: Quat, eps: f32) -> bool {
        // Quantization error plus the q/-q ambiguity: compare both signs.
        let d = |s: f32| {
            (a.x - s * b.x).abs() < eps
                && (a.y - s * b.y).abs() < eps
                && (a.z - s * b.z).abs() < eps
                && (a.w - s * b.w).abs() < eps
        };
        d(1.0) || d(-1.0)
    }

    #[test]
    fn quaternion_round_trips_each_largest_component() {
        // One quat per "largest component" case; encode then decode must return
        // the same rotation within 15-bit quantization error.
        let quats = [
            Quat {
                x: 0.9,
                y: 0.1,
                z: 0.2,
                w: 0.3,
            }, // x largest
            Quat {
                x: 0.1,
                y: -0.9,
                z: 0.2,
                w: 0.3,
            }, // y largest, negative
            Quat {
                x: 0.2,
                y: 0.1,
                z: 0.92,
                w: 0.1,
            }, // z largest
            Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            }, // identity, w largest
        ];
        for q0 in quats {
            // normalize
            let n = (q0.x * q0.x + q0.y * q0.y + q0.z * q0.z + q0.w * q0.w).sqrt();
            let q = Quat {
                x: q0.x / n,
                y: q0.y / n,
                z: q0.z / n,
                w: q0.w / n,
            };
            let words = encode_quaternion(q);
            let mut bytes = Vec::new();
            for w in words {
                bytes.extend_from_slice(&w.to_le_bytes());
            }
            let back = decode_quaternion(&bytes);
            assert!(
                approx_quat(q, back, 1e-3),
                "quat {q:?} -> {words:x?} -> {back:?}"
            );
        }
    }

    fn quat_bytes(words: [u16; 3]) -> Vec<u8> {
        let mut b = Vec::new();
        for w in words {
            b.extend_from_slice(&w.to_le_bytes());
        }
        b
    }

    #[test]
    fn quaternion_re_encode_is_stable() {
        // The byte-faithful property: a *validly* encoded quaternion (one whose
        // dropped component is genuinely the largest, i.e. exactly what Valve's
        // encoder writes) round-trips encode -> decode -> encode to identical
        // words, because decode rebuilds the dropped component as the largest.
        let quats = [
            Quat {
                x: 0.9,
                y: 0.1,
                z: 0.2,
                w: 0.3,
            },
            Quat {
                x: 0.1,
                y: -0.85,
                z: 0.2,
                w: 0.3,
            },
            Quat {
                x: 0.2,
                y: 0.1,
                z: 0.94,
                w: 0.1,
            },
            Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
        ];
        for q0 in quats {
            let n = (q0.x * q0.x + q0.y * q0.y + q0.z * q0.z + q0.w * q0.w).sqrt();
            let q = Quat {
                x: q0.x / n,
                y: q0.y / n,
                z: q0.z / n,
                w: q0.w / n,
            };
            let words = encode_quaternion(q);
            let decoded = decode_quaternion(&quat_bytes(words));
            let again = encode_quaternion(decoded);
            assert_eq!(
                again, words,
                "quat words not stable: {words:x?} -> {again:x?}"
            );
        }
    }

    #[test]
    fn float_round_trips_within_range() {
        let r = QuantRange {
            start: -10.0,
            length: 25.0,
        };
        for u in [0u16, 1, 12345, 32767, 40000, 65535] {
            let v = decode_float(u, r);
            assert_eq!(encode_float(v, r), u, "float u16 not stable for {u}");
        }
    }

    /// A frame stream + offsets + settings round-trips: decode -> encode
    /// reproduces the bytes and offsets exactly, and a re-decode is identical.
    #[test]
    fn frame_stream_round_trips() {
        // bone 0: animated rotation only; bone 1: static; bone 2: animated
        // translation + scale.
        let r = |s: f32, l: f32| QuantRange {
            start: s,
            length: l,
        };
        let settings = vec![
            TrackSettings {
                translation_range: [r(0.0, 1.0); 3],
                scale_range: r(1.0, 0.0),
                constant_rotation: Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
                rotation_static: false,
                translation_static: true,
                scale_static: true,
            },
            TrackSettings {
                translation_range: [r(0.0, 1.0); 3],
                scale_range: r(1.0, 0.0),
                constant_rotation: Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
                rotation_static: true,
                translation_static: true,
                scale_static: true,
            },
            TrackSettings {
                translation_range: [r(-5.0, 10.0), r(-5.0, 10.0), r(-5.0, 10.0)],
                scale_range: r(0.5, 2.0),
                constant_rotation: Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
                rotation_static: true,
                translation_static: false,
                scale_static: false,
            },
        ];
        // Build a 2-frame stream: per frame, bone0 quat (3w), bone2
        // translation (3w) + scale (1w) = 7 words/frame. The quat words come
        // from encoding a real unit quaternion (a hand-picked word triple is not
        // generally a valid smallest-three encoding).
        let unit = |x: f32, y: f32, z: f32, w: f32| {
            let n = (x * x + y * y + z * z + w * w).sqrt();
            encode_quaternion(Quat {
                x: x / n,
                y: y / n,
                z: z / n,
                w: w / n,
            })
        };
        let q0 = unit(0.8, 0.1, 0.2, 0.3);
        let q1 = unit(0.1, 0.2, 0.9, 0.1);
        let frame0 = [q0[0], q0[1], q0[2], 10000, 20000, 30000, 12345];
        let frame1 = [q1[0], q1[1], q1[2], 40000, 50000, 60000, 54321];
        let mut bytes = Vec::new();
        for w in frame0.iter().chain(frame1.iter()) {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        let offsets = vec![0u32, 7];

        let tracks = decode_frames(&settings, &bytes, &offsets, 2).expect("decode frames");
        let clip = NmClip {
            skeleton_ref: String::new(),
            frame_count: 2,
            duration: 0.0,
            additive: false,
            tracks,
            compressed_pose_data: bytes.clone(),
            compressed_pose_offsets: offsets.clone(),
        };
        let (data2, offsets2) = encode_compressed_pose(&clip);
        assert_eq!(offsets2, offsets, "offsets must match");
        assert_eq!(data2, bytes, "re-encoded stream must be byte-identical");

        // re-decode is identical track-for-track.
        let tracks2 = decode_frames(&settings, &data2, &offsets2, 2).expect("re-decode");
        assert_eq!(tracks2, clip.tracks);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // verbose struct literals, not real complexity
    fn nm_clip_to_clip_maps_by_name_and_fills_static() {
        use super::super::math::Mat4;
        use super::super::skeleton::Bone;

        let bone = |name: &str| Bone {
            name: name.to_owned(),
            parent: None,
            flags: 0,
            position: Vec3::default(),
            rotation: Quat::default(),
            local_bind: Mat4::IDENTITY,
            global_bind: Mat4::IDENTITY,
            inverse_bind: Mat4::IDENTITY,
        };
        // Model skeleton in a *different* order than the NM skeleton, plus an extra
        // bone the clip never names.
        let model = Skeleton {
            bones: vec![bone("spine"), bone("root"), bone("extra")],
        };
        let nm = NmSkeleton {
            bone_names: vec!["root".into(), "spine".into()],
        };

        let r = |s: f32| QuantRange {
            start: s,
            length: 0.0,
        };
        // track 0 (root): animated rotation, static translation (5,6,7), static scale.
        let track0 = NmTrack {
            settings: TrackSettings {
                translation_range: [r(5.0), r(6.0), r(7.0)],
                scale_range: r(1.0),
                constant_rotation: Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
                rotation_static: false,
                translation_static: true,
                scale_static: true,
            },
            rotations: Some(vec![
                Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
                Quat {
                    x: 0.1,
                    y: 0.0,
                    z: 0.0,
                    w: 0.995,
                },
                Quat {
                    x: 0.2,
                    y: 0.0,
                    z: 0.0,
                    w: 0.98,
                },
            ]),
            translations: None,
            scales: None,
        };
        // track 1 (spine): fully static.
        let track1 = NmTrack {
            settings: TrackSettings {
                translation_range: [r(0.0), r(0.0), r(0.0)],
                scale_range: r(2.0),
                constant_rotation: Quat {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    w: 1.0,
                },
                rotation_static: true,
                translation_static: true,
                scale_static: true,
            },
            rotations: None,
            translations: None,
            scales: None,
        };
        let clip = NmClip {
            skeleton_ref: String::new(),
            frame_count: 3,
            duration: 0.1,
            additive: false,
            tracks: vec![track0, track1],
            compressed_pose_data: Vec::new(),
            compressed_pose_offsets: Vec::new(),
        };

        let out = nm_clip_to_clip(&clip, &nm, &model, "test");
        assert_eq!(out.frame_count, 3);
        assert_eq!(
            out.tracks.len(),
            2,
            "both named bones map; extra is untouched"
        );

        // root -> model bone index 1; every channel filled, len == frames.
        let root = out.tracks.iter().find(|t| t.bone == 1).expect("root track");
        assert_eq!(root.rotations.as_ref().unwrap().len(), 3);
        let tr = root.translations.as_ref().unwrap();
        assert_eq!(tr.len(), 3);
        // static translation held at the range starts across all frames.
        assert!(tr.iter().all(|v| (v.x - 5.0).abs() < 1e-6
            && (v.y - 6.0).abs() < 1e-6
            && (v.z - 7.0).abs() < 1e-6));
        // animated rotation preserved.
        assert!((root.rotations.as_ref().unwrap()[2].x - 0.2).abs() < 1e-6);

        // spine -> model bone index 0; static scale held at 2.0.
        let spine = out
            .tracks
            .iter()
            .find(|t| t.bone == 0)
            .expect("spine track");
        assert!(spine
            .scales
            .as_ref()
            .unwrap()
            .iter()
            .all(|&s| (s - 2.0).abs() < 1e-6));
    }

    #[test]
    fn fully_static_clip_has_empty_stream() {
        let settings = vec![TrackSettings {
            translation_range: [QuantRange {
                start: 0.0,
                length: 0.0,
            }; 3],
            scale_range: QuantRange {
                start: 1.0,
                length: 0.0,
            },
            constant_rotation: Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            },
            rotation_static: true,
            translation_static: true,
            scale_static: true,
        }];
        let tracks = decode_frames(&settings, &[], &[], 10).expect("static decode");
        assert!(tracks[0].rotations.is_none());
        let clip = NmClip {
            skeleton_ref: String::new(),
            frame_count: 10,
            duration: 0.0,
            additive: false,
            tracks,
            compressed_pose_data: Vec::new(),
            compressed_pose_offsets: Vec::new(),
        };
        let (data2, offsets2) = encode_compressed_pose(&clip);
        assert_eq!(data2, [] as [u8; 0]);
        assert_eq!(offsets2, vec![0u32; 10]);
    }
}
