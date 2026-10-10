//! Generate an **exaggerated** animation as a real `.glb` on disk, so the glTF
//! import path (`nm_clip_import_glb` / `morphic::model::read_glb_animation`) can be
//! tested in-game WITHOUT Blender. The written `.glb` is structurally identical to a
//! Blender export: bone-named joint nodes + per-bone TRS samplers. Feed it to
//! `nm_clip_import_glb` to splice it back into the slot and pack an addon.
//!
//! The motion: take the slot clip's already-animated rotation bones and layer a big
//! oscillating tilt (a repeated "headbang/wobble") onto each, ramping over the clip.
//! Only already-animated bones are touched, so the re-import is an equal-length,
//! byte-faithful in-place splice (the most engine-safe path) -- just unmistakable.
//!
//! `--bob BONE:UNITS` (repeatable) also pushes one bone along its own
//! parent-space offset by UNITS, eased in, held through the middle of the clip,
//! eased out (so `neck_0:20` stretches the neck 20 units). A bone the slot keeps
//! translation-static only lands through the full re-importer
//! (`nm_clip_import_glb --full`): the in-game test for adding a translation
//! channel. Amplitude 0 leaves the rotations as they are, so the bob is the only
//! visible motion.
//!
//! Usage:
//!   cargo run --release -p vpkmerge-core --example gen_obvious_anim_glb -- \
//!       <pak01_dir.vpk> <mesh_entry.vmdl_c> <clip_entry.vnmclip_c> <out.glb> \
//!       [amplitude_deg] [cycles] [--bob BONE:UNITS ...]

// File paths in the usage doc above are not Rust items.
#![allow(clippy::doc_markdown, clippy::cast_precision_loss)]

use std::f32::consts::PI;

use anyhow::{Context, Result};
use morphic::model::{
    decode, decode_nm_clip, decode_nm_skeleton, BoneTrack, Clip, Model, Quat, Vec3,
};

fn qmul(a: Quat, b: Quat) -> Quat {
    Quat {
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
    }
}

fn normalize(q: Quat) -> Quat {
    let n = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
    Quat {
        x: q.x / n,
        y: q.y / n,
        z: q.z / n,
        w: q.w / n,
    }
}

fn axis_angle(axis: Vec3, deg: f32) -> Quat {
    let r = deg.to_radians() * 0.5;
    let s = r.sin();
    Quat {
        x: axis.x * s,
        y: axis.y * s,
        z: axis.z * s,
        w: r.cos(),
    }
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    Vec3 {
        x: a.x - b.x,
        y: a.y - b.y,
        z: a.z - b.z,
    }
}

fn cross(a: Vec3, b: Vec3) -> Vec3 {
    Vec3 {
        x: a.y * b.z - a.z * b.y,
        y: a.z * b.x - a.x * b.z,
        z: a.x * b.y - a.y * b.x,
    }
}

fn model_bone(model: &Model, name: &str) -> Option<usize> {
    model.skeleton.bones.iter().position(|b| b.name == name)
}

fn world_pos(model: &Model, name: &str) -> Option<Vec3> {
    let i = model_bone(model, name)?;
    Some(
        model.skeleton.bones[i]
            .global_bind
            .transform_point(Vec3::default()),
    )
}

/// A world-space tilt axis from the skeleton's own geometry (up = pelvis->head,
/// forward = feet facing; axis = up x forward), so the oscillation reads as a
/// forward/back bow. Falls back to world X if the landmark bones are missing.
fn tilt_axis_world(model: &Model) -> Vec3 {
    let fallback = Vec3 {
        x: 1.0,
        y: 0.0,
        z: 0.0,
    };
    let (Some(head), Some(pelvis)) = (world_pos(model, "head"), world_pos(model, "pelvis")) else {
        return fallback;
    };
    let up = sub(head, pelvis).normalized();
    let (Some(ankle_l), Some(ball_l)) = (world_pos(model, "ankle_L"), world_pos(model, "ball_L"))
    else {
        return fallback;
    };
    let toe = sub(ball_l, ankle_l);
    let d = toe.x * up.x + toe.y * up.y + toe.z * up.z;
    let forward = Vec3 {
        x: toe.x - d * up.x,
        y: toe.y - d * up.y,
        z: toe.z - d * up.z,
    }
    .normalized();
    let axis = cross(up, forward).normalized();
    if axis.x.is_finite() && (axis.x.abs() + axis.y.abs() + axis.z.abs()) > 0.1 {
        axis
    } else {
        fallback
    }
}

