//! Source 2 resource container: header + block table.
//!
//! See [`header`] for the binary layout. We only need DATA for texture
//! inspect; RERL/REDI/NTRO are skipped over by ignoring those block entries.

mod header;

pub use header::{Block, Resource};

use crate::error::DecodeError;

pub const BLOCK_TYPE_DATA: [u8; 4] = *b"DATA";

/// Resource header length in bytes: a u32 file size, two u16 versions, then a
/// u32 block-table offset and a u32 block count.
const HEADER_LEN: usize = 16;

impl<'a> Resource<'a> {
    pub fn data_block(&self) -> Result<&'a [u8], DecodeError> {
        self.find_block(BLOCK_TYPE_DATA)
            .ok_or(DecodeError::MissingDataBlock)
    }

    pub fn data_block_meta(&self) -> Result<Block, DecodeError> {
        self.find_block_meta(BLOCK_TYPE_DATA)
            .ok_or(DecodeError::MissingDataBlock)
    }

    /// Rebuild the resource container with the `DATA` block replaced by
    /// `new_data`, keeping every other block (e.g. `RED2` edit/dependency info)
    /// byte-for-byte. The block table is recomputed because the new `DATA` may
    /// differ in size from the original.
    ///
    /// Blocks are laid out in their original order, each 16-byte aligned (the
    /// alignment Valve's own files use). Block offsets in the table are what the
    /// engine reads, so the alignment choice itself is not load-bearing.
    pub fn rebuild_with_data(&self, new_data: &[u8]) -> Result<Vec<u8>, DecodeError> {
        let raw = self.raw();
        let blocks = self.blocks();
        let block_count = blocks.len();
        let resource_version = u16::from_le_bytes([raw[6], raw[7]]);

        // Resolve each block's payload bytes (DATA swapped, others copied).
        let mut payloads: Vec<&[u8]> = Vec::with_capacity(block_count);
        for b in blocks {
            if b.kind == BLOCK_TYPE_DATA {
                payloads.push(new_data);
            } else {
                let start = b.offset as usize;
                let end = start
                    .checked_add(b.size as usize)
                    .ok_or(DecodeError::BadResource("block extent overflow"))?;
                let bytes = raw
                    .get(start..end)
                    .ok_or(DecodeError::BadResource("block out of range"))?;
                payloads.push(bytes);
            }
        }

        // Table sits immediately after the 16-byte header, so block_offset
        // (measured from the field at byte 8) is 8.
        let table_len = block_count * 12;
        let mut cursor = align16(HEADER_LEN + table_len);

        // First pass: absolute payload offsets.
        let mut abs_offsets = Vec::with_capacity(block_count);
        for p in &payloads {
            abs_offsets.push(cursor);
            cursor = align16(cursor + p.len());
        }
        let total_len = cursor;

        let mut out = vec![0u8; total_len];
        out[0..4].copy_from_slice(&u32::try_from(total_len).unwrap_or(0).to_le_bytes());
        out[4..6].copy_from_slice(&12u16.to_le_bytes()); // header_version
        out[6..8].copy_from_slice(&resource_version.to_le_bytes());
        out[8..12].copy_from_slice(&8u32.to_le_bytes()); // block_offset
        out[12..16].copy_from_slice(&u32::try_from(block_count).unwrap_or(0).to_le_bytes());

        for (i, b) in blocks.iter().enumerate() {
            let entry = HEADER_LEN + i * 12;
            out[entry..entry + 4].copy_from_slice(&b.kind);
            let off_field_pos = entry + 4;
            let rel = u32::try_from(abs_offsets[i] - off_field_pos)
                .map_err(|_| DecodeError::BadResource("block rel offset overflow"))?;
            out[off_field_pos..off_field_pos + 4].copy_from_slice(&rel.to_le_bytes());
            let size = u32::try_from(payloads[i].len())
                .map_err(|_| DecodeError::BadResource("block too large"))?;
            out[off_field_pos + 4..off_field_pos + 8].copy_from_slice(&size.to_le_bytes());
        }

        for (off, p) in abs_offsets.iter().zip(&payloads) {
            out[*off..*off + p.len()].copy_from_slice(p);
        }

        Ok(out)
    }
}

fn align16(n: usize) -> usize {
    (n + 15) & !15
}
