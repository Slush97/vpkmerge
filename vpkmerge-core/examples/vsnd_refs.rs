//! Find every soundevent that references a given `.vsnd_c` clip path.
//!
//! Answers "what else plays this clip?" before you swap it in place, which is
//! exactly how a hitsound swap leaks into the main menu.
//!
//! Usage: cargo run -p vpkmerge-core --example vsnd_refs -- <pak01_dir.vpk> <substring>
use anyhow::Result;
use vpkmerge_core::soundevents::SoundEvents;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let vpk_path = &args[1];
    let needle = args[2].to_lowercase();

    let vpk = valve_pak::open(vpk_path)?;
    let mut files: Vec<String> = vpk
        .file_paths()
        .filter(|p| p.ends_with(".vsndevts_c"))
        .cloned()
        .collect();
    files.sort();

    let mut hits = 0usize;
    for entry in &files {
        let se = match SoundEvents::from_vpk(vpk_path, entry) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("skip {entry}: {e}");
                continue;
            }
        };
        let json = se.to_json();
        let Some(obj) = json.as_object() else {
            continue;
        };
        for (event, body) in obj {
            let blob = serde_json::to_string(body)
                .unwrap_or_default()
                .to_lowercase();
            if blob.contains(&needle) {
                hits += 1;
                let refs: Vec<String> = collect_strings(body)
                    .into_iter()
                    .filter(|s| s.to_lowercase().contains(&needle))
                    .collect();
                println!("{entry}\n  event: {event}");
                for r in refs {
                    println!("    ref: {r}");
                }
                if let Some(base) = body.get("base").and_then(|v| v.as_str()) {
                    println!("    base: {base}");
                }
            }
        }
    }
    println!(
        "\n{hits} event(s) reference '{needle}' across {} files",
        files.len()
    );
    Ok(())
}

fn collect_strings(v: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    walk(v, &mut out);
    out
}

fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) => out.push(s.clone()),
        serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
        serde_json::Value::Object(o) => o.values().for_each(|x| walk(x, out)),
        _ => {}
    }
}
