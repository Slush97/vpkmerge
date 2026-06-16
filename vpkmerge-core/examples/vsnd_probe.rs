//! Probe a `.vsnd_c` container: block table, the KV3 metadata block (CTRL or
//! DATA, whichever is non-empty), and the appended streaming-audio tail.
//!
//! Usage: cargo run --release --example vsnd_probe -- <file.vsnd_c>

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: vsnd_probe <file.vsnd_c>");
    let bytes = std::fs::read(&path)?;
    println!("file: {} ({} bytes)", path, bytes.len());

    let file_size = u32le(&bytes, 0);
    let header_version = u16le(&bytes, 4);
    let resource_version = u16le(&bytes, 6);
    let block_offset = u32le(&bytes, 8);
    let block_count = u32le(&bytes, 12);
    println!(
        "header: file_size={file_size} hver={header_version} rver={resource_version} \
         block_offset={block_offset} block_count={block_count}"
    );
    println!(
        "tail after file_size: {} bytes (likely the MP3)",
        bytes.len() as i64 - file_size as i64
    );

    let table_start = 8 + block_offset as usize;
    let mut cur = table_start;
    let mut meta_block: Option<(String, usize, usize)> = None;
    for _ in 0..block_count {
        let kind = String::from_utf8_lossy(&bytes[cur..cur + 4]).to_string();
        let off_field = cur + 4;
        let rel = u32le(&bytes, off_field) as usize;
        let size = u32le(&bytes, cur + 8) as usize;
        let abs = off_field + rel;
        println!("  block {kind:>4}  abs_offset={abs} size={size}");
        // The sound metadata lives in CTRL (RED2 is compile/dependency info).
        if kind == "CTRL"
            && size > 0
            && bytes.len() >= abs + 4
            && &bytes[abs + 1..abs + 4] == b"3VK"
        {
            meta_block = Some((kind.clone(), abs, size));
        }
        cur += 12;
    }

    if let Some((kind, abs, size)) = meta_block {
        println!("\n== decoding metadata block {kind} (KV3) ==");
        let kv = morphic::kv3::decode(&bytes[abs..abs + size])?;
        println!("{kv:#?}");
    } else {
        println!("\n(no KV3 metadata block found)");
    }
    Ok(())
}
