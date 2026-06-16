//! End-to-end resourcecompiler acceptance gate for the Rust soul-container prep.
//!
//! It prepares source content, runs resourcecompiler through Proton, packs the
//! compiled game addon into a VPK, then inspects the VPK. It intentionally never
//! writes to the installed proof VPK.
//!
//! Usage:
//!   cargo run -p vpkmerge-core --example soul_container_acceptance -- \
//!     <input.glb> <out_dir.vpk> --addon piplup_rust_acceptance

use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const INSTALLED_PROOF_VPK: &str =
    "/home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/addons/pak06_dir.vpk";

fn main() -> Result<()> {
    let args = Args::parse()?;
    guard_output_path(&args.output_vpk)?;

    let temp_source = tempfile::tempdir().context("creating temporary source root")?;
    let options = vpkmerge_core::SoulContainerImportOptions::default();
    let prepared =
        vpkmerge_core::prepare_soul_container_import(&args.glb, temp_source.path(), &options)
            .with_context(|| format!("preparing {}", args.glb.display()))?;

    println!("prepared source_root {}", prepared.source_root.display());
    println!("prepared fbx {}", prepared.fbx_path.display());
    println!(
        "prepared mesh {} verts, {} tris, {} materials",
        prepared.vertex_count,
        prepared.triangle_count,
        prepared.materials.len()
    );
    println!(
        "prepared expected_source_span [{:.6}, {:.6}, {:.6}]",
        prepared.expected_source_bounds.span[0],
        prepared.expected_source_bounds.span[1],
        prepared.expected_source_bounds.span[2]
    );

    let backend = vpkmerge_core::SoulContainerCompileBackend::ResourceCompiler(
        vpkmerge_core::ResourceCompilerBackend {
            addon: args.addon.clone(),
            csdk_root: args.csdk_root.clone(),
            proton: args.proton.clone(),
            steam_root: args.steam_root.clone(),
            proton_prefix: args.proton_prefix.clone(),
            force: true,
            keep_staging: args.keep_staging,
            extra_args: Vec::new(),
        },
    );
    let report = vpkmerge_core::compile_soul_container_source(
        temp_source.path(),
        &options,
        &backend,
        &args.output_vpk,
    )?;
    println!(
        "compiled addon {} packed_entries {} output {}",
        report.addon,
        report.packed_entries,
        report.output_vpk.display()
    );

    validate_output(&args.output_vpk, &prepared, &options)?;
    println!("acceptance ok");
    Ok(())
}

#[derive(Debug)]
struct Args {
    glb: PathBuf,
    output_vpk: PathBuf,
    addon: String,
    csdk_root: PathBuf,
    proton: PathBuf,
    steam_root: PathBuf,
    proton_prefix: PathBuf,
    keep_staging: bool,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut paths = Vec::new();
        let mut addon = format!("vpkmerge_soul_accept_{}", std::process::id());
        let mut csdk_root = PathBuf::from("/home/esoc/csdk12/Reduced_CSDK_12");
        let mut proton = PathBuf::from(
            "/home/esoc/.local/share/Steam/steamapps/common/Proton - Experimental/proton",
        );
        let mut steam_root = PathBuf::from("/home/esoc/.local/share/Steam");
        let mut proton_prefix = PathBuf::from("/tmp/proton-vpkmerge-rc");
        let mut keep_staging = true;

        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--help" || arg == "-h" {
                print_usage();
                std::process::exit(0);
            } else if arg == "--addon" {
                addon = next_value(&mut args, "--addon")?
                    .to_string_lossy()
                    .into_owned();
            } else if arg == "--csdk-root" {
                csdk_root = PathBuf::from(next_value(&mut args, "--csdk-root")?);
            } else if arg == "--proton" {
                proton = PathBuf::from(next_value(&mut args, "--proton")?);
            } else if arg == "--steam-root" {
                steam_root = PathBuf::from(next_value(&mut args, "--steam-root")?);
            } else if arg == "--proton-prefix" {
                proton_prefix = PathBuf::from(next_value(&mut args, "--proton-prefix")?);
            } else if arg == "--discard-staging" {
                keep_staging = false;
            } else if arg.to_string_lossy().starts_with("--") {
                bail!("unknown option: {}", arg.to_string_lossy());
            } else {
                paths.push(PathBuf::from(arg));
            }
        }

        if paths.len() != 2 {
            print_usage();
            bail!("expected input GLB and output VPK path");
        }
        Ok(Self {
            glb: paths.remove(0),
            output_vpk: paths.remove(0),
            addon,
            csdk_root,
            proton,
            steam_root,
            proton_prefix,
            keep_staging,
        })
    }
}

