use vpkmerge_core::read_vpk_entry;
fn walk(v: &morphic::kv3::Value, path: &str, depth: usize) {
    if depth > 6 {
        return;
    }
    match v {
        morphic::kv3::Value::Object(fields) => {
            for (k, val) in fields {
                let lk = k.to_lowercase();
                if lk.contains("camera") || lk.contains("offset") {
                    match val {
                        morphic::kv3::Value::Object(_) | morphic::kv3::Value::Array(_) => {
                            println!("{path}/{k}: <nested>")
                        }
                        other => println!("{path}/{k} = {other:?}"),
                    }
                }
                walk(val, &format!("{path}/{k}"), depth + 1);
            }
        }
        morphic::kv3::Value::Array(items) => {
            for (i, it) in items.iter().enumerate() {
                walk(it, &format!("{path}[{i}]"), depth + 1);
            }
        }
        _ => {}
    }
}
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let bytes = read_vpk_entry(&a[1], "scripts/heroes.vdata_c")?;
    let data = morphic::decode_kv3_resource(&bytes)?;
    let key = format!("hero_{}", a[2]);
    if let Some(h) = data.get(&key) {
        walk(h, &key, 0);
    }
    Ok(())
}
