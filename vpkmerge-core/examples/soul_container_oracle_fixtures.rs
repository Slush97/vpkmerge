//! Freeze CSDK12 compiler-oracle fixtures for the constrained soul-container path.
//!
//! This writes one persistent fixture directory per GLB:
//!
//! ```text
//! <out-dir>/<case>/
//!   input.glb
//!   source/                 # Rust-prepared source tree
//!   compiled_game/          # resourcecompiler output tree
//!   <addon>_dir.vpk
//!   v0sanity.txt
//!   oracle.json             # material refs, VTEX formats, bounds, draw counts
//! ```
//!
//! Usage:
//!   cargo run -p vpkmerge-core --example soul_container_oracle_fixtures -- \
//!     --out-dir /tmp/soul-oracle \
//!     --case piplup=/home/esoc/Downloads/piplup.glb \
//!     --case cinna=/home/esoc/Downloads/75ee040c5394475481652b9064889728.glb \
//!     --force

use anyhow::{bail, Context, Result};
use serde_json::{json, Value as Json};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const GUARDED_INSTALLED_VPKS: &[&str] = &[
    "/home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/addons/pak06_dir.vpk",
    "/home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/addons/pak38_dir.vpk",
];

fn main() -> Result<()> {
    let args = Args::parse()?;
    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("creating {}", args.out_dir.display()))?;

    for case in &args.cases {
        freeze_case(&args, case)?;
    }

    println!(
        "froze {} oracle fixture(s) under {}",
        args.cases.len(),
        args.out_dir.display()
    );
    Ok(())
}

#[derive(Debug)]
struct Args {
    out_dir: PathBuf,
    cases: Vec<Case>,
    csdk_root: PathBuf,
    proton: PathBuf,
    steam_root: PathBuf,
    proton_prefix: PathBuf,
    force: bool,
    keep_csdk_staging: bool,
}

#[derive(Debug)]
struct Case {
    name: String,
    glb: PathBuf,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut out_dir = None;
        let mut cases = Vec::new();
        let mut csdk_root = PathBuf::from("/home/esoc/csdk12/Reduced_CSDK_12");
        let mut proton = PathBuf::from(
            "/home/esoc/.local/share/Steam/steamapps/common/Proton - Experimental/proton",
        );
        let mut steam_root = PathBuf::from("/home/esoc/.local/share/Steam");
        let mut proton_prefix = PathBuf::from("/tmp/proton-vpkmerge-rc");
        let mut force = false;
        let mut keep_csdk_staging = false;

        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--help" || arg == "-h" {
                print_usage();
                std::process::exit(0);
            } else if arg == "--out-dir" {
                out_dir = Some(PathBuf::from(next_value(&mut args, "--out-dir")?));
            } else if arg == "--case" {
                cases.push(parse_case(next_value(&mut args, "--case")?)?);
            } else if arg == "--csdk-root" {
                csdk_root = PathBuf::from(next_value(&mut args, "--csdk-root")?);
            } else if arg == "--proton" {
                proton = PathBuf::from(next_value(&mut args, "--proton")?);
            } else if arg == "--steam-root" {
                steam_root = PathBuf::from(next_value(&mut args, "--steam-root")?);
            } else if arg == "--proton-prefix" {
                proton_prefix = PathBuf::from(next_value(&mut args, "--proton-prefix")?);
            } else if arg == "--force" {
                force = true;
            } else if arg == "--keep-csdk-staging" {
                keep_csdk_staging = true;
            } else if arg.to_string_lossy().starts_with("--") {
                bail!("unknown option: {}", arg.to_string_lossy());
            } else {
                let glb = PathBuf::from(&arg);
                let name = glb
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(safe_case_name)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("case_{}", cases.len() + 1));
                cases.push(Case { name, glb });
            }
        }

        let out_dir = out_dir.context("--out-dir is required")?;
        if cases.is_empty() {
            print_usage();
            bail!("at least one --case NAME=GLB or positional GLB is required");
        }

        Ok(Self {
            out_dir,
            cases,
            csdk_root,
            proton,
            steam_root,
            proton_prefix,
            force,
            keep_csdk_staging,
        })
    }
}

