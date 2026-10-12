//! Copy-and-repoint for hero recolor bakes.
//!
//! A recipe's `shared_entries` are assets another hero, an item or a neutral
//! also renders, so a bake must never write them in place. [`isolate`] takes a
//! bake's per-entry output and relocates it: a shared entry the hero reaches
//! through one of its own files (a private particle, a pinned material, or
//! another copy) is written as a hero-local copy and that file is repointed at
//! the copy. A changed shared entry nothing private reaches is dropped, so it
//! stays vanilla for the hero too.
//!
//! The invariant every bake output keeps: no shared entry is ever written at
//! its own path, so two heroes' addons never override the same file through a
//! shared asset.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

use crate::hero_recolor::HeroRecolorRecipe;

/// What [`isolate`] did to one bake's output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IsolationStats {
    /// Hero-local copies of shared entries written.
    pub copies: usize,
    /// Hero files whose references were redirected at a copy.
    pub repointed: usize,
    /// Changed shared entries no private hero file reaches: left vanilla.
    pub kept_vanilla: usize,
}

/// Whether `entry` falls under one of the recipe's `shared_entries`.
#[must_use]
pub fn is_shared(recipe: &HeroRecolorRecipe, entry: &str) -> bool {
    recipe
        .shared_entries
        .iter()
        .any(|p| entry.starts_with(p.as_str()))
}

/// Where the hero-local copy of `entry` goes: the same directory, with the
/// codename in the file name so two heroes' copies never collide.
#[must_use]
pub fn copy_path(codename: &str, entry: &str) -> String {
    let (stem, ext) = entry.rsplit_once('.').unwrap_or((entry, ""));
    format!("{stem}_{codename}_recolor.{ext}")
}

/// The resource name a compiled entry is referenced by (`x.vpcf_c` -> `x.vpcf`).
fn resource_name(entry: &str) -> &str {
    entry.strip_suffix("_c").unwrap_or(entry)
}

/// Redirects a resource's references, `(old, new)` resource names.
pub(crate) type Repoint<'a> = &'a dyn Fn(&[u8], &[(String, String)]) -> Result<Vec<u8>>;

