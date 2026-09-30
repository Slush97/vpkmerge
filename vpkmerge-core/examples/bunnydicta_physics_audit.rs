use anyhow::Result;
use morphic::kv3::Value;
fn walk(v: &Value) {
    match v {
        Value::Object(items) => {
            for (k, v) in items {
                if k == "m_CtrlName" {
                    println!("Physics controllers: {v:?}");
                } else {
                    walk(v)
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                walk(v)
            }
        }
        _ => (),
    }
}
fn main() -> Result<()> {
    let path = std::env::args().nth(1).unwrap();
    let b = vpkmerge_core::read_vpk_entry(&path, "models/heroes_staging/hornet_v3/hornet.vmdl_c")?;
    let r = morphic::resource::Resource::parse(&b)?;
    if let Some(p) = r.find_block(*b"PHYS") {
        walk(&morphic::kv3::decode(p)?)
    } else {
        println!("No embedded PHYS block")
    };
    Ok(())
}
