//! Scan a VPK for .vmat_c materials carrying dynamic expressions
//! (non-empty m_dynamicParams / m_dynamicTextureParams) and dump the
//! param names + compiled bytecode as hex.
//!
//! usage: cargo run --release --example scan_dynexpr -- <pak_dir.vpk> [entry-substring] [--hex]

use morphic::kv3::Value;

fn dyn_params(root: &Value, table: &str) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    if let Some(Value::Array(items)) = root.get(table) {
        for it in items {
            let name = it
                .get("m_name")
                .and_then(Value::as_str)
                .unwrap_or("?")
                .to_string();
            let bytes = match it.get("m_value") {
                Some(Value::Binary(b)) => b.clone(),
                Some(Value::Array(a)) => a
                    .iter()
                    .filter_map(|v| v.as_int().and_then(|i| u8::try_from(i).ok()))
                    .collect(),
                _ => Vec::new(),
            };
            out.push((name, bytes));
        }
    }
    out
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let pak = args
        .next()
        .expect("usage: scan_dynexpr <pak_dir.vpk> [filter] [--hex]");
    let rest: Vec<String> = args.collect();
    let show_hex = rest.iter().any(|a| a == "--hex");
    let filter = rest.iter().find(|a| !a.starts_with("--")).cloned();

    let info = vpkmerge_core::inspect(&pak)?;
    let mats: Vec<&String> = info
        .file_paths
        .iter()
        .filter(|p| p.ends_with(".vmat_c"))
        .filter(|p| filter.as_ref().is_none_or(|f| p.contains(f.as_str())))
        .collect();
    eprintln!("{} candidate materials", mats.len());

    let mut hits = 0usize;
    for (i, entry) in mats.iter().enumerate() {
        if i % 500 == 0 {
            eprintln!("  ...{i}/{}", mats.len());
        }
        let Ok(bytes) = vpkmerge_core::read_vpk_entry(&pak, entry) else {
            continue;
        };
        let Ok(root) = morphic::decode_kv3_resource(&bytes) else {
            continue;
        };
        let shader = root
            .get("m_shaderName")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let dp = dyn_params(&root, "m_dynamicParams");
        let dtp = dyn_params(&root, "m_dynamicTextureParams");
        if dp.is_empty() && dtp.is_empty() {
            continue;
        }
        hits += 1;
        println!("== {entry}  [{shader}]");
        for (tag, list) in [("param", &dp), ("texparam", &dtp)] {
            for (name, code) in list {
                println!("   {tag} {name}  ({} bytes)", code.len());
                if show_hex {
                    let hex: String = code.iter().map(|b| format!("{b:02x}")).collect();
                    println!("     {hex}");
                }
            }
        }
    }
    eprintln!("{hits} materials with dynamic expressions");
    Ok(())
}