fn freeze_case(args: &Args, case: &Case) -> Result<()> {
    require_file(&case.glb, "input GLB")?;
    let case_dir = args.out_dir.join(&case.name);
    if case_dir.exists() {
        if !args.force {
            bail!(
                "fixture case already exists; pass --force to replace it: {}",
                case_dir.display()
            );
        }
        std::fs::remove_dir_all(&case_dir)
            .with_context(|| format!("removing {}", case_dir.display()))?;
    }
    std::fs::create_dir_all(&case_dir)
        .with_context(|| format!("creating {}", case_dir.display()))?;

    let input_copy = case_dir.join("input.glb");
    std::fs::copy(&case.glb, &input_copy)
        .with_context(|| format!("copying {} -> {}", case.glb.display(), input_copy.display()))?;

    let options = vpkmerge_core::SoulContainerImportOptions::default();
    let source_root = case_dir.join("source");
    let prepared = vpkmerge_core::prepare_soul_container_import(&case.glb, &source_root, &options)
        .with_context(|| format!("preparing {}", case.glb.display()))?;

    let addon = format!("vpkmerge_soul_oracle_{}", safe_case_name(&case.name));
    let output_vpk = case_dir.join(format!("{addon}_dir.vpk"));
    guard_output_path(&output_vpk)?;

    let backend = vpkmerge_core::SoulContainerCompileBackend::ResourceCompiler(
        vpkmerge_core::ResourceCompilerBackend {
            addon: addon.clone(),
            csdk_root: args.csdk_root.clone(),
            proton: args.proton.clone(),
            steam_root: args.steam_root.clone(),
            proton_prefix: args.proton_prefix.clone(),
            force: true,
            keep_staging: true,
            extra_args: Vec::new(),
        },
    );

    let report =
        vpkmerge_core::compile_soul_container_source(&source_root, &options, &backend, &output_vpk)
            .with_context(|| format!("compiling oracle case {}", case.name))?;

    let compiled_fixture = case_dir.join("compiled_game");
    copy_tree(&report.compiled_root, &compiled_fixture)?;

    let (oracle, v0sanity) =
        inspect_oracle(case, &input_copy, &prepared, &report, &compiled_fixture)?;
    let oracle_path = case_dir.join("oracle.json");
    let oracle_bytes = serde_json::to_vec_pretty(&oracle)?;
    std::fs::write(&oracle_path, oracle_bytes)
        .with_context(|| format!("writing {}", oracle_path.display()))?;

    std::fs::write(case_dir.join("v0sanity.txt"), v0sanity)
        .with_context(|| format!("writing {}", case_dir.join("v0sanity.txt").display()))?;

    if !args.keep_csdk_staging {
        let content_addon = args
            .csdk_root
            .join("content")
            .join("citadel_addons")
            .join(&addon);
        std::fs::remove_dir_all(content_addon).ok();
        std::fs::remove_dir_all(&report.compiled_root).ok();
    }

    println!(
        "{}: packed {} entries -> {}",
        case.name,
        report.packed_entries,
        output_vpk.display()
    );
    Ok(())
}

