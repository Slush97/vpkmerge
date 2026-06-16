//! Dump the MDAT block draw calls of a .vmdl_c file (path, not vpk).
//! Usage: cargo run --release --example mdat_dump -- <file.vmdl_c>
use morphic::kv3::Value;

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// Manually locate a named block's bytes from the resource header (the resource
/// module is private; mirror examples/resource_blocks.rs).
fn find_block<'a>(b: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let block_offset = rd_u32(b, 8) as usize;
    let block_count = rd_u32(b, 12);
    let mut p = 8 + block_offset;
    for _ in 0..block_count {
        let ty = &b[p..p + 4];
        let rel = rd_u32(b, p + 4) as usize;
        let abs = p + 4 + rel;
        let size = rd_u32(b, p + 8) as usize;
        if ty == kind {
            return Some(&b[abs..abs + size]);
        }
        p += 12;
    }
    None
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("file.vmdl_c");
    let bytes = std::fs::read(&path)?;
    let mdat = find_block(&bytes, b"MDAT").expect("no MDAT");
    let v = morphic::kv3::decode(mdat)?;
    let sos = v
        .get("m_sceneObjects")
        .and_then(Value::as_array)
        .expect("no scene objects");
    println!("scene objects: {}", sos.len());
    for (si, so) in sos.iter().enumerate() {
        let dcs = so
            .get("m_drawCalls")
            .and_then(Value::as_array)
            .unwrap_or(&[]);
        println!("  so[{si}] draw calls: {}", dcs.len());
        for (di, dc) in dcs.iter().enumerate() {
            println!("  -- dc[{di}] --");
            if let Value::Object(pairs) = dc {
                for (k, val) in pairs {
                    let short: String = match val {
                        Value::Object(_) | Value::Array(_) => {
                            format!("{val:?}").chars().take(100).collect()
                        }
                        _ => format!("{val:?}"),
                    };
                    println!("       {k} = {short}");
                }
            }
        }
    }
    Ok(())
}
