//! Model skeleton, ported from VRF `Skeleton.FromModelData` +
//! `Bone` (`ResourceTypes/ModelAnimation`). Built from the model `DATA` block's
//! `m_modelSkeleton`; this is the joint set the glTF skin uses, and its bone
//! *names* are the retarget key Grimoire matches the shared animation clips
//! against, so they must equal what VRF emits.
//!
//! Local bind pose = `fromQuat(rotation) * translate(position)` (scale is
//! deliberately ignored, matching VRF's `Bone`). Global bind pose chains up the
//! parent hierarchy; the inverse-bind matrix is its inverse.

// KV3 stores bone flags as wider integers and positions/rotations as f64-widened
// f32; narrowing them back is exact for real model data. Sign/range are checked
// before the index casts that need it.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use crate::error::DecodeError;
use crate::kv3::Value;

use super::math::{Mat4, Quat, Vec3};

/// One bone in the model skeleton.
#[derive(Debug, Clone)]
pub struct Bone {
    pub name: String,
    /// Index of the parent bone, or `None` for a root.
    pub parent: Option<usize>,
    /// `ModelSkeletonBoneFlags`, stored verbatim for later filtering.
    pub flags: u32,
    /// Parent-space (local) translation.
    pub position: Vec3,
    /// Parent-space (local) rotation.
    pub rotation: Quat,
    /// Local (parent-space) bind pose.
    pub local_bind: Mat4,
    /// Model-space bind pose (local chained through ancestors).
    pub global_bind: Mat4,
    /// Inverse of [`Bone::global_bind`]: the glTF inverse-bind matrix.
    pub inverse_bind: Mat4,
}

/// The model's bone hierarchy.
#[derive(Debug, Clone)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
}

impl Skeleton {
    /// Index of the bone named `name`. An exact match wins; otherwise a bone whose
    /// name differs only in ASCII case, when exactly one does. The engine matches
    /// bone names without case: Kelvin's animation skeleton names `Pelvis` where his
    /// model has `pelvis`, and he stands in game.
    #[must_use]
    pub fn find_bone(&self, name: &str) -> Option<usize> {
        if let Some(index) = self.bones.iter().position(|b| b.name == name) {
            return Some(index);
        }
        let mut folded = self
            .bones
            .iter()
            .enumerate()
            .filter(|(_, b)| b.name.eq_ignore_ascii_case(name));
        match (folded.next(), folded.next()) {
            (Some((index, _)), None) => Some(index),
            _ => None,
        }
    }

