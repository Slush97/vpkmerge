//! Dump the full block table of a Source 2 resource: index, fourcc, offset,
//! size, end, and the gap to the next block (padding). Reveals the alignment
//! Valve uses so a byte-identical reassembler can reproduce it.
//! Usage: dump_block_layout <file.vmdl_c>
fn main() {
    let path = std::env::args().nth(1).expect("file arg");
    let bytes = std::fs::read(&path).expect("read");

    let file_size = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let header_version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let resource_version = u16::from_le_bytes([bytes[6], bytes[7]]);
    let block_offset = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let block_count = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    println!(
        "file_len={} header.file_size={file_size} header_version={header_version} \
         resource_version={resource_version} block_offset={block_offset} block_count={block_count}",
        bytes.len()
    );

    let res = morphic::resource::Resource::parse(&bytes).expect("parse");
    let blocks = res.blocks();
    // Collect (kind, offset, size), sort by offset to compute gaps in file order.
    let mut entries: Vec<(usize, [u8; 4], u32, u32)> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (i, b.kind, b.offset, b.size))
        .collect();
    entries.sort_by_key(|e| e.2);

    let table_start = 8 + block_offset as usize;
    let table_end = table_start + block_count as usize * 12;
    println!("block table: [{table_start}..{table_end}) first payload gap below");

    let mut prev_end = table_end;
    for (decl_idx, kind, off, size) in &entries {
        let k = String::from_utf8_lossy(kind);
        let end = off + size;
        let gap_before = *off as i64 - prev_end as i64;
        println!(
            "decl#{decl_idx:<2} {k} off={off:<9} size={size:<9} end={end:<9} gap_before={gap_before}"
        );
        prev_end = end as usize;
    }
    let tail = bytes.len() as i64 - prev_end as i64;
    println!("tail_after_last_block={tail}");
}
