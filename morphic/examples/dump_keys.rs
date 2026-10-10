// Print every object key path containing a needle (case-insensitive), with a value preview.
// usage: dump_keys <file.vmdl_c> <needle>
fn walk(v: &morphic::kv3::Value, path: &str, needle: &str, depth: usize) {
    if depth > 8 {
        return;
    }
    match v {
        morphic::kv3::Value::Object(f) => {
            for (k, val) in f {
                let p = format!("{path}/{k}");
                if k.to_lowercase().contains(needle) {
                    let kind = match val {
                        morphic::kv3::Value::Array(a) => format!("array[{}]", a.len()),
                        morphic::kv3::Value::Object(_) => "object".into(),
                        morphic::kv3::Value::String(s) => {
                            format!("str({})", s.chars().take(30).collect::<String>())
                        }
                        other => format!("{other:?}").chars().take(40).collect(),
                    };
                    println!("{p} = {kind}");
                }
                walk(val, &p, needle, depth + 1);
            }
        }
        morphic::kv3::Value::Array(a) => {
            for (i, x) in a.iter().enumerate().take(3) {
                walk(x, &format!("{path}[{i}]"), needle, depth + 1);
            }
        }
        _ => {}
    }
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).unwrap();
    let tree = morphic::decode_kv3_resource(&bytes).unwrap();
    walk(&tree, "", &a[2].to_lowercase(), 0);
}
