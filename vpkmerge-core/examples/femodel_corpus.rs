//! Scan VPKs for every `.vmdl_c` that carries a cloth `FeModel` and emit one
//! JSON line per model: the shape of `m_pFeModel` (scalar values + array
//! lengths per key) plus the compile provenance from `RED2` (special
//! dependencies = compiler identifiers/versions, input dependency sources).
//!
//! This is the raw corpus for deriving the "Valve-complete" FeModel dialect
//! from shipped + community cloth that is known to run in-engine.
//!
//! Usage: cargo run --release -p vpkmerge-core --example femodel_corpus -- <vpk>... > corpus.jsonl

use morphic::kv3::Value;
use morphic::resource::Resource;
use serde_json::{json, Map, Value as J};

fn fe_of(root: &Value) -> Option<&Value> {
    if let Some(fe) = root.get("m_pFeModel").or_else(|| root.get("m_feModel")) {
        return Some(fe);
    }
    root.get("m_parts")
        .and_then(Value::as_array)?
        .iter()
        .find_map(|p| p.get("m_pFeModel"))
}

fn scalar(v: &Value) -> J {
    match v {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Int(n) => json!(n),
        Value::UInt(n) => json!(n),
        Value::Double(d) => json!(d),
        Value::String(s) => J::String(s.clone()),
        Value::Binary(b) => json!({ "binary": b.len() }),
        Value::Array(a) => json!({ "len": a.len() }),
        Value::Object(o) => json!({ "obj": o.len() }),
    }
}

fn shape(fe: &Value) -> J {
    let mut m = Map::new();
    if let Value::Object(items) = fe {
        for (k, v) in items {
            m.insert(k.clone(), scalar(v));
        }
    }
    J::Object(m)
}

