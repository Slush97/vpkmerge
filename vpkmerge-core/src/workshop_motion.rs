//! Animation-only glTF bundles from the current game's graph-referenced NM clips.
use anyhow::{ensure, Context, Result};
use std::path::{Path, PathBuf};

/// Export non-additive clips mapped by bone name onto the selected model's rig.
/// Root translation stays under the preview stage's control. Rotation is authored pose data.
///
/// # Errors
/// Rejects unsafe resource paths, additive clips, oversized bundles and incompatible rigs.
pub fn export_motions(
    paths: &[PathBuf],
    model_entry: &str,
    clips: &[(String, String)],
    output: &Path,
) -> Result<()> {
    ensure!(
        !clips.is_empty() && clips.len() <= 8,
        "Choose one to eight motion clips"
    );
    let vpks = paths
        .iter()
        .map(valve_pak::open)
        .collect::<Result<Vec<_>>>()?;
    let resource = |entry: &str| -> Result<Vec<u8>> {
        ensure!(
            entry.starts_with("models/")
                && !entry.contains("..")
                && !entry.contains('\\')
                && !entry.contains(':'),
            "Invalid motion resource path"
        );
        crate::model::read_entry(&vpks, entry)
            .with_context(|| format!("Missing motion resource: {entry}"))
    };
    let mut model = morphic::model::decode(&resource(model_entry)?)?;
    let root = model
        .skeleton
        .bones
        .iter()
        .position(|bone| bone.name == "root_motion");
    let legacy = if clips.iter().any(|(_, path)| path.starts_with("legacy:")) {
        embedded_clips(&vpks, model_entry, &mut model)?
    } else {
        model.animations.clear();
        Vec::new()
    };
    model.meshes.clear();
    let mut frames = 0;
    for (name, path) in clips {
        ensure!(
            name.len() <= 100 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "Invalid motion name"
        );
        if let Some(legacy_name) = path.strip_prefix("legacy:") {
            let mut animation = legacy
                .iter()
                .find(|animation| animation.name == legacy_name)
                .cloned()
                .with_context(|| format!("Missing embedded animation: {legacy_name}"))?;
            ensure!(
                !legacy_name.contains("add") && !legacy_name.contains("delta"),
                "Embedded additive clips require a base"
            );
            frames += u32::try_from(animation.frame_count)?;
            ensure!(frames <= 2400, "Motion bundle exceeds frame budget");
            animation.name.clone_from(name);
            strip_root_translation(&mut animation, root);
            model.animations.push(animation);
            continue;
        }
        let clip = morphic::model::decode_nm_clip(&resource(&compiled(path))?)?;
        ensure!(
            !clip.additive,
            "Additive clip requires an explicit base and mask: {path}"
        );
        frames += clip.frame_count;
        ensure!(
            frames <= 2400 && clip.frame_count > 1,
            "Motion bundle exceeds frame budget or contains a static pose"
        );
        let skeleton =
            morphic::model::decode_nm_skeleton(&resource(&compiled(&clip.skeleton_ref))?)?;
        let mut animation =
            morphic::model::nm_clip_to_clip(&clip, &skeleton, &model.skeleton, name);
        let rig_bones = animated_bone_count(model.skeleton.bones.iter().map(|b| b.name.as_str()));
        ensure!(
            motion_matches_rig(animation.tracks.len(), clip.tracks.len(), rig_bones),
            "Motion does not match the selected hero's rig: {} of {} source tracks matched ({} model bones)",
            animation.tracks.len(), clip.tracks.len(), model.skeleton.bones.len()
        );
        strip_root_translation(&mut animation, root);
        model.animations.push(animation);
    }
    let bytes = morphic::model::to_glb(&model)?;
    std::fs::write(output, bytes)?;
    Ok(())
}

fn compiled(path: &str) -> String {
    if path.ends_with("_c") {
        path.to_string()
    } else {
        format!("{path}_c")
    }
}

