//! Detect mods that ship an outdated copy of a game `.vdata_c` file.
//!
//! A mod VPK that carries e.g. `scripts/heroes.vdata_c` overrides the **whole**
//! file in game. If Valve added heroes, abilities or fields after the mod was
//! built, the mod silently deletes them. [`check_vdata`] decodes each `.vdata_c`
//! the mod ships and the game's copy as KV3 and compares them structurally:
//! keys the game has and the mod lacks mean the file is stale, while pure value
//! differences are indistinguishable from the mod's intended edits.

use std::path::Path;

use anyhow::{Context, Result};
use morphic::kv3::Value;
use serde::Serialize;

/// How a mod's `.vdata_c` compares with the game's copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum VdataStatus {
    /// Decodes equal to the game's copy (or byte-identical): redundant but harmless.
    Current,
    /// Same structure as the game's copy, only values differ: the mod's intended
    /// edits, possibly mixed with values Valve rebalanced since (indistinguishable
    /// without the build the mod was made against).
    Modified,
    /// The structure has drifted: the game has fields the mod's copy lacks, or the
    /// mod's copy has fields the game dropped.
    Outdated,
    /// The game ships no file at this path (compiler leftovers like
    /// `panorama/image_compiler.vdata_c`, or a custom file). Nothing to compare.
    NotInGame,
    /// Either copy failed to decode as KV3.
    Undecodable,
}

/// Comparison of one `.vdata_c` entry in a mod against the game's copy.
#[derive(Debug, Clone, Serialize)]
pub struct VdataReport {
    /// VPK entry path, e.g. `scripts/heroes.vdata_c`.
    pub entry: String,
    pub status: VdataStatus,
    /// Key paths the game's copy has and the mod's lacks. These are exactly what
    /// the mod removes in game. Shallowest first, so a whole missing hero or
    /// ability surfaces before its individual fields.
    pub missing: Vec<String>,
    /// Key paths only the mod's copy has (fields the game has since dropped).
    pub extra: Vec<String>,
    /// Leaf paths present in both whose values differ. Mixes the mod's intended
    /// edits with any later Valve value changes.
    pub changed: Vec<String>,
    /// Decode error text for [`VdataStatus::Undecodable`], prefixed `mod: ` or `game: `.
    pub error: Option<String>,
}

impl VdataReport {
    fn new(entry: &str, status: VdataStatus) -> Self {
        Self {
            entry: entry.to_string(),
            status,
            missing: Vec::new(),
            extra: Vec::new(),
            changed: Vec::new(),
            error: None,
        }
    }
}

/// Compare every `.vdata_c` in `mod_vpk` (sorted by path) against `base_vpk`,
/// normally the game's `citadel/pak01_dir.vpk`. Returns an empty list when the
/// mod ships none.
///
/// # Errors
/// Fails if either VPK cannot be opened. Per-entry problems are reported in the
/// returned [`VdataReport`]s instead.
pub fn check_vdata<M: AsRef<Path>, B: AsRef<Path>>(
    mod_vpk: M,
    base_vpk: B,
) -> Result<Vec<VdataReport>> {
    let mod_path = mod_vpk.as_ref();
    let base_path = base_vpk.as_ref();
    let mod_pak =
        valve_pak::open(mod_path).with_context(|| format!("opening mod {}", mod_path.display()))?;
    let base_pak = valve_pak::open(base_path)
        .with_context(|| format!("opening base {}", base_path.display()))?;

    let mut entries: Vec<String> = mod_pak
        .file_paths()
        .filter(|p| p.ends_with(".vdata_c"))
        .cloned()
        .collect();
    entries.sort();

    let mut reports = Vec::with_capacity(entries.len());
    for entry in entries {
        let mod_bytes = mod_pak
            .get_file(&entry)
            .and_then(|mut f| f.read_all())
            .with_context(|| format!("reading {entry:?} from {}", mod_path.display()))?;
        let Ok(mut base_file) = base_pak.get_file(&entry) else {
            reports.push(VdataReport::new(&entry, VdataStatus::NotInGame));
            continue;
        };
        let base_bytes = base_file
            .read_all()
            .with_context(|| format!("reading {entry:?} from {}", base_path.display()))?;
        reports.push(compare(&entry, &mod_bytes, &base_bytes));
    }
    Ok(reports)
}

fn compare(entry: &str, mod_bytes: &[u8], base_bytes: &[u8]) -> VdataReport {
    if mod_bytes == base_bytes {
        return VdataReport::new(entry, VdataStatus::Current);
    }
    let mut report = VdataReport::new(entry, VdataStatus::Undecodable);
    let mod_tree = match morphic::decode_kv3_resource(mod_bytes) {
        Ok(v) => v,
        Err(e) => {
            report.error = Some(format!("mod: {e}"));
            return report;
        }
    };
    let base_tree = match morphic::decode_kv3_resource(base_bytes) {
        Ok(v) => v,
        Err(e) => {
            report.error = Some(format!("game: {e}"));
            return report;
        }
    };
    diff(
        &base_tree,
        &mod_tree,
        "",
        &mut report.missing,
        &mut report.extra,
        &mut report.changed,
    );
    report.missing.sort_by_key(|p| depth(p));
    report.extra.sort_by_key(|p| depth(p));
    report.status = status_of(&report);
    report
}