fn next_value(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    name: &str,
) -> Result<std::ffi::OsString> {
    args.next()
        .with_context(|| format!("{name} requires a value"))
}

fn print_usage() {
    eprintln!(
        "usage: soul_container_acceptance <input.glb> <out_dir.vpk> \
         [--addon NAME] [--csdk-root PATH] [--proton PATH] [--steam-root PATH] \
         [--proton-prefix PATH] [--discard-staging]"
    );
}

fn guard_output_path(output_vpk: &Path) -> Result<()> {
    let proof = Path::new(INSTALLED_PROOF_VPK);
    if output_vpk == proof {
        bail!(
            "refusing to overwrite installed proof VPK: {}",
            proof.display()
        );
    }
    if output_vpk.exists()
        && proof.exists()
        && output_vpk.canonicalize()? == proof.canonicalize()?
    {
        bail!(
            "refusing to overwrite installed proof VPK: {}",
            proof.display()
        );
    }
    Ok(())
}

fn validate_output(
    output_vpk: &Path,
    prepared: &vpkmerge_core::SoulContainerPreparedSource,
    options: &vpkmerge_core::SoulContainerImportOptions,
) -> Result<()> {
    let info = vpkmerge_core::inspect(output_vpk)
        .with_context(|| format!("inspecting {}", output_vpk.display()))?;
    let entries: BTreeSet<_> = info.file_paths.into_iter().collect();
    let model_entry = format!("{}/soul_container.vmdl_c", prepared.model_rel);
    if !entries.contains(&model_entry) {
        bail!("missing compiled model entry {model_entry}");
    }

    let vmat_count = entries
        .iter()
        .filter(|entry| {
            entry.starts_with(&format!("{}/materials/", prepared.model_rel))
                && entry.ends_with(".vmat_c")
        })
        .count();
    let vtex_count = entries
        .iter()
        .filter(|entry| entry.ends_with(".vtex_c"))
        .count();
    if vmat_count < prepared.materials.len() {
        bail!(
            "expected at least {} .vmat_c outputs, found {vmat_count}",
            prepared.materials.len()
        );
    }
    if vtex_count < prepared.materials.len() {
        bail!(
            "expected at least {} .vtex_c outputs, found {vtex_count}",
            prepared.materials.len()
        );
    }

    let vpk = valve_pak::open(output_vpk)?;
    let model_bytes = vpk
        .get_file(&model_entry)
        .with_context(|| format!("reading {model_entry} from {}", output_vpk.display()))?
        .read_all()?;
    let model_info = morphic::model::inspect(&model_bytes)?;
    if !model_info.has_embedded_geometry {
        bail!("compiled model has no embedded geometry");
    }

    let model = morphic::model::decode(&model_bytes)
        .context("decoding compiled model; CTRL must contain embedded_meshes")?;
    let draw_calls: usize = model.meshes.iter().map(|mesh| mesh.primitives.len()).sum();
    if draw_calls != prepared.materials.len() {
        bail!(
            "expected {} draw calls/material slots, decoded {draw_calls}",
            prepared.materials.len()
        );
    }

    let materials = model.materials();
    if materials.len() != prepared.materials.len() {
        bail!(
            "expected {} material refs, decoded {}: {:?}",
            prepared.materials.len(),
            materials.len(),
            materials
        );
    }
    for material in &materials {
        if !is_source_relative_material(material, &prepared.model_rel) {
            bail!("material ref is not Source-relative for this model: {material}");
        }
    }

    let bounds = model
        .position_bounds()
        .context("decoded model has no position bounds")?;
    let span = [
        bounds.max[0] - bounds.min[0],
        bounds.max[1] - bounds.min[1],
        bounds.max[2] - bounds.min[2],
    ];
    let largest = span.into_iter().fold(0.0_f32, f32::max);
    if (largest - options.target_largest_axis).abs() > 0.05 {
        bail!(
            "largest bounds axis mismatch: expected {:.3}, got {:.6} span={:?}",
            options.target_largest_axis,
            largest,
            span
        );
    }

    println!(
        "validated embedded geometry: mesh_parts={} draw_calls={} materials={} bounds_span={:?} vmat_c={} vtex_c={}",
        model_info.mesh_parts,
        draw_calls,
        materials.len(),
        span,
        vmat_count,
        vtex_count
    );
    Ok(())
}

fn is_source_relative_material(material: &str, model_rel: &str) -> bool {
    material.starts_with(&format!("{model_rel}/materials/"))
        && material.ends_with(".vmat")
        && !material.starts_with('/')
        && !material.starts_with('\\')
        && !material.contains(':')
        && !material.contains("//")
}
