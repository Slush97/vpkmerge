//! Source 2 model animation decode, a faithful port of VRF
//! `ResourceTypes/ModelAnimation` (`Animation`, `AnimationDataChannel`,
//! `AnimationSegmentDecoder`, `Frame`, `SegmentHelpers`, and the 10
//! `SegmentDecoders/CCompressed*` classes).
//!
//! A hero `.vmdl_c` carries its animations as three sibling blocks:
//! - `ANIM`: `m_animArray` (clips), `m_decoderArray` (segment decoder names),
//!   `m_segmentArray` (per-channel compressed frame buffers).
//! - `AGRP`: `m_decodeKey.m_dataChannelArray` (the Position/Angle/Scale channels;
//!   each carries the bone *names* its elements target).
//! - `ASEQ` (optional): the named sequences that become the exported clip names.
//!
//! Channels map onto the model `DATA` skeleton **by bone name** (the decode-key's
//! own bone array is not needed), so a clip drives that hero's own bind pose with
//! no retargeting. Decoded transforms are kept in raw Source/local space, exactly
//! like the bind-pose bone nodes the `.glb` writer emits; the axis/scale
//! `TRANSFORMSOURCETOGLTF` lives only on the skeleton wrapper node.

// i16 header fields, i32 packed-quat magnitudes, and f64 KV3 floats narrow to
// the f32/usize the decoders use; all index math is bounds-checked via `.get`.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::collections::{HashMap, HashSet};

use crate::error::DecodeError;
use crate::kv3::{self, Value};
use crate::resource::Resource;

use super::math::{Quat, Vec3};
use super::skeleton::Skeleton;

/// One decoded animation clip: per-bone keyframe tracks in raw Source local
/// space. Bones not animated by the clip carry no track (they keep their bind
/// pose via the skeleton node's transform).
#[derive(Debug, Clone)]
pub struct Clip {
    pub name: String,
    pub fps: f32,
    pub frame_count: usize,
    pub looping: bool,
    pub tracks: Vec<BoneTrack>,
}

/// Per-bone keyframes. Each present channel has exactly `frame_count` samples
/// (bind-pose default in frames a segment did not write), so the emit layer can
/// build one sampler per channel against a shared per-clip time accessor.
#[derive(Debug, Clone)]
pub struct BoneTrack {
    /// Index into the model [`Skeleton::bones`].
    pub bone: usize,
    pub translations: Option<Vec<Vec3>>,
    pub rotations: Option<Vec<Quat>>,
    pub scales: Option<Vec<f32>>,
}

/// Decodes every animation a `.vmdl_c` carries. Returns an empty vec (not an
/// error) when the model has no animation blocks, so a model that fails here
/// still exports its static mesh.
pub fn decode_all(resource: &Resource<'_>, skeleton: &Skeleton) -> Result<Vec<Clip>, DecodeError> {
    if skeleton.bones.is_empty() {
        return Ok(Vec::new());
    }
    let (Some(anim_bytes), Some(agrp_bytes)) =
        (resource.find_block(*b"ANIM"), resource.find_block(*b"AGRP"))
    else {
        return Ok(Vec::new());
    };

    let anim = kv3::decode(anim_bytes)?;
    let agrp = kv3::decode(agrp_bytes)?;
    let aseq = resource.find_block(*b"ASEQ").map(kv3::decode).transpose()?;

    let decode_key = agrp
        .get("m_decodeKey")
        .ok_or(DecodeError::Model("AGRP missing m_decodeKey"))?;
    let channels = build_channels(decode_key, skeleton);
    let segments = build_segments(&anim, &channels);

    let mut clips = Vec::new();
    for (name, looping, desc) in clip_descs(&anim, aseq.as_ref()) {
        if let Some(clip) = decode_clip(name, looping, desc, &segments, skeleton) {
            clips.push(clip);
        }
    }
    Ok(clips)
}

// ---- decode key channels -------------------------------------------------

/// The transform attribute a channel feeds. VRF's `data` (flex) and any unknown
/// variable map to `None`, and segments on such channels are dropped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Attr {
    Position,
    Angle,
    Scale,
}

