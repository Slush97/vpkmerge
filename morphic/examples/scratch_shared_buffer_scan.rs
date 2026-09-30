// Throwaway: find parts whose draw calls SHARE one vertex buffer, and dump the
// range fields so we can copy Valve's convention.
// usage: cargo run -p morphic --example scratch_shared_buffer_scan -- <vpk> <entry>...
use std::collections::HashMap;
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(Path::new(&a[1])).unwrap();
    let mut shown = 0;
    for entry in &a[2..] {
        let Ok(bytes) = vpk.get_file(entry).and_then(|mut f| f.read_all()) else {
            continue;
        };
        let Ok(dcs) = morphic::model::draw_call_targets(&bytes) else {
            continue;
        };
        // group by (mesh, vertex buffer)
        let mut by: HashMap<(String, usize), Vec<_>> = HashMap::new();
        for dc in &dcs {
            by.entry((dc.mesh_name.clone(), dc.vertex_block))
                .or_default()
                .push(dc);
        }
        for ((mesh, _), calls) in by {
            if calls.len() < 2 {
                continue;
            }
            println!(
                "{entry}  part={mesh}  {} draw calls share one buffer",
                calls.len()
            );
            for dc in &calls {
                println!(
                    "   start_index={:<8} index_count={:<8} applied_offset={:<8} vertex_count={:<8} base_vertex={:<6} mat={}",
                    dc.start_index,
                    dc.index_count,
                    dc.applied_index_offset,
                    dc.vertex_count,
                    dc.base_vertex,
                    dc.material.rsplit('/').next().unwrap_or("")
                );
            }
            shown += 1;
            if shown >= 6 {
                return;
            }
        }
    }
    if shown == 0 {
        println!("no shared-buffer multi-draw-call parts found");
    }
}
