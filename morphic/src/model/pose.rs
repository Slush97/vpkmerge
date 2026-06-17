//! Bakes one animation frame into the mesh as static geometry.
//!
//! The hero export normally emits a bind-pose *skinned* mesh plus animation
//! clips, which a renderer drives. For a small still preview (a hero card) we
//! instead want a *static posed* mesh: pick one clip, sample one frame, fold the
//! skinning into the vertex positions, and drop the skeleton, skin weights, and
//! clips entirely. The result is a plain-mesh GLB the size of a static prop.
//!
//! Two entry points:
//! - [`bake_pose`] uses the model's own clips (a base-game hero `.vmdl_c`).
//! - [`bake_pose_from`] takes clips from a *donor* model and maps them onto this
//!   model's skeleton **by bone name**. Skin mods ship the mesh + rig but no
//!   clips, so a skin is posed with the matching base-game hero's clip. This is
//!   safe because it is the same hero (same rig); it is NOT the risky cross-hero
//!   retarget (a different hero's skeleton).

use std::collections::HashMap;

use super::glb::default_weights;
use super::math::{Mat4, Quat, Vec3};
use super::mesh::{MeshPart, VertexBuffer};
use super::skeleton::{Bone, Skeleton};
use super::{BoneTrack, Clip, Model};

/// A bone's parent-space (local) transform, sampled from an external clip whose
/// tracks are keyed by bone *name* rather than by this model's bone index (e.g.
/// a loose NM clip; see [`super::nm`]). Composed the same way as a posed bone's
/// local in [`posed_local`], so a value equal to the bone's bind reproduces bind.
#[derive(Debug, Clone, Copy)]
pub struct LocalPose {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: f32,
}

/// Bakes a single frame of the first matching clip into the mesh, returning a
/// static [`Model`] (empty skeleton, no skin weights, no animations).
///
/// `clips` is a priority list of clip names; the first the model carries wins
/// (case-insensitive). `frame` is clamped to the clip's range. When the model
/// has no skeleton, or none of `clips` is present, the mesh is returned as a
/// static model at its bind pose (still stripped of skeleton/skin/clips) rather
/// than failing, so a static prop or a clipless skin still exports cleanly.
#[must_use]
pub fn bake_pose(model: &Model, clips: &[&str], frame: usize) -> Model {
    let palette = if model.skeleton.bones.is_empty() {
        None
    } else {
        self_palette(model, clips, frame)
    };
    bake_with(model, palette.as_deref())
}

/// Like [`bake_pose`], but the clips come from `donor` (e.g. the base-game hero
/// model) and are mapped onto `model`'s skeleton by bone name. For a skin mod
/// that ships no clips of its own; the donor must be the same hero so the rigs
/// match. Falls back to `model`'s own clips when the donor lacks the clip, then
/// to the static bind pose.
#[must_use]
pub fn bake_pose_from(model: &Model, donor: &Model, clips: &[&str], frame: usize) -> Model {
    if model.skeleton.bones.is_empty() {
        return bake_with(model, None);
    }
    let palette =
        donor_palette(model, donor, clips, frame).or_else(|| self_palette(model, clips, frame));
    bake_with(model, palette.as_deref())
}

/// Bakes a single static pose whose per-bone local transforms are supplied by
/// *bone name* (`pose_by_name`), mapping them onto `model`'s own skeleton. Bones
/// absent from the map keep their bind local. This is the by-name path
/// [`bake_pose_from`] uses, generalized to a clip source that is not a [`Clip`]
/// (a loose NM pose, decoded in [`super::nm`]): same hero, same rig, no
/// retargeting. An empty skeleton, or an empty map, yields the static bind pose.
#[must_use]
pub fn bake_pose_named<S: std::hash::BuildHasher>(
    model: &Model,
    pose_by_name: &HashMap<String, LocalPose, S>,
) -> Model {
    if model.skeleton.bones.is_empty() {
        return bake_with(model, None);
    }
    let posed_local = model
        .skeleton
        .bones
        .iter()
        .map(|b| {
            if is_secondary_motion_bone(&b.name) {
                return b.local_bind;
            }
            match pose_by_name.get(&b.name) {
                Some(lp) => Mat4::from_scale(lp.scale)
                    .mul(&Mat4::from_quaternion(lp.rotation))
                    .mul(&Mat4::from_translation(lp.translation)),
                None => b.local_bind,
            }
        })
        .collect::<Vec<_>>();
    let palette = finish_palette(&model.skeleton, &posed_local);
    bake_with(model, Some(&palette))
}

