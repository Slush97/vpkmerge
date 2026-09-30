// For every .vpcf_c under a prefix, print each renderer's color-scale literal,
// flagging WHITE/bright ones (a white card face a hue-recolor can't darken).
// Usage: cargo run --example scan_renderers -- <vpk> <prefix>
use morphic::kv3::Value;

fn lit_color(v: &Value) -> Option<[i64; 3]> {
    let Value::Array(items) = v else { return None };
    if items.len() < 3 {
        return None;
    }
    let mut c = [0i64; 3];
    for (i, it) in items.iter().take(3).enumerate() {
        c[i] = match it {
            Value::Int(n) => *n,
            Value::UInt(u) => *u as i64,
            _ => return None,
        };
    }
    Some(c)
}

fn walk(v: &Value, path: &str, out: &mut Vec<(String, [i64; 3])>) {
    match v {
        Value::Object(p) => {
            for (k, c) in p {
                if k == "m_LiteralColor" {
                    if let Some(col) = lit_color(c) {
                        out.push((path.to_string(), col));
                    }
                }
                walk(c, &format!("{path}/{k}"), out);
            }
        }
        Value::Array(items) => {
            for (i, it) in items.iter().enumerate() {
                walk(it, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(&args[1])?;
    let prefix = &args[2];
    let entries: Vec<String> = vpk
        .file_paths()
        .filter(|p| p.ends_with(".vpcf_c") && p.starts_with(prefix))
        .cloned()
        .collect();
    for entry in &entries {
        let mut f = vpk.get_file(entry).expect("entry");
        let bytes = f.read_all()?;
        let Ok(tree) = morphic::decode_kv3_resource(&bytes) else {
            continue;
        };
        let mut cols = Vec::new();
        walk(&tree, "", &mut cols);
        // keep only renderer color-scale literals
        let rend: Vec<_> = cols
            .iter()
            .filter(|(p, _)| p.contains("m_Renderers") && p.contains("m_vecColorScale"))
            .collect();
        if rend.is_empty() {
            continue;
        }
        let short = entry.trim_start_matches("particles/abilities/wraith/");
        for (p, c) in rend {
            let bright = c.iter().min().copied().unwrap_or(0) >= 180; // near-white/bright
            let flag = if bright { "  <== BRIGHT/WHITE" } else { "" };
            let r = p.split("m_Renderers").nth(1).unwrap_or(p);
            println!("{short:48}  R{r:<6} = {c:?}{flag}");
        }
    }
    Ok(())
}
