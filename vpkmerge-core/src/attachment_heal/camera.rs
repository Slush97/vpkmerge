//! Restore missing camera inputs and reframe legacy poses absent from the game.
use anyhow::{ensure, Context, Result};
use morphic::kv3::{Format, Seg, Value, ValueFlags};
use morphic::resource::Resource;

use super::{Attachment, ROOT_BONE};
use crate::model_metadata;
use std::collections::HashMap;

pub(super) const SETTINGS: &str = "CitadelCameraSettings_t";
const REQUIRED: [&str; 5] = [
    "standing_pivot",
    "crouching_pivot",
    "near_00",
    "far_00",
    "gunaim_00",
];

pub(super) struct Template {
    required: Vec<(usize, Value)>,
    flags: ValueFlags,
    settings: Option<String>,
}

impl Template {
    pub(super) fn read(model: &[u8]) -> Result<Self> {
        let resource = Resource::parse(model)?;
        let mut required = Vec::new();
        let mut flags = ValueFlags::new();
        for raw in super::mdat_blocks(&resource) {
            let (tree, source_flags) = morphic::kv3::decode_flagged(raw?)?;
            let Some(atts) = tree.get("m_attachments").and_then(Value::as_array) else {
                continue;
            };
            for name in REQUIRED {
                if let Some((index, att)) = atts.iter().enumerate().find(|(_, a)| {
                    a.get("key").and_then(Value::as_str) == Some(name)
                        && a.get("value")
                            .and_then(|v| v.get("m_influenceNames"))
                            .and_then(Value::as_array)
                            .and_then(|v| v.first())
                            .and_then(Value::as_str)
                            == Some(ROOT_BONE)
                }) {
                    required.push((index, att.clone()));
                }
            }
            flags = source_flags;
            break;
        }
        let settings = if required.is_empty() {
            None
        } else if let Some(raw) = resource.find_block(*b"DATA") {
            let tree = morphic::kv3::decode(raw)?;
            metadata(&tree)
                .map(|text| model_metadata::block(text, SETTINGS))
                .transpose()?
                .flatten()
                .map(str::to_owned)
        } else {
            None
        };
        Ok(Self {
            required,
            flags,
            settings,
        })
    }

