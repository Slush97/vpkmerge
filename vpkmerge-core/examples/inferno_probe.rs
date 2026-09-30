// Survey Infernus's particle kit: what imagery is still stock, and what BEHAVIOR
// classes are there to manipulate.
//
// `inferno_water` currently changes colour + sheet imagery. This probe exists to
// answer the next question: which texture inputs are still pointing at stock
// (non-water) sheets, and which operator / initializer / force classes his particles
// actually carry, so a behaviour pass edits real classes rather than guessed ones.
//
// usage:
//   cargo run --release -p vpkmerge-core --example inferno_probe -- <pak01_dir.vpk> \
//     [--textures] [--classes] [--params] [--inputs] [--grep SUBSTR] [--tex SUBSTR]
//     [--dump CLASS]
use anyhow::Result;
use morphic::kv3::Value;
use std::collections::{BTreeMap, BTreeSet};

const PREFIXES: &[&str] = &[
    "particles/abilities/inferno/",
    "particles/weapon_fx/inferno/",
];

fn walk_classes(v: &Value, out: &mut BTreeMap<String, BTreeSet<String>>, entry: &str) {
    match v {
        Value::Object(pairs) => {
            if let Some(c) = v.get("_class").and_then(Value::as_str) {
                out.entry(c.to_string())
                    .or_default()
                    .insert(entry.to_string());
            }
            for (_, child) in pairs {
                walk_classes(child, out, entry);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_classes(item, out, entry);
            }
        }
        _ => {}
    }
}

/// Every renderer texture input as `(renderer class, m_hTexture, role fields)`.
/// The role fields are what say whether a sheet is visible imagery or a
/// distortion / mask channel, which decides whether repointing it is safe.
fn texture_input_roles(tree: &Value) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let Some(renderers) = tree.get("m_Renderers").and_then(Value::as_array) else {
        return out;
    };
    for renderer in renderers {
        let class = renderer
            .get("_class")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        let Some(inputs) = renderer.get("m_vecTexturesInput").and_then(Value::as_array) else {
            continue;
        };
        for input in inputs {
            let Some(tex) = input.get("m_hTexture").and_then(Value::as_str) else {
                continue;
            };
            let Some(fields) = input.as_object() else {
                continue;
            };
            let role = fields
                .iter()
                .filter(|(k, _)| k.as_str() != "m_hTexture")
                .map(|(k, v)| {
                    let rendered = match v {
                        Value::String(s) => s.clone(),
                        Value::Int(i) => format!("{i}"),
                        Value::Double(d) => format!("{d}"),
                        Value::Bool(b) => format!("{b}"),
                        _ => return String::new(),
                    };
                    format!("{k}={rendered}")
                })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            out.push((class.clone(), tex.to_string(), role));
        }
    }
    out
}

/// Every `m_hTexture` under the renderers.
fn texture_inputs(tree: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(renderers) = tree.get("m_Renderers").and_then(Value::as_array) else {
        return out;
    };
    for renderer in renderers {
        let Some(inputs) = renderer.get("m_vecTexturesInput").and_then(Value::as_array) else {
            continue;
        };
        for input in inputs {
            if let Some(tex) = input.get("m_hTexture").and_then(Value::as_str) {
                out.push(tex.to_string());
            }
        }
    }
    out
}

