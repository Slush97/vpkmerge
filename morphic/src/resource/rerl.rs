//! `RERL`: a resource's external reference list (VRF `ResourceExtRefList`), the
//! precache list of every resource the file names.
//!
//! Layout: a `u32` offset to the entries (counted from the start of the block),
//! then a `u32` entry count; each 16-byte entry is a `u64` id followed by an
//! `i64` offset (counted from that field) to a NUL-terminated name. Names sit
//! after the entry table in entry order, with no padding.
//!
//! Conventions observed over every `RERL` in pak01 (27K blocks across `.vpcf_c`,
//! `.vmat_c`, `.vmdl_c`, `.vnmgraph_c`, `.vnmclip_c`, ...), all of which
//! [`write_rerl`] reproduces byte for byte:
//! - names are lowercase and uncompiled (`materials/x.vtex`, never `.vtex_c`);
//! - the id is [`resource_id`] of the name exactly as stored;
//! - entries are sorted by name, byte-wise;
//! - Valve's compiler starts the entry table at offset 8 or 12 (four zero bytes
//!   between header and table), whichever 8-byte-aligns the table in the file.
//!   Rebuilt containers place blocks 16-aligned, where 8 is the aligned choice.

use crate::error::DecodeError;

const ENTRIES_AT: usize = 8;

fn murmur64b(data: &[u8], seed: u64) -> u64 {
    const M: u32 = 0x5bd1_e995;
    let mix = |k: u32| {
        let k = k.wrapping_mul(M);
        (k ^ (k >> 24)).wrapping_mul(M)
    };
    let word = |c: &[u8]| u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
    #[allow(clippy::cast_possible_truncation)]
    let (mut h1, mut h2) = ((seed as u32) ^ data.len() as u32, (seed >> 32) as u32);
    let mut rest = data;
    while rest.len() >= 8 {
        h1 = h1.wrapping_mul(M) ^ mix(word(&rest[..4]));
        h2 = h2.wrapping_mul(M) ^ mix(word(&rest[4..8]));
        rest = &rest[8..];
    }
    if rest.len() >= 4 {
        h1 = h1.wrapping_mul(M) ^ mix(word(&rest[..4]));
        rest = &rest[4..];
    }
    if !rest.is_empty() {
        for (i, b) in rest.iter().enumerate() {
            h2 ^= u32::from(*b) << (8 * i);
        }
        h2 = h2.wrapping_mul(M);
    }
    h1 = (h1 ^ (h2 >> 18)).wrapping_mul(M);
    h2 = (h2 ^ (h1 >> 22)).wrapping_mul(M);
    h1 = (h1 ^ (h2 >> 17)).wrapping_mul(M);
    h2 = (h2 ^ (h1 >> 19)).wrapping_mul(M);
    (u64::from(h1) << 32) | u64::from(h2)
}

/// The id a `RERL` entry carries for `name`: `MurmurHash64B` of the name's bytes,
/// seed `0xEDABCDEF`. Pass the name as `RERL` stores it (lowercase, uncompiled).
#[must_use]
pub fn resource_id(name: &str) -> u64 {
    murmur64b(name.as_bytes(), 0xEDAB_CDEF)
}

/// Parse a `RERL` block into `(id, name)` entries, in stored order.
pub fn read_rerl(block: &[u8]) -> Result<Vec<(u64, String)>, DecodeError> {
    parse(block).ok_or(DecodeError::BadResource("malformed RERL block"))
}

fn parse(block: &[u8]) -> Option<Vec<(u64, String)>> {
    let u32_at = |o: usize| -> Option<usize> {
        let b = block.get(o..o.checked_add(4)?)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let (entries, count) = (u32_at(0)?, u32_at(4)?);
    (0..count)
        .map(|i| {
            let e = entries.checked_add(i.checked_mul(16)?)?;
            let field = e.checked_add(8)?;
            let id = u64::from_le_bytes(block.get(e..field)?.try_into().ok()?);
            let rel = i64::from_le_bytes(block.get(field..field.checked_add(8)?)?.try_into().ok()?);
            let at = usize::try_from(i64::try_from(field).ok()?.checked_add(rel)?).ok()?;
            let tail = block.get(at..)?;
            let len = tail.iter().position(|&c| c == 0)?;
            Some((id, String::from_utf8_lossy(&tail[..len]).into_owned()))
        })
        .collect()
}

/// Serialize `(id, name)` entries into a `RERL` block, in the given order. The
/// caller keeps the list sorted by name (see the module docs).
#[must_use]
pub fn write_rerl(refs: &[(u64, String)]) -> Vec<u8> {
    let fit = |n: usize| u32::try_from(n).expect("RERL offsets fit u32");
    let mut out = vec![0u8; ENTRIES_AT + refs.len() * 16];
    out[0..4].copy_from_slice(&fit(ENTRIES_AT).to_le_bytes());
    out[4..8].copy_from_slice(&fit(refs.len()).to_le_bytes());
    for (i, (id, name)) in refs.iter().enumerate() {
        let e = ENTRIES_AT + i * 16;
        out[e..e + 8].copy_from_slice(&id.to_le_bytes());
        let rel = out.len() - (e + 8);
        out[e + 8..e + 16].copy_from_slice(&(rel as u64).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }
    out
}
