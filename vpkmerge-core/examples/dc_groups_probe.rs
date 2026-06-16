//! Probe: expand the stock single-draw-call soul_container.vmdl_c into N draw
//! calls via morphic::model::set_draw_call_groups, and verify the result decodes
//! to N draw calls with the expected materials + index ranges.
//!
//! Run: cargo run --release --example dc_groups_probe -- <pak01_dir.vpk>

use morphic::model::{draw_call_targets, set_draw_call_groups, DrawCallGroup};

const MODEL: &str = "models/props_gameplay/soul_container/soul_container.vmdl_c";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pak = std::env::args().nth(1).expect("arg1: pak01_dir.vpk");
    let vpk = valve_pak::open(&pak)?;
    let bytes = vpk
        .get_file(MODEL)
        .expect("soul_container in pak")
        .read_all()?;

    let before = draw_call_targets(&bytes)?;
    println!("before: {} draw call(s)", before.len());
    for d in &before {
        println!(
            "  start={} count={} applied={} verts={} mat={}",
            d.start_index, d.index_count, d.applied_index_offset, d.vertex_count, d.material
        );
    }
    let total_idx = before[0].index_count;
    let total_vtx = before[0].vertex_count;

    // Split the single index buffer into 3 contiguous slices (must be multiples
    // of 3 to stay triangle-aligned).
    let third = (total_idx / 9) * 3;
    let groups = vec![
        DrawCallGroup {
            material: "models/props_gameplay/soul_container/materials/aaa.vmat".into(),
            start_index: 0,
            index_count: third,
            vertex_start: 0,
            vertex_end: total_vtx,
        },
        DrawCallGroup {
            material: "models/props_gameplay/soul_container/materials/bbb.vmat".into(),
            start_index: third,
            index_count: third,
            vertex_start: 0,
            vertex_end: total_vtx,
        },
        DrawCallGroup {
            material: "models/props_gameplay/soul_container/materials/ccc.vmat".into(),
            start_index: third * 2,
            index_count: total_idx - third * 2,
            vertex_start: 0,
            vertex_end: total_vtx,
        },
    ];

    let out = set_draw_call_groups(&bytes, &groups, total_vtx)?;
    let after = draw_call_targets(&out)?;
    println!("\nafter: {} draw call(s)", after.len());
    for d in &after {
        println!(
            "  start={} count={} applied={} verts={} base={} mat={}",
            d.start_index,
            d.index_count,
            d.applied_index_offset,
            d.vertex_count,
            d.base_vertex,
            d.material
        );
    }

    assert_eq!(after.len(), 3, "expected 3 draw calls");
    for (i, (d, g)) in after.iter().zip(&groups).enumerate() {
        assert_eq!(d.material, g.material, "dc[{i}] material");
        assert_eq!(d.start_index, g.start_index, "dc[{i}] start");
        assert_eq!(d.index_count, g.index_count, "dc[{i}] count");
    }
    // Slices must tile the whole index buffer with no gap/overlap.
    let covered: usize = after.iter().map(|d| d.index_count).sum();
    assert_eq!(
        covered, total_idx,
        "slices must cover the whole index buffer"
    );
    println!("\nOK: 3 draw calls, materials + ranges correct, slices tile [0,{total_idx})");
    Ok(())
}
