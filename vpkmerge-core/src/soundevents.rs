//! Read, modify, and re-emit Deadlock soundevents (`.vsndevts_c`).
//!
//! A soundevents file is a Source 2 compiled resource whose `DATA` block is
//! binary KV3: a tree of named events, each typically carrying a `base`
//! (inherited template), a `vsnd_files` list (the `.vsnd_c` clips it can play),
//! and params like `volume` / `pitch`. The KV3 codec lives in `morphic`; this
//! module is the soundevents-aware layer the CLI and (eventually) Grimoire use:
//! load from a file or VPK, project to JSON, swap clip paths, tweak params, and
//! re-emit a loadable (uncompressed) file.
//!
//! Re-emitting keeps the original bytes around so the format GUID and the `RED2`
//! block survive unchanged; only the `DATA` block is rewritten.

use std::path::Path;

use anyhow::{Context, Result};
use morphic::kv3::Value;

/// Conventional keys inside a soundevent.
const KEY_VSND_FILES: &str = "vsnd_files";
const KEY_BASE: &str = "base";
const KEY_VOLUME: &str = "volume";

/// A decoded soundevents resource, plus the original file bytes needed to
/// re-encode without disturbing the `RED2` block or format GUID.
pub struct SoundEvents {
    original: Vec<u8>,
    /// The decoded root: an object mapping event name -> event object.
    pub root: Value,
}

/// One event's at-a-glance shape, for the human-readable summary.
#[derive(Debug, Clone)]
pub struct EventSummary {
    pub name: String,
    pub base: Option<String>,
    pub vsnd_count: usize,
    pub volume: Option<f64>,
}

