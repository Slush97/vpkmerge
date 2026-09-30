//! Decode-search every KV3-bearing resource in a VPK for a string needle.
//!
//! A raw byte grep misses anything in an LZ4-compressed KV3 block (most
//! `.vdata_c` / `.vpcf_c` string tables), so this decodes each resource's DATA
//! block and walks the tree. Prints `entry\tkeypath = value` per hit.
//!
//! Usage: kv3_scan_vpk <vpk> <ext,ext,...> <needle>
use anyhow::Result;
use morphic::kv3::Value;

fn walk(v: &Value, path: &str, needle: &str, entry: &str) {
    match v {
        Value::Object(f) => {
            for (k, val) in f {
                walk(val, &format!("{path}/{k}"), needle, entry);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                walk(x, &format!("{path}[{i}]"), needle, entry);
            }
        }
        Value::String(s) => {
            if s.to_lowercase().contains(needle) {
                println!("{entry}\t{path} = {s}");
            }
        }
        _ => {}
    }
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let vpk_path = &a[1];
    let exts: Vec<String> = a[2].split(',').map(str::to_string).collect();
    let needle = a[3].to_lowercase();

    let vpk = valve_pak::open(vpk_path)?;
    let mut files: Vec<String> = vpk
        .file_paths()
        .filter(|p| exts.iter().any(|e| p.ends_with(e.as_str())))
        .cloned()
        .collect();
    files.sort();
    eprintln!("scanning {} entries", files.len());

    let mut failed = 0usize;
    for entry in &files {
        let Ok(bytes) = vpkmerge_core::read_vpk_entry(vpk_path, entry) else {
            failed += 1;
            continue;
        };
        match morphic::decode_kv3_resource(&bytes) {
            Ok(tree) => walk(&tree, "", &needle, entry),
            Err(_) => failed += 1,
        }
    }
    eprintln!("{} entries could not be decoded", failed);
    Ok(())
}
