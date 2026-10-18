//! Bounded repairs derived from the installed game. Existing mod values are
//! retained; unsupported structural changes remain explicit findings.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use morphic::kv3::{self, Seg, Value, ValueFlags};
use morphic::resource::Resource;
use serde::Serialize;

type RepairPlan = (Vec<CompatibilityReport>, BTreeMap<String, Vec<u8>>);

mod localization;
mod skeleton;
mod sounds;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepairStatus {
    Repairable,
    Unsupported,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompatibilityReport {
    pub entry: String,
    pub kind: String,
    pub status: RepairStatus,
    pub changes: usize,
    pub detail: String,
}

/// The base is the installed `citadel/pak01_dir.vpk`. Loose localization files
/// beside it take precedence, matching the game's update layout.
pub struct CompatibilityRepairer {
    base: valve_pak::VPK,
    root: PathBuf,
    files: BTreeMap<String, Option<Vec<u8>>>,
}

pub(crate) struct Document {
    pub value: Value,
    pub flags: ValueFlags,
}

impl Document {
    pub fn read(bytes: &[u8]) -> Result<Self> {
        let resource = Resource::parse(bytes)?;
        ensure!(
            resource
                .blocks()
                .iter()
                .filter(|b| b.kind == *b"DATA")
                .count()
                == 1,
            "Expected exactly one DATA block"
        );
        let (value, flags) = kv3::decode_flagged(resource.data_block()?)?;
        validate(&value, 0)?;
        Ok(Self { value, flags })
    }

    pub fn write(&self, original: &[u8]) -> Result<Vec<u8>> {
        let resource = Resource::parse(original)?;
        // A resource may have unrepresented streamed data after its blocks.
        let end = resource
            .blocks()
            .iter()
            .map(|b| b.offset as usize + b.size as usize)
            .max()
            .unwrap_or(0);
        ensure!(
            end <= original.len()
                && original.len() - end < 16
                && original[end..].iter().all(|byte| *byte == 0),
            "Streamed resource tails require a separate adapter"
        );
        let format = kv3::Format::from_payload(resource.data_block()?)?;
        let data = kv3::encode_flagged(&self.value, &format, &self.flags);
        let (actual, flags) = kv3::decode_flagged(&data)?;
        ensure!(
            actual == self.value && flags == self.flags,
            "KV3 round-trip changed values or reference flags"
        );
        let output = resource.rebuild_with_data(&data)?;
        let rebuilt = Resource::parse(&output)?;
        for (i, block) in resource.blocks().iter().enumerate() {
            if block.kind != *b"DATA" {
                ensure!(
                    resource.get_block_by_index(i) == rebuilt.get_block_by_index(i),
                    "Non-DATA block changed"
                );
            }
        }
        Ok(output)
    }
}

fn validate(value: &Value, depth: usize) -> Result<()> {
    ensure!(depth <= 96, "KV3 nesting exceeds repair limit");
    match value {
        Value::Object(pairs) => {
            let mut keys = BTreeSet::new();
            for (key, child) in pairs {
                ensure!(
                    keys.insert(key.to_ascii_lowercase()),
                    "Ambiguous object key: {key}"
                );
                validate(child, depth + 1)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                validate(child, depth + 1)?;
            }
        }
        Value::Double(number) => ensure!(number.is_finite(), "Non-finite KV3 number"),
        _ => {}
    }
    Ok(())
}

pub(crate) fn copy_flags(source: &ValueFlags, from: &[Seg], target: &mut ValueFlags, to: &[Seg]) {
    target.retain(|path, _| !path.starts_with(to));
    for (path, flag) in source {
        if let Some(suffix) = path.strip_prefix(from) {
            let mut path = to.to_vec();
            path.extend_from_slice(suffix);
            target.insert(path, *flag);
        }
    }
}

/// Only absent object fields are filled. Arrays and existing scalar values are
/// authored units: guessing whether an omission was intentional would erase mods.
fn fill_missing(
    target: &mut Value,
    base: &Value,
    path: &mut Vec<Seg>,
    flags: &mut ValueFlags,
    base_flags: &ValueFlags,
) -> usize {
    let (Value::Object(target), Value::Object(base)) = (target, base) else {
        return 0;
    };
    let mut count = 0;
    for (key, value) in base {
        if key == "_editor" || key == "_include" {
            continue;
        }
        if let Some((authored_key, authored)) =
            target.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key))
        {
            // Case-only differences are preserved, including their flag paths.
            let mut base_path = path.clone();
            base_path.push(Seg::Key(key.clone()));
            path.push(Seg::Key(authored_key.clone()));
            let mut translated = ValueFlags::new();
            copy_flags(base_flags, &base_path, &mut translated, path);
            count += fill_missing(authored, value, path, flags, &translated);
            path.pop();
        } else {
            path.push(Seg::Key(key.clone()));
            copy_flags(base_flags, path, flags, path);
            path.pop();
            target.push((key.clone(), value.clone()));
            count += 1;
        }
    }
    count
}

impl CompatibilityRepairer {
    pub fn new(base: &Path) -> Result<Self> {
        Ok(Self {
            base: valve_pak::open(base)?,
            root: base
                .parent()
                .context("Base VPK has no parent")?
                .to_path_buf(),
            files: BTreeMap::new(),
        })
    }

