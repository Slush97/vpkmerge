use morphic::kv3::Value;
use vpkmerge_core::read_vpk_entry;
fn show(v: &Value, ind: usize) {
    let pad = "  ".repeat(ind);
    match v {
        Value::Object(p) => {
            for (k, val) in p {
                match val {
                    Value::Object(_) | Value::Array(_) => {
                        println!("{pad}{k}:");
                        show(val, ind + 1);
                    }
                    _ => println!("{pad}{k} = {val:?}"),
                }
            }
        }
        Value::Array(a) => {
            for (i, val) in a.iter().enumerate() {
                println!("{pad}[{i}]");
                show(val, ind + 1);
            }
        }
        _ => println!("{pad}{v:?}"),
    }
}
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let d = morphic::decode_kv3_resource(&read_vpk_entry(&a[1], &a[2])?)?;
    for k in ["m_animGraph2Refs", "m_vecNmSkeletonRefs"] {
        println!("=== {k} ===");
        if let Some(v) = d.get(k) {
            show(v, 1);
        } else {
            println!("  <absent>");
        }
    }
    Ok(())
}
