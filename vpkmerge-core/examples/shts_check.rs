// Report a particle texture's VTEX extra-data blocks, in particular whether it
// carries SHEET data (type 2) -- i.e. whether it is a sprite-sheet flipbook whose
// cell grid a renderer samples, or a plain continuous texture.
//
// Repointing a particle from one sheet to another is only apples-to-apples when
// both sides agree on this: swapping a flipbook for a continuous texture (or vice
// versa) changes how the renderer walks UVs.
//
// Layout (see morphic::texture's parse_nonpow2_dims for the same table):
// `u32 extra_data_offset @32`, `u32 extra_data_count @36`; entry table begins at
// `32 + extra_data_offset`; each 12-byte entry is `u32 type, u32 rel_offset, u32 size`.
//
// usage: cargo run -p vpkmerge-core --example shts_check -- <vpk> <entry.vtex_c>...

/// VRF `VTexExtraData` discriminants.
fn extra_data_name(t: u32) -> &'static str {
    match t {
        0 => "UNKNOWN",
        1 => "FALLBACK_BITS",
        2 => "SHEET",
        3 => "FILL_TO_POWER_OF_TWO",
        4 => "COMPRESSED_MIP_SIZE",
        5 => "CUBEMAP_RADIANCE_SH",
        _ => "?",
    }
}

/// Locate the DATA block (the texture header) inside the resource envelope, via
/// the block table at offset 8: `u32 rel_offset @8`, `u32 count @12`, then
/// 12-byte entries of `[4]tag, u32 rel_offset, u32 size` (offset relative to the
/// entry's own offset field).
fn data_block(bytes: &[u8]) -> Option<&[u8]> {
    let u32_at = |o: usize| -> Option<u32> {
        bytes
            .get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let block_offset = u32_at(8)? as usize;
    let block_count = u32_at(12)? as usize;
    let table = 8usize.checked_add(block_offset)?;
    for i in 0..block_count {
        let e = table.checked_add(i.checked_mul(12)?)?;
        let tag = bytes.get(e..e + 4)?;
        let rel = u32_at(e + 4)? as usize;
        let size = u32_at(e + 8)? as usize;
        if tag == b"DATA" {
            let start = e.checked_add(4)?.checked_add(rel)?;
            return bytes.get(start..start.checked_add(size)?);
        }
    }
    None
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 3 {
        eprintln!("usage: shts_check <vpk> <entry.vtex_c>...");
        std::process::exit(2);
    }
    for entry in &a[2..] {
        let bytes = vpkmerge_core::read_vpk_entry(&a[1], entry)?;
        let Some(data) = data_block(&bytes) else {
            println!("{:<10} {entry}  (no DATA block)", "ERR");
            continue;
        };
        let u32_at = |o: usize| -> Option<u32> {
            data.get(o..o + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        };
        let (Some(off), Some(count)) = (u32_at(32), u32_at(36)) else {
            println!("{:<10} {entry}  (short header)", "ERR");
            continue;
        };
        let mut kinds = Vec::new();
        let table_base = 32usize.saturating_add(off as usize);
        for i in 0..count as usize {
            if let Some(t) = u32_at(table_base + i * 12) {
                kinds.push(extra_data_name(t).to_string());
            }
        }
        let sheet = kinds.iter().any(|k| k == "SHEET");
        println!(
            "{:<10} {:<62} extras=[{}]",
            if sheet { "FLIPBOOK" } else { "plain" },
            entry.rsplit('/').next().unwrap_or(entry),
            kinds.join(",")
        );
    }
    Ok(())
}
