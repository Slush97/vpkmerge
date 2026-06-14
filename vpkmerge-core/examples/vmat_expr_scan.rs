//! Find shipped .vmat_c materials that carry dynamic expressions and print
//! each one decompiled, for cross-checking against VRF (oracle dynexpr).
//!
//! usage: cargo run --example vmat_expr_scan -- <pak01_dir.vpk> [limit] [prefix]

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let pak = a
        .next()
        .expect("usage: vmat_expr_scan <pak> [limit] [prefix]");
    let limit: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(30);
    let prefix = a.next().unwrap_or_default();

    let info = vpkmerge_core::inspect(&pak)?;
    let mut hits = 0usize;
    for entry in info
        .file_paths
        .iter()
        .filter(|p| p.ends_with(".vmat_c") && p.starts_with(&prefix))
    {
        let Ok(bytes) = vpkmerge_core::read_vpk_entry(&pak, entry) else {
            continue;
        };
        let Ok(v) = morphic::decode_kv3_resource(&bytes) else {
            continue;
        };
        let attrs: Vec<String> = v
            .get("m_renderAttributesUsed")
            .and_then(morphic::kv3::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        for table in ["m_dynamicParams", "m_dynamicTextureParams"] {
            let Some(morphic::kv3::Value::Array(params)) = v.get(table) else {
                continue;
            };
            for p in params {
                let Some(name) = p.get("m_name").and_then(morphic::kv3::Value::as_str) else {
                    continue;
                };
                let bytes = match p.get("m_value") {
                    Some(morphic::kv3::Value::Binary(b)) => b.clone(),
                    Some(morphic::kv3::Value::Array(items)) => items
                        .iter()
                        .filter_map(|x| x.as_int().and_then(|n| u8::try_from(n).ok()))
                        .collect(),
                    _ => continue,
                };
                let src = morphic::vfx_expr::decompile(&bytes, &attrs)
                    .unwrap_or_else(|e| format!("<error: {e}>"));
                // round-trip self-check on the real blob
                let rt = morphic::vfx_expr::compile(&src)
                    .map(|c| c.bytecode == bytes)
                    .unwrap_or(false);
                let mark = if rt { "ok" } else { "RT-FAIL" };
                println!("{entry}\n  {name} = {src}   [{mark}]");
                if !rt {
                    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                    println!("    hex: {hex}");
                }
            }
        }
        hits += 1;
        if hits >= limit {
            break;
        }
    }
    Ok(())
}
