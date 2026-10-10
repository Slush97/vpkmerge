// Dump m_attachments (name + influence bone(s) + offset/rotation) from a compiled .vmdl_c.
// usage: dump_attachments <file.vmdl_c>
fn find<'a>(v: &'a morphic::kv3::Value, key: &str) -> Option<&'a morphic::kv3::Value> {
    match v {
        morphic::kv3::Value::Object(f) => {
            for (k, val) in f {
                if k == key {
                    return Some(val);
                }
                if let Some(r) = find(val, key) {
                    return Some(r);
                }
            }
            None
        }
        morphic::kv3::Value::Array(a) => a.iter().find_map(|x| find(x, key)),
        _ => None,
    }
}
fn s(v: &morphic::kv3::Value) -> String {
    match v {
        morphic::kv3::Value::String(x) | morphic::kv3::Value::Resource(x) => x.clone(),
        morphic::kv3::Value::Array(a) => {
            format!("[{}]", a.iter().map(s).collect::<Vec<_>>().join(","))
        }
        morphic::kv3::Value::Double(d) => format!("{d:.3}"),
        morphic::kv3::Value::Int(i) => i.to_string(),
        other => format!("{other:?}").chars().take(40).collect(),
    }
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).unwrap();
    let tree = morphic::decode_kv3_resource(&bytes).unwrap();
    let att = find(&tree, "m_attachments");
    match att {
        Some(morphic::kv3::Value::Array(items)) => {
            println!("m_attachments: {} entries", items.len());
            for it in items {
                let name = find(it, "m_name").map(s).unwrap_or_default();
                let infl = find(it, "m_influenceNames").map(s).unwrap_or_default();
                let off = find(it, "m_vInfluenceOffsets").map(s).unwrap_or_default();
                let rot = find(it, "m_vInfluenceRotations").map(s).unwrap_or_default();
                println!("  {name:20} infl={infl} off={off} rot={rot}");
            }
        }
        _ => println!("no m_attachments array found (att={:?})", att.is_some()),
    }
}