    /// Builds the skeleton from the model `DATA` KV3 tree (`m_modelSkeleton`).
    /// Returns an empty skeleton when the model carries no skeleton data.
    pub fn from_model_data(data: &Value) -> Result<Skeleton, DecodeError> {
        let Some(skel) = data.get("m_modelSkeleton") else {
            return Ok(Skeleton { bones: Vec::new() });
        };

        let names = skel
            .get("m_boneName")
            .and_then(Value::as_array)
            .ok_or(DecodeError::Model("skeleton missing m_boneName"))?;
        let parents = skel
            .get("m_nParent")
            .and_then(Value::as_array)
            .ok_or(DecodeError::Model("skeleton missing m_nParent"))?;
        let flags = skel.get("m_nFlag").and_then(Value::as_array);
        let positions = skel
            .get("m_bonePosParent")
            .and_then(Value::as_array)
            .ok_or(DecodeError::Model("skeleton missing m_bonePosParent"))?;
        let rotations = skel
            .get("m_boneRotParent")
            .and_then(Value::as_array)
            .ok_or(DecodeError::Model("skeleton missing m_boneRotParent"))?;

        let count = names.len();
        if parents.len() != count || positions.len() != count || rotations.len() != count {
            return Err(DecodeError::Model("skeleton array length mismatch"));
        }

        // First pass: names, parents, local transforms, local bind pose.
        let mut bones: Vec<Bone> = Vec::with_capacity(count);
        for i in 0..count {
            let name = names[i]
                .as_str()
                .ok_or(DecodeError::Model("bone name not a string"))?
                .to_owned();
            let parent_raw = parents[i]
                .as_int()
                .ok_or(DecodeError::Model("bone parent not an int"))?;
            let parent = if parent_raw < 0 {
                None
            } else {
                Some(parent_raw as usize)
            };
            let flag = flags
                .and_then(|f| f.get(i))
                .and_then(Value::as_uint)
                .unwrap_or(0) as u32;
            let position = read_vec3(&positions[i])?;
            let rotation = read_quat(&rotations[i])?;
            let local_bind = Mat4::from_quaternion(rotation).mul(&Mat4::from_translation(position));

            bones.push(Bone {
                name,
                parent,
                flags: flag,
                position,
                rotation,
                local_bind,
                global_bind: Mat4::IDENTITY,
                inverse_bind: Mat4::IDENTITY,
            });
        }

        // A parent must precede its child for a single forward pass to resolve
        // global poses; Source 2 skeletons are emitted parent-first, but guard
        // it rather than silently producing wrong matrices.
        for (i, bone) in bones.iter().enumerate() {
            if let Some(p) = bone.parent {
                if p >= i {
                    return Err(DecodeError::Model("bone parent not topologically ordered"));
                }
            }
        }

        // Second pass: global bind = local * parent_global, then invert.
        for i in 0..count {
            let global = match bones[i].parent {
                Some(p) => bones[i].local_bind.mul(&bones[p].global_bind),
                None => bones[i].local_bind,
            };
            let inverse = global
                .invert()
                .ok_or(DecodeError::Model("bind pose not invertible"))?;
            bones[i].global_bind = global;
            bones[i].inverse_bind = inverse;
        }

        Ok(Skeleton { bones })
    }

    /// Bone names sorted ascending: the stable set used to validate against the
    /// golden skin and to retarget clips by name.
    #[must_use]
    pub fn sorted_bone_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.bones.iter().map(|b| b.name.clone()).collect();
        names.sort();
        names
    }
}

/// Reads the per-mesh bone remap table from the model `DATA` block.
///
/// Mirrors VRF `Model.GetRemapTable`: slices `m_remappingTable` using
/// `m_remappingTableStarts[meshIndex] .. [meshIndex + 1]`. The result maps a
/// mesh-local `BLENDINDICES` value to a model skeleton bone index. Returns
/// `None` when the mesh has no remap entry (treated as identity by the caller).
pub fn remap_table(data: &Value, mesh_index: usize) -> Option<Vec<usize>> {
    let starts = data
        .get("m_remappingTableStarts")
        .and_then(Value::as_array)?;
    if mesh_index >= starts.len() {
        return None;
    }
    let table = data.get("m_remappingTable").and_then(Value::as_array)?;

    let start = usize::try_from(starts[mesh_index].as_int()?).ok()?;
    let end = if mesh_index + 1 < starts.len() {
        usize::try_from(starts[mesh_index + 1].as_int()?).ok()?
    } else {
        table.len()
    };
    if start > end || end > table.len() {
        return None;
    }

    let mut out = Vec::with_capacity(end - start);
    for entry in &table[start..end] {
        out.push(usize::try_from(entry.as_int()?).ok()?);
    }
    Some(out)
}

/// Inverts a mesh bone-remap table (mesh-local -> model bone, as
/// [`remap_table`] returns) into a `model bone -> mesh-local` lookup. The read
/// path applies the remap *forward* (on-disk `BLENDINDICES` are local, decoded to
/// model bones); the write path needs the inverse to put a model-space `JOINTS_0`
/// back into the mesh's local palette. If two local slots map to the same model
/// bone (palette duplicate), the lowest local wins (either references the same
/// bone, so the result is identical).
#[must_use]
pub fn invert_remap(remap: &[usize]) -> std::collections::HashMap<usize, u16> {
    let mut inv = std::collections::HashMap::with_capacity(remap.len());
    for (local, &model) in remap.iter().enumerate() {
        if let Ok(local_u16) = u16::try_from(local) {
            inv.entry(model).or_insert(local_u16);
        }
    }
    inv
}