impl SoundEvents {
    /// Decode a standalone `.vsndevts_c` file from disk.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_bytes(bytes)
    }

    /// Decode a `.vsndevts_c` entry out of a VPK (chunked VPKs are transparent;
    /// pass the `_dir.vpk`).
    pub fn from_vpk(vpk_path: impl AsRef<Path>, entry: &str) -> Result<Self> {
        let vpk_path = vpk_path.as_ref();
        let vpk =
            valve_pak::open(vpk_path).with_context(|| format!("opening {}", vpk_path.display()))?;
        let mut file = vpk
            .get_file(entry)
            .with_context(|| format!("locating {entry} in {}", vpk_path.display()))?;
        let bytes = file
            .read_all()
            .with_context(|| format!("reading {entry}"))?;
        Self::from_bytes(bytes)
    }

    /// Decode from raw resource bytes already in memory.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let root = morphic::decode_kv3_resource(&bytes).context("decoding KV3 DATA block")?;
        Ok(Self {
            original: bytes,
            root,
        })
    }

    /// Event names in file order.
    #[must_use]
    pub fn event_names(&self) -> Vec<&str> {
        match &self.root {
            Value::Object(pairs) => pairs.iter().map(|(k, _)| k.as_str()).collect(),
            _ => Vec::new(),
        }
    }

    /// One [`EventSummary`] per top-level event, in file order.
    #[must_use]
    pub fn summaries(&self) -> Vec<EventSummary> {
        let Value::Object(pairs) = &self.root else {
            return Vec::new();
        };
        pairs
            .iter()
            .map(|(name, event)| EventSummary {
                name: name.clone(),
                base: event
                    .get(KEY_BASE)
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                vsnd_count: event
                    .get(KEY_VSND_FILES)
                    .and_then(Value::as_array)
                    .map_or(0, <[Value]>::len),
                volume: event.get(KEY_VOLUME).and_then(Value::as_f64),
            })
            .collect()
    }

    /// Project the decoded tree to JSON (for stdout / Grimoire).
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        value_to_json(&self.root)
    }

    /// Replace the decoded tree from an edited JSON projection.
    ///
    /// The projection ([`to_json`](Self::to_json)) is lossy: KV3 distinguishes a
    /// signed `Int` from an unsigned `UInt`, but both render as a bare JSON
    /// number. A naive whole-tree rebuild would therefore re-type every unsigned
    /// field to signed, so editing one event's `volume` could silently flip
    /// unrelated `UInt` fields across the whole file. Instead this reconciles the
    /// edited JSON against the existing tree key-by-key, holding each original
    /// value's KV3 type wherever the edit still fits it; genuinely new keys or
    /// array elements fall back to JSON-inferred types.
    pub fn replace_from_json(&mut self, json: serde_json::Value) {
        self.root = reconcile_json(&self.root, json);
    }

    /// Replace every clip path equal to `from` with `to`, anywhere in the tree.
    /// Returns the number of strings rewritten.
    pub fn swap_vsnd(&mut self, from: &str, to: &str) -> usize {
        let mut count = 0;
        self.root.for_each_string_mut(&mut |s| {
            if s == from {
                to.clone_into(s);
                count += 1;
            }
        });
        count
    }

    /// Set an event's `vsnd_files` to `paths`, replacing whatever is there (a
    /// single bare string for one-clip events, or an existing array). With more
    /// than one path the engine picks one at random per play, which is the whole
    /// of a "sound randomizer": list the clips and the engine rolls the dice.
    /// Returns false if the named event does not exist.
    pub fn set_vsnd_files(&mut self, event: &str, paths: &[String]) -> bool {
        self.set_string_array_field(event, KEY_VSND_FILES, paths)
    }

    /// Set an event string-list field, e.g. `vsnd_files` or a layered music
    /// field such as `vsnd_files_draft`.
    pub fn set_string_array_field(&mut self, event: &str, field: &str, paths: &[String]) -> bool {
        let Some(event_val) = self.root.get_mut(event) else {
            return false;
        };
        let Value::Object(pairs) = event_val else {
            return false;
        };
        let arr = Value::Array(paths.iter().map(|p| Value::String(p.clone())).collect());
        if let Some((_, v)) = pairs.iter_mut().find(|(k, _)| k == field) {
            *v = arr;
        } else {
            pairs.push((field.to_owned(), arr));
        }
        true
    }

    /// Set a numeric-array field (e.g. `startpoint`, `endpoint`, `sync_bpm`) on
    /// one event. These loop-geometry fields are stored as KV3 arrays of one or
    /// more doubles (`endpoint = [43.355]`), so a scalar [`set_event_field`]
    /// would write the wrong node type and the engine would ignore it. Replaces
    /// the field if present, inserts it otherwise. Returns false if the named
    /// event does not exist.
    pub fn set_double_array_field(&mut self, event: &str, field: &str, values: &[f64]) -> bool {
        let Some(event_val) = self.root.get_mut(event) else {
            return false;
        };
        let Value::Object(pairs) = event_val else {
            return false;
        };
        let arr = Value::Array(values.iter().copied().map(Value::Double).collect());
        if let Some((_, v)) = pairs.iter_mut().find(|(k, _)| k == field) {
            *v = arr;
        } else {
            pairs.push((field.to_owned(), arr));
        }
        true
    }

    /// Set a numeric field (e.g. `volume`, `pitch`) on one event to a double.
    /// Replaces the field if present, inserts it otherwise. Returns false if the
    /// named event does not exist.
    pub fn set_event_field(&mut self, event: &str, field: &str, value: f64) -> bool {
        let Some(event_val) = self.root.get_mut(event) else {
            return false;
        };
        let Value::Object(pairs) = event_val else {
            return false;
        };
        if let Some((_, v)) = pairs.iter_mut().find(|(k, _)| k == field) {
            *v = Value::Double(value);
        } else {
            pairs.push((field.to_owned(), Value::Double(value)));
        }
        true
    }

    /// Re-encode to a complete resource file: uncompressed KV3 v4 `DATA`, the
    /// original format GUID, and the original `RED2` block, ready to pack into
    /// an addon VPK.
    pub fn encode(&self) -> Result<Vec<u8>> {
        morphic::encode_kv3_resource(&self.original, &self.root)
            .context("re-encoding soundevents resource")
    }

    /// Size in bytes of the original (LZ4-packed) file.
    #[must_use]
    pub fn original_len(&self) -> usize {
        self.original.len()
    }
}

pub(crate) fn value_to_json(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Int(i) => J::Number((*i).into()),
        Value::UInt(u) => J::Number((*u).into()),
        Value::Double(d) => serde_json::Number::from_f64(*d).map_or(J::Null, J::Number),
        Value::String(s) => J::String(s.clone()),
        // Soundevents never carry binary blobs; model them as a byte array so
        // the projection stays total.
        Value::Binary(bytes) => J::Array(bytes.iter().map(|b| J::Number((*b).into())).collect()),
        Value::Array(items) => J::Array(items.iter().map(value_to_json).collect()),
        Value::Object(pairs) => {
            let mut map = serde_json::Map::new();
            for (k, child) in pairs {
                map.insert(k.clone(), value_to_json(child));
            }
            J::Object(map)
        }
    }
}

