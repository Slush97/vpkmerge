//! Structural probe of candidate swap-target prop models.
//! Usage: cargo run -p vpkmerge-core --example probe_props -- <vpk> <entry1> [entry2...]
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let vpk_path = &args[1];
    println!(
        "{:<62} {:>7} {:>5} {:>4} {:>4} {:>5} {:>5} {:>10}",
        "entry", "KB", "geom", "prt", "anim", "phys", "fmt", "bounds(uu)"
    );
    for entry in &args[2..] {
        match vpkmerge_core::read_vpk_entry(vpk_path, entry) {
            Ok(bytes) => {
                let kb = bytes.len() / 1024;
                let info = morphic::model::inspect(&bytes);
                match info {
                    Ok(i) => {
                        // bounds via position_bounds (decode top mesh) when modern
                        let bounds = morphic::model::decode(&bytes)
                            .ok()
                            .and_then(|m| m.position_bounds())
                            .map(|b| {
                                let d = [
                                    b.max[0] - b.min[0],
                                    b.max[1] - b.min[1],
                                    b.max[2] - b.min[2],
                                ];
                                format!("{:.0}x{:.0}x{:.0}", d[0], d[1], d[2])
                            })
                            .unwrap_or_else(|| "-".into());
                        let fmt = if i.has_embedded_geometry {
                            "modern"
                        } else {
                            "legacy"
                        };
                        let short = entry.trim_start_matches("models/");
                        println!(
                            "{:<62} {:>7} {:>5} {:>4} {:>4} {:>5} {:>5} {:>10}",
                            short,
                            kb,
                            i.mesh_parts,
                            i.mesh_parts,
                            if i.has_skeleton_anim { "Y" } else { "." },
                            if i.has_physics { "Y" } else { "." },
                            fmt,
                            bounds
                        );
                    }
                    Err(e) => println!("{entry:<62} {kb:>7}  ERR {e}"),
                }
            }
            Err(e) => println!("{entry:<62}   MISSING {e}"),
        }
    }
    Ok(())
}
