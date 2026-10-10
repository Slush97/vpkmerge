// Dump EVERY numeric array (len 3-4) in a .vpcf_c with its path, whether int
// (0-255) or float (0-1) - so gradient/float colors that readcolor (int-only)
// misses are visible. Usage: cargo run --example dump_colors -- <vpk> <entry>
use morphic::kv3::Value;

fn nums(v: &Value) -> Option<(Vec<f64>, bool)> {
    let Value::Array(items) = v else { return None };
    if items.len() != 3 && items.len() != 4 {
        return None;
    }
    let mut out = Vec::new();
    let mut any_float = false;
    for it in items {
        match it {
            Value::Int(n) => out.push(*n as f64),
            Value::UInt(u) => out.push(*u as f64),
            Value::Double(d) => {
                any_float = true;
                out.push(*d);
            }
            _ => return None,
        }
    }
    Some((out, any_float))
}

fn walk(v: &Value, path: &str, out: &mut Vec<(String, Vec<f64>, bool)>) {
    if let Some((c, f)) = nums(v) {
        out.push((path.to_string(), c, f));
        return;
    }
    match v {
        Value::Object(p) => {
            for (k, c) in p {
                walk(c, &format!("{path}/{k}"), out);
            }
        }
        Value::Array(items) => {
            for (i, it) in items.iter().enumerate() {
                walk(it, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let bytes = vpkmerge_core::read_vpk_entry(&args[1], &args[2])?;
    let tree = morphic::decode_kv3_resource(&bytes)?;
    let mut out = Vec::new();
    walk(&tree, "", &mut out);
    for (path, c, is_float) in &out {
        let kind = if *is_float { "float" } else { "int  " };
        // flag a guess at the visual color
        let v: Vec<String> = c.iter().map(|x| format!("{x:.3}")).collect();
        println!("[{kind}] {path} = [{}]", v.join(", "));
    }
    eprintln!("{} numeric arrays in {}", out.len(), args[2]);
    Ok(())
}