fn provenance(res: &Resource) -> J {
    let Some(red) = res.find_block(*b"RED2") else {
        return J::Null;
    };
    let Ok(root) = morphic::kv3::decode(red) else {
        return json!("RED2 undecodable");
    };
    let special: Vec<J> = root
        .get("m_SpecialDependencies")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|d| {
                    json!([
                        d.get("m_String").and_then(Value::as_str).unwrap_or(""),
                        d.get("m_CompilerIdentifier")
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                        d.get("m_nUserData").and_then(Value::as_uint).unwrap_or(0),
                    ])
                })
                .collect()
        })
        .unwrap_or_default();
    let inputs: Vec<J> = root
        .get("m_InputDependencies")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|d| d.get("m_RelativeFilename").and_then(Value::as_str))
                .map(|s| J::String(s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    json!({ "special": special, "inputs": inputs })
}

fn f(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

fn median(mut xs: Vec<f64>) -> f64 {
    xs.retain(|x| x.is_finite());
    if xs.is_empty() {
        return f64::NAN;
    }
    xs.sort_by(f64::total_cmp);
    xs[xs.len() / 2]
}

fn find(p: &mut [usize], x: usize) -> usize {
    let mut r = x;
    while p[r] != r {
        r = p[r];
    }
    p[x] = r;
    r
}

/// One line per connected cloth piece (union of rods over dynamic nodes):
/// the per-piece solver parameters a fabric preset is derived from.
fn pieces(fe: &Value) -> Vec<J> {
    let arr = |k: &str| fe.get(k).and_then(Value::as_array).unwrap_or(&[]);
    let n = fe.get("m_nNodeCount").and_then(Value::as_uint).unwrap_or(0) as usize;
    let n_static = fe
        .get("m_nStaticNodes")
        .and_then(Value::as_uint)
        .unwrap_or(0) as usize;
    let names: Vec<&str> = arr("m_CtrlName")
        .iter()
        .map(|v| v.as_str().unwrap_or(""))
        .collect();
    let integ = arr("m_NodeIntegrator");
    let inv = arr("m_NodeInvMasses");
    let radii = arr("m_NodeCollisionRadii");
    let rods = arr("m_Rods");
    let mut parent: Vec<usize> = (0..n).collect();
    let rod_nodes = |r: &Value| -> Option<(usize, usize)> {
        let nn = r.get("nNode")?.as_array()?;
        Some((
            nn.first()?.as_uint()? as usize,
            nn.get(1)?.as_uint()? as usize,
        ))
    };
    for r in rods {
        if let Some((a, b)) = rod_nodes(r) {
            if a < n && b < n && a >= n_static && b >= n_static {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                parent[ra] = rb;
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for i in n_static..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut out = Vec::new();
    for members in groups.values() {
        let set: std::collections::HashSet<usize> = members.iter().copied().collect();
        let mut anchors = std::collections::BTreeSet::new();
        let (mut len, mut stretch, mut relax, mut w0) = (vec![], vec![], vec![], vec![]);
        for r in rods {
            let Some((a, b)) = rod_nodes(r) else { continue };
            let (ia, ib) = (set.contains(&a), set.contains(&b));
            if !(ia || ib) {
                continue;
            }
            if !ia && a < n_static {
                anchors.insert(names.get(a).copied().unwrap_or(""));
            }
            if !ib && b < n_static {
                anchors.insert(names.get(b).copied().unwrap_or(""));
            }
            let (mx, mn) = (f(r, "flMaxDist"), f(r, "flMinDist"));
            len.push(mx);
            if mx > 0.0 {
                stretch.push(mn / mx);
            }
            relax.push(f(r, "flRelaxationFactor"));
            w0.push(f(r, "flWeight0"));
        }
        let pick = |k: &str| {
            median(
                members
                    .iter()
                    .filter_map(|&i| integ.get(i))
                    .map(|v| f(v, k))
                    .collect(),
            )
        };
        let first = names.get(members[0]).copied().unwrap_or("");
        out.push(json!({
            "first": first,
            "nodes": members.len(),
            "anchors": anchors.into_iter().collect::<Vec<_>>(),
            "rods": len.len(),
            "rod_len": median(len),
            "min_over_max": median(stretch),
            "relax": median(relax),
            "weight0": median(w0),
            "damping": pick("flPointDamping"),
            "force_attr": pick("flAnimationForceAttraction"),
            "vert_attr": pick("flAnimationVertexAttraction"),
            "gravity": pick("flGravity"),
            "inv_mass": median(members.iter().filter_map(|&i| inv.get(i)).filter_map(Value::as_f64).collect()),
            "coll_radius": median(members.iter().filter_map(|&i| radii.get(i - n_static)).filter_map(Value::as_f64).collect()),
        }));
    }
    out
}

fn main() -> anyhow::Result<()> {
    let piece_mode = std::env::args().any(|a| a == "--pieces");
    for vpk_path in std::env::args().skip(1).filter(|a| a != "--pieces") {
        let vpk = match valve_pak::open(&vpk_path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skip {vpk_path}: {e}");
                continue;
            }
        };
        let paths: Vec<String> = vpk
            .file_paths()
            .filter(|p| p.ends_with(".vmdl_c"))
            .cloned()
            .collect();
        for entry in paths {
            let Ok(mut f) = vpk.get_file(&entry) else {
                continue;
            };
            let Ok(bytes) = f.read_all() else { continue };
            let Ok(res) = Resource::parse(&bytes) else {
                continue;
            };
            let Some(phys) = res.find_block(*b"PHYS") else {
                continue;
            };
            let Ok(root) = morphic::kv3::decode(phys) else {
                eprintln!("PHYS undecodable: {vpk_path} {entry}");
                continue;
            };
            let Some(fe) = fe_of(&root) else { continue };
            if matches!(fe, Value::Null) {
                continue;
            }
            if piece_mode {
                for p in pieces(fe) {
                    println!("{}", json!({ "vpk": vpk_path, "entry": entry, "piece": p }));
                }
                continue;
            }
            let line = json!({
                "vpk": vpk_path,
                "entry": entry,
                "bytes": bytes.len(),
                "fe": shape(fe),
                "red2": provenance(&res),
            });
            println!("{line}");
        }
    }
    Ok(())
}