/// A decode-key data channel resolved against the model skeleton: `remap[bone]`
/// is the channel element id that drives that model bone, or `-1` if no element
/// (the channel never names that bone). Mirrors VRF `AnimationDataChannel`.
struct Channel {
    attr: Option<Attr>,
    remap: Vec<i32>,
}

fn build_channels(decode_key: &Value, skeleton: &Skeleton) -> Vec<Channel> {
    let Some(arr) = decode_key
        .get("m_dataChannelArray")
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };

    arr.iter()
        .map(|ch| {
            let attr = match ch.get("m_szVariableName").and_then(Value::as_str) {
                Some("Position") => Some(Attr::Position),
                Some("Angle") => Some(Attr::Angle),
                Some("Scale") => Some(Attr::Scale),
                _ => None,
            };

            let mut remap = vec![-1i32; skeleton.bones.len()];
            let names = ch.get("m_szElementNameArray").and_then(Value::as_array);
            let indices = ch.get("m_nElementIndexArray").and_then(Value::as_array);
            if let (Some(names), Some(indices)) = (names, indices) {
                for i in 0..names.len().min(indices.len()) {
                    let (Some(name), Some(elem)) = (names[i].as_str(), indices[i].as_int()) else {
                        continue;
                    };
                    if let Some(bone) = skeleton
                        .bones
                        .iter()
                        .position(|b| b.name.eq_ignore_ascii_case(name))
                    {
                        remap[bone] = elem as i32;
                    }
                }
            }
            Channel { attr, remap }
        })
        .collect()
}

// ---- segments ------------------------------------------------------------

#[derive(Clone, Copy)]
enum Decoder {
    StaticFullVec3,
    StaticVec3,
    FullVec3,
    AnimVec3,
    DeltaVec3,
    StaticQuat,
    AnimQuat,
    FullQuat,
    StaticFloat,
    FullFloat,
}

impl Decoder {
    fn from_name(name: &str) -> Option<Decoder> {
        Some(match name {
            "CCompressedStaticFullVector3" => Decoder::StaticFullVec3,
            "CCompressedStaticVector3" => Decoder::StaticVec3,
            "CCompressedFullVector3" => Decoder::FullVec3,
            "CCompressedAnimVector3" => Decoder::AnimVec3,
            "CCompressedDeltaVector3" => Decoder::DeltaVec3,
            "CCompressedStaticQuaternion" => Decoder::StaticQuat,
            "CCompressedAnimQuaternion" => Decoder::AnimQuat,
            "CCompressedFullQuaternion" => Decoder::FullQuat,
            "CCompressedStaticFloat" => Decoder::StaticFloat,
            "CCompressedFullFloat" => Decoder::FullFloat,
            _ => return None,
        })
    }
}

/// One built segment, shared across every clip (segments are global; clips only
/// reference them by index through their frame blocks). `wanted[k]` indexes the
/// per-frame element array; `remap[k]` is the model bone it writes.
struct Segment {
    decoder: Decoder,
    attr: Attr,
    element_count: usize,
    wanted: Vec<usize>,
    remap: Vec<usize>,
    payload: Vec<u8>,
}

/// Builds the global segment table from `ANIM`. Mirrors VRF
/// `Animation.BuildSegmentArray`: a segment whose decoder is unhandled or whose
/// channel attribute is unknown becomes `None` and is skipped at decode time.
fn build_segments(anim: &Value, channels: &[Channel]) -> Vec<Option<Segment>> {
    let decoder_names: Vec<&str> = anim
        .get("m_decoderArray")
        .and_then(Value::as_array)
        .map_or_default(|a| {
            a.iter()
                .map(|d| d.get("m_szName").and_then(Value::as_str).unwrap_or(""))
                .collect()
        });

    let Some(seg_arr) = anim.get("m_segmentArray").and_then(Value::as_array) else {
        return Vec::new();
    };

    seg_arr
        .iter()
        .map(|seg| build_one_segment(seg, &decoder_names, channels))
        .collect()
}

