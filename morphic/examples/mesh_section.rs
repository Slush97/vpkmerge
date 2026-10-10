//! Dump the DATA mesh-section fields that a binary mesh-swap must reconcile.
//! Usage: mesh_section <file.vmdl_c>
use morphic::kv3::Value;

fn shape(v: &Value) -> String {
    match v {
        Value::Array(a) => format!("array[{}]", a.len()),
        Value::Object(o) => format!("object{{{}}}", o.len()),
        Value::Int(x) => format!("Int({x})"),
        Value::UInt(x) => format!("UInt({x})"),
        Value::Double(x) => format!("Double({x})"),
        Value::String(s) => format!("String({s:?})"),
        Value::Resource(s) => format!("Resource({s:?})"),
        other => format!("{other:?}").chars().take(40).collect(),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let t = morphic::decode_kv3_resource(&bytes).expect("decode");
    for k in [
        "m_refMeshes",
        "m_refMeshGroupMasks",
        "m_refLODGroupMasks",
        "m_lodGroupSwitchDistances",
        "m_meshGroups",
        "m_materialGroups",
        "m_nDefaultMeshGroupMask",
        "m_remappingTable",
        "m_remappingTableStarts",
        "m_refPhysGroupMasks",
    ] {
        match t.get(k) {
            Some(v) => println!("{k:28} = {}", shape(v)),
            None => println!("{k:28} = <absent>"),
        }
    }
    // m_refMeshes: show each entry (block ref + name?)
    if let Some(rm) = t.get("m_refMeshes").and_then(Value::as_array) {
        println!("\n-- m_refMeshes ({}) --", rm.len());
        for (i, m) in rm.iter().enumerate() {
            println!("  [{i}] = {}", shape(m));
            if let Some(o) = m.as_object() {
                for (k, v) in o {
                    println!("       {k} = {}", shape(v));
                }
            }
        }
    }
    // m_remappingTableStarts: the per-mesh slice boundaries
    if let Some(rs) = t.get("m_remappingTableStarts").and_then(Value::as_array) {
        let vals: Vec<String> = rs
            .iter()
            .map(|x| {
                x.as_int()
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| "?".into())
            })
            .collect();
        println!("\n-- m_remappingTableStarts = [{}]", vals.join(", "));
    }
    if let Some(mg) = t.get("m_meshGroups").and_then(Value::as_array) {
        println!(
            "-- m_meshGroups: {:?}",
            mg.iter()
                .map(|x| x.as_str().unwrap_or("?"))
                .collect::<Vec<_>>()
        );
    }
    // block layout
    let res = morphic::resource::Resource::parse(&bytes).expect("parse");
    let mut kinds: Vec<(String, u32, u32)> = res
        .blocks()
        .iter()
        .map(|b| {
            (
                String::from_utf8_lossy(&b.kind).to_string(),
                b.offset,
                b.size,
            )
        })
        .collect();
    kinds.sort_by_key(|x| x.1);
    println!("\n-- blocks ({}) --", kinds.len());
    for (k, off, sz) in &kinds {
        println!("  {k} off={off} size={sz}");
    }
}
