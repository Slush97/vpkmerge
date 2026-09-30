// Dump m_modelInfo.m_keyValueText from a compiled .vmdl_c (decodes the resource,
// decompresses the v5 block), optionally grepping for a substring.
// usage: dump_keyvaluetext <file.vmdl_c> [needle]
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
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).unwrap();
    let tree = morphic::decode_kv3_resource(&bytes).unwrap();
    let kvt = find(&tree, "m_keyValueText")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if let Some(needle) = a.get(2) {
        for (i, line) in kvt.lines().enumerate() {
            if line.to_lowercase().contains(&needle.to_lowercase()) {
                // print a little context window
                println!("{}", line.trim_end());
            }
        }
    } else {
        println!("{kvt}");
    }
}
