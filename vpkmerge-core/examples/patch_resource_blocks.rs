//! Copy named Source 2 resource blocks from one VPK entry into another resource.
//!
//! Usage:
//!   cargo run -p vpkmerge-core --example patch_resource_blocks -- \
//!     <target.vpk> <target-entry> <donor.vpk> <donor-entry> <out-file> RERL RED2

use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
struct Block {
    kind: [u8; 4],
    offset: usize,
    size: usize,
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .context("u16 read out of range")?
            .try_into()?,
    ))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .context("u32 read out of range")?
            .try_into()?,
    ))
}

fn parse_blocks(bytes: &[u8]) -> Result<Vec<Block>> {
    if bytes.len() < 16 {
        bail!("resource too short");
    }
    let header_version = u16_at(bytes, 4)?;
    if header_version != 12 {
        bail!("unexpected resource header version {header_version}");
    }
    let block_table_start = 8usize
        .checked_add(u32_at(bytes, 8)? as usize)
        .context("block table offset overflow")?;
    let block_count = u32_at(bytes, 12)? as usize;
    let mut blocks = Vec::with_capacity(block_count);
    for i in 0..block_count {
        let entry = block_table_start + i * 12;
        let mut kind = [0u8; 4];
        kind.copy_from_slice(
            bytes
                .get(entry..entry + 4)
                .context("block kind out of range")?,
        );
        let rel = u32_at(bytes, entry + 4)? as usize;
        let size = u32_at(bytes, entry + 8)? as usize;
        let offset = (entry + 4)
            .checked_add(rel)
            .context("block offset overflow")?;
        bytes
            .get(offset..offset + size)
            .with_context(|| format!("block {} out of range", kind_string(kind)))?;
        blocks.push(Block { kind, offset, size });
    }
    Ok(blocks)
}

fn align16(n: usize) -> usize {
    (n + 15) & !15
}

fn kind_string(kind: [u8; 4]) -> String {
    String::from_utf8_lossy(&kind).into_owned()
}

fn parse_kind(raw: &str) -> Result<[u8; 4]> {
    let bytes = raw.as_bytes();
    if bytes.len() != 4 {
        bail!("block kind must be exactly 4 bytes: {raw}");
    }
    Ok([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn block_payload<'a>(bytes: &'a [u8], blocks: &[Block], kind: [u8; 4]) -> Result<&'a [u8]> {
    let block = blocks
        .iter()
        .find(|b| b.kind == kind)
        .with_context(|| format!("missing block {}", kind_string(kind)))?;
    Ok(&bytes[block.offset..block.offset + block.size])
}

fn rebuild(target: &[u8], replacements: &BTreeMap<[u8; 4], Vec<u8>>) -> Result<Vec<u8>> {
    let blocks = parse_blocks(target)?;
    let resource_version = u16_at(target, 6)?;
    let block_count = blocks.len();
    let table_len = block_count
        .checked_mul(12)
        .context("block table length overflow")?;
    let mut cursor = align16(16 + table_len);
    let mut payloads: Vec<(&[u8], [u8; 4])> = Vec::with_capacity(block_count);
    for block in &blocks {
        let payload = replacements
            .get(&block.kind)
            .map(Vec::as_slice)
            .unwrap_or(&target[block.offset..block.offset + block.size]);
        payloads.push((payload, block.kind));
    }
    let mut offsets = Vec::with_capacity(block_count);
    for (payload, _) in &payloads {
        offsets.push(cursor);
        cursor = align16(cursor + payload.len());
    }

    let total_len = cursor;
    let mut out = vec![0u8; total_len];
    out[0..4].copy_from_slice(&(total_len as u32).to_le_bytes());
    out[4..6].copy_from_slice(&12u16.to_le_bytes());
    out[6..8].copy_from_slice(&resource_version.to_le_bytes());
    out[8..12].copy_from_slice(&8u32.to_le_bytes());
    out[12..16].copy_from_slice(&(block_count as u32).to_le_bytes());
    for (i, ((payload, kind), offset)) in payloads.iter().zip(&offsets).enumerate() {
        let entry = 16 + i * 12;
        out[entry..entry + 4].copy_from_slice(kind);
        let off_field = entry + 4;
        out[off_field..off_field + 4]
            .copy_from_slice(&((*offset - off_field) as u32).to_le_bytes());
        out[off_field + 4..off_field + 8].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        out[*offset..*offset + payload.len()].copy_from_slice(payload);
    }
    Ok(out)
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let target_vpk = args.next().context("target.vpk")?;
    let target_entry = args.next().context("target-entry")?;
    let donor_vpk = args.next().context("donor.vpk")?;
    let donor_entry = args.next().context("donor-entry")?;
    let out = PathBuf::from(args.next().context("out-file")?);
    let kinds: Vec<[u8; 4]> = args.map(|a| parse_kind(&a)).collect::<Result<_>>()?;
    if kinds.is_empty() {
        bail!("provide at least one block kind");
    }

    let target = vpkmerge_core::read_vpk_entry(&target_vpk, &target_entry)
        .with_context(|| format!("reading target {target_entry}"))?;
    let donor = vpkmerge_core::read_vpk_entry(&donor_vpk, &donor_entry)
        .with_context(|| format!("reading donor {donor_entry}"))?;
    let donor_blocks = parse_blocks(&donor)?;

    let mut replacements = BTreeMap::new();
    for kind in kinds {
        let payload = block_payload(&donor, &donor_blocks, kind)?.to_vec();
        println!("copy {} ({} bytes)", kind_string(kind), payload.len());
        replacements.insert(kind, payload);
    }

    let rebuilt = rebuild(&target, &replacements)?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, &rebuilt)?;
    println!("wrote {} ({} bytes)", out.display(), rebuilt.len());
    Ok(())
}