    pub(super) fn issues(&self, model: &[u8]) -> Result<Vec<String>> {
        let resource = Resource::parse(model)?;
        let mut names = Vec::new();
        for raw in super::mdat_blocks(&resource) {
            let tree = morphic::kv3::decode(raw?)?;
            for (_, missing) in self.missing(&tree)? {
                let name = attachment_name(missing)?.to_owned();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        if self.settings_update(&resource)?.is_some() {
            names.push(SETTINGS.into());
        }
        Ok(names)
    }

    fn missing<'a>(&'a self, tree: &Value) -> Result<Vec<(usize, &'a Value)>> {
        let Some(atts) = tree.get("m_attachments").and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        let mut missing = Vec::new();
        for (index, att) in &self.required {
            let name = attachment_name(att)?;
            if !atts
                .iter()
                .any(|a| a.get("key").and_then(Value::as_str) == Some(name))
            {
                missing.push((*index, att));
            }
        }
        if !missing.is_empty() {
            ensure!(
                tree.get("m_skeleton")
                    .and_then(|s| s.get("m_bones"))
                    .and_then(Value::as_array)
                    .is_some_and(|bones| bones
                        .iter()
                        .any(|b| b.get("m_boneName").and_then(Value::as_str) == Some(ROOT_BONE))),
                "Missing root_motion bone for camera attachments"
            );
        }
        Ok(missing)
    }

    pub(super) fn restore_block(&self, raw: &[u8]) -> Result<Option<(Vec<u8>, Vec<String>)>> {
        let (mut tree, mut flags) = morphic::kv3::decode_flagged(raw)?;
        let missing: Vec<(usize, Value)> = self
            .missing(&tree)?
            .into_iter()
            .map(|(i, v)| (i, v.clone()))
            .collect();
        if missing.is_empty() {
            return Ok(None);
        }
        let Some(Value::Array(atts)) = tree.get_mut("m_attachments") else {
            unreachable!()
        };
        let mut names = Vec::new();
        for (source_index, att) in missing {
            let source = [Seg::Key("m_attachments".into()), Seg::Index(source_index)];
            let target = [Seg::Key("m_attachments".into()), Seg::Index(atts.len())];
            for (path, flag) in &self.flags {
                if let Some(tail) = path.strip_prefix(source.as_slice()) {
                    let mut path = target.to_vec();
                    path.extend_from_slice(tail);
                    flags.insert(path, *flag);
                }
            }
            names.push(attachment_name(&att)?.to_owned());
            atts.push(att);
        }
        let encoded = morphic::kv3::encode_flagged(&tree, &Format::from_payload(raw)?, &flags);
        let (decoded, decoded_flags) = morphic::kv3::decode_flagged(&encoded)?;
        ensure!(
            decoded == tree && decoded_flags == flags,
            "Camera attachments did not round trip"
        );
        Ok(Some((encoded, names)))
    }

    fn settings_update(&self, resource: &Resource<'_>) -> Result<Option<String>> {
        let Some(settings) = &self.settings else {
            return Ok(None);
        };
        let tree = morphic::kv3::decode(resource.data_block()?)?;
        let text = metadata(&tree).context("Missing camera model metadata")?;
        model_metadata::insert_missing(text, settings, SETTINGS)
    }

    pub(super) fn restore_settings(&self, model: &[u8]) -> Result<Option<Vec<u8>>> {
        let resource = Resource::parse(model)?;
        let Some(updated) = self.settings_update(&resource)? else {
            return Ok(None);
        };
        let raw = resource.data_block()?;
        let (mut tree, flags) = morphic::kv3::decode_flagged(raw)?;
        *tree
            .get_mut("m_modelInfo")
            .and_then(|v| v.get_mut("m_keyValueText"))
            .context("Missing camera model metadata")? = Value::String(updated);
        let encoded = morphic::kv3::encode_flagged(&tree, &Format::from_payload(raw)?, &flags);
        let (decoded, decoded_flags) = morphic::kv3::decode_flagged(&encoded)?;
        ensure!(
            decoded == tree && decoded_flags == flags,
            "Camera settings did not round trip"
        );
        Ok(Some(resource.rebuild_with_data(&encoded)?))
    }
}

fn metadata(tree: &Value) -> Option<&str> {
    tree.get("m_modelInfo")?.get("m_keyValueText")?.as_str()
}

fn attachment_name(att: &Value) -> Result<&str> {
    att.get("key")
        .and_then(Value::as_str)
        .context("Missing camera attachment name")
}

fn pose_name(name: &str) -> bool {
    ["near_", "far_", "gunaim_"].iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|suffix| suffix.len() == 2 && suffix.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn unit(quaternion: &[f64]) -> Option<[f64; 4]> {
    let [x, y, z, w] = quaternion else {
        return None;
    };
    let norm = (x * x + y * y + z * z + w * w).sqrt();
    (norm.is_finite() && norm > 1e-9).then(|| [x / norm, y / norm, z / norm, w / norm])
}

fn right([x, y, z, w]: [f64; 4]) -> [f64; 3] {
    [
        2.0 * (x * y - w * z),
        1.0 - 2.0 * (x * x + z * z),
        2.0 * (y * z + w * x),
    ]
}

/// Old root coordinates became (z, x, y). Extra camera poses have no same-name
/// game donor. Rotate them only when that known conversion restores their right
/// axis to the game's pivot axis. Current poses and authored tweaks stay intact.
pub(super) fn reframe_extra(
    att: &Attachment,
    base: &HashMap<String, Attachment>,
) -> Option<Attachment> {
    if att.parent != ROOT_BONE || base.contains_key(&att.name) || !pose_name(&att.name) {
        return None;
    }
    let reference = right(unit(base.get("standing_pivot")?.rotations.first()?)?);
    let [x, y, z, w] = unit(att.rotations.first()?)?;
    let converted = [
        (w + x + z - y) * 0.5,
        (w + y + x - z) * 0.5,
        (w + z + y - x) * 0.5,
        (w - x - y - z) * 0.5,
    ];
    let dot = |a: [f64; 3]| a.iter().zip(reference).map(|(a, b)| a * b).sum::<f64>();
    if dot(right([x, y, z, w])) >= 0.5 || dot(right(converted)) < 30_f64.to_radians().cos() {
        return None;
    }
    let [ox, oy, oz] = att.offsets.first()?.as_slice() else {
        return None;
    };
    if ![ox, oy, oz].iter().all(|v| v.is_finite()) {
        return None;
    }
    let mut updated = att.clone();
    updated.offsets[0] = vec![*oz, *ox, *oy];
    updated.rotations[0] = converted.to_vec();
    Some(updated)
}
