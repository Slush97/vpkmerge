use morphic::kv3::Value;
use vpkmerge_core::read_vpk_entry;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let b = read_vpk_entry(&a[1], &a[2])?;
    let d = morphic::decode_kv3_resource(&b)?;
    for key in [
        "m_refMeshes",
        "m_refAnimGroups",
        "m_meshGroups",
        "m_modelArchetype",
        "m_nFlags",
        "m_refPhysicsData",
    ] {
        if let Some(v) = d.get(key) {
            match v {
                Value::Array(arr) => {
                    println!("{key} = [{} entries]", arr.len());
                    for e in arr.iter().take(6) {
                        if let Some(s) = e.as_str() {
                            println!("    {s}");
                        } else {
                            println!("    {e:?}");
                        }
                    }
                }
                other => println!("{key} = {other:?}"),
            }
        } else {
            println!("{key} = <absent>");
        }
    }
    Ok(())
}
