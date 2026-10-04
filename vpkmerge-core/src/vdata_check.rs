//! Detect mods that ship an outdated copy of a game `.vdata_c` file.
//!
//! A mod VPK that carries e.g. `scripts/heroes.vdata_c` overrides the **whole**
//! file in game. If Valve added heroes, abilities or fields after the mod was
//! built, the mod silently deletes them. [`check_vdata`] decodes each `.vdata_c`
//! the mod ships and the game's copy as KV3 and compares them structurally:
//! keys the game has and the mod lacks mean the file is stale, while pure value
//! differences are indistinguishable from the mod's intended edits.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    /// Either copy failed to read or decode as KV3.
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
    /// ability surfaces before its individual fields. Array elements are
    /// `path[i]`, or `path[=value]` for arrays of plain values, matched by value
    /// because Valve inserts into them mid-array.
    pub missing: Vec<String>,
    /// Key paths only the mod's copy has (fields the game has since dropped),
    /// in the same notation as `missing`.
    pub extra: Vec<String>,
    /// Leaf paths present in both whose values differ. Mixes the mod's intended
    /// edits with any later Valve value changes.
    pub changed: Vec<String>,
    /// Error text for [`VdataStatus::Undecodable`], prefixed `mod: ` or `game: `.
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

    fn undecodable(entry: &str, error: String) -> Self {
        Self {
            error: Some(error),
            ..Self::new(entry, VdataStatus::Undecodable)
        }
    }
}

/// Compare every `.vdata_c` in `mod_vpk` (sorted by path) against `base_vpk`,
/// normally the game's `citadel/pak01_dir.vpk`. Returns an empty list when the
/// mod ships none. Use [`VdataChecker`] to check several mods against one pak.
///
/// # Errors
/// Fails if either VPK cannot be opened. Per-entry problems are reported in the
/// returned [`VdataReport`]s instead.
pub fn check_vdata<M: AsRef<Path>, B: AsRef<Path>>(
    mod_vpk: M,
    base_vpk: B,
) -> Result<Vec<VdataReport>> {
    VdataChecker::new(base_vpk).check(mod_vpk)
}

/// Checks mods against one game pak. The pak is opened the first time a mod
/// ships a `.vdata_c`, and each game file is read and decoded at most once.
pub struct VdataChecker {
    base_path: PathBuf,
    base: Option<valve_pak::VPK>,
    files: HashMap<String, Option<BaseFile>>,
}

struct BaseFile {
    bytes: Vec<u8>,
    tree: Option<Result<Value, String>>,
}

impl VdataChecker {
    pub fn new<B: AsRef<Path>>(base_vpk: B) -> Self {
        Self {
            base_path: base_vpk.as_ref().to_path_buf(),
            base: None,
            files: HashMap::new(),
        }
    }

    /// Compare every `.vdata_c` in `mod_vpk` (sorted by path) against the game
    /// pak. A mod that ships none returns an empty list without touching the pak.
    ///
    /// # Errors
    /// Fails if either VPK cannot be opened. Per-entry problems are reported in
    /// the returned [`VdataReport`]s instead.
    pub fn check<M: AsRef<Path>>(&mut self, mod_vpk: M) -> Result<Vec<VdataReport>> {
        let mod_path = mod_vpk.as_ref();
        let mod_pak = valve_pak::open(mod_path)
            .with_context(|| format!("opening mod {}", mod_path.display()))?;
        let mut entries: Vec<String> = mod_pak
            .file_paths()
            .filter(|p| p.ends_with(".vdata_c"))
            .cloned()
            .collect();
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        entries.sort();

        let base = match &mut self.base {
            Some(base) => base,
            slot @ None => slot.insert(
                valve_pak::open(&self.base_path)
                    .with_context(|| format!("opening base {}", self.base_path.display()))?,
            ),
        };
        let mut reports = Vec::with_capacity(entries.len());
        for entry in entries {
            let mod_bytes = match mod_pak.get_file(&entry).and_then(|mut f| f.read_all()) {
                Ok(bytes) => bytes,
                Err(e) => {
                    reports.push(VdataReport::undecodable(&entry, format!("mod: {e:#}")));
                    continue;
                }
            };
            reports.push(match base_file(base, &mut self.files, &entry) {
                Ok(Some(base_file)) => compare(&entry, &mod_bytes, base_file),
                Ok(None) => VdataReport::new(&entry, VdataStatus::NotInGame),
                Err(e) => VdataReport::undecodable(&entry, format!("game: {e:#}")),
            });
        }
        Ok(reports)
    }
}

/// The game's copy of `entry`, read on first use. `None` when the game has none.
fn base_file<'a>(
    base: &valve_pak::VPK,
    files: &'a mut HashMap<String, Option<BaseFile>>,
    entry: &str,
) -> Result<Option<&'a mut BaseFile>> {
    if !files.contains_key(entry) {
        let file = match base.get_file(entry) {
            Ok(mut f) => Some(BaseFile {
                bytes: f.read_all()?,
                tree: None,
            }),
            Err(_) => None,
        };
        files.insert(entry.to_string(), file);
    }
    Ok(files.get_mut(entry).and_then(Option::as_mut))
}

fn compare(entry: &str, mod_bytes: &[u8], base: &mut BaseFile) -> VdataReport {
    if mod_bytes == base.bytes {
        return VdataReport::new(entry, VdataStatus::Current);
    }
    let mod_tree = match morphic::decode_kv3_resource(mod_bytes) {
        Ok(v) => v,
        Err(e) => return VdataReport::undecodable(entry, format!("mod: {e}")),
    };
    let base_tree = base.tree.get_or_insert_with(|| {
        morphic::decode_kv3_resource(&base.bytes).map_err(|e| e.to_string())
    });
    match base_tree {
        Ok(tree) => report_from_trees(entry, tree, &mod_tree),
        Err(e) => VdataReport::undecodable(entry, format!("game: {e}")),
    }
}