/// Maps a skinned mesh's model-space `JOINTS_0` into a target mesh's local
/// `BLENDINDICES` space using that mesh's bone remap (local -> model). This is
/// T1d-c: the new mesh is skinned against the exported hero skeleton (glTF joint
/// indices == model bone indices), but the replaced part's draw call resolves
/// `BLENDINDICES` through the *target mesh's* palette, so each influence must be
/// expressed as a local palette slot.
///
/// Per influence lane: a *significant* influence (non-zero weight, or lane 0 when
/// no weights are supplied) must resolve to a local slot, else this errors so a
/// mis-skinned part fails loudly instead of binding to the wrong bone. A
/// non-significant lane (zero weight) maps to its local slot if the bone is in the
/// palette, else to local 0 (its index is unused at render time).
///
/// `weights` is optional: a rigid mesh (`BLENDINDICES` but no `BLENDWEIGHT`)
/// passes `None`, and only lane 0 is treated as significant.
pub fn localize_joints(
    joints: &[[u16; 4]],
    weights: Option<&[[f32; 4]]>,
    remap: &[usize],
) -> Result<Vec<[u16; 4]>, DecodeError> {
    if let Some(w) = weights {
        if w.len() != joints.len() {
            return Err(DecodeError::Model("JOINTS_0 / WEIGHTS_0 count mismatch"));
        }
    }
    let inv = invert_remap(remap);

    let mut out = Vec::with_capacity(joints.len());
    for (i, j) in joints.iter().enumerate() {
        let mut local = [0u16; 4];
        for (k, &model) in j.iter().enumerate() {
            let significant = match weights {
                Some(w) => w[i][k] > 0.0,
                None => k == 0,
            };
            match inv.get(&usize::from(model)) {
                Some(&l) => local[k] = l,
                None if !significant => local[k] = 0,
                None => {
                    return Err(DecodeError::Model(
                        "JOINTS_0 references a model bone outside the target mesh's bone palette \
                         (weight-paint the new mesh to bones the replaced part already uses)",
                    ))
                }
            }
        }
        out.push(local);
    }
    Ok(out)
}

fn read_vec3(v: &Value) -> Result<Vec3, DecodeError> {
    let a = v
        .as_array()
        .ok_or(DecodeError::Model("vec3 not an array"))?;
    if a.len() < 3 {
        return Err(DecodeError::Model("vec3 too short"));
    }
    Ok(Vec3 {
        x: f64_to_f32(&a[0])?,
        y: f64_to_f32(&a[1])?,
        z: f64_to_f32(&a[2])?,
    })
}

fn read_quat(v: &Value) -> Result<Quat, DecodeError> {
    let a = v
        .as_array()
        .ok_or(DecodeError::Model("quat not an array"))?;
    if a.len() < 4 {
        return Err(DecodeError::Model("quat too short"));
    }
    Ok(Quat {
        x: f64_to_f32(&a[0])?,
        y: f64_to_f32(&a[1])?,
        z: f64_to_f32(&a[2])?,
        w: f64_to_f32(&a[3])?,
    })
}

fn f64_to_f32(v: &Value) -> Result<f32, DecodeError> {
    v.as_f64()
        .map(|d| d as f32)
        .ok_or(DecodeError::Model("expected numeric component"))
}

#[cfg(test)]
mod remap_tests {
    use super::*;

