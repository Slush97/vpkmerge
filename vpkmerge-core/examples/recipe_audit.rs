// Audit every pinned hero-recolor recipe against a live Deadlock VPK.
//
// For each pinned codename it checks that the recipe's particle prefixes still
// have `.vpcf_c` content and that every pinned texture/material/model entry still
// resolves verbatim. Texture/material paths embed a content-hash suffix
// (`..._g_tselfillum_670d93d.vtex_c`), so a Valve re-export silently renames the
// file and the pinned path goes stale; for each stale entry this prints the
// most likely successor (same stem, different hash) so the recipe can be repointed.
//
// Usage:
//   cargo run -p vpkmerge-core --example recipe_audit -- <pak01_dir.vpk> [--codename frank]

use std::collections::BTreeSet;

use vpkmerge_core::hero_recolor::{pinned_hero_codenames, recipe_for, HeroRecolorRecipe};

/// Strip the trailing `_<hex>` content-hash segment and the extension, leaving a
/// stable stem to match a renamed successor against. `foo_g_tselfillum_670d93d.vtex_c`
/// -> `foo_g_tselfillum`. A path with no trailing hex segment (e.g. a `.vmat_c`)
/// just loses its extension.
fn stem_without_hash(path: &str) -> String {
    let (no_ext, _) = path.rsplit_once('.').unwrap_or((path, ""));
    match no_ext.rsplit_once('_') {
        Some((head, tail)) if !tail.is_empty() && tail.chars().all(|c| c.is_ascii_hexdigit()) => {
            head.to_string()
        }
        _ => no_ext.to_string(),
    }
}

/// Candidate successors for a stale entry: current paths with the same hashless
/// stem and the same extension.
fn successors<'a>(path: &str, present: &'a BTreeSet<String>) -> Vec<&'a String> {
    let stem = stem_without_hash(path);
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    present
        .iter()
        .filter(|p| p.ends_with(ext) && stem_without_hash(p) == stem)
        .collect()
}

fn audit_entry(kind: &str, path: &str, present: &BTreeSet<String>, stale: &mut Vec<String>) {
    if present.contains(path) {
        return;
    }
    let succ = successors(path, present);
    stale.push(path.to_string());
    println!("    STALE {kind}: {path}");
    if succ.is_empty() {
        println!("          -> no successor found (removed?)");
    } else {
        for s in succ {
            println!("          -> successor: {s}");
        }
    }
}

fn audit_recipe(recipe: &HeroRecolorRecipe, present: &BTreeSet<String>) -> usize {
    let mut stale: Vec<String> = Vec::new();

    // Particle prefixes: count `.vpcf_c` present; 0 means the dir was renamed/removed.
    let mut prefix_issues = Vec::new();
    for prefix in &recipe.particle_prefixes {
        let n = present
            .iter()
            .filter(|p| p.starts_with(prefix.as_str()) && p.ends_with(".vpcf_c"))
            .count();
        if n == 0 {
            prefix_issues.push(prefix.clone());
        }
    }

    let header_needed = !prefix_issues.is_empty() || {
        // pre-scan whether any pinned entry is stale
        recipe
            .texture_entries
            .iter()
            .chain(recipe.material_entries.iter())
            .chain(recipe.model_entries.iter())
            .any(|e| !present.contains(e))
    };

    println!(
        "{} {:<12} particles:{} tex:{} mat:{} model:{}",
        if header_needed { "ISSUE" } else { "ok   " },
        recipe.codename,
        recipe.particle_prefixes.len(),
        recipe.texture_entries.len(),
        recipe.material_entries.len(),
        recipe.model_entries.len(),
    );

    for p in &prefix_issues {
        println!("    EMPTY particle prefix (0 .vpcf_c): {p}");
        for s in successors(&format!("{p}x.vpcf_c"), present)
            .into_iter()
            .take(3)
        {
            println!("          -> nearby: {s}");
        }
    }
    for e in &recipe.texture_entries {
        audit_entry("texture", e, present, &mut stale);
    }
    for e in &recipe.material_entries {
        audit_entry("material", e, present, &mut stale);
    }
    for e in &recipe.model_entries {
        audit_entry("model", e, present, &mut stale);
    }

    if let Some(pt) = &recipe.preview_texture {
        if !present.contains(pt) {
            println!("    STALE preview_texture: {pt}");
        }
    }

    stale.len() + prefix_issues.len()
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let vpk_path = args
        .next()
        .expect("usage: recipe_audit <pak01_dir.vpk> [--codename X]");
    let mut only: Option<String> = None;
    while let Some(a) = args.next() {
        if a == "--codename" {
            only = args.next();
        }
    }

    let vpk = valve_pak::open(&vpk_path)?;
    let present: BTreeSet<String> = vpk
        .file_paths()
        .map(std::string::ToString::to_string)
        .collect();
    eprintln!("pak entries: {}", present.len());

    let codenames: Vec<&str> = match &only {
        Some(c) => vec![c.as_str()],
        None => pinned_hero_codenames().to_vec(),
    };

    let mut total_issue_heroes = 0;
    let mut total_issues = 0;
    for code in codenames {
        let Some(recipe) = recipe_for(code) else {
            println!("?? no recipe for {code}");
            continue;
        };
        let n = audit_recipe(&recipe, &present);
        if n > 0 {
            total_issue_heroes += 1;
            total_issues += n;
        }
    }

    eprintln!("\nheroes with issues: {total_issue_heroes}, total stale/empty: {total_issues}");
    Ok(())
}
