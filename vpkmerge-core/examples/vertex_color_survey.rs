// How common is F_VERTEX_COLOR on Deadlock hero materials? A material with that
// flag draws its base color from the mesh's baked per-vertex COLOR rather than
// (or on top of) g_tColor, so any value variation there is painted into the
// geometry and invisible to texture-level tooling.
//
// Reports every models/heroes* material carrying the flag, with its
// g_fVertexColorStrength1 and the dimensions of its g_tColor (a tiny albedo
// means vertex color is doing all the work).
//
// usage: cargo run --example vertex_color_survey -- <vpk>

fn main() -> anyhow::Result<()> {
    let vpk_path = std::env::args().nth(1).expect("vpk");
    let vpk = valve_pak::open(&vpk_path)?;

    let mut entries: Vec<String> = vpk
        .file_paths()
        .filter(|p| p.ends_with(".vmat_c") && p.starts_with("models/heroes"))
        .cloned()
        .collect();
    entries.sort();
    println!("scanning {} hero materials\n", entries.len());

    let mut hits = Vec::new();
    for entry in &entries {
        let targets = vpkmerge_core::vmat_style::VmatTargets::Entries(vec![entry.clone()]);
        let Ok(mats) = vpkmerge_core::vmat_style::list_materials(&vpk_path, None, &targets) else {
            continue;
        };
        let Some(m) = mats.first() else { continue };
        if !m
            .flags
            .iter()
            .any(|(f, v)| f == "F_VERTEX_COLOR" && *v != 0)
        {
            continue;
        }
        let strength = m
            .floats
            .iter()
            .find(|(n, _)| n == "g_fVertexColorStrength1")
            .map_or(0.0, |(_, v)| *v);
        let albedo = m
            .textures
            .iter()
            .find(|(k, _)| k == "g_tColor")
            .map(|(_, p)| p.clone());
        let dims = albedo.as_ref().and_then(|p| {
            let e = if p.ends_with("_c") {
                p.clone()
            } else {
                format!("{p}_c")
            };
            let bytes = vpkmerge_core::read_vpk_entry(&vpk_path, &e).ok()?;
            let info = morphic::parse_texture_header(&bytes).ok()?;
            Some((info.width, info.height))
        });
        hits.push((entry.clone(), strength, dims));
    }

    println!("{} material(s) with F_VERTEX_COLOR:\n", hits.len());
    println!("  {:<10} {:>11}  entry", "albedo", "strength");
    for (entry, strength, dims) in &hits {
        let d = dims.map_or_else(|| "?".to_string(), |(w, h)| format!("{w}x{h}"));
        let flag = if dims.is_some_and(|(w, h)| w <= 8 && h <= 8) {
            "  <- albedo is a flat swatch; vertex color IS the base color"
        } else {
            ""
        };
        println!("  {d:<10} {strength:>11}  {entry}{flag}");
    }
    Ok(())
}
