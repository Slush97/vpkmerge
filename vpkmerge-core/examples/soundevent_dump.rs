//! Dump every soundevent in a VPK as `file\tevent\tbase\tclip,clip,...`.
//! One flat table so you can grep for "who plays this clip / who inherits this
//! event" without re-decoding 149 files each time.
//!
//! Usage: cargo run -p vpkmerge-core --example soundevent_dump -- <pak01_dir.vpk>
use anyhow::Result;
use vpkmerge_core::soundevents::SoundEvents;

fn main() -> Result<()> {
    let vpk_path = std::env::args()
        .nth(1)
        .expect("usage: soundevent_dump <vpk>");
    let vpk = valve_pak::open(&vpk_path)?;
    let mut files: Vec<String> = vpk
        .file_paths()
        .filter(|p| p.ends_with(".vsndevts_c"))
        .cloned()
        .collect();
    files.sort();

    for entry in &files {
        let Ok(se) = SoundEvents::from_vpk(&vpk_path, entry) else {
            eprintln!("skip {entry}");
            continue;
        };
        let json = se.to_json();
        let Some(obj) = json.as_object() else {
            continue;
        };
        for (event, body) in obj {
            let base = body.get("base").and_then(|v| v.as_str()).unwrap_or("");
            // `vsnd_files` is an array for a randomizer pool but a bare string
            // for a single-clip event; both shapes ship in the live pak.
            let clips: Vec<&str> = match body.get("vsnd_files") {
                Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).collect(),
                Some(serde_json::Value::String(s)) => vec![s.as_str()],
                _ => Vec::new(),
            };
            println!("{entry}\t{event}\t{base}\t{}", clips.join(","));
        }
    }
    Ok(())
}