fn inspect_oracle(
    case: &Case,
    input_copy: &Path,
    prepared: &vpkmerge_core::SoulContainerPreparedSource,
    report: &vpkmerge_core::SoulContainerCompileReport,
    compiled_fixture: &Path,
) -> Result<(Json, String)> {
    let vpk = valve_pak::open(&report.output_vpk)
        .with_context(|| format!("opening {}", report.output_vpk.display()))?;
    let mut entries: Vec<String> = vpk.file_paths().cloned().collect();
    entries.sort();
    let entry_set: BTreeSet<_> = entries.iter().cloned().collect();

    let model_entry = format!("{}/soul_container.vmdl_c", prepared.model_rel);
    if !entry_set.contains(&model_entry) {
        bail!("missing compiled model entry {model_entry}");
    }
    let model_bytes = vpk
        .get_file(&model_entry)
        .with_context(|| format!("reading {model_entry} from {}", report.output_vpk.display()))?
        .read_all()?;
    let model_info = morphic::model::inspect(&model_bytes)?;
    let model = morphic::model::decode(&model_bytes)?;
    let v0sanity = v0sanity_text(&model_entry, &model);
    let draw_calls: usize = model.meshes.iter().map(|mesh| mesh.primitives.len()).sum();
    let bounds = model
        .position_bounds()
        .context("decoded model has no position bounds")?;
    let bounds_span = [
        bounds.max[0] - bounds.min[0],
        bounds.max[1] - bounds.min[1],
        bounds.max[2] - bounds.min[2],
    ];
    let material_refs = model.materials();

    let vmat_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.ends_with(".vmat_c"))
        .cloned()
        .collect();
    let vtex_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.ends_with(".vtex_c"))
        .cloned()
        .collect();

    let compiled_materials = vmat_entries
        .iter()
        .map(|entry| inspect_material(&vpk, entry))
        .collect::<Result<Vec<_>>>()?;
    let compiled_textures = vtex_entries
        .iter()
        .map(|entry| inspect_texture(&vpk, entry))
        .collect::<Result<Vec<_>>>()?;

    let oracle = json!({
        "case": case.name,
        "input_glb": input_copy,
        "addon": report.addon,
        "source_root": prepared.source_root,
        "compiled_game_root": compiled_fixture,
        "csdk_compiled_root": report.compiled_root,
        "vpk": report.output_vpk,
        "packed_entries": report.packed_entries,
        "entries": entries,
        "prepared": {
            "model_rel": prepared.model_rel,
            "vertex_count": prepared.vertex_count,
            "triangle_count": prepared.triangle_count,
            "scale": prepared.scale,
            "imported_bounds": bounds_json(&prepared.imported_bounds),
            "fbx_bounds": bounds_json(&prepared.fbx_bounds),
            "expected_source_bounds": bounds_json(&prepared.expected_source_bounds),
            "materials": prepared.materials.iter().map(|m| json!({
                "name": m.name,
                "source_material": m.source_material,
                "vmat": m.vmat_path,
                "color_texture": m.color_texture_path,
            })).collect::<Vec<_>>(),
        },
        "model": {
            "entry": model_entry,
            "has_embedded_geometry": model_info.has_embedded_geometry,
            "mesh_parts": model_info.mesh_parts,
            "index_buffers": model_info.index_buffers,
            "has_physics": model_info.has_physics,
            "has_skeleton_anim": model_info.has_skeleton_anim,
            "vertex_bytes": model_info.vertex_bytes,
            "total_vertices": model.total_vertices(),
            "gltf_vertex_total": model.gltf_vertex_total(),
            "total_indices": model.total_indices(),
            "draw_calls": draw_calls,
            "material_refs": material_refs,
            "bounds": {
                "min": bounds.min,
                "max": bounds.max,
                "span": bounds_span,
                "largest_axis": bounds_span.into_iter().fold(0.0_f32, f32::max),
            },
            "blocks": model_info.blocks.iter().map(|b| json!({
                "kind": b.kind,
                "size": b.size,
            })).collect::<Vec<_>>(),
        },
        "materials": {
            "count": vmat_entries.len(),
            "entries": compiled_materials,
        },
        "textures": {
            "count": vtex_entries.len(),
            "entries": compiled_textures,
        },
    });
    Ok((oracle, v0sanity))
}

fn inspect_material(vpk: &valve_pak::VPK, entry: &str) -> Result<Json> {
    let bytes = vpk
        .get_file(entry)
        .with_context(|| format!("reading {entry}"))?
        .read_all()?;
    match morphic::material::parse(&bytes) {
        Ok(mat) => Ok(json!({
            "entry": entry,
            "material_name": mat.name,
            "shader": mat.shader_name,
            "texture_params": mat.texture_params,
            "int_params": mat.int_params,
            "float_params": mat.float_params,
            "vector_params": mat.vector_params,
        })),
        Err(err) => Ok(json!({
            "entry": entry,
            "parse_error": err.to_string(),
        })),
    }
}

fn inspect_texture(vpk: &valve_pak::VPK, entry: &str) -> Result<Json> {
    let bytes = vpk
        .get_file(entry)
        .with_context(|| format!("reading {entry}"))?
        .read_all()?;
    match morphic::inspect(&bytes) {
        Ok(info) => Ok(json!({
            "entry": entry,
            "format": info.format.name(),
            "width": info.width,
            "height": info.height,
            "depth": info.depth,
            "mip_count": info.mip_count,
            "flags": info.flags.bits(),
        })),
        Err(err) => Ok(json!({
            "entry": entry,
            "parse_error": err.to_string(),
        })),
    }
}