/// Builds the static output `Model` from `model`'s meshes and an optional skin
/// palette. `None` (no skeleton / no clip) yields the static bind-pose mesh.
fn bake_with(model: &Model, palette: Option<&[Mat4]>) -> Model {
    let meshes = model
        .meshes
        .iter()
        .map(|part| bake_part(part, palette))
        .collect();
    Model {
        skeleton: Skeleton { bones: Vec::new() },
        meshes,
        animations: Vec::new(),
    }
}

/// Skin palette from the model's own clips (tracks index this skeleton).
fn self_palette(model: &Model, clips: &[&str], frame: usize) -> Option<Vec<Mat4>> {
    let bones = &model.skeleton.bones;
    let clip = pick_clip(&model.animations, clips)?;
    let f = frame.min(clip.frame_count.saturating_sub(1));

    let mut track_for: Vec<Option<&BoneTrack>> = vec![None; bones.len()];
    for tr in &clip.tracks {
        if tr.bone < bones.len() {
            track_for[tr.bone] = Some(tr);
        }
    }
    let posed_local = bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if is_secondary_motion_bone(&b.name) {
                b.local_bind
            } else {
                posed_local(b, track_for[i], f)
            }
        })
        .collect::<Vec<_>>();
    Some(finish_palette(&model.skeleton, &posed_local))
}

/// Skin palette from a donor model's clips, mapped onto `model`'s skeleton by
/// bone name. `None` when the donor carries none of the candidate clips.
fn donor_palette(model: &Model, donor: &Model, clips: &[&str], frame: usize) -> Option<Vec<Mat4>> {
    let clip = pick_clip(&donor.animations, clips)?;
    let f = frame.min(clip.frame_count.saturating_sub(1));

    // donor bone name -> its track in this clip.
    let mut by_name: HashMap<&str, &BoneTrack> = HashMap::new();
    for tr in &clip.tracks {
        if let Some(db) = donor.skeleton.bones.get(tr.bone) {
            by_name.insert(db.name.as_str(), tr);
        }
    }
    let posed_local = model
        .skeleton
        .bones
        .iter()
        .map(|b| {
            if is_secondary_motion_bone(&b.name) {
                b.local_bind
            } else {
                posed_local(b, by_name.get(b.name.as_str()).copied(), f)
            }
        })
        .collect::<Vec<_>>();
    Some(finish_palette(&model.skeleton, &posed_local))
}

/// Bones whose final render pose is normally resolved by Source 2 secondary
/// motion / cloth rather than by treating the animation track as an ordinary
/// authored bone pose. For static preview GLBs there is no PHYS solver, so using
/// these tracks literally can freeze fabric mid-sim and detach coat tails,
/// skirts, or ribbons from the posed body. Keeping their local bind transform
/// lets the already-posed parent bone carry the chain while preserving the
/// authored resting shape.
fn is_secondary_motion_bone(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("$cloth")
        || lower.starts_with("cloth")
        || lower.starts_with("tail_")
        || lower == "tail"
        || lower == "tail_end"
        || lower.starts_with("coattail")
        || lower.starts_with("coat_tail")
        || lower.contains("skirt")
        || lower.contains("cape")
        || lower.contains("scarf")
        || lower.contains("tassel")
        || lower.starts_with("flap")
        || lower.starts_with("ribbon")
}

/// First clip whose name matches a candidate (case-insensitive, in priority order).
fn pick_clip<'a>(animations: &'a [Clip], clips: &[&str]) -> Option<&'a Clip> {
    clips.iter().find_map(|want| {
        animations
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(want))
    })
}

/// The bone's posed local transform: its sampled channels (bind value where a
/// channel is absent), composed scale * rot * translation to match `Skeleton`'s
/// bind build, so an unchanged channel reproduces the bind local exactly. No
/// track => the bind local unchanged.
fn posed_local(bone: &Bone, track: Option<&BoneTrack>, f: usize) -> Mat4 {
    match track {
        None => bone.local_bind,
        Some(tr) => {
            let t = sample_at(tr.translations.as_ref(), f, bone.position);
            let r = sample_at(tr.rotations.as_ref(), f, bone.rotation);
            let s = sample_at(tr.scales.as_ref(), f, 1.0);
            Mat4::from_scale(s)
                .mul(&Mat4::from_quaternion(r))
                .mul(&Mat4::from_translation(t))
        }
    }
}

