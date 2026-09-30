//! Replace a model's draw-call group from a donor `.glb` and pack an addon VPK,
//! writing the rebuilt buffers **uncompressed**.
//!
//! This is the engine-safe twin of `vpkmerge model edit --replace-group`, which
//! goes through [`morphic::model::replace_mesh_group`] and re-emits morphic's
//! meshopt codec v1. Deadlock's meshopt decoder garbles that payload, so a
//! model whose buffers are meshopt-compressed (every Deadlock hero) renders as
//! scrambled geometry in game even though it round-trips fine offline. The
//! uncompressed variant flips `m_bMeshoptCompressed` to false and stores raw
//! buffers, which the engine reads natively -- the same discipline the vertex
//! colour recolor path uses.
//!
//! Usage:
//!   replace_group_uncompressed <vpk> <base_vpk|-> <entry> <material_substr> <donor.glb> <out_dir.vpk>

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 6 {
        bail!(
            "usage: replace_group_uncompressed <vpk> <base_vpk|-> <entry> <material_substr> \
             <donor.glb> <out_dir.vpk>"
        );
    }
    let (vpk, base_arg, entry, needle, glb, out) =
        (&a[0], &a[1], a[2].as_str(), a[3].as_str(), &a[4], &a[5]);
    let base: Option<PathBuf> = (base_arg != "-").then(|| PathBuf::from(base_arg));
    let base_ref: Option<&Path> = base.as_deref();

    let inspection = vpkmerge_core::model::inspect_model_parts(vpk, entry, base_ref)?;
    let selected: Vec<_> = inspection
        .draw_calls
        .iter()
        .filter(|c| c.material.to_lowercase().contains(&needle.to_lowercase()))
        .collect();
    if selected.is_empty() {
        bail!(
            "no draw call whose material contains {needle:?}; materials: {}",
            inspection
                .draw_calls
                .iter()
                .map(|c| c.material.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for c in &selected {
        println!(
            "selected mesh {} (mesh_index {}, primitive {}) material {} \
             [{} verts, {} indices]",
            c.mesh_name, c.mesh_index, c.primitive_index, c.material, c.vertex_count, c.index_count
        );
    }

    let selections: Vec<morphic::model::PrimitiveSelection> = selected
        .iter()
        .map(|c| morphic::model::PrimitiveSelection {
            mesh_index: c.mesh_index,
            primitive_index: c.primitive_index,
        })
        .collect();

    let bytes = read_model(vpk, base_ref, entry)?;
    let glb_bytes = std::fs::read(glb).with_context(|| format!("reading {glb}"))?;
    let donors = morphic::model::read_edited_primitives(&glb_bytes)
        .context("reading donor primitives from glb")?;
    println!(
        "donor glb: {} primitive(s), {} verts, {} indices",
        donors.len(),
        donors[0].vertex_buffer.element_count,
        donors[0].indices.len()
    );

    let (edited, report) =
        morphic::model::replace_mesh_group_uncompressed(&bytes, &selections, &donors)
            .with_context(|| format!("replacing group in {entry}"))?;

    for part in &report.replaced_parts {
        println!(
            "rebuilt part {:?} ({}): {} -> {} verts",
            part.mesh_name, part.material, part.old_vertex_count, part.new_vertex_count
        );
    }
    println!(
        "{} draw call(s) replaced; model {} -> {} bytes",
        report.replaced_draw_calls,
        bytes.len(),
        edited.len()
    );

    vpkmerge_core::pack(&[(entry, edited.as_slice())], out)
        .with_context(|| format!("packing {out}"))?;
    println!("packed {entry} into {out}");
    Ok(())
}

/// Reads the model entry, preferring the mod VPK and falling back to the base.
fn read_model(vpk: &str, base: Option<&Path>, entry: &str) -> Result<Vec<u8>> {
    if let Ok(bytes) = vpkmerge_core::read_vpk_entry(vpk, entry) {
        return Ok(bytes);
    }
    let base = base.context("model entry not in vpk and no base given")?;
    vpkmerge_core::read_vpk_entry(base, entry)
        .with_context(|| format!("model entry {entry} not found in vpk or base"))
}