fn bounds_json(bounds: &vpkmerge_core::SoulContainerBounds) -> Json {
    json!({
        "min": bounds.min,
        "max": bounds.max,
        "span": bounds.span,
        "largest_axis": bounds.span.into_iter().fold(0.0_f32, f32::max),
    })
}

fn v0sanity_text(entry: &str, model: &morphic::model::Model) -> String {
    let mut out = format!(
        "{entry}: {} meshes, {} total verts\n",
        model.meshes.len(),
        model.total_vertices()
    );
    if let Some(bb) = model.position_bounds() {
        let span = [
            bb.max[0] - bb.min[0],
            bb.max[1] - bb.min[1],
            bb.max[2] - bb.min[2],
        ];
        out.push_str(&format!("  bounds min={:?} max={:?}\n", bb.min, bb.max));
        out.push_str(&format!(
            "  span={:?}  (a horse/knight should be tens of source units, finite, not 0/NaN/huge)\n",
            span
        ));
    }
    for mesh in &model.meshes {
        for vb in &mesh.vertex_buffers {
            let p = &vb.positions;
            let n = &vb.normals;
            out.push_str(&format!(
                "  mesh {} vb: {} verts  pos[0..2]={:?}  normal[0]={:?}\n",
                mesh.name,
                p.len(),
                &p[..p.len().min(2)],
                n.first()
            ));
            let bad = p
                .iter()
                .filter(|q| q.iter().any(|c| !c.is_finite()))
                .count();
            if bad > 0 {
                out.push_str(&format!(
                    "    !! {bad} non-finite positions (v0 decode broken)\n"
                ));
            }
            break;
        }
    }
    out
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    require_dir(src, "source directory")?;
    std::fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_tree(&src_path, &dst_path)?;
        } else if ty.is_file() {
            if let Some(parent) = dst_path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            std::fs::copy(&src_path, &dst_path).with_context(|| {
                format!("copying {} -> {}", src_path.display(), dst_path.display())
            })?;
        }
    }
    Ok(())
}

fn parse_case(raw: OsString) -> Result<Case> {
    let s = raw.to_string_lossy();
    let Some((name, path)) = s.split_once('=') else {
        bail!("--case must be NAME=GLB, got {s}");
    };
    let name = safe_case_name(name);
    if name.is_empty() {
        bail!("--case name must contain at least one ASCII alphanumeric");
    }
    Ok(Case {
        name,
        glb: PathBuf::from(path),
    })
}

fn safe_case_name(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_underscore = false;
    for b in raw.bytes().map(|b| b.to_ascii_lowercase()) {
        if b.is_ascii_alphanumeric() {
            out.push(char::from(b));
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    out.trim_matches('_').to_string()
}

fn guard_output_path(output_vpk: &Path) -> Result<()> {
    for guarded in GUARDED_INSTALLED_VPKS {
        let guarded = Path::new(guarded);
        if output_vpk == guarded {
            bail!("refusing to overwrite installed VPK: {}", guarded.display());
        }
        if output_vpk.exists()
            && guarded.exists()
            && output_vpk.canonicalize()? == guarded.canonicalize()?
        {
            bail!("refusing to overwrite installed VPK: {}", guarded.display());
        }
    }
    Ok(())
}

fn require_file(path: &Path, label: &str) -> Result<()> {
    if !path.is_file() {
        bail!("{label} not found: {}", path.display());
    }
    Ok(())
}

fn require_dir(path: &Path, label: &str) -> Result<()> {
    if !path.is_dir() {
        bail!("{label} not found: {}", path.display());
    }
    Ok(())
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
        "usage: soul_container_oracle_fixtures --out-dir DIR \
         --case NAME=INPUT.glb [--case NAME=INPUT.glb ...] \
         [--csdk-root PATH] [--proton PATH] [--steam-root PATH] \
         [--proton-prefix PATH] [--force] [--keep-csdk-staging]"
    );
}