/// Forward kinematics on `target` (posed global = posed local chained through
/// parents; bones are topologically ordered) then the skinning matrix per bone,
/// `inverse_bind * posed_global`.
fn finish_palette(target: &Skeleton, posed_local: &[Mat4]) -> Vec<Mat4> {
    let mut posed_global = vec![Mat4::IDENTITY; target.bones.len()];
    for (i, bone) in target.bones.iter().enumerate() {
        posed_global[i] = match bone.parent {
            Some(p) => posed_local[i].mul(&posed_global[p]),
            None => posed_local[i],
        };
    }
    target
        .bones
        .iter()
        .enumerate()
        .map(|(i, bone)| bone.inverse_bind.mul(&posed_global[i]))
        .collect()
}

/// One channel sample at `frame`, or `default` when the channel is absent or
/// the frame is out of range.
fn sample_at<T: Copy>(track: Option<&Vec<T>>, frame: usize, default: T) -> T {
    track.and_then(|v| v.get(frame)).copied().unwrap_or(default)
}

/// Bakes one mesh part. Vertex buffers with a joint stream are linear-blend
/// skinned by `palette`; jointless buffers (static decor, or the whole part when
/// `palette` is `None`) pass through unchanged. Output buffers carry no
/// joints/weights, so the GLB writer emits them as plain geometry.
fn bake_part(part: &MeshPart, palette: Option<&[Mat4]>) -> MeshPart {
    let vertex_buffers = part
        .vertex_buffers
        .iter()
        .map(|vb| bake_buffer(vb, part.bone_weight_count, palette))
        .collect();
    MeshPart {
        vertex_buffers,
        ..part.clone()
    }
}

fn bake_buffer(
    vb: &VertexBuffer,
    bone_weight_count: usize,
    palette: Option<&[Mat4]>,
) -> VertexBuffer {
    // No palette, or a buffer with no joints: copy verbatim minus joint/weight
    // data so it renders as plain static geometry.
    let Some(palette) = palette.filter(|_| !vb.joints.is_empty()) else {
        return VertexBuffer {
            joints: Vec::new(),
            weights: Vec::new(),
            ..vb.clone()
        };
    };

    let count = vb.element_count;
    let weights = if vb.weights.is_empty() {
        default_weights(count, bone_weight_count)
    } else {
        vb.weights.clone()
    };

    let mut positions = Vec::with_capacity(count);
    let mut normals = Vec::with_capacity(vb.normals.len());
    let mut tangents = Vec::with_capacity(vb.tangents.len());

    for (i, (&joint, &weight)) in vb.joints.iter().zip(weights.iter()).enumerate() {
        let m = blended(palette, joint, weight);
        let p = vb.positions[i];
        let pp = m.transform_point(Vec3 {
            x: p[0],
            y: p[1],
            z: p[2],
        });
        positions.push([pp.x, pp.y, pp.z]);

        if let Some(n) = vb.normals.get(i) {
            let nn = m
                .transform_vector(Vec3 {
                    x: n[0],
                    y: n[1],
                    z: n[2],
                })
                .normalized();
            normals.push([nn.x, nn.y, nn.z]);
        }
        if let Some(t) = vb.tangents.get(i) {
            let tt = m
                .transform_vector(Vec3 {
                    x: t[0],
                    y: t[1],
                    z: t[2],
                })
                .normalized();
            tangents.push([tt.x, tt.y, tt.z, t[3]]);
        }
    }

    VertexBuffer {
        positions,
        normals,
        tangents,
        joints: Vec::new(),
        weights: Vec::new(),
        ..vb.clone()
    }
}

/// Weighted sum of up to four joint matrices (linear-blend skinning), normalized
/// by total weight. Falls back to identity when no influence resolves, keeping
/// the vertex in place rather than collapsing it to the origin.
fn blended(palette: &[Mat4], joints: [u16; 4], weights: [f32; 4]) -> Mat4 {
    let mut acc = Mat4 { m: [0.0; 16] };
    let mut total = 0.0f32;
    for k in 0..4 {
        let w = weights[k];
        if w == 0.0 {
            continue;
        }
        if let Some(j) = palette.get(joints[k] as usize) {
            acc = acc.add(&j.scaled(w));
            total += w;
        }
    }
    if total <= 1e-6 {
        Mat4::IDENTITY
    } else if (total - 1.0).abs() > 1e-3 {
        acc.scaled(1.0 / total)
    } else {
        acc
    }
}