    /// `invert_remap` turns a subset palette (local -> model) into model -> local.
    #[test]
    fn invert_remap_inverts_a_subset_palette() {
        // The gun's real palette prefix: local 2 -> model 4, local 4 -> model 7.
        let remap = vec![0usize, 1, 4, 5, 7, 31];
        let inv = invert_remap(&remap);
        assert_eq!(inv.get(&0), Some(&0));
        assert_eq!(inv.get(&1), Some(&1));
        assert_eq!(inv.get(&4), Some(&2));
        assert_eq!(inv.get(&5), Some(&3));
        assert_eq!(inv.get(&7), Some(&4));
        assert_eq!(inv.get(&31), Some(&5));
        assert_eq!(inv.get(&99), None, "bone not in palette");
    }

    /// A palette duplicate resolves to the lowest local (both reference the same
    /// model bone, so the choice is render-equivalent).
    #[test]
    fn invert_remap_keeps_lowest_local_on_duplicate() {
        let remap = vec![3usize, 7, 3];
        let inv = invert_remap(&remap);
        assert_eq!(inv.get(&3), Some(&0), "first local wins");
    }

    /// Model-space joints are mapped into the local palette; the read path's
    /// forward remap would turn them back into the original model bones.
    #[test]
    fn localize_joints_maps_into_local_palette() {
        let remap = vec![0usize, 1, 4, 5, 7, 31];
        // model bones 7, 4, 1, 0 -> local 4, 2, 1, 0.
        let joints = vec![[7u16, 4, 1, 0]];
        let weights = vec![[0.5f32, 0.3, 0.2, 0.0]];
        let local = localize_joints(&joints, Some(&weights), &remap).expect("localize");
        assert_eq!(local, vec![[4, 2, 1, 0]]);
        // The forward remap recovers the model bones for the weighted lanes.
        for k in 0..3 {
            assert_eq!(remap[usize::from(local[0][k])], usize::from(joints[0][k]));
        }
    }

    /// A significant (non-zero-weight) influence on a bone outside the palette is
    /// a hard error: the part would otherwise bind to the wrong bone.
    #[test]
    fn localize_joints_errors_on_significant_bone_outside_palette() {
        let remap = vec![0usize, 1, 4];
        let joints = vec![[9u16, 0, 0, 0]]; // model bone 9 not in palette
        let weights = vec![[1.0f32, 0.0, 0.0, 0.0]];
        assert!(localize_joints(&joints, Some(&weights), &remap).is_err());
    }

    /// A zero-weight lane referencing an out-of-palette bone is tolerated (maps to
    /// local 0); its index is unused at render time.
    #[test]
    fn localize_joints_ignores_zero_weight_out_of_palette() {
        let remap = vec![0usize, 1, 4];
        let joints = vec![[4u16, 99, 99, 99]];
        let weights = vec![[1.0f32, 0.0, 0.0, 0.0]];
        let local = localize_joints(&joints, Some(&weights), &remap).expect("localize");
        assert_eq!(local, vec![[2, 0, 0, 0]]);
    }

    /// A rigid mesh (no weights): only lane 0 must resolve; the rest map to 0.
    #[test]
    fn localize_joints_rigid_only_lane0_significant() {
        let remap = vec![0usize, 1, 4, 7];
        let joints = vec![[7u16, 88, 88, 88]];
        let local = localize_joints(&joints, None, &remap).expect("localize rigid");
        assert_eq!(local, vec![[3, 0, 0, 0]]);

        // But a rigid mesh whose lane 0 is out of palette still errors.
        let bad = vec![[88u16, 0, 0, 0]];
        assert!(localize_joints(&bad, None, &remap).is_err());
    }

    #[test]
    fn find_bone_prefers_exact_then_unique_case_fold() {
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
        let kelvin = Skeleton {
            bones: vec![bone("root_motion"), bone("pelvis")],
        };
        assert_eq!(kelvin.find_bone("Pelvis"), Some(1));
        assert_eq!(kelvin.find_bone("thigh_L"), None);
        let both = Skeleton {
            bones: vec![bone("Pelvis"), bone("pelvis")],
        };
        assert_eq!(both.find_bone("pelvis"), Some(1));
        assert_eq!(both.find_bone("PELVIS"), None, "ambiguous fold");
    }
}
