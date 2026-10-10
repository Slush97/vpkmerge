// Dev throwaway: report whether each named resource's KV3 block carries binary
// blobs, and probe whether a blob-bearing material accepts a *new* dynamic
// expression param (the only route to a colour channel whose static lane is a
// tagless KV3 zero and so has no bytes to patch).
//
// usage: blobcheck <vpk> <entry.vmat_c>...            report blob status
//        blobcheck <vpk> --try-expr <entry.vmat_c> <PARAM> <EXPR>
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    if a.get(2).map(String::as_str) == Some("--try-expr") {
        let (entry, param, src) = (&a[3], &a[4], &a[5]);
        let bytes = vpkmerge_core::read_vpk_entry(&a[1], entry)?;
        println!(
            "before: blobs={}",
            morphic::kv3_resource_has_blobs(&bytes).unwrap_or(true)
        );
        let edit = vpkmerge_core::VmatEdit::expr(param.clone(), src)?;
        let (patched, stats) = vpkmerge_core::patch_vmat_params(&bytes, &[edit])?;
        println!(
            "patch: set={} inserted={} failed={:?}",
            stats.set, stats.inserted, stats.failed
        );
        if !stats.failed.is_empty() {
            anyhow::bail!("refused");
        }
        // Re-decode and read the expression back out, which is the offline
        // equivalent of asking the engine whether it can still parse the block.
        let tree = morphic::decode_kv3_resource(&patched)
            .map_err(|e| anyhow::anyhow!("re-decode failed: {e}"))?;
        let dyn_params = tree
            .get("m_dynamicParams")
            .and_then(morphic::kv3::Value::as_array)
            .map(<[morphic::kv3::Value]>::to_vec)
            .unwrap_or_default();
        println!("m_dynamicParams now has {} entries", dyn_params.len());
        let attrs: Vec<String> = tree
            .get("m_renderAttributesUsed")
            .and_then(morphic::kv3::Value::as_array)
            .map(|v| {
                v.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        for p in &dyn_params {
            let name = p
                .get("m_name")
                .and_then(morphic::kv3::Value::as_str)
                .unwrap_or("?");
            let blob = match p.get("m_value") {
                Some(morphic::kv3::Value::Binary(b)) => b.clone(),
                _ => {
                    println!("  {name}: <not a blob>");
                    continue;
                }
            };
            match morphic::vfx_expr::decompile(&blob, &attrs) {
                Ok(src) => println!("  {name} = {src}"),
                Err(e) => println!("  {name} = <decompile failed: {e}>"),
            }
        }
        println!(
            "after: blobs={} bytes {} -> {}",
            morphic::kv3_resource_has_blobs(&patched).unwrap_or(true),
            bytes.len(),
            patched.len()
        );
        // The two size fields whose drift makes Source 2 reject the KV3 outright
        // ("Bad KV3 data"), which is what a blobbed-material edit got wrong before.
        // morphic guards these inside its blob-replace path; the array-insert path
        // is a different one, so check the produced block directly.
        let block = morphic::resource::Resource::parse(&patched)
            .map_err(|e| anyhow::anyhow!("parse: {e}"))?
            .data_block()
            .map_err(|e| anyhow::anyhow!("data block: {e}"))?
            .to_vec();
        let i32_at = |o: usize| -> i64 {
            i64::from(i32::from_le_bytes([
                block[o],
                block[o + 1],
                block[o + 2],
                block[o + 3],
            ]))
        };
        let (total_unc, unc1, unc2) = (i32_at(48), i32_at(72), i32_at(80));
        let (total_comp, comp1, comp2) = (i32_at(52), i32_at(76), i32_at(84));
        println!(
            "  sizeUncTotal@48  = {total_unc}   unc1+unc2  = {}",
            unc1 + unc2
        );
        println!(
            "  sizeCompTotal@52 = {total_comp}   comp1+comp2 = {}",
            comp1 + comp2
        );
        anyhow::ensure!(total_unc == unc1 + unc2, "sizeUncTotal != unc1+unc2");
        anyhow::ensure!(total_comp == comp1 + comp2, "sizeCompTotal != comp1+comp2");
        println!("  v5 size invariants OK");
        return Ok(());
    }
    for e in &a[2..] {
        let b = vpkmerge_core::read_vpk_entry(&a[1], e)?;
        println!(
            "{:>6}  {e}",
            if morphic::kv3_resource_has_blobs(&b).unwrap_or(true) {
                "BLOBS"
            } else {
                "plain"
            }
        );
    }
    Ok(())
}
