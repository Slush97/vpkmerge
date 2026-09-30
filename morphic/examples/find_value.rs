//! Print the full key-path to every String/Resource VALUE (or object key) that
//! contains a needle, with a short value preview. Complements dump_keys (which
//! matches keys only).
//! Usage: find_value <file.vmdl_c> <needle>
use morphic::kv3::Value;

fn walk(v: &Value, path: &str, needle: &str) {
    match v {
        Value::Object(f) => {
            for (k, val) in f {
                let p = format!("{path}/{k}");
                if k.to_lowercase().contains(needle) {
                    println!("KEY  {p}");
                }
                walk(val, &p, needle);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                walk(x, &format!("{path}[{i}]"), needle);
            }
        }
        Value::String(s) | Value::Resource(s) => {
            if s.to_lowercase().contains(needle) {
                let prev: String = s.chars().take(60).collect();
                println!("VAL  {path} = {prev:?}");
            }
        }
        _ => {}
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let tree = morphic::decode_kv3_resource(&bytes).expect("decode");
    walk(&tree, "", &a[2].to_lowercase());
}