pub(crate) fn value_from_json(v: serde_json::Value) -> Value {
    use serde_json::Value as J;
    match v {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(u) = n.as_u64() {
                Value::UInt(u)
            } else if let Some(f) = n.as_f64() {
                Value::Double(f)
            } else {
                Value::Null
            }
        }
        J::String(s) => Value::String(s),
        J::Array(items) => Value::Array(items.into_iter().map(value_from_json).collect()),
        J::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, value_from_json(value)))
                .collect(),
        ),
    }
}

/// Merge an edited JSON value onto the original KV3 value, preserving the
/// original's KV3 type where the edit is representable in it. This is what keeps
/// the lossy JSON projection from re-typing unsigned values to signed on a
/// soundevents round-trip (JSON has one number type; KV3 has `Int` and `UInt`).
/// New subtrees (keys/elements absent from the original) use [`value_from_json`].
pub(crate) fn reconcile_json(orig: &Value, json: serde_json::Value) -> Value {
    use serde_json::Value as J;
    match (orig, json) {
        (Value::Object(orig_pairs), J::Object(mut map)) => {
            let mut out = Vec::with_capacity(orig_pairs.len());
            // Retained keys keep the original KV3 order and type.
            for (key, orig_child) in orig_pairs {
                if let Some(child) = map.remove(key) {
                    out.push((key.clone(), reconcile_json(orig_child, child)));
                }
            }
            // Keys added in the edited JSON have no original to match; append them.
            for (key, child) in map {
                out.push((key, value_from_json(child)));
            }
            Value::Object(out)
        }
        (Value::Array(orig_items), J::Array(items)) => Value::Array(
            items
                .into_iter()
                .enumerate()
                .map(|(i, child)| match orig_items.get(i) {
                    Some(orig_child) => reconcile_json(orig_child, child),
                    None => value_from_json(child),
                })
                .collect(),
        ),
        // Numeric scalars: hold the original KV3 type when the edit fits it, so
        // an untouched (or still-in-range) `UInt`/`Int`/`Double` does not drift.
        (Value::UInt(_), J::Number(n)) if n.as_u64().is_some() => Value::UInt(n.as_u64().unwrap()),
        (Value::Int(_), J::Number(n)) if n.as_i64().is_some() => Value::Int(n.as_i64().unwrap()),
        (Value::Double(_), J::Number(n)) if n.as_f64().is_some() => {
            Value::Double(n.as_f64().unwrap())
        }
        // Type changed (or a non-numeric edit): take the edit as JSON infers it.
        (_, json) => value_from_json(json),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../morphic/fixtures/kv3/gigawatt.vsndevts_c"
    );

    #[test]
    fn loads_summarizes_edits_and_reencodes() {
        let mut se = SoundEvents::from_file(FIXTURE).expect("load fixture");

        let summaries = se.summaries();
        assert_eq!(summaries.len(), 44);
        let fire = summaries
            .iter()
            .find(|s| s.name == "Seven.Wpn.Fire")
            .expect("Seven.Wpn.Fire summary");
        assert_eq!(fire.base.as_deref(), Some("Base.Weapon.Pistol"));
        assert_eq!(fire.vsnd_count, 7);

        // Edits.
        let swapped = se.swap_vsnd(
            "sounds/weapons/gigawatt/gigawatt_weapon_fire_01.vsnd",
            "sounds/custom/my_fire.vsnd",
        );
        assert_eq!(swapped, 1);
        assert!(se.set_event_field("Seven.Wpn.Fire", "volume", 0.25));
        assert!(!se.set_event_field("No.Such.Event", "volume", 0.5));

        // Randomizer: set an event's clip list to several paths (the engine then
        // picks one per play). Works whether the event held one clip or many.
        let clips = vec!["sounds/a.vsnd".to_owned(), "sounds/b.vsnd".to_owned()];
        assert!(se.set_vsnd_files("Seven.Wpn.Fire", &clips));
        assert!(!se.set_vsnd_files("No.Such.Event", &clips));

        // Re-encode and reload; edits must survive the compiled round-trip.
        let bytes = se.encode().expect("encode");
        let back = SoundEvents::from_bytes(bytes).expect("reload");
        let fire = back.root.get("Seven.Wpn.Fire").unwrap();
        assert_eq!(fire.get("volume").and_then(Value::as_f64), Some(0.25));
        // The clip list survives as the two-element array we set.
        let files = fire.get("vsnd_files").and_then(Value::as_array).unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].as_str(), Some("sounds/a.vsnd"));
        assert_eq!(files[1].as_str(), Some("sounds/b.vsnd"));
    }

    #[test]
    fn edit_encode_pack_round_trips_from_vpk() {
        // The Grimoire path: edit a param, encode, pack into a standalone VPK at
        // an entry path, then decode straight back out of that VPK.
        const ENTRY: &str = "soundevents/hero/gigawatt.vsndevts_c";
        let tmp = tempfile::tempdir().expect("tempdir");
        let out = tmp.path().join("sndevts_chunk_dir.vpk");

        let mut se = SoundEvents::from_file(FIXTURE).expect("load fixture");
        assert!(se.set_event_field("Seven.Wpn.Fire", "volume", -9.0));
        let bytes = se.encode().expect("encode");
        crate::pack(&[(ENTRY, bytes.as_slice())], &out).expect("pack");

        let back = SoundEvents::from_vpk(&out, ENTRY).expect("decode from packed vpk");
        let fire = back.root.get("Seven.Wpn.Fire").expect("event present");
        assert_eq!(fire.get("volume").and_then(Value::as_f64), Some(-9.0));
    }

    #[test]
    fn json_projection_is_ordered_and_typed() {
        let se = SoundEvents::from_file(FIXTURE).expect("load fixture");
        let json = se.to_json();
        let fire = &json["Seven.Wpn.Fire"];
        // `base` is the first key (KV3 order preserved by serde_json preserve_order).
        let first_key = fire.as_object().unwrap().keys().next().unwrap();
        assert_eq!(first_key, "base");
        assert!(fire["vsnd_files"].is_array());
    }

    #[test]
    fn json_projection_import_reencodes() {
        let mut se = SoundEvents::from_file(FIXTURE).expect("load fixture");
        let mut json = se.to_json();
        json["Seven.Wpn.Fire"]["volume"] = serde_json::json!(-3.5);
        se.replace_from_json(json);
        let bytes = se.encode().expect("encode");
        let back = SoundEvents::from_bytes(bytes).expect("reload");
        let fire = back.root.get("Seven.Wpn.Fire").unwrap();
        assert_eq!(fire.get("volume").and_then(Value::as_f64), Some(-3.5));
    }

    #[test]
    fn reconcile_preserves_untouched_kv3_number_types() {
        // A tree mixing the three numeric KV3 types. The JSON projection collapses
        // Int and UInt to one number type, so a naive rebuild would re-type `u`.
        let orig = Value::Object(vec![
            ("u".to_owned(), Value::UInt(48_000)),
            ("i".to_owned(), Value::Int(-5)),
            ("d".to_owned(), Value::Double(0.5)),
        ]);
        // Edit only `d`; `u` and `i` are unchanged in the projection.
        let mut json = value_to_json(&orig);
        json["d"] = serde_json::json!(0.75);

        let merged = reconcile_json(&orig, json);
        assert!(matches!(merged.get("u"), Some(Value::UInt(48_000))));
        assert!(matches!(merged.get("i"), Some(Value::Int(-5))));
        assert!(matches!(merged.get("d"), Some(Value::Double(_))));
        assert_eq!(merged.get("d").and_then(Value::as_f64), Some(0.75));
    }

    #[test]
    fn reconcile_keeps_type_for_in_range_numeric_edit() {
        let orig = Value::Object(vec![
            ("u".to_owned(), Value::UInt(10)),
            ("d".to_owned(), Value::Double(1.0)),
        ]);
        let mut json = value_to_json(&orig);
        json["u"] = serde_json::json!(20); // still non-negative -> stays UInt
        json["d"] = serde_json::json!(4); // integer literal -> stays Double

        let merged = reconcile_json(&orig, json);
        assert!(matches!(merged.get("u"), Some(Value::UInt(20))));
        assert!(matches!(merged.get("d"), Some(Value::Double(_))));
    }

    #[test]
    fn reconcile_falls_back_for_new_keys_and_type_changes() {
        let orig = Value::Object(vec![("u".to_owned(), Value::UInt(1))]);
        let mut json = value_to_json(&orig);
        json["u"] = serde_json::json!(-3); // negative -> no longer fits UInt
        json["added"] = serde_json::json!(7); // brand-new key

        let merged = reconcile_json(&orig, json);
        assert!(matches!(merged.get("u"), Some(Value::Int(-3))));
        assert!(matches!(merged.get("added"), Some(Value::Int(7))));
    }
}
