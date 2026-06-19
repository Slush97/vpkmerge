//! Round-trip for the glTF animation importer (`model::gltf_import`), the
//! engine side of the Blender authoring loop.
//!
//! The authoring contract is: the `.glb` writer names each joint node after its
//! bone and keeps per-bone TRS in raw Source local space (only the skeleton
//! wrapper node carries the axis/scale transform), so an animation read back maps
//! to bones by name with no inverse transform. These tests assert that contract
//! holds against the real writer:
//!  - `glb_writer_reader_round_trip`: author a synthetic skeleton + clip, export
//!    via `to_glb`, read it back with `read_glb_animation`, and confirm every
//!    bone's translation/rotation/scale samples and times survive.
//!  - `apply_animation_maps_by_name_and_respects_constraints`: map an imported
//!    animation onto a real fixture clip and confirm the in-place-encoder limits
//!    (rotations edit+add, translation/scale edit-only) are honored per bone name.

// Frame indices widen to f32 to build keyframe ramps; exact for these tiny counts.
#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;

use morphic::model::{
    apply_animation, decode_nm_clip, read_glb_animation, to_glb, Bone, BoneTrack, Clip,
    GltfAnimation, GltfBoneTrack, Mat4, Model, NmSkeleton, Quat, Skeleton, Vec3,
};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/fixtures/nm/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"))
}

fn bone(name: &str, parent: Option<usize>) -> Bone {
    Bone {
        name: name.to_owned(),
        parent,
        flags: 0,
        position: Vec3::default(),
        rotation: Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        },
        local_bind: Mat4::IDENTITY,
        global_bind: Mat4::IDENTITY,
        inverse_bind: Mat4::IDENTITY,
    }
}

/// A small yaw-about-Z quaternion, used to build a known rotation ramp.
fn yaw(deg: f32) -> Quat {
    let half = deg.to_radians() * 0.5;
    Quat {
        x: 0.0,
        y: 0.0,
        z: half.sin(),
        w: half.cos(),
    }
}

#[test]
fn glb_writer_reader_round_trip() {
    // Author a 3-bone skeleton and a 5-frame clip: one bone gets a rotation ramp,
    // another a translation slide, another a scale ramp. The writer/reader pair
    // must reproduce every channel keyed by bone name.
    let skeleton = Skeleton {
        bones: vec![
            bone("root", None),
            bone("spine", Some(0)),
            bone("arm_R", Some(1)),
        ],
    };
    let frames = 5usize;
    let fps = 30.0f32;

    let rotations: Vec<Quat> = (0..frames)
        .map(|f| yaw(90.0 * f as f32 / (frames - 1) as f32))
        .collect();
    let translations: Vec<Vec3> = (0..frames)
        .map(|f| Vec3 {
            x: 10.0 * f as f32,
            y: 0.0,
            z: 0.0,
        })
        .collect();
    let scales: Vec<f32> = (0..frames).map(|f| 1.0 + 0.25 * f as f32).collect();

    let clip = Clip {
        name: "test_clip".to_owned(),
        fps,
        frame_count: frames,
        looping: false,
        tracks: vec![
            BoneTrack {
                bone: 0,
                translations: None,
                rotations: Some(rotations.clone()),
                scales: None,
            },
            BoneTrack {
                bone: 1,
                translations: Some(translations.clone()),
                rotations: None,
                scales: None,
            },
            BoneTrack {
                bone: 2,
                translations: None,
                rotations: None,
                scales: Some(scales.clone()),
            },
        ],
    };

    let model = Model {
        skeleton,
        meshes: Vec::new(),
        animations: vec![clip],
        cloth: None,
    };

    let glb = to_glb(&model).expect("export synthetic model to glb");
    let anim = read_glb_animation(&glb, None).expect("read animation back");

    assert_eq!(anim.name.as_deref(), Some("test_clip"));

    // root: rotation ramp.
    let root = anim.bones.get("root").expect("root bone present");
    let got_rot = root.rotations.as_ref().expect("root has rotations");
    assert_eq!(got_rot.len(), frames);
    for (f, (&(t, q), want)) in got_rot.iter().zip(&rotations).enumerate() {
        assert!(
            (t - f as f32 / fps).abs() < 1e-5,
            "rotation key {f} time {t} != {}",
            f as f32 / fps
        );
        let dot = (q.x * want.x + q.y * want.y + q.z * want.z + q.w * want.w).abs();
        assert!(dot > 0.9999, "rotation key {f}: {q:?} != {want:?}");
    }
    assert!(root.translations.is_none() && root.scales.is_none());

    // spine: translation slide.
    let spine = anim.bones.get("spine").expect("spine bone present");
    let got_tr = spine.translations.as_ref().expect("spine has translations");
    assert_eq!(got_tr.len(), frames);
    for (&(_, v), want) in got_tr.iter().zip(&translations) {
        assert!(
            (v.x - want.x).abs() < 1e-4
                && (v.y - want.y).abs() < 1e-4
                && (v.z - want.z).abs() < 1e-4,
            "translation {v:?} != {want:?}"
        );
    }

    // arm_R: scale ramp (writer emits [s,s,s]; reader takes x).
    let arm = anim.bones.get("arm_R").expect("arm bone present");
    let got_sc = arm.scales.as_ref().expect("arm has scales");
    assert_eq!(got_sc.len(), frames);
    for (&(_, s), want) in got_sc.iter().zip(&scales) {
        assert!((s - want).abs() < 1e-4, "scale {s} != {want}");
    }
}

