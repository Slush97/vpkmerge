//! Verify that an outfit preserves original body/gun buffers and uses uncompressed clothing.
use anyhow::{ensure, Context, Result};
fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    ensure!(
        a.len() == 3,
        "usage: bunnydicta_verify_safe original.vpk candidate.vpk"
    );
    let entry = "models/heroes_staging/hornet_v3/hornet.vmdl_c";
    let source = vpkmerge_core::read_vpk_entry(&a[1], entry)?;
    let output = vpkmerge_core::read_vpk_entry(&a[2], entry)?;
    let sr = morphic::resource::Resource::parse(&source)?;
    let out = morphic::resource::Resource::parse(&output)?;
    let before = morphic::model::draw_call_targets(&source)?;
    let after = morphic::model::draw_call_targets(&output)?;
    for dc in before
        .iter()
        .filter(|d| d.mesh_name == "body" || d.mesh_name == "gun")
    {
        let new = after
            .iter()
            .find(|n| n.mesh_name == dc.mesh_name && n.primitive_index == dc.primitive_index)
            .context("missing draw call")?;
        ensure!(
            dc.index_count == new.index_count && dc.material == new.material,
            "draw call changed"
        );
        ensure!(
            sr.get_block_by_index(dc.vertex_block) == out.get_block_by_index(new.vertex_block),
            "original vertex bytes changed"
        );
        ensure!(
            sr.get_block_by_index(dc.index_block) == out.get_block_by_index(new.index_block),
            "original index bytes changed"
        );
        ensure!(
            sr.get_block_by_index(dc.data_block) == out.get_block_by_index(new.data_block),
            "original mesh descriptors changed"
        );
    }
    let targets = morphic::model::vertex_targets(&output)?;
    let cloth = after
        .iter()
        .find(|d| d.mesh_name == "clothes" && d.primitive_index == 0)
        .context("clothing")?;
    ensure!(
        !targets
            .iter()
            .find(|t| t.block_index == cloth.vertex_block)
            .unwrap()
            .meshopt,
        "clothing is compressed"
    );
    let model = morphic::model::decode(&output)?;
    let skel = morphic::model::decode_skeleton(&output)?;
    for mesh in &model.meshes {
        for prim in &mesh.primitives {
            let vb = &mesh.vertex_buffers[prim.vertex_buffer];
            for &idx in &prim.indices {
                let i = idx as usize;
                ensure!(i < vb.positions.len(), "index out of bounds");
                ensure!(
                    vb.positions[i]
                        .iter()
                        .all(|p| p.is_finite() && p.abs() < 200.),
                    "invalid model bounds"
                );
                if mesh.name == "clothes" && prim.material.contains("clothingmaterial") {
                    ensure!(
                        (vb.weights[i].iter().sum::<f32>() - 1.).abs() < 0.02,
                        "invalid weights"
                    );
                    for k in 0..4 {
                        if vb.weights[i][k] > 0.0001 {
                            let name = &skel.bones[vb.joints[i][k] as usize].name;
                            ensure!(
                                !name.contains("hair") && !name.contains("bunnyEar"),
                                "hair-weight contamination"
                            );
                        }
                    }
                }
            }
        }
    }
    println!("PASS: original body/gun vertex bytes, index bytes and mesh descriptors unchanged; clothing uncompressed; indices, bounds and clothing weights valid.");
    Ok(())
}