/// Relocates the shared entries of a bake's output.
///
/// `packed` is the bake's output keyed by source entry path. `candidates` are
/// the hero files the bake may patch in place even when it changed nothing in
/// them (every particle it walked, every pinned material); together with the
/// non-shared keys of `packed` they are the private referrers. `read` resolves
/// an entry's original bytes, `refs` lists the compiled entry paths a resource
/// references, and `repoint` redirects references by resource name.
pub(crate) fn isolate(
    recipe: &HeroRecolorRecipe,
    candidates: &[String],
    packed: BTreeMap<String, Vec<u8>>,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
    refs: &dyn Fn(&[u8]) -> Vec<String>,
    repoint: Repoint,
) -> Result<(BTreeMap<String, Vec<u8>>, IsolationStats)> {
    if recipe.shared_entries.is_empty() {
        return Ok((packed, IsolationStats::default()));
    }
    let shared = |e: &str| is_shared(recipe, e);
    let current = |e: &str| packed.get(e).cloned().or_else(|| read(e));

    let private: BTreeSet<String> = packed
        .keys()
        .chain(candidates)
        .filter(|e| !shared(e))
        .cloned()
        .collect();
    // Only particles and materials are repointed: a model's material references
    // live in its mesh draw calls, which this step does not edit.
    let shared_refs = |entry: &str| -> BTreeSet<String> {
        if !(entry.ends_with(".vpcf_c") || entry.ends_with(".vmat_c")) {
            return BTreeSet::new();
        }
        current(entry)
            .map(|b| refs(&b).into_iter().filter(|r| shared(r)).collect())
            .unwrap_or_default()
    };

    let mut referrer_refs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in private {
        let rs = shared_refs(&e);
        if !rs.is_empty() {
            referrer_refs.insert(e, rs);
        }
    }

    // Every shared entry reachable from a private referrer, with its own shared
    // references.
    let mut reached: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut queue: Vec<String> = referrer_refs.values().flatten().cloned().collect();
    while let Some(e) = queue.pop() {
        if reached.contains_key(&e) {
            continue;
        }
        let rs = shared_refs(&e);
        queue.extend(rs.iter().filter(|r| !reached.contains_key(*r)).cloned());
        reached.insert(e, rs);
    }

    // A copy is worth writing when the bake changed it or it leads to a copy
    // that is; an unchanged leaf keeps pointing at the original.
    let mut useful: BTreeSet<String> = reached
        .keys()
        .filter(|e| packed.contains_key(*e))
        .cloned()
        .collect();
    loop {
        let grown: Vec<String> = reached
            .iter()
            .filter(|(e, rs)| !useful.contains(*e) && rs.iter().any(|r| useful.contains(r)))
            .map(|(e, _)| e.clone())
            .collect();
        if grown.is_empty() {
            break;
        }
        useful.extend(grown);
    }

    let remap = |rs: &BTreeSet<String>| -> Vec<(String, String)> {
        rs.iter()
            .filter(|r| useful.contains(*r))
            .map(|r| {
                (
                    resource_name(r).to_owned(),
                    resource_name(&copy_path(&recipe.codename, r)).to_owned(),
                )
            })
            .collect()
    };

    let mut stats = IsolationStats::default();
    let mut out: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for e in &useful {
        let bytes = current(e).unwrap_or_default();
        let map = remap(&reached[e]);
        let bytes = if map.is_empty() {
            bytes
        } else {
            repoint(&bytes, &map)?
        };
        out.insert(copy_path(&recipe.codename, e), bytes);
        stats.copies += 1;
    }
    for (e, bytes) in packed {
        if !shared(&e) {
            out.insert(e, bytes);
        } else if !useful.contains(&e) {
            stats.kept_vanilla += 1;
        }
    }
    for (e, rs) in &referrer_refs {
        let map = remap(rs);
        if map.is_empty() {
            continue;
        }
        let bytes = match out.get(e) {
            Some(b) => b.clone(),
            None => read(e).unwrap_or_default(),
        };
        out.insert(e.clone(), repoint(&bytes, &map)?);
        stats.repointed += 1;
    }
    Ok((out, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake resource is its references, one per line, after a `#tag` line.
    fn res(tag: &str, refs: &[&str]) -> Vec<u8> {
        std::iter::once(format!("#{tag}"))
            .chain(refs.iter().map(|r| (*r).to_owned()))
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes()
    }

    fn fake_refs(bytes: &[u8]) -> Vec<String> {
        std::str::from_utf8(bytes)
            .unwrap()
            .lines()
            .skip(1)
            .map(|l| format!("{l}_c"))
            .collect()
    }

    fn fake_repoint(bytes: &[u8], map: &[(String, String)]) -> Vec<u8> {
        let mut s = std::str::from_utf8(bytes).unwrap().to_owned();
        for (old, new) in map {
            assert!(s.contains(old.as_str()), "{old} not referenced");
            s = s.replace(old.as_str(), new);
        }
        s.into_bytes()
    }

    fn recipe(shared: &[&str]) -> HeroRecolorRecipe {
        HeroRecolorRecipe {
            codename: "hero".into(),
            particle_prefixes: vec!["p/hero/".into()],
            texture_entries: Vec::new(),
            material_entries: Vec::new(),
            model_entries: Vec::new(),
            shared_entries: shared.iter().map(|s| (*s).to_owned()).collect(),
            preview_texture: None,
        }
    }

    fn run(
        shared: &[&str],
        vanilla: &[(&str, Vec<u8>)],
        packed: &[(&str, Vec<u8>)],
    ) -> (BTreeMap<String, Vec<u8>>, IsolationStats) {
        let vanilla: BTreeMap<String, Vec<u8>> = vanilla
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        let candidates: Vec<String> = vanilla.keys().cloned().collect();
        let packed = packed
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect();
        isolate(
            &recipe(shared),
            &candidates,
            packed,
            &|e| vanilla.get(e).cloned(),
            &fake_refs,
            &|b, m| Ok(fake_repoint(b, m)),
        )
        .unwrap()
    }

    #[test]
    fn no_shared_entries_is_a_passthrough() {
        let packed = [("p/hero/a.vpcf_c", res("red", &[]))];
        let (out, stats) = run(&[], &[], &packed);
        assert_eq!(out.len(), 1);
        assert_eq!(stats, IsolationStats::default());
    }

    #[test]
    fn shared_child_is_copied_and_its_private_parent_repointed() {
        let vanilla = [
            ("p/hero/root.vpcf_c", res("plain", &["p/hero/kid.vpcf"])),
            ("p/hero/kid.vpcf_c", res("plain", &[])),
        ];
        let packed = [("p/hero/kid.vpcf_c", res("red", &[]))];
        let (out, stats) = run(&["p/hero/kid"], &vanilla, &packed);
        assert!(!out.contains_key("p/hero/kid.vpcf_c"));
        assert_eq!(out["p/hero/kid_hero_recolor.vpcf_c"], res("red", &[]));
        assert_eq!(
            out["p/hero/root.vpcf_c"],
            res("plain", &["p/hero/kid_hero_recolor.vpcf"])
        );
        assert_eq!(
            stats,
            IsolationStats {
                copies: 1,
                repointed: 1,
                kept_vanilla: 0
            }
        );
    }

    #[test]
    fn unchanged_shared_link_is_copied_only_to_reach_a_changed_one() {
        let vanilla = [
            ("p/hero/root.vpcf_c", res("plain", &["g/link.vpcf"])),
            (
                "g/link.vpcf_c",
                res("plain", &["g/leaf.vpcf", "g/other.vpcf"]),
            ),
            ("g/leaf.vpcf_c", res("plain", &[])),
            ("g/other.vpcf_c", res("plain", &[])),
        ];
        let packed = [("g/leaf.vpcf_c", res("red", &[]))];
        let (out, stats) = run(&["g/"], &vanilla, &packed);
        assert_eq!(
            out["g/link_hero_recolor.vpcf_c"],
            res("plain", &["g/leaf_hero_recolor.vpcf", "g/other.vpcf"])
        );
        assert!(!out.contains_key("g/other_hero_recolor.vpcf_c"));
        assert_eq!(
            out["p/hero/root.vpcf_c"],
            res("plain", &["g/link_hero_recolor.vpcf"])
        );
        assert_eq!(stats.copies, 2);
        assert_eq!(stats.repointed, 1);
    }

    #[test]
    fn shared_entry_reached_only_from_outside_stays_vanilla() {
        let vanilla = [("p/hero/spawned_by_vdata.vpcf_c", res("plain", &[]))];
        let packed = [("p/hero/spawned_by_vdata.vpcf_c", res("red", &[]))];
        let (out, stats) = run(&["p/hero/spawned_by_vdata"], &vanilla, &packed);
        assert!(out.is_empty());
        assert_eq!(stats.kept_vanilla, 1);
    }

    #[test]
    fn repointed_referrer_keeps_its_own_recolor() {
        let vanilla = [
            ("p/hero/root.vpcf_c", res("plain", &["m/decal.vmat"])),
            ("m/decal.vmat_c", res("plain", &[])),
        ];
        let packed = [
            ("p/hero/root.vpcf_c", res("red", &["m/decal.vmat"])),
            ("m/decal.vmat_c", res("stamped", &[])),
        ];
        let (out, _) = run(&["m/decal.vmat_c"], &vanilla, &packed);
        assert_eq!(
            out["p/hero/root.vpcf_c"],
            res("red", &["m/decal_hero_recolor.vmat"])
        );
        assert_eq!(out["m/decal_hero_recolor.vmat_c"], res("stamped", &[]));
    }

    #[test]
    fn cycles_terminate() {
        let vanilla = [
            ("p/hero/root.vpcf_c", res("plain", &["g/a.vpcf"])),
            ("g/a.vpcf_c", res("plain", &["g/b.vpcf"])),
            ("g/b.vpcf_c", res("plain", &["g/a.vpcf"])),
        ];
        let packed = [("g/b.vpcf_c", res("red", &["g/a.vpcf"]))];
        let (out, stats) = run(&["g/"], &vanilla, &packed);
        assert_eq!(stats.copies, 2);
        assert_eq!(
            out["g/b_hero_recolor.vpcf_c"],
            res("red", &["g/a_hero_recolor.vpcf"])
        );
    }

    #[test]
    fn copy_path_keeps_the_directory_and_extension() {
        assert_eq!(
            copy_path("necro", "materials/particle/foo_psd_1a2b.vtex_c"),
            "materials/particle/foo_psd_1a2b_necro_recolor.vtex_c"
        );
    }
}
