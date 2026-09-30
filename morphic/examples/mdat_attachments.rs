// Print every MDAT block's CRenderMesh.m_attachments (name, influences, offsets,
// rotations). usage: mdat_attachments <file.vmdl_c> [name-filter]
use morphic::kv3::Value;

fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(f) => f
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
            .or_else(|| f.iter().find_map(|(_, v)| find(v, key))),
        Value::Array(a) => a.iter().find_map(|x| find(x, key)),
        _ => None,
    }
}

fn s(v: &Value) -> String {
    match v {
        Value::String(x) => x.clone(),
        Value::Array(a) => format!("[{}]", a.iter().map(s).collect::<Vec<_>>().join(",")),
        Value::Double(d) => format!("{d:.4}"),
        Value::Int(i) => i.to_string(),
        other => format!("{other:?}"),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let filter = a.get(2).cloned().unwrap_or_default();
    let bytes = std::fs::read(&a[1]).unwrap();
    let res = morphic::resource::Resource::parse(&bytes).unwrap();
    for (i, b) in res.blocks().iter().enumerate() {
        if &b.kind != b"MDAT" {
            continue;
        }
        let payload = res.get_block_by_index(i).unwrap();
        let Ok(tree) = morphic::kv3::decode(payload) else {
            println!("block {i}: MDAT not KV3-decodable");
            continue;
        };
        let Some(Value::Array(items)) = find(&tree, "m_attachments") else {
            println!("block {i}: no m_attachments");
            continue;
        };
        println!("block {i}: {} attachments", items.len());
        for it in items {
            let name = find(it, "m_name").map(s).unwrap_or_default();
            if !filter.is_empty() && !filter.split(',').any(|f| name == f) {
                continue;
            }
            println!(
                "  {name:18} infl={} off={} rot={}",
                find(it, "m_influenceNames").map(s).unwrap_or_default(),
                find(it, "m_vInfluenceOffsets").map(s).unwrap_or_default(),
                find(it, "m_vInfluenceRotations").map(s).unwrap_or_default()
            );
        }
    }
}