// NM rigs and render rigs can each carry extra helper, cloth or weapon bones.
// Compare the shared tracks against the smaller rig rather than penalizing extras.
fn motion_matches_rig(matched: usize, source: usize, target: usize) -> bool {
    matched >= 2 && matched * 2 >= source.min(target)
}

// `$cloth_*` bones are posed by the cloth solver, never by motion clips, so a
// cloth-heavy mod rig would otherwise fall under the match bar.
fn animated_bone_count<'a>(names: impl IntoIterator<Item = &'a str>) -> usize {
    names
        .into_iter()
        .filter(|name| !name.starts_with("$cloth_"))
        .count()
}

fn strip_root_translation(animation: &mut morphic::model::Clip, root: Option<usize>) {
    for track in &mut animation.tracks {
        if Some(track.bone) == root {
            track.translations = None;
        }
    }
}

fn embedded_clips(
    vpks: &[valve_pak::VPK],
    model_entry: &str,
    model: &mut morphic::model::Model,
) -> Result<Vec<morphic::model::Clip>> {
    let mut legacy = std::mem::take(&mut model.animations);
    if legacy.is_empty() {
        for pak in vpks.iter().skip(1) {
            let Some(bytes) = crate::model::read_entry(std::slice::from_ref(pak), model_entry)
            else {
                continue;
            };
            let donor = morphic::model::decode(&bytes)?;
            if donor.animations.is_empty() {
                continue;
            }
            legacy = donor
                .animations
                .into_iter()
                .map(|mut animation| {
                    animation.tracks = animation
                        .tracks
                        .into_iter()
                        .filter_map(|mut track| {
                            let name = &donor.skeleton.bones.get(track.bone)?.name;
                            track.bone = model
                                .skeleton
                                .bones
                                .iter()
                                .position(|bone| &bone.name == name)?;
                            Some(track)
                        })
                        .collect();
                    animation
                })
                .collect();
            break;
        }
    }
    Ok(legacy)
}

#[cfg(test)]
mod tests {
    use super::{animated_bone_count, motion_matches_rig, strip_root_translation};
    use morphic::model::{BoneTrack, Clip, Quat, Vec3};

    #[test]
    fn in_place_preview_keeps_authored_root_orientation_and_body_translation() {
        let rotation = Quat {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        };
        let translation = Vec3 {
            x: 30.0,
            y: 0.0,
            z: 0.0,
        };
        let mut clip = Clip {
            name: "standing".into(),
            fps: 30.0,
            frame_count: 2,
            looping: true,
            tracks: (0..2)
                .map(|bone| BoneTrack {
                    bone,
                    translations: Some(vec![translation; 2]),
                    rotations: Some(vec![rotation; 2]),
                    scales: Some(vec![1.0; 2]),
                })
                .collect(),
        };
        strip_root_translation(&mut clip, Some(0));
        assert_eq!(clip.tracks[0].translations, None);
        assert_eq!(clip.tracks[0].rotations, Some(vec![rotation; 2]));
        assert_eq!(clip.tracks[0].scales, Some(vec![1.0; 2]));
        assert_eq!(clip.tracks[1].translations, Some(vec![translation; 2]));
    }

    #[test]
    fn tolerates_extra_bones_on_either_rig_but_rejects_incidental_matches() {
        assert!(motion_matches_rig(61, 118, 63));
        assert!(motion_matches_rig(75, 80, 250));
        assert!(motion_matches_rig(104, 167, 167));
        assert!(!motion_matches_rig(2, 80, 250));
        assert!(!motion_matches_rig(0, 0, 0));
    }

    #[test]
    fn cloth_bones_do_not_count_against_the_target_rig() {
        let cloth: Vec<String> = (0..180).map(|i| format!("$cloth_{i}")).collect();
        let body: Vec<String> = (0..60).map(|i| format!("bone_{i}")).collect();
        let rig = animated_bone_count(cloth.iter().chain(&body).map(String::as_str));
        assert_eq!(rig, 60);
        assert!(!motion_matches_rig(58, 118, 240));
        assert!(motion_matches_rig(58, 118, rig));
    }
}