    fn game_bytes(&mut self, entry: &str) -> Result<Option<Vec<u8>>> {
        ensure!(
            !entry.contains('\\') && !entry.split('/').any(|s| matches!(s, "" | "." | "..")),
            "Unsafe resource path"
        );
        if let Some(value) = self.files.get(entry) {
            return Ok(value.clone());
        }
        let loose = self.root.join(entry);
        let bytes = if entry.starts_with("resource/localization/") && loose.is_file() {
            Some(std::fs::read(loose)?)
        } else if self.base.file_paths().any(|p| p == entry) {
            Some(self.base.get_file(entry)?.read_all()?)
        } else {
            None
        };
        self.files.insert(entry.into(), bytes.clone());
        Ok(bytes)
    }

    fn plan(&mut self, input: &Path, animation_overrides: bool) -> Result<RepairPlan> {
        let pak = valve_pak::open(input)?;
        let mut paths: Vec<_> = pak.file_paths().cloned().collect();
        paths.sort();
        let own_animations = paths.iter().any(|p| skeleton::is_animation(p));
        let own_skeletons = paths.iter().any(|p| p.ends_with(".vnmskel_c"));
        let mut reports = Vec::new();
        let mut edits = BTreeMap::new();
        for entry in paths {
            let kind = if matches!(
                entry.as_str(),
                "scripts/abilities.vdata_c" | "scripts/heroes.vdata_c"
            ) {
                "data"
            } else if entry.starts_with("resource/localization/")
                && Path::new(&entry)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("txt"))
            {
                "localization"
            } else if entry.starts_with("models/")
                && (entry.ends_with(".vnmskel_c") || entry.ends_with(".vmdl_c"))
            {
                "skeleton"
            } else {
                continue;
            };
            let Some(base) = self.game_bytes(&entry)? else {
                continue;
            };
            let mut source = pak.get_file(&entry)?.read_all()?;
            if source == base {
                continue;
            }
            let kinds: &[&str] = if entry.ends_with(".vmdl_c") {
                &["sounds", "skeleton"]
            } else {
                std::slice::from_ref(&kind)
            };
            for &kind in kinds {
                let result: Result<Option<(Vec<u8>, usize)>> = (|| {
                    if kind == "localization" {
                        return localization::repair(&source, &base);
                    }
                    if kind == "sounds" {
                        return sounds::repair(&source, &base, self);
                    }
                    let mut old = Document::read(&source)?;
                    let current = Document::read(&base)?;
                    if kind == "data" {
                        let count = fill_missing(
                            &mut old.value,
                            &current.value,
                            &mut Vec::new(),
                            &mut old.flags,
                            &current.flags,
                        );
                        return if count == 0 {
                            Ok(None)
                        } else {
                            Ok(Some((old.write(&source)?, count)))
                        };
                    }
                    skeleton::repair(
                        &entry,
                        &source,
                        &base,
                        old,
                        &current,
                        own_animations
                            || animation_overrides
                            || (entry.ends_with(".vmdl_c") && own_skeletons),
                        self,
                    )
                })();
                match result {
                    Ok(Some((bytes, changes))) => {
                        source = bytes;
                        edits.insert(entry.clone(), source.clone());
                        reports.push(CompatibilityReport {
                            entry: entry.clone(),
                            kind: kind.into(),
                            status: RepairStatus::Repairable,
                            changes,
                            detail: String::new(),
                        });
                    }
                    Ok(None) => {}
                    Err(error) => reports.push(CompatibilityReport {
                        entry: entry.clone(),
                        kind: kind.into(),
                        status: RepairStatus::Unsupported,
                        changes: 0,
                        detail: format!("{error:#}"),
                    }),
                }
            }
        }
        Ok((reports, edits))
    }

    pub fn check(
        &mut self,
        input: &Path,
        animation_overrides: bool,
    ) -> Result<Vec<CompatibilityReport>> {
        self.plan(input, animation_overrides)
            .map(|(reports, _)| reports)
    }

    /// Write a new archive and verify every entry against either the intended
    /// replacement or the input. Never writes to the input or installed game.
    pub fn heal(
        &mut self,
        input: &Path,
        output: &Path,
        animation_overrides: bool,
    ) -> Result<Vec<CompatibilityReport>> {
        ensure!(
            input != output && !output.exists(),
            "Repair output must be a new file"
        );
        let (reports, edits) = self.plan(input, animation_overrides)?;
        if edits.is_empty() {
            return Ok(reports);
        }
        let extras: Vec<_> = edits
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        crate::embed_metadata(input, None, &extras, &[], output)?;
        let result = verify_archive(input, output, &edits);
        if let Err(error) = result {
            let _ = std::fs::remove_file(output);
            bail!("Repair archive verification failed: {error:#}");
        }
        Ok(reports)
    }
}

fn verify_archive(input: &Path, output: &Path, edits: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    let original = valve_pak::open(input)?;
    let actual = valve_pak::open(output)?;
    let before: BTreeSet<_> = original.file_paths().collect();
    let after: BTreeSet<_> = actual.file_paths().collect();
    ensure!(before == after, "Archive entry set changed");
    for path in before {
        let expected = if let Some(bytes) = edits.get(path) {
            bytes.clone()
        } else {
            original.get_file(path)?.read_all()?
        };
        ensure!(
            actual.get_file(path)?.read_all()? == expected,
            "Entry changed unexpectedly: {path}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