fn main() -> Result<()> {
    let mut positional: Vec<String> = Vec::new();
    let mut bobs: Vec<(String, f32)> = Vec::new();
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        if arg == "--bob" {
            let v = raw.next().context("--bob needs BONE:UNITS")?;
            let (bone, units) = v.split_once(':').context("--bob needs BONE:UNITS")?;
            bobs.push((bone.to_owned(), units.parse().context("--bob units")?));
        } else {
            positional.push(arg);
        }
    }
    let mut args = positional.into_iter();
    let pak = args.next().context("missing arg: pak01_dir.vpk")?;
    let mesh_entry = args.next().context("missing arg: mesh entry")?;
    let clip_entry = args.next().context("missing arg: clip entry")?;
    let out = args.next().context("missing arg: out.glb")?;
    let amplitude: f32 = args.next().map_or(45.0, |s| s.parse().unwrap_or(45.0));
    let cycles: f32 = args.next().map_or(2.0, |s| s.parse().unwrap_or(2.0));

    let clip_bytes = vpkmerge_core::read_vpk_entry(&pak, &clip_entry)?;
    let clip = decode_nm_clip(&clip_bytes).context("decode clip")?;
    let skel = decode_nm_skeleton(&vpkmerge_core::read_vpk_entry(
        &pak,
        &resolve_skel(&clip.skeleton_ref),
    )?)
    .context("decode vnmskel")?;
    let model =
        decode(&vpkmerge_core::read_vpk_entry(&pak, &mesh_entry)?).context("decode mesh")?;
    let frames = clip.frame_count as usize;

    let world_axis = tilt_axis_world(&model);
    println!(
        "slot {clip_entry}: {frames} frames, amplitude {amplitude} deg x {cycles} cycles\n  \
         tilt axis (world) [{:.2}, {:.2}, {:.2}]",
        world_axis.x, world_axis.y, world_axis.z
    );

    // For every bone the clip already animates in rotation, layer a big oscillating
    // tilt onto its local rotation track. Local axis = world axis expressed in the
    // bone's bind frame, so a post-multiplied delta tilts it the same way in world.
    let mut tracks: Vec<BoneTrack> = Vec::new();
    for (i, t) in clip.tracks.iter().enumerate() {
        let Some(rots) = &t.rotations else { continue };
        let Some(name) = skel.bone_names.get(i) else {
            continue;
        };
        let Some(mi) = model_bone(&model, name) else {
            continue;
        };
        let local_axis = model.skeleton.bones[mi]
            .inverse_bind
            .transform_vector(world_axis)
            .normalized();
        let amped: Vec<Quat> = rots
            .iter()
            .enumerate()
            .map(|(f, q)| {
                let tt = if frames > 1 {
                    f as f32 / (frames - 1) as f32
                } else {
                    0.0
                };
                // Window the oscillation so it eases in/out (no pop at the loop seam).
                let window = (PI * tt).sin();
                let amount = amplitude * window * (2.0 * PI * cycles * tt).sin();
                normalize(qmul(*q, axis_angle(local_axis, amount)))
            })
            .collect();
        tracks.push(BoneTrack {
            bone: mi,
            translations: None,
            rotations: Some(amped),
            scales: None,
        });
    }
    anyhow::ensure!(
        !tracks.is_empty(),
        "clip animates no rotation bones present in the mesh skeleton"
    );

    for (bone, units) in &bobs {
        let ti = skel
            .bone_names
            .iter()
            .position(|n| n == bone)
            .with_context(|| format!("--bob: no bone {bone} in the clip skeleton"))?;
        let mi = model_bone(&model, bone).with_context(|| format!("--bob: {bone} not in mesh"))?;
        let t = &clip.tracks[ti];
        let r = &t.settings.translation_range;
        let base: Vec<Vec3> = match &t.translations {
            Some(v) => v.clone(),
            None => vec![
                Vec3 {
                    x: r[0].start,
                    y: r[1].start,
                    z: r[2].start,
                };
                frames
            ],
        };
        let dir = base[0].normalized();
        let bobbed: Vec<Vec3> = base
            .iter()
            .enumerate()
            .map(|(f, p)| {
                let tt = if frames > 1 {
                    f as f32 / (frames - 1) as f32
                } else {
                    0.0
                };
                let k = units * (tt / 0.15).min((1.0 - tt) / 0.15).clamp(0.0, 1.0);
                Vec3 {
                    x: p.x + dir.x * k,
                    y: p.y + dir.y * k,
                    z: p.z + dir.z * k,
                }
            })
            .collect();
        println!(
            "  bob {bone}: {units} units along its offset (slot translation {})",
            if t.translations.is_some() {
                "animated"
            } else {
                "STATIC: needs --full"
            }
        );
        match tracks.iter_mut().find(|bt| bt.bone == mi) {
            Some(bt) => bt.translations = Some(bobbed),
            None => tracks.push(BoneTrack {
                bone: mi,
                translations: Some(bobbed),
                rotations: None,
                scales: None,
            }),
        }
    }
    println!("  animated {} bone rotation track(s)", tracks.len());

    let mut out_model = model;
    out_model.animations = vec![Clip {
        name: "obvious".to_owned(),
        fps: clip.fps(),
        frame_count: frames,
        looping: false,
        tracks,
    }];

    let glb = morphic::model::to_glb(&out_model).context("write glb")?;
    std::fs::write(&out, &glb)?;
    println!(
        "wrote {out} ({} bytes)\nnext: import it with the nm_clip_import_glb example",
        glb.len()
    );
    Ok(())
}

fn resolve_skel(reference: &str) -> String {
    if reference.ends_with("_c") {
        reference.to_owned()
    } else {
        format!("{reference}_c")
    }
}