fn depth(path: &str) -> usize {
    path.matches('/').count()
}

fn status_of(report: &VdataReport) -> VdataStatus {
    if !report.missing.is_empty() || !report.extra.is_empty() {
        VdataStatus::Outdated
    } else if !report.changed.is_empty() {
        VdataStatus::Modified
    } else {
        VdataStatus::Current
    }
}

fn diff(
    base: &Value,
    modded: &Value,
    path: &str,
    missing: &mut Vec<String>,
    extra: &mut Vec<String>,
    changed: &mut Vec<String>,
) {
    match (base, modded) {
        (Value::Object(base_pairs), Value::Object(mod_pairs)) => {
            for (key, base_val) in base_pairs {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}/{key}")
                };
                match modded.get(key) {
                    Some(mod_val) => diff(base_val, mod_val, &child, missing, extra, changed),
                    None => missing.push(child),
                }
            }
            for (key, _) in mod_pairs {
                if base.get(key).is_none() {
                    extra.push(if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}/{key}")
                    });
                }
            }
        }
        (Value::Array(base_items), Value::Array(mod_items))
            if base_items.len() == mod_items.len() =>
        {
            for (i, (b, m)) in base_items.iter().zip(mod_items).enumerate() {
                diff(b, m, &format!("{path}[{i}]"), missing, extra, changed);
            }
        }
        _ => {
            if base != modded {
                changed.push(path.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(pairs: &[(&str, Value)]) -> Value {
        Value::Object(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
        )
    }

    fn run(base: &Value, modded: &Value) -> VdataReport {
        let mut r = VdataReport::new("x.vdata_c", VdataStatus::Current);
        diff(
            base,
            modded,
            "",
            &mut r.missing,
            &mut r.extra,
            &mut r.changed,
        );
        r.status = status_of(&r);
        r
    }

    #[test]
    fn missing_object_reported_once() {
        let base = obj(&[
            ("a", Value::Int(1)),
            ("hero", obj(&[("x", Value::Int(1)), ("y", Value::Int(2))])),
        ]);
        let modded = obj(&[("a", Value::Int(1))]);
        let r = run(&base, &modded);
        assert_eq!(r.missing, ["hero"]);
        assert!(r.extra.is_empty() && r.changed.is_empty());
        assert_eq!(r.status, VdataStatus::Outdated);
    }

    #[test]
    fn extra_key() {
        let base = obj(&[("a", Value::Int(1))]);
        let modded = obj(&[("a", Value::Int(1)), ("b", Value::Int(2))]);
        let r = run(&base, &modded);
        assert_eq!(r.extra, ["b"]);
        assert_eq!(r.status, VdataStatus::Outdated);
    }

    #[test]
    fn changed_scalar_is_modified() {
        let base = obj(&[("s", obj(&[("hp", Value::Int(1))]))]);
        let modded = obj(&[("s", obj(&[("hp", Value::Int(2))]))]);
        let r = run(&base, &modded);
        assert_eq!(r.changed, ["s/hp"]);
        assert_eq!(r.status, VdataStatus::Modified);
    }

    #[test]
    fn equal_arrays_are_current() {
        let base = obj(&[("v", Value::Array(vec![Value::Int(1)]))]);
        assert_eq!(run(&base, &base.clone()).status, VdataStatus::Current);
    }

    #[test]
    fn equal_length_arrays_recurse() {
        let item = |n| obj(&[("bar", Value::Int(n))]);
        let base = obj(&[(
            "ab",
            obj(&[("m_vecFoo", Value::Array(vec![item(0), item(1), item(2)]))]),
        )]);
        let modded = obj(&[(
            "ab",
            obj(&[("m_vecFoo", Value::Array(vec![item(0), item(1), item(9)]))]),
        )]);
        assert_eq!(run(&base, &modded).changed, ["ab/m_vecFoo[2]/bar"]);
    }

    #[test]
    fn unequal_length_array_is_changed() {
        let base = obj(&[("v", Value::Array(vec![Value::Int(1)]))]);
        let modded = obj(&[("v", Value::Array(vec![Value::Int(1), Value::Int(2)]))]);
        let r = run(&base, &modded);
        assert_eq!(r.changed, ["v"]);
        assert_eq!(r.status, VdataStatus::Modified);
    }

    #[test]
    fn mismatched_kinds_are_changed() {
        let base = obj(&[("v", Value::Int(1))]);
        let modded = obj(&[("v", Value::String("1".into()))]);
        assert_eq!(run(&base, &modded).changed, ["v"]);
    }

    #[test]
    fn missing_sorted_shallowest_first() {
        let base = obj(&[
            ("a", obj(&[("deep", Value::Int(1))])),
            ("top", Value::Int(1)),
        ]);
        let modded = obj(&[("a", obj(&[]))]);
        let mut r = VdataReport::new("x", VdataStatus::Current);
        diff(
            &base,
            &modded,
            "",
            &mut r.missing,
            &mut r.extra,
            &mut r.changed,
        );
        assert_eq!(r.missing, ["a/deep", "top"]);
        r.missing.sort_by_key(|p| depth(p));
        assert_eq!(r.missing, ["top", "a/deep"]);
    }
}
