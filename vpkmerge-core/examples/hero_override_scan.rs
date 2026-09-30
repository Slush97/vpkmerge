// Which addon VPKs touch a given hero, and at what level?
//
// Compatibility between two mods for the same hero is decided per VPK **entry
// path**: the winning pak supplies that whole file, there is no blending. So a
// model-level edit (`.vmdl_c`) and a texture-level edit (`.vtex_c`) coexist
// fine, while two model-level edits hard-conflict.
//
// Classifies every entry each pak contributes for the hero into `model`
// (`.vmdl_c`), `material` (`.vmat_c`), `texture` (`.vtex_c`), or `other`, so a
// "will these stack?" question is answered by looking for overlap in the same
// column rather than by trial and error.
//
// usage: cargo run --example hero_override_scan -- <addons-dir> <path-substring>...
//   e.g. ... -- /path/to/citadel/addons hornet_v3 vindicta

use std::collections::BTreeMap;
use std::path::Path;

fn classify(entry: &str) -> &'static str {
    if entry.ends_with(".vmdl_c") {
        "model"
    } else if entry.ends_with(".vmat_c") {
        "material"
    } else if entry.ends_with(".vtex_c") {
        "texture"
    } else {
        "other"
    }
}

fn scan(path: &Path, needles: &[String]) -> Option<(BTreeMap<&'static str, Vec<String>>, usize)> {
    let vpk = valve_pak::open(path).ok()?;
    let mut by_kind: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let mut total = 0usize;
    for entry in vpk.file_paths() {
        total += 1;
        let lower = entry.to_ascii_lowercase();
        if needles.iter().any(|n| lower.contains(n)) {
            by_kind
                .entry(classify(entry))
                .or_default()
                .push(entry.clone());
        }
    }
    Some((by_kind, total))
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("addons dir");
    let needles: Vec<String> = args.map(|s| s.to_ascii_lowercase()).collect();
    anyhow::ensure!(!needles.is_empty(), "give at least one path substring");
    println!("matching entries containing any of: {needles:?}\n");

    // Enabled paks sit in the addons dir; Grimoire parks disabled ones in
    // `.disabled/`, which the engine never mounts.
    let mut roots = vec![(std::path::PathBuf::from(&dir), "enabled")];
    let disabled = Path::new(&dir).join(".disabled");
    if disabled.is_dir() {
        roots.push((disabled, "disabled"));
    }

    // Who else edits the model file: the actual conflict question.
    let mut model_editors: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for (root, state) in &roots {
        let mut paks: Vec<_> = std::fs::read_dir(root)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with("_dir.vpk"))
            })
            .collect();
        paks.sort();

        for pak in &paks {
            let Some((by_kind, total)) = scan(pak, &needles) else {
                println!("  [{state}] {}  (unreadable)", pak.display());
                continue;
            };
            if by_kind.is_empty() {
                continue;
            }
            let name = pak.file_name().unwrap().to_string_lossy();
            let counts: Vec<String> = by_kind
                .iter()
                .map(|(k, v)| format!("{k}:{}", v.len()))
                .collect();
            println!(
                "[{state:<8}] {name:<52} {total:>6} entries total | hero hits {}",
                counts.join(" ")
            );
            for (kind, entries) in &by_kind {
                if *kind == "model" || entries.len() <= 3 {
                    for e in entries {
                        println!("             {kind:<9} {e}");
                        if *kind == "model" {
                            model_editors
                                .entry(e.clone())
                                .or_default()
                                .push(format!("{name} ({state})"));
                        }
                    }
                } else {
                    println!("             {kind:<9} {} entries", entries.len());
                }
            }
        }
    }

    println!("\n=== model-file (.vmdl_c) contention ===");
    if model_editors.is_empty() {
        println!("  none: no installed pak overrides a model for this hero");
    }
    for (entry, paks) in &model_editors {
        let verdict = if paks.len() > 1 {
            "CONFLICT: only the highest-priority pak's copy loads"
        } else {
            "sole editor"
        };
        println!("  {entry}\n    {verdict}");
        for p in paks {
            println!("      - {p}");
        }
    }
    Ok(())
}
