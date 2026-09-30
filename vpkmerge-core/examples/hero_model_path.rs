use vpkmerge_core::read_vpk_entry;
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let bytes = read_vpk_entry(&a[1], "scripts/heroes.vdata_c")?;
    let data = morphic::decode_kv3_resource(&bytes)?;
    let key = format!("hero_{}", a[2]);
    match data.get(&key) {
        Some(h) => {
            println!(
                "m_strModelName     = {:?}",
                h.get("m_strModelName").and_then(|v| v.as_str())
            );
            println!(
                "m_bPlayerSelectable= {:?}",
                h.get("m_bPlayerSelectable").and_then(|v| v.as_bool())
            );
            for k in [
                "m_strModelNameLowDetail",
                "m_strAlternativeModelName",
                "m_strModelSkin",
                "m_strHeroAnimGraph",
                "m_strAnimGraphName",
            ] {
                if let Some(v) = h.get(k).and_then(|v| v.as_str()) {
                    println!("{k} = {v:?}");
                }
            }
        }
        None => println!("no {key} in heroes.vdata_c"),
    }
    Ok(())
}
