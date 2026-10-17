use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Context, Result};
use morphic::kv3::{Seg, Value};

use super::{copy_flags, CompatibilityRepairer, Document};

const FIELDS: [&str; 2] = ["m_animGraph2Refs", "m_vecNmSkeletonRefs"];

pub(super) fn is_animation(path: &str) -> bool {
    [
        ".vnmclip_c",
        ".vnmgraph_c",
        ".vanmgrph_c",
        ".vanim_c",
        ".vagrp_c",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
}

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value]> {
    value
        .get(key)
        .and_then(Value::as_array)
        .with_context(|| format!("Missing array {key}"))
}

fn names(value: &Value, key: &str) -> Result<BTreeMap<String, usize>> {
    let mut names = BTreeMap::new();
    let mut folded = BTreeSet::new();
    for (i, value) in array(value, key)?.iter().enumerate() {
        let name = value.as_str().context("Bone name is not a string")?;
        ensure!(
            folded.insert(name.to_lowercase()),
            "Ambiguous bone name: {name}"
        );
        names.insert(name.to_ascii_lowercase(), i);
    }
    ensure!(!names.is_empty(), "Empty skeleton");
    Ok(names)
}

fn parents(value: &Value, key: &str, count: usize) -> Result<Vec<Option<usize>>> {
    let raw = array(value, key)?;
    ensure!(raw.len() == count, "Skeleton parent count differs");
    let parents = raw
        .iter()
        .map(|value| {
            let parent = value.as_int().context("Invalid bone parent")?;
            if parent == -1 {
                return Ok(None);
            }
            let parent = usize::try_from(parent)?;
            ensure!(parent < count, "Bone parent out of bounds");
            Ok(Some(parent))
        })
        .collect::<Result<Vec<_>>>()?;
    for start in 0..count {
        let mut seen = BTreeSet::new();
        let mut at = Some(start);
        while let Some(index) = at {
            ensure!(seen.insert(index), "Cyclic skeleton hierarchy");
            at = parents[index];
        }
    }
    Ok(parents)
}

fn numbers(value: &Value, count: usize) -> Result<Vec<f64>> {
    let items = value.as_array().context("Invalid pose vector")?;
    ensure!(items.len() == count, "Invalid pose dimensions");
    items
        .iter()
        .map(|v| {
            v.as_f64()
                .filter(|n| n.is_finite())
                .context("Invalid pose number")
        })
        .collect()
}

fn close(a: &[f64], b: &[f64]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() <= 0.0001)
}

fn rotation(a: &[f64], b: &[f64]) -> bool {
    let unit = |q: &[f64]| (q.iter().map(|v| v * v).sum::<f64>() - 1.0).abs() < 0.001;
    unit(a) && unit(b) && (close(a, b) || a.iter().zip(b).all(|(a, b)| (a + b).abs() <= 0.0001))
}

fn same_pose(a: &Value, b: &Value) -> Result<bool> {
    let a = numbers(a, 8)?;
    let b = numbers(b, 8)?;
    Ok(rotation(&a[..4], &b[..4]) && close(&a[4..], &b[4..]))
}

fn named_parent(names: &[Value], parents: &[Option<usize>], index: usize) -> Option<String> {
    parents[index]
        .and_then(|i| names[i].as_str())
        .map(str::to_ascii_lowercase)
}