#[test]
fn apply_animation_maps_by_name_and_respects_constraints() {
    // Map an imported animation onto a real fixture clip. The synthetic skeleton
    // names the tracks bone0, bone1, ...; we drive a rotation onto a
    // static-rotation track (an ADD) and confirm:
    //  - it lands on the track whose bone name we animated,
    //  - the frame count matches the slot (time-stretch resample),
    //  - a translation aimed at a static-translation track is IGNORED (edit-only),
    //  - tracks the animation does not name are untouched.
    let bytes = fixture("yamato_reload_idle_quick.vnmclip_c");
    let clip = decode_nm_clip(&bytes).expect("decode fixture clip");
    let names: Vec<String> = (0..clip.tracks.len()).map(|i| format!("bone{i}")).collect();
    let skel = NmSkeleton {
        bone_names: names.clone(),
    };

    let rot_target = clip
        .tracks
        .iter()
        .position(|t| t.rotations.is_none())
        .expect("a static-rotation track");
    // A *different* track that is static in translation, so the two edits don't
    // collide on one bone name.
    let trans_static = clip
        .tracks
        .iter()
        .enumerate()
        .position(|(i, t)| i != rot_target && t.translations.is_none())
        .expect("a second static-translation track");

    // Source animation: a 0 -> 60deg yaw ramp on the rotation target (sampled on
    // its own clock), plus a translation slide aimed at a static-translation
    // track (which must be refused by the edit-only rule).
    let src_frames = 4usize;
    let rot_keys: Vec<(f32, Quat)> = (0..src_frames)
        .map(|f| {
            let t = f as f32 / (src_frames - 1) as f32; // 0..1 seconds
            (t, yaw(60.0 * t))
        })
        .collect();
    let trans_keys: Vec<(f32, Vec3)> = (0..src_frames)
        .map(|f| {
            let t = f as f32 / (src_frames - 1) as f32;
            (
                t,
                Vec3 {
                    x: 50.0 * t,
                    y: 0.0,
                    z: 0.0,
                },
            )
        })
        .collect();

    let mut bones = HashMap::new();
    bones.insert(
        names[rot_target].clone(),
        GltfBoneTrack {
            rotations: Some(rot_keys),
            ..Default::default()
        },
    );
    bones.insert(
        names[trans_static].clone(),
        GltfBoneTrack {
            translations: Some(trans_keys),
            ..Default::default()
        },
    );
    let anim = GltfAnimation {
        name: Some("import".to_owned()),
        bones,
    };

    let edited = apply_animation(&clip, &skel, &anim);

    // Frame count preserved; the rotation was ADDED to the static track.
    assert_eq!(edited.frame_count, clip.frame_count);
    let added = edited.tracks[rot_target]
        .rotations
        .as_ref()
        .expect("rotation added to the static-rotation track");
    assert_eq!(added.len(), clip.frame_count as usize);
    // Stretched onto the slot: frame 0 ~identity, last frame ~60deg about Z.
    assert!(added[0].z.abs() < 0.05, "first frame near identity");
    let last = added[added.len() - 1];
    assert!(
        last.z.abs() > 0.2,
        "last frame reaches a clear yaw: {last:?}"
    );

    // The translation aimed at a static-translation track was refused (edit-only).
    assert!(
        edited.tracks[trans_static].translations.is_none(),
        "adding a translation channel must be ignored, not applied"
    );

    // Every track the animation did not name is byte-identical to the original.
    for (i, (a, b)) in clip.tracks.iter().zip(edited.tracks.iter()).enumerate() {
        if i != rot_target && i != trans_static {
            assert_eq!(a, b, "unnamed track {i} must be untouched");
        }
    }
    // The static-translation track only had its (refused) translation touched; its
    // other channels are unchanged.
    assert_eq!(
        clip.tracks[trans_static].rotations, edited.tracks[trans_static].rotations,
        "refused-translation track's rotation unchanged"
    );
}