fn report_from_trees(entry: &str, base: &Value, modded: &Value) -> VdataReport {
    let mut report = VdataReport::new(entry, VdataStatus::Current);
    diff(base, modded, "", &mut report);
    report.missing.sort_by_key(|p| depth(p));
    report.extra.sort_by_key(|p| depth(p));
    if !report.missing.is_empty() || !report.extra.is_empty() {
        report.status = VdataStatus::Outdated;
    } else if !report.changed.is_empty() {
        report.status = VdataStatus::Modified;
    }
    report
}

fn depth(path: &str) -> usize {
    path.matches('/').count()
}

fn child(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}/{key}")
    }
}

fn diff(base: &Value, modded: &Value, path: &str, report: &mut VdataReport) {
    match (base, modded) {
        (Value::Object(base_pairs), Value::Object(mod_pairs)) => {
            for (key, base_val) in base_pairs {
                match modded.get(key) {
                    Some(mod_val) => diff(base_val, mod_val, &child(path, key), report),
                    None => report.missing.push(child(path, key)),
                }
            }
            for (key, _) in mod_pairs {
                if base.get(key).is_none() {
                    report.extra.push(child(path, key));
                }
            }
        }
        (Value::Array(base_items), Value::Array(mod_items)) => {
            if base_items.len() != mod_items.len() {
                if let (Some(base_texts), Some(mod_texts)) =
                    (scalar_texts(base_items), scalar_texts(mod_items))
                {
                    diff_by_value(base_texts, mod_texts, path, report);
                    return;
                }
            }
            for (i, (b, m)) in base_items.iter().zip(mod_items).enumerate() {
                diff(b, m, &format!("{path}[{i}]"), report);
            }
            for i in mod_items.len()..base_items.len() {
                report.missing.push(format!("{path}[{i}]"));
            }
            for i in base_items.len()..mod_items.len() {
                report.extra.push(format!("{path}[{i}]"));
            }
        }
        _ => {
            if base != modded {
                report.changed.push(path.to_string());
            }
        }
    }
}

/// Multiset difference of two arrays of plain values: game values the mod
/// lacks are missing, mod values the game lacks are extra.
fn diff_by_value(base: Vec<String>, mut modded: Vec<String>, path: &str, report: &mut VdataReport) {
    for item in base {
        match modded.iter().position(|m| *m == item) {
            Some(i) => {
                modded.remove(i);
            }
            None => report.missing.push(format!("{path}[={item}]")),
        }
    }
    report
        .extra
        .extend(modded.into_iter().map(|item| format!("{path}[={item}]")));
}

/// Display text of every item, or `None` if any item is not a plain value.
fn scalar_texts(items: &[Value]) -> Option<Vec<String>> {
    items
        .iter()
        .map(|item| match item {
            Value::Bool(b) => Some(b.to_string()),
            Value::Int(n) => Some(n.to_string()),
            Value::UInt(n) => Some(n.to_string()),
            Value::Double(n) => Some(n.to_string()),
            Value::String(s) => Some(s.clone()),
            Value::Null | Value::Binary(_) | Value::Array(_) | Value::Object(_) => None,
        })
        .collect()
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

    fn strs(items: &[&str]) -> Value {
        Value::Array(items.iter().map(|s| Value::String((*s).into())).collect())
    }

    fn run(base: &Value, modded: &Value) -> VdataReport {
        report_from_trees("x.vdata_c", base, modded)
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
    fn equal_length_value_array_change_is_modified() {
        let base = obj(&[("v", Value::Array(vec![Value::Int(1), Value::Int(2)]))]);
        let modded = obj(&[("v", Value::Array(vec![Value::Int(1), Value::Int(3)]))]);
        let r = run(&base, &modded);
        assert_eq!(r.changed, ["v[1]"]);
        assert_eq!(r.status, VdataStatus::Modified);
    }

    // Shape of the 2026 m_vecDisplayStats change: Valve inserted a stat mid-array.
    #[test]
    fn value_inserted_mid_array_is_missing() {
        let base = obj(&[("v", strs(&["EMaxHealth", "ELifesteal", "EMeleeResist"]))]);
        let modded = obj(&[("v", strs(&["EMaxHealth", "EMeleeResist"]))]);
        let r = run(&base, &modded);
        assert_eq!(r.missing, ["v[=ELifesteal]"]);
        assert!(r.extra.is_empty() && r.changed.is_empty());
        assert_eq!(r.status, VdataStatus::Outdated);
    }

    #[test]
    fn value_dropped_from_game_array_is_extra() {
        let base = obj(&[("v", strs(&["a"]))]);
        let modded = obj(&[("v", strs(&["a", "a"]))]);
        let r = run(&base, &modded);
        assert_eq!(r.extra, ["v[=a]"]);
        assert!(r.missing.is_empty());
        assert_eq!(r.status, VdataStatus::Outdated);
    }

    #[test]
    fn object_array_grown_reports_tail_by_index() {
        let item = |n| obj(&[("bar", Value::Int(n))]);
        let base = obj(&[("v", Value::Array(vec![item(0), item(1), item(2)]))]);
        let modded = obj(&[("v", Value::Array(vec![item(0), item(5)]))]);
        let r = run(&base, &modded);
        assert_eq!(r.missing, ["v[2]"]);
        assert_eq!(r.changed, ["v[1]/bar"]);
        assert_eq!(r.status, VdataStatus::Outdated);
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
        assert_eq!(run(&base, &modded).missing, ["top", "a/deep"]);
    }
}
