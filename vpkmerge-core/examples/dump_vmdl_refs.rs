use morphic::kv3::Value;
use vpkmerge_core::read_vpk_entry;
fn walk(v: &Value, path: &str, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(p) => {
            for (k, val) in p {
                let np = format!("{path}/{k}");
                if let Some(s) = val.as_str() {
                    if !s.is_empty() {
                        out.push((np.clone(), s.to_string()));
                    }
                }
                walk(val, &np, out);
            }
        }
        Value::Array(a) => {
            for (i, val) in a.iter().enumerate() {
                walk(val, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let b = read_vpk_entry(&a[1], &a[2])?;
    let d = morphic::decode_kv3_resource(&b)?;
    if let Value::Object(p) = &d {
        eprintln!(
            "top-level DATA keys: {:?}",
            p.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>()
        );
    }
    let mut out = vec![];
    walk(&d, "", &mut out);
    for (k, s) in &out {
        if s.contains("anim")
            || s.contains("skel")
            || s.contains("graph")
            || s.contains("prefab")
            || s.contains("gamedata")
            || k.to_lowercase().contains("anim")
            || k.to_lowercase().contains("skel")
            || k.to_lowercase().contains("graph")
        {
            println!("{k} = {s}");
        }
    }
    Ok(())
}