fn build_one_segment(seg: &Value, decoder_names: &[&str], channels: &[Channel]) -> Option<Segment> {
    let local_channel = usize::try_from(seg.get("m_nLocalChannel")?.as_int()?).ok()?;
    let channel = channels.get(local_channel)?;
    let attr = channel.attr?;

    let Value::Binary(container) = seg.get("m_container")? else {
        return None;
    };
    if container.len() < 8 {
        return None;
    }
    let decoder_idx = i16::from_le_bytes([container[0], container[1]]) as usize;
    let num_elements = i16::from_le_bytes([container[4], container[5]]) as usize;
    let end = 8usize.checked_add(num_elements.checked_mul(2)?)?;
    if end > container.len() {
        return None;
    }
    let decoder = Decoder::from_name(decoder_names.get(decoder_idx).copied().unwrap_or(""))?;

    // The segment's own element-id list (its "bone list").
    let elements: Vec<i16> = (0..num_elements)
        .map(|k| i16::from_le_bytes([container[8 + k * 2], container[9 + k * 2]]))
        .collect();

    // For each model bone this channel drives, find where its element id sits in
    // this segment's element list; keep the ones present (VRF's wanted/remap).
    let mut wanted = Vec::new();
    let mut remap = Vec::new();
    for (bone, &elem_id) in channel.remap.iter().enumerate() {
        if elem_id < 0 {
            continue;
        }
        if let Some(pos) = elements.iter().position(|&e| i32::from(e) == elem_id) {
            wanted.push(pos);
            remap.push(bone);
        }
    }

    Some(Segment {
        decoder,
        attr,
        element_count: num_elements,
        wanted,
        remap,
        payload: container[end..].to_vec(),
    })
}

impl Segment {
    /// Decodes this segment's contribution to one frame, writing onto the
    /// per-bone current-frame arrays. `frame` is the block-local frame index.
    fn read(&self, frame: usize, pos: &mut [Vec3], rot: &mut [Quat], scale: &mut [f32]) {
        let d = &self.payload;
        let off = frame * self.element_count;
        match self.decoder {
            Decoder::StaticFullVec3 => self.each_vec3(pos, |w| le_vec3(d, w * 12)),
            Decoder::StaticVec3 => self.each_vec3(pos, |w| le_half3(d, w * 6)),
            Decoder::FullVec3 => self.each_vec3(pos, |w| le_vec3(d, (off + w) * 12)),
            Decoder::AnimVec3 => self.each_vec3(pos, |w| le_half3(d, (off + w) * 6)),
            Decoder::DeltaVec3 => {
                let base_len = self.element_count * 12;
                self.each_vec3(pos, |w| {
                    Some(le_vec3(d, w * 12)? + le_half3(d, base_len + (off + w) * 6)?)
                });
            }
            Decoder::StaticQuat => self.each_quat(rot, |w| read_packed_quat(d, w * 6)),
            Decoder::AnimQuat => self.each_quat(rot, |w| read_packed_quat(d, (off + w) * 6)),
            Decoder::FullQuat => self.each_quat(rot, |w| le_quat(d, (off + w) * 16)),
            Decoder::StaticFloat => self.each_float(scale, |w| le_f32(d, w * 4)),
            Decoder::FullFloat => self.each_float(scale, |w| le_f32(d, (off + w) * 4)),
        }
    }

    fn each_vec3(&self, pos: &mut [Vec3], read: impl Fn(usize) -> Option<Vec3>) {
        if self.attr != Attr::Position {
            return;
        }
        for (k, &bone) in self.remap.iter().enumerate() {
            if let Some(v) = read(self.wanted[k]) {
                pos[bone] = v;
            }
        }
    }

    fn each_quat(&self, rot: &mut [Quat], read: impl Fn(usize) -> Option<Quat>) {
        if self.attr != Attr::Angle {
            return;
        }
        for (k, &bone) in self.remap.iter().enumerate() {
            if let Some(q) = read(self.wanted[k]) {
                rot[bone] = q;
            }
        }
    }

    fn each_float(&self, scale: &mut [f32], read: impl Fn(usize) -> Option<f32>) {
        if self.attr != Attr::Scale {
            return;
        }
        for (k, &bone) in self.remap.iter().enumerate() {
            if let Some(s) = read(self.wanted[k]) {
                scale[bone] = s;
            }
        }
    }
}