/// Collect every numeric leaf whose key matches one of `keys`, with its owning class.
fn walk_params(
    v: &Value,
    keys: &[&str],
    class: &str,
    entry: &str,
    out: &mut Vec<(String, String, String, String)>,
) {
    match v {
        Value::Object(pairs) => {
            let here = v.get("_class").and_then(Value::as_str).unwrap_or(class);
            for (k, child) in pairs {
                if keys.iter().any(|w| k.contains(w)) {
                    let rendered = match child {
                        Value::Double(d) => Some(format!("{d}")),
                        Value::Int(i) => Some(format!("{i}")),
                        Value::Array(a)
                            if a.iter()
                                .all(|x| matches!(x, Value::Double(_) | Value::Int(_))) =>
                        {
                            Some(format!(
                                "[{}]",
                                a.iter()
                                    .map(|x| match x {
                                        Value::Double(d) => format!("{d}"),
                                        Value::Int(i) => format!("{i}"),
                                        _ => String::new(),
                                    })
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ))
                        }
                        _ => None,
                    };
                    if let Some(r) = rendered {
                        out.push((here.to_string(), k.clone(), r, entry.to_string()));
                    }
                }
                walk_params(child, keys, here, entry, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_params(item, keys, class, entry, out);
            }
        }
        _ => {}
    }
}

/// First subtree carrying `_class == class`, for reading a real operator's field
/// names off the live pak instead of guessing them.
fn find_class<'a>(v: &'a Value, class: &str) -> Option<&'a Value> {
    match v {
        Value::Object(pairs) => {
            if v.get("_class").and_then(Value::as_str) == Some(class) {
                return Some(v);
            }
            pairs.iter().find_map(|(_, c)| find_class(c, class))
        }
        Value::Array(items) => items.iter().find_map(|i| find_class(i, class)),
        _ => None,
    }
}

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    if argv.len() < 2 {
        eprintln!(
            "usage: inferno_probe <pak01_dir.vpk> [--textures] [--classes] [--params] \
             [--inputs] [--grep SUBSTR] [--tex SUBSTR] [--dump CLASS]"
        );
        std::process::exit(2);
    }
    let pak = argv[1].clone();
    let want = |f: &str| argv.iter().any(|a| a == f);
    let dump: Option<String> = argv
        .iter()
        .position(|a| a == "--dump")
        .and_then(|i| argv.get(i + 1))
        .cloned();
    let all = !want("--textures")
        && !want("--classes")
        && !want("--params")
        && !want("--inputs")
        && dump.is_none();
    let grep: Option<String> = argv
        .iter()
        .position(|a| a == "--grep")
        .and_then(|i| argv.get(i + 1))
        .cloned();
    // `--grep` narrows the particle list; `--tex` narrows which texture inputs
    // `--inputs` prints, which is the question "who uses this sheet, and as what".
    let tex_filter: Option<String> = argv
        .iter()
        .position(|a| a == "--tex")
        .and_then(|i| argv.get(i + 1))
        .cloned();

    let vpk = valve_pak::open(&pak)?;
    let particles: Vec<String> = {
        let mut v: Vec<String> = vpk
            .file_paths()
            .filter(|p| PREFIXES.iter().any(|pre| p.starts_with(pre)) && p.ends_with(".vpcf_c"))
            .filter(|p| grep.as_ref().is_none_or(|g| p.contains(g.as_str())))
            .cloned()
            .collect();
        v.sort();
        v
    };
    println!("{} particles\n", particles.len());

    let mut textures: BTreeMap<String, usize> = BTreeMap::new();
    let mut classes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut params: Vec<(String, String, String, String)> = Vec::new();
    let param_keys = [
        "Gravity", "Drag", "Lifespan", "Lifetime", "Radius", "Speed", "Rate", "Noise", "Jitter",
    ];

    let mut dumped = 0usize;
    for entry in &particles {
        let bytes = vpkmerge_core::read_vpk_entry(&pak, entry)?;
        let tree = morphic::decode_kv3_resource(&bytes)
            .map_err(|e| anyhow::anyhow!("decoding {entry}: {e}"))?;
        if let Some(class) = &dump {
            if dumped < 3 {
                if let Some(found) = find_class(&tree, class) {
                    println!("-- {entry}\n{found:#?}\n");
                    dumped += 1;
                }
            }
        }
        for t in texture_inputs(&tree) {
            *textures.entry(t).or_default() += 1;
        }
        if want("--inputs") {
            for (class, tex, role) in texture_input_roles(&tree) {
                if tex_filter.as_ref().is_none_or(|g| tex.contains(g.as_str())) {
                    println!("{entry}\n  {class:<22} {tex}\n      {role}");
                }
            }
        }
        walk_classes(&tree, &mut classes, entry);
        walk_params(&tree, &param_keys, "", entry, &mut params);
    }

    if all || want("--textures") {
        println!("== texture inputs ({} distinct) ==", textures.len());
        let mut rows: Vec<_> = textures.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        for (t, n) in rows {
            println!("  {n:>3}x {t}");
        }
        println!();
    }

    if all || want("--classes") {
        println!("== classes ({} distinct) ==", classes.len());
        let mut rows: Vec<_> = classes.iter().collect();
        rows.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
        for (c, entries) in rows {
            println!("  {:>3} particles  {c}", entries.len());
        }
        println!();
    }

    if all || want("--params") {
        println!("== behaviour params ==");
        let mut by: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        for (class, key, val, entry) in &params {
            by.entry((class.clone(), key.clone()))
                .or_default()
                .push(format!("{val}  <- {entry}"));
        }
        for ((class, key), vals) in &by {
            println!("  {class} :: {key}  ({} uses)", vals.len());
            let mut distinct: BTreeMap<&str, usize> = BTreeMap::new();
            for v in vals {
                let value = v.split("  <- ").next().unwrap_or(v);
                *distinct.entry(value).or_default() += 1;
            }
            let mut rows: Vec<_> = distinct.iter().collect();
            rows.sort_by(|a, b| b.1.cmp(a.1));
            for (v, n) in rows.iter().take(8) {
                println!("        {n:>3}x {v}");
            }
        }
    }
    Ok(())
}