/// Replacing the sampling layout is safe only when surviving named bones retain
/// their parents and reference frames. Custom masks are left untouched by this
/// first adapter, rather than guessed onto another skeleton.
fn compatible_skeleton(old: &Value, current: &Value) -> Result<bool> {
    let old_names = names(old, "m_boneIDs")?;
    let current_names = names(current, "m_boneIDs")?;
    let old_parents = parents(old, "m_parentIndices", old_names.len())?;
    let current_parents = parents(current, "m_parentIndices", current_names.len())?;
    let old_list = array(old, "m_boneIDs")?;
    let current_list = array(current, "m_boneIDs")?;
    let old_lod = old
        .get("m_numBonesToSampleAtLowLOD")
        .and_then(Value::as_uint)
        .context("Missing sampling LOD")?;
    let current_lod = current
        .get("m_numBonesToSampleAtLowLOD")
        .and_then(Value::as_uint)
        .context("Missing current sampling LOD")?;
    ensure!(
        old_lod <= old_names.len() as u64 && current_lod <= current_names.len() as u64,
        "Invalid sampling LOD"
    );
    for field in ["m_parentSpaceReferencePose", "m_modelSpaceReferencePose"] {
        let a = array(old, field)?;
        let b = array(current, field)?;
        ensure!(
            a.len() == old_names.len() && b.len() == current_names.len(),
            "Reference-pose count differs"
        );
        for (name, &index) in &old_names {
            let &other = current_names
                .get(name)
                .with_context(|| format!("Custom or removed bone: {name}"))?;
            ensure!(
                ((index as u64) < old_lod) == ((other as u64) < current_lod),
                "Changed sampling LOD for {name}"
            );
            ensure!(
                named_parent(old_list, &old_parents, index)
                    == named_parent(current_list, &current_parents, other),
                "Changed parent for {name}"
            );
            ensure!(
                same_pose(&a[index], &b[other])?,
                "Changed reference pose for {name}"
            );
        }
    }
    for (key, authored) in old.as_object().context("Invalid skeleton")? {
        if matches!(
            key.as_str(),
            "m_boneIDs"
                | "m_parentIndices"
                | "m_parentSpaceReferencePose"
                | "m_modelSpaceReferencePose"
                | "m_numBonesToSampleAtLowLOD"
                | "m_nSpecialDependencyHash"
        ) {
            continue;
        }
        ensure!(
            current.get(key) == Some(authored),
            "Custom skeleton metadata or masks: {key}"
        );
    }
    // New fields are not inferred as safe metadata.
    for (key, _) in current.as_object().context("Invalid current skeleton")? {
        ensure!(
            old.get(key).is_some(),
            "Unrecognized new skeleton field: {key}"
        );
    }
    Ok(old != current)
}

fn model_pose(rig: &Value, index: usize) -> Result<(Vec<f64>, Vec<f64>, f64)> {
    let p = numbers(
        array(rig, "m_bonePosParent")?
            .get(index)
            .context("Missing bone position")?,
        3,
    )?;
    let r = numbers(
        array(rig, "m_boneRotParent")?
            .get(index)
            .context("Missing bone rotation")?,
        4,
    )?;
    let scale = array(rig, "m_boneScaleParent")?
        .get(index)
        .and_then(Value::as_f64)
        .context("Missing bone scale")?;
    ensure!(scale.is_finite(), "Invalid bone scale");
    Ok((p, r, scale))
}

fn prove_model(old: &Value, current: &Value, sampled: &Value) -> Result<()> {
    let a = old
        .get("m_modelSkeleton")
        .context("Missing authored model rig")?;
    let b = current
        .get("m_modelSkeleton")
        .context("Missing current model rig")?;
    let old_names = names(a, "m_boneName")?;
    let current_names = names(b, "m_boneName")?;
    let old_parents = parents(a, "m_nParent", old_names.len())?;
    let current_parents = parents(b, "m_nParent", current_names.len())?;
    let old_list = array(a, "m_boneName")?;
    let current_list = array(b, "m_boneName")?;
    let sampled_names = names(sampled, "m_boneIDs")?;
    let sampled_parents = parents(sampled, "m_parentIndices", sampled_names.len())?;
    let sampled_list = array(sampled, "m_boneIDs")?;
    let mut required = BTreeSet::new();
    for (name, &sample) in &sampled_names {
        let Some(&model_index) = current_names.get(name) else {
            ensure!(
                !old_names.contains_key(name),
                "Authored deformation bone has no current model counterpart: {name}"
            );
            // A sampling-only branch may be absent from both render models.
            // It cannot lead to a deformation bone, and needs a shared ancestor.
            let mut ancestor = sampled_parents[sample];
            let mut anchored = false;
            while let Some(at) = ancestor {
                let parent_name = sampled_list[at]
                    .as_str()
                    .context("Invalid sampled parent")?;
                if current_names.contains_key(&parent_name.to_ascii_lowercase()) {
                    anchored = true;
                    break;
                }
                ancestor = sampled_parents[at];
            }
            ensure!(anchored, "Unanchored sampling branch: {name}");
            for (child_name, &child) in &sampled_names {
                let mut ancestor = sampled_parents[child];
                while let Some(at) = ancestor {
                    if at == sample {
                        ensure!(
                            !current_names.contains_key(child_name)
                                && !old_names.contains_key(child_name),
                            "Missing sampled ancestor affects {child_name}"
                        );
                        break;
                    }
                    ancestor = sampled_parents[at];
                }
            }
            continue;
        };
        let mut index = Some(model_index);
        while let Some(at) = index {
            required.insert(at);
            index = current_parents[at];
        }
    }
    for other in required {
        let name = current_list[other]
            .as_str()
            .context("Invalid bone name")?
            .to_ascii_lowercase();
        let &index = old_names
            .get(&name)
            .with_context(|| format!("Required animation bone missing: {name}"))?;
        ensure!(
            named_parent(old_list, &old_parents, index)
                == named_parent(current_list, &current_parents, other),
            "Changed animation parent: {name}"
        );
        let (ap, ar, az) = model_pose(a, index)?;
        let (bp, br, bz) = model_pose(b, other)?;
        ensure!(
            close(&ap, &bp) && rotation(&ar, &br) && (az - bz).abs() <= 0.0001,
            "Changed authored bind pose: {name}"
        );
    }
    Ok(())
}

