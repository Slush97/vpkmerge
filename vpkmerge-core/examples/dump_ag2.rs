use morphic::kv3::Value;
use vpkmerge_core::read_vpk_entry;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let d = morphic::decode_kv3_resource(&read_vpk_entry(&a[1], &a[2])?)?;
    for k in ["m_animGraph2Refs", "m_vecNmSkeletonRefs"] {
        match d.get(k) {
            Some(Value::Array(arr)) => println!("{k}: {} entries", arr.len()),
            Some(o) => println!("{k}: {o:?}"),
            None => println!("{k}: <absent>"),
        }
    }
    Ok(())
}
