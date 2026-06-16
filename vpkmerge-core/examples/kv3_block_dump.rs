//! Dump one KV3 block from a Source 2 resource entry.
//!
//! Usage:
//!   cargo run -p vpkmerge-core --example kv3_block_dump -- <vpk> <entry> <BLOCK>

use anyhow::{bail, Context, Result};

fn u32le(bytes: &[u8], offset: usize) -> Result<u32> {
    let end = offset.checked_add(4).context("offset overflow")?;
    let slice = bytes
        .get(offset..end)
        .with_context(|| format!("reading u32 at {offset}"))?;
    Ok(u32::from_le_bytes(slice.try_into()?))
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let vpk_path = args.next().context("vpk")?;
    let entry = args.next().context("entry")?;
    let block = args.next().context("BLOCK")?;
    if block.len() != 4 {
        bail!("BLOCK must be 4 ASCII bytes");
    }
    let block = block.as_bytes();

    let bytes = vpkmerge_core::read_vpk_entry(&vpk_path, &entry)?;
    let table = 8 + usize::try_from(u32le(&bytes, 8)?)?;
    let count = usize::try_from(u32le(&bytes, 12)?)?;
    for index in 0..count {
        let row = table + index * 12;
        let kind = bytes
            .get(row..row + 4)
            .with_context(|| format!("reading block kind {index}"))?;
        if kind != block {
            continue;
        }
        let offset = row + 4 + usize::try_from(u32le(&bytes, row + 4)?)?;
        let size = usize::try_from(u32le(&bytes, row + 8)?)?;
        let payload = bytes
            .get(offset..offset + size)
            .with_context(|| format!("reading block payload {index}"))?;
        let value = morphic::kv3::decode(payload)?;
        println!("{value:#?}");
        return Ok(());
    }
    bail!(
        "block {} not found in {entry}",
        String::from_utf8_lossy(block)
    );
}
