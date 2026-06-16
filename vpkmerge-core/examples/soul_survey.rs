// Survey how soul-container mods are actually built: for each VPK, the entry
// layout, and for its soul_container.vmdl_c the vertex count, referenced
// materials (does it reuse the stock material or ship its own?), and whether the
// vertex buffers are meshopt-compressed (the tell for "Valve resourcecompiler
// output" vs "could be assembled raw").
//
// usage: cargo run --release --example soul_survey -- <dir.vpk>...
use anyhow::{Context, Result};
use morphic::kv3::Value;

const MODEL: &str = "models/props_gameplay/soul_container/soul_container.vmdl_c";

fn read_all(vpk: &valve_pak::VPK, entry: &str) -> Result<Vec<u8>> {
    let mut f = vpk
        .get_file(entry)
        .with_context(|| format!("entry {entry} not found"))?;
    Ok(f.read_all()?)
}

fn u32_at(d: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?) as usize)
}

/// Extract a named Source 2 block's payload bytes. Each block-table entry stores
/// its payload offset relative to the offset field's own position.
fn block_payload(d: &[u8], tag: &[u8; 4]) -> Option<Vec<u8>> {
    let block_offset = u32_at(d, 8)?;
    let block_count = u32_at(d, 12)?;
    let base = 8 + block_offset;
    for i in 0..block_count {
        let off = base + i * 12;
        if d.get(off..off + 4)? == tag {
            let off_field = off + 4;
            let start = off_field + u32_at(d, off_field)?;
            let size = u32_at(d, off + 8)?;
            return Some(d.get(start..start + size)?.to_vec());
        }
    }
    None
}

/// Walk the CTRL block KV3 and report whether ANY embedded vertex buffer has
/// `m_bMeshoptCompressed = true`.
fn any_meshopt(ctrl: &Value) -> bool {
    fn walk(v: &Value, found: &mut bool) {
        match v {
            Value::Object(pairs) => {
                for (k, child) in pairs {
                    if k == "m_bMeshoptCompressed" {
                        if let Some(b) = child.as_bool() {
                            *found |= b;
                        }
                    }
                    walk(child, found);
                }
            }
            Value::Array(items) => items.iter().for_each(|c| walk(c, found)),
            _ => {}
        }
    }
    let mut found = false;
    walk(ctrl, &mut found);
    found
}

fn main() -> Result<()> {
    for path in std::env::args().skip(1) {
        println!("==== {path} ====");
        let vpk = match valve_pak::open(&path) {
            Ok(v) => v,
            Err(e) => {
                println!("  open failed: {e}\n");
                continue;
            }
        };
        let mut entries: Vec<String> = vpk.file_paths().cloned().collect();
        entries.sort();
        println!("  entries ({}):", entries.len());
        for e in &entries {
            println!("    {e}");
        }

        let Ok(model_bytes) = read_all(&vpk, MODEL) else {
            println!("  (no soul_container.vmdl_c)\n");
            continue;
        };
        match morphic::model::decode(&model_bytes) {
            Ok(m) => {
                println!("  vmdl: {} bytes", model_bytes.len());
                println!("    vertices : {}", m.total_vertices());
                println!("    materials: {:?}", m.materials());
            }
            Err(e) => println!("  vmdl decode failed: {e}"),
        }
        // CTRL block -> meshopt flag.
        match block_payload(&model_bytes, b"CTRL") {
            Some(ctrl_bytes) => match morphic::kv3::decode(&ctrl_bytes) {
                Ok(ctrl) => println!("    meshopt  : {}", any_meshopt(&ctrl)),
                Err(e) => println!("    meshopt  : (CTRL decode failed: {e})"),
            },
            None => println!("    meshopt  : (no CTRL block)"),
        }
        println!();
    }
    Ok(())
}
