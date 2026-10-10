// For every `.vpcf_c` under a prefix, list the materials it renders with, so a
// caller can build a sheet-swap map (e.g. fire sheets -> water sheets) before
// repointing anything.
//
// Two views:
//   by-particle   one block per .vpcf_c, its material refs indented
//   by-material   one line per material, sorted by how many particles use it,
//                 flagged SHARED when the material also lives outside the prefix
//
// usage: cargo run -p vpkmerge-core --example particle_material_refs -- \
//          <vpk> <prefix> [by-particle|by-material]
use morphic::kv3::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Collect every string leaf in a KV3 tree. Resource refs decode to plain
/// strings (the ref flag is dropped, the path content is intact).
fn strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        Value::Object(o) => o.iter().for_each(|(_, x)| strings(x, out)),
        _ => {}
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: particle_material_refs <vpk> <prefix> [by-particle|by-material]");
        std::process::exit(2);
    }
    let view = args.get(3).map_or("by-material", String::as_str);
    let vpk = valve_pak::open(&args[1])?;
    let prefix = args[2].to_lowercase();

    let particles: Vec<String> = {
        let mut v: Vec<String> = vpk
            .file_paths()
            .filter(|p| {
                let lp = p.to_lowercase();
                lp.starts_with(&prefix) && lp.ends_with(".vpcf_c")
            })
            .cloned()
            .collect();
        v.sort();
        v
    };

    // material -> particles that reference it
    let mut users: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut undecodable = 0usize;

    for entry in &particles {
        let Ok(mut f) = vpk.get_file(entry) else {
            continue;
        };
        let bytes = f.read_all()?;
        let Ok(kv) = morphic::decode_kv3_resource(&bytes) else {
            undecodable += 1;
            continue;
        };
        let mut leaves = Vec::new();
        strings(&kv, &mut leaves);
        // Sprite-card renderers bind `.vtex` sheets directly (m_vecTexturesInput
        // -> m_hTexture); model renderers go through a `.vmat`. Collect both.
        let mats: BTreeSet<String> = leaves
            .into_iter()
            .filter(|s| {
                let l = s.trim_end_matches("_c");
                l.ends_with(".vmat") || l.ends_with(".vtex")
            })
            .map(|s| s.trim_end_matches("_c").to_string())
            .collect();
        if view == "by-particle" {
            let short = entry.rsplit('/').next().unwrap_or(entry);
            println!("{short}");
            for m in &mats {
                println!("    {m}");
            }
        }
        for m in mats {
            users.entry(m).or_default().insert(entry.clone());
        }
    }

    if view == "by-material" {
        // Sort by descending user count so the workhorse sheets come first.
        let mut rows: Vec<(&String, &BTreeSet<String>)> = users.iter().collect();
        rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
        println!("{:>5}  {:<8}  MATERIAL", "USES", "SCOPE");
        for (mat, us) in rows {
            // A material inside the hero's own particle/material namespace is
            // safe to edit; one outside it is shared with other effects.
            let lm = mat.to_lowercase();
            let hero = prefix
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or_default();
            let scope = if !hero.is_empty() && lm.contains(hero) {
                "hero"
            } else {
                "SHARED"
            };
            println!("{:>5}  {:<8}  {}", us.len(), scope, mat);
        }
    }

    eprintln!(
        "{} particles, {} distinct materials, {} undecodable",
        particles.len(),
        users.len(),
        undecodable
    );
    Ok(())
}
