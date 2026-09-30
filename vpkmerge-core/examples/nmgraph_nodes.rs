//! Print a compiled NM animgraph (`.vnmgraph_c`) one node per line:
//! `idx path class field=value ...`, plus control parameters and resources.
//!
//! Usage: cargo run --release -p vpkmerge-core --example nmgraph_nodes -- <vpk> <entry>

use morphic::kv3::Value;

fn short(v: &Value) -> String {
    match v {
        Value::Int(i) => i.to_string(),
        Value::Double(d) => d.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Array(a) => format!("[{}]", a.iter().map(short).collect::<Vec<_>>().join(",")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{k}={}", short(v)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        other => format!("{other:?}"),
    }
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let doc = morphic::decode_kv3_resource(&vpkmerge_core::read_vpk_entry(&a[1], &a[2])?)?;
    let arr = |k: &str| match doc.get(k) {
        Some(Value::Array(v)) => v.clone(),
        _ => Vec::new(),
    };
    for (i, p) in arr("m_controlParameterIDs").iter().enumerate() {
        println!("param {i}: {}", short(p));
    }
    for (i, r) in arr("m_resources").iter().enumerate() {
        println!("res {i}: {}", short(r));
    }
    let paths = arr("m_nodePaths");
    for (i, n) in arr("m_nodes").iter().enumerate() {
        let Value::Object(fields) = n else { continue };
        let class = n.get("_class").map(short).unwrap_or_default();
        let rest: Vec<String> = fields
            .iter()
            .filter(|(k, _)| k != "_class" && k != "m_nNodeIdx")
            .map(|(k, v)| format!("{}={}", k.trim_start_matches("m_"), short(v)))
            .collect();
        let path = paths.get(i).map(short).unwrap_or_default();
        println!("{i} [{path}] {class} {}", rest.join(" "));
    }
    Ok(())
}
