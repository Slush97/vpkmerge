//! Print the key-path of every string value (or key) in a KV3 resource that
//! contains a needle. Works on any compiled Source 2 resource whose DATA block
//! is KV3 (vdata_c, vsndevts_c, vpcf_c, ...), so it sees through LZ4 string
//! compression that a raw grep misses.
//!
//! Usage: kv3_find <file> <needle>
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
        Value::String(s) => {
            if s.to_lowercase().contains(needle) {
                println!("VAL  {path} = {s}");
            }
        }
        _ => {}
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let needle = a[2].to_lowercase();
    let tree = morphic::decode_kv3_resource(&bytes).expect("decode kv3 resource");
    walk(&tree, "", &needle);
}