// ---- clip selection (ANIM vs ASEQ) ---------------------------------------

/// A clip to decode: display name, looping flag, and the `m_animArray` entry
/// that holds its frame data. Resolves VRF's two paths: when an `ASEQ` block is
/// present, one clip per sequence (named by the sequence) plus any anim not
/// referenced by a sequence; otherwise one clip per anim.
fn clip_descs<'a>(anim: &'a Value, aseq: Option<&'a Value>) -> Vec<(String, bool, &'a Value)> {
    let anim_array = anim
        .get("m_animArray")
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    match aseq {
        Some(seq) => from_sequence(seq, anim_array),
        None => anim_array
            .iter()
            .filter_map(|a| Some((anim_name(a)?.to_owned(), anim_looping(a), a)))
            .collect(),
    }
}

fn from_sequence<'a>(seq: &'a Value, anim_array: &'a [Value]) -> Vec<(String, bool, &'a Value)> {
    let mut lookup: HashMap<&str, &Value> = HashMap::new();
    for a in anim_array {
        if let Some(n) = anim_name(a) {
            lookup.insert(n, a);
        }
    }

    let seq_names = seq
        .get("m_localSequenceNameArray")
        .and_then(Value::as_array)
        .unwrap_or(&[]);
    let seq_descs = seq
        .get("m_localS1SeqDescArray")
        .and_then(Value::as_array)
        .unwrap_or(&[]);

    let mut processed: HashSet<String> = HashSet::new();
    let mut out = Vec::new();

    for sd in seq_descs {
        let ref_index = sd
            .get("m_fetch")
            .and_then(|f| f.get("m_localReferenceArray"))
            .and_then(Value::as_array)
            .and_then(|refs| refs.first())
            .and_then(Value::as_int);
        let (Some(ref_index), Some(seq_name)) =
            (ref_index, sd.get("m_sName").and_then(Value::as_str))
        else {
            continue;
        };
        let Some(ref_name) = usize::try_from(ref_index)
            .ok()
            .and_then(|i| seq_names.get(i))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(&anim) = lookup.get(ref_name) else {
            continue;
        };

        let looping = sd
            .get("m_flags")
            .and_then(|f| f.get("m_bLooping"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        processed.insert(seq_name.to_owned());
        out.push((seq_name.to_owned(), looping, anim));
    }

    // Anims not surfaced as a sequence are exported under their own name.
    for a in anim_array {
        let Some(name) = anim_name(a) else { continue };
        if processed.contains(name) {
            continue;
        }
        out.push((name.to_owned(), anim_looping(a), a));
    }

    out
}

fn anim_name(anim: &Value) -> Option<&str> {
    anim.get("m_name").and_then(Value::as_str)
}

fn anim_looping(anim: &Value) -> bool {
    anim.get("m_flags")
        .and_then(|f| f.get("m_bLooping"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

// ---- per-clip frame decode -----------------------------------------------

struct FrameBlock {
    start: usize,
    end: usize,
    segments: Vec<usize>,
}

fn decode_clip(
    name: String,
    looping: bool,
    anim: &Value,
    segments: &[Option<Segment>],
    skeleton: &Skeleton,
) -> Option<Clip> {
    let fps = anim.get("fps").and_then(Value::as_f64)? as f32;
    let pdata = anim.get("m_pData")?;

    let frame_blocks: Vec<FrameBlock> = pdata
        .get("m_frameblockArray")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(parse_frame_block)
        .collect();

    // Frame count from m_nFrames, falling back to the highest block end (the
    // single-pose "ref" anim stores no m_nFrames).
    let frame_count = pdata
        .get("m_nFrames")
        .and_then(Value::as_int)
        .map(|n| n as usize)
        .filter(|&n| n > 0)
        .unwrap_or_else(|| frame_blocks.iter().map(|fb| fb.end + 1).max().unwrap_or(0));
    if frame_count == 0 {
        return None;
    }

    let bones = skeleton.bones.len();

    // Which (bone, channel) pairs any referenced segment writes -> the bones
    // that get a track. Untouched bones keep their bind pose with no track.
    let (has_pos, has_rot, has_scale) = animated_channels(&frame_blocks, segments, bones);

    let mut out_pos: Vec<Option<Vec<Vec3>>> = has_pos
        .iter()
        .map(|&on| on.then(|| Vec::with_capacity(frame_count)))
        .collect();
    let mut out_rot: Vec<Option<Vec<Quat>>> = has_rot
        .iter()
        .map(|&on| on.then(|| Vec::with_capacity(frame_count)))
        .collect();
    let mut out_scale: Vec<Option<Vec<f32>>> = has_scale
        .iter()
        .map(|&on| on.then(|| Vec::with_capacity(frame_count)))
        .collect();

    let mut pos = vec![Vec3::default(); bones];
    let mut rot = vec![Quat::default(); bones];
    let mut scale = vec![1.0f32; bones];

    for f in 0..frame_count {
        // Reset to bind pose, then layer in every block covering this frame.
        for (b, bone) in skeleton.bones.iter().enumerate() {
            pos[b] = bone.position;
            rot[b] = bone.rotation;
            scale[b] = 1.0;
        }
        for fb in &frame_blocks {
            if f >= fb.start && f <= fb.end {
                let local = f - fb.start;
                for &si in &fb.segments {
                    if let Some(seg) = segments.get(si).and_then(Option::as_ref) {
                        seg.read(local, &mut pos, &mut rot, &mut scale);
                    }
                }
            }
        }
        for b in 0..bones {
            if let Some(v) = out_pos[b].as_mut() {
                v.push(pos[b]);
            }
            if let Some(v) = out_rot[b].as_mut() {
                v.push(rot[b]);
            }
            if let Some(v) = out_scale[b].as_mut() {
                v.push(scale[b]);
            }
        }
    }

    let tracks: Vec<BoneTrack> = (0..bones)
        .filter_map(|b| {
            let (t, r, s) = (out_pos[b].take(), out_rot[b].take(), out_scale[b].take());
            (t.is_some() || r.is_some() || s.is_some()).then_some(BoneTrack {
                bone: b,
                translations: t,
                rotations: r,
                scales: s,
            })
        })
        .collect();

    Some(Clip {
        name,
        fps,
        frame_count,
        looping,
        tracks,
    })
}

/// Flags, per model bone, which transform channels any segment referenced by
/// this clip writes. A bone touched by no segment keeps its bind pose and gets
/// no track at all.
fn animated_channels(
    frame_blocks: &[FrameBlock],
    segments: &[Option<Segment>],
    bones: usize,
) -> (Vec<bool>, Vec<bool>, Vec<bool>) {
    let mut has_pos = vec![false; bones];
    let mut has_rot = vec![false; bones];
    let mut has_scale = vec![false; bones];
    for fb in frame_blocks {
        for &si in &fb.segments {
            if let Some(seg) = segments.get(si).and_then(Option::as_ref) {
                let flags = match seg.attr {
                    Attr::Position => &mut has_pos,
                    Attr::Angle => &mut has_rot,
                    Attr::Scale => &mut has_scale,
                };
                for &bone in &seg.remap {
                    flags[bone] = true;
                }
            }
        }
    }
    (has_pos, has_rot, has_scale)
}

fn parse_frame_block(fb: &Value) -> Option<FrameBlock> {
    let start = usize::try_from(fb.get("m_nStartFrame")?.as_int()?).ok()?;
    let end = usize::try_from(fb.get("m_nEndFrame")?.as_int()?).ok()?;
    let segments = fb
        .get("m_segmentIndexArray")
        .and_then(Value::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|v| usize::try_from(v.as_int()?).ok())
        .collect();
    Some(FrameBlock {
        start,
        end,
        segments,
    })
}

// ---- little-endian readers -----------------------------------------------

fn le_f32(d: &[u8], off: usize) -> Option<f32> {
    let b = d.get(off..off + 4)?;
    Some(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn le_f16(d: &[u8], off: usize) -> Option<f32> {
    let b = d.get(off..off + 2)?;
    Some(half::f16::from_le_bytes([b[0], b[1]]).to_f32())
}

fn le_vec3(d: &[u8], off: usize) -> Option<Vec3> {
    Some(Vec3 {
        x: le_f32(d, off)?,
        y: le_f32(d, off + 4)?,
        z: le_f32(d, off + 8)?,
    })
}

fn le_half3(d: &[u8], off: usize) -> Option<Vec3> {
    Some(Vec3 {
        x: le_f16(d, off)?,
        y: le_f16(d, off + 2)?,
        z: le_f16(d, off + 4)?,
    })
}

fn le_quat(d: &[u8], off: usize) -> Option<Quat> {
    Some(Quat {
        x: le_f32(d, off)?,
        y: le_f32(d, off + 4)?,
        z: le_f32(d, off + 8)?,
        w: le_f32(d, off + 12)?,
    })
}

/// Decodes Source's 6-byte packed quaternion, port of VRF
/// `SegmentHelpers.ReadQuaternion`: three 14-bit magnitudes (bit 6 selects
/// recenter, bit 7 the sign), the dropped largest component reconstructed from
/// the unit-length constraint, then swizzled back into place by `s1`/`s2`.
// x/y/z/w are the quaternion components; b/c/d are the byte slice, scale, and
// payload. These short names mirror VRF's `ReadQuaternion` one-for-one.
#[allow(clippy::many_single_char_names)]
fn read_packed_quat(d: &[u8], off: usize) -> Option<Quat> {
    let b = d.get(off..off + 6)?;

    let i1 = i32::from(b[0]) + (i32::from(b[1] & 63) << 8);
    let i2 = i32::from(b[2]) + (i32::from(b[3] & 63) << 8);
    let i3 = i32::from(b[4]) + (i32::from(b[5] & 63) << 8);

    let s1 = b[1] & 128;
    let s2 = b[3] & 128;
    let s3 = b[5] & 128;

    let c = std::f32::consts::FRAC_PI_4.sin() / 16384.0;
    let x = if b[1] & 64 == 0 {
        c * (i1 - 16384) as f32
    } else {
        c * i1 as f32
    };
    let y = if b[3] & 64 == 0 {
        c * (i2 - 16384) as f32
    } else {
        c * i2 as f32
    };
    let z = if b[5] & 64 == 0 {
        c * (i3 - 16384) as f32
    } else {
        c * i3 as f32
    };

    let mut w = (1.0 - x * x - y * y - z * z).max(0.0).sqrt();
    if s3 == 128 {
        w = -w;
    }

    Some(match (s1 == 128, s2 == 128) {
        (true, true) => Quat {
            x: y,
            y: z,
            z: w,
            w: x,
        },
        (true, false) => Quat {
            x: z,
            y: w,
            z: x,
            w: y,
        },
        (false, true) => Quat {
            x: w,
            y: x,
            z: y,
            w: z,
        },
        (false, false) => Quat { x, y, z, w },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_quat_identity() {
        // bytes -> i=0, bit6 set (no recenter) so x=y=z=0, signs clear:
        // w = sqrt(1) = 1, no swizzle => identity (0,0,0,1).
        let q = read_packed_quat(&[0x00, 0x40, 0x00, 0x40, 0x00, 0x40], 0).unwrap();
        assert!((q.x).abs() < 1e-6);
        assert!((q.y).abs() < 1e-6);
        assert!((q.z).abs() < 1e-6);
        assert!((q.w - 1.0).abs() < 1e-6);
    }

    #[test]
    fn packed_quat_is_unit_length() {
        // Arbitrary byte patterns must still decode to a (near) unit quaternion.
        for seed in [0x1234_5678u32, 0x9abc_def0, 0x0f0f_0f0f, 0xdead_beef] {
            let bytes = [
                seed as u8,
                (seed >> 4) as u8,
                (seed >> 8) as u8,
                (seed >> 12) as u8,
                (seed >> 16) as u8,
                (seed >> 20) as u8,
            ];
            let q = read_packed_quat(&bytes, 0).unwrap();
            let len = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "len {len} for {bytes:?}");
        }
    }

    #[test]
    fn short_buffers_do_not_panic() {
        assert!(read_packed_quat(&[0, 0, 0], 0).is_none());
        assert!(le_vec3(&[0, 0, 0, 0], 0).is_none());
        assert!(le_half3(&[0, 0], 0).is_none());
    }
}
