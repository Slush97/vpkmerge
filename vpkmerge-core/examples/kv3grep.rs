//! Print the KV3 field path of every string value containing a substring,
//! for any KV3-resource entry in a VPK (vdata, vpcf, ...).
//!
//! Usage: cargo run --release --example kv3grep -- <vpk> <entry> <substring>

use morphic::kv3::Value;

fn walk(v: &Value, path: &str, needle: &str) {
    match v {
        Value::String(s) if s.contains(needle) => println!("{path} = {s}"),
        Value::Object(fields) => {
            for (k, val) in fields {
                walk(val, &format!("{path}/{k}"), needle);
            }
        }
        Value::Array(items) => {
            for (i, val) in items.iter().enumerate() {
                walk(val, &format!("{path}[{i}]"), needle);
            }
        }
        _ => {}
    }
}

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let vpk = a.next().expect("vpk");
    let entry = a.next().expect("entry");
    let needle = a.next().expect("substring");
    let bytes = vpkmerge_core::read_vpk_entry(&vpk, &entry)?;
    let doc = morphic::decode_kv3_resource(&bytes)?;
    walk(&doc, "", &needle);
    Ok(())
}
