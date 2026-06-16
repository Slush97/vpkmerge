//! Prepare soul-container Source 2 source content from a GLB without Blender.
//!
//! Usage:
//!   cargo run -p vpkmerge-core --example prepare_soul_container -- <input.glb> <source_root>

use anyhow::{Context, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let input = PathBuf::from(
        args.next()
            .context("usage: prepare_soul_container <input.glb> <source_root>")?,
    );
    let source_root = PathBuf::from(args.next().context("source_root")?);

    let report = vpkmerge_core::prepare_soul_container_import(
        &input,
        &source_root,
        &vpkmerge_core::SoulContainerImportOptions::default(),
    )?;

    println!("source_root {}", report.source_root.display());
    println!("vmdl {}", report.vmdl_path.display());
    println!("fbx {}", report.fbx_path.display());
    println!(
        "mesh {} verts, {} tris, {} materials",
        report.vertex_count,
        report.triangle_count,
        report.materials.len()
    );
    println!("scale {}", report.scale);
    println!(
        "expected_source_span [{:.6}, {:.6}, {:.6}]",
        report.expected_source_bounds.span[0],
        report.expected_source_bounds.span[1],
        report.expected_source_bounds.span[2]
    );
    for material in &report.materials {
        println!(
            "material {} -> {}",
            material.source_material,
            material.vmat_path.display()
        );
    }
    Ok(())
}