fn absent(value: Option<&Value>) -> bool {
    value.is_none_or(|v| matches!(v, Value::Null) || v.as_array().is_some_and(<[Value]>::is_empty))
}

fn references(value: &Value, paths: &mut Vec<String>) {
    match value {
        Value::String(path) if path.ends_with(".vnmgraph") => paths.push(format!("{path}_c")),
        Value::Array(items) => {
            for item in items {
                references(item, paths);
            }
        }
        Value::Object(pairs) => {
            for (_, value) in pairs {
                references(value, paths);
            }
        }
        _ => {}
    }
}

pub(super) fn repair(
    entry: &str,
    source: &[u8],
    base: &[u8],
    mut old: Document,
    current: &Document,
    animations: bool,
    game: &mut CompatibilityRepairer,
) -> Result<Option<(Vec<u8>, usize)>> {
    if entry.ends_with(".vnmskel_c") {
        let layout_fields = [
            "m_boneIDs",
            "m_parentIndices",
            "m_parentSpaceReferencePose",
            "m_modelSpaceReferencePose",
            "m_numBonesToSampleAtLowLOD",
        ];
        if layout_fields
            .iter()
            .all(|key| old.value.get(key) == current.value.get(key))
        {
            return Ok(None);
        }
        ensure!(
            !animations,
            "Animation overrides require a coordinated clip migration"
        );
        ensure!(
            old.flags == current.flags,
            "Changed skeleton reference flags require another adapter"
        );
        if compatible_skeleton(&old.value, &current.value)? {
            return Ok(Some((base.to_vec(), 1)));
        }
        return Ok(None);
    }
    if FIELDS.iter().any(|key| !absent(old.value.get(key))) {
        return Ok(None);
    }
    if FIELDS.iter().any(|key| absent(current.value.get(key))) {
        return Ok(None);
    }
    ensure!(
        !animations,
        "Animation overrides prevent restoring vanilla bindings"
    );
    let skeletons = array(&current.value, FIELDS[1])?;
    ensure!(
        skeletons.len() == 1,
        "Multiple sampling skeletons require another adapter"
    );
    let path = skeletons[0]
        .as_str()
        .context("Invalid skeleton reference")?;
    let bytes = game
        .game_bytes(&format!("{path}_c"))?
        .context("Sampling skeleton is missing from the game")?;
    let sampled = Document::read(&bytes)?;
    prove_model(&old.value, &current.value, &sampled.value)?;
    let mut graphs = Vec::new();
    references(
        current
            .value
            .get(FIELDS[0])
            .context("Missing graph bindings")?,
        &mut graphs,
    );
    ensure!(!graphs.is_empty(), "No current animation graphs");
    for graph in graphs {
        ensure!(
            game.game_bytes(&graph)?.is_some(),
            "Current animation graph is missing: {graph}"
        );
    }
    let Value::Object(pairs) = &mut old.value else {
        anyhow::bail!("Invalid model DATA")
    };
    for key in FIELDS {
        let value = current
            .value
            .get(key)
            .context("Missing animation interface")?
            .clone();
        if let Some((_, target)) = pairs.iter_mut().find(|(k, _)| k == key) {
            *target = value;
        } else {
            pairs.push((key.into(), value));
        }
        let path = [Seg::Key(key.into())];
        copy_flags(&current.flags, &path, &mut old.flags, &path);
    }
    // Only the two binding fields change. The whole deformation rig, materials,
    // embedded meshes, attachments and their inverse binds remain authored.
    Ok(Some((old.write(source)?, 2)))
}
