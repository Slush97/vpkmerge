//! Probe the partial pure Rust soul-container material/texture compiler.
//!
//! This prepares a GLB, emits generated `.vmat_c` and `.vtex_c` files into a
//! loose compiled tree, packs the same partial tree into a VPK, and inspects the
//! generated resources with morphic. It intentionally does not emit `.vmdl_c`.
//!
//! Usage:
//!   cargo run -p vpkmerge-core --example soul_container_pure_probe -- \
//!     <input.glb> <compiled_root> <out_dir.vpk> [--oracle oracle.json] [--force]
//!   cargo run -p vpkmerge-core --example soul_container_pure_probe -- \
//!     --source-root <source_root> <compiled_root> <out_dir.vpk> [--oracle oracle.json] [--force]

use anyhow::{bail, Context, Result};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const GUARDED_INSTALLED_VPKS: &[&str] = &[
    "/home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/addons/pak06_dir.vpk",
    "/home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/addons/pak38_dir.vpk",
];

fn main() -> Result<()> {
    let args = Args::parse()?;
    guard_output_path(&args.output_vpk)?;
    guard_compiled_root(&args.compiled_root)?;

    let options = vpkmerge_core::SoulContainerImportOptions::default();

    let report = if let Some(source_root) = &args.source_root {
        println!("using prepared source_root {}", source_root.display());
        vpkmerge_core::compile_soul_container_source_pure_rust(
            source_root,
            &options,
            &args.compiled_root,
            &args.output_vpk,
            args.force,
        )?
    } else {
        let glb = args.glb.as_ref().context("input GLB is required")?;
        let source_root = tempfile::tempdir().context("creating temporary source root")?;
        let prepared =
            vpkmerge_core::prepare_soul_container_import(glb, source_root.path(), &options)
                .with_context(|| format!("preparing {}", glb.display()))?;

        println!(
            "prepared {} verts, {} tris, {} materials",
            prepared.vertex_count,
            prepared.triangle_count,
            prepared.materials.len()
        );
        println!(
            "expected_source_span [{:.6}, {:.6}, {:.6}]",
            prepared.expected_source_bounds.span[0],
            prepared.expected_source_bounds.span[1],
            prepared.expected_source_bounds.span[2]
        );

        vpkmerge_core::compile_soul_container_prepared_pure_rust(
            &prepared,
            &args.compiled_root,
            &args.output_vpk,
            args.force,
        )?
    };
    println!(
        "pure partial packed {} entries -> {}",
        report.packed_entries,
        report.output_vpk.display()
    );
    println!("loose compiled tree {}", report.compiled_root.display());

    let generated = inspect_generated(&report.output_vpk)?;
    println!(
        "generated summary: {} materials, {} textures",
        generated.materials.len(),
        generated.textures.len()
    );
    for texture in &generated.textures {
        println!(
            "  {} {}x{} {} mips={} flags=0x{:04x}",
            texture.entry,
            texture.width,
            texture.height,
            texture.format,
            texture.mip_count,
            texture.flags
        );
    }
    compare_oracle(args.oracle.as_deref(), &generated)?;
    println!("pure probe ok: material/texture resources parse");
    Ok(())
}

#[derive(Debug)]
struct Args {
    glb: Option<PathBuf>,
    source_root: Option<PathBuf>,
    compiled_root: PathBuf,
    output_vpk: PathBuf,
    oracle: Option<PathBuf>,
    force: bool,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut paths = Vec::new();
        let mut source_root = None;
        let mut oracle = None;
        let mut force = false;

        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--help" || arg == "-h" {
                print_usage();
                std::process::exit(0);
            } else if arg == "--source-root" {
                source_root = Some(PathBuf::from(next_value(&mut args, "--source-root")?));
            } else if arg == "--oracle" {
                oracle = Some(PathBuf::from(next_value(&mut args, "--oracle")?));
            } else if arg == "--force" {
                force = true;
            } else if arg.to_string_lossy().starts_with("--") {
                bail!("unknown option: {}", arg.to_string_lossy());
            } else {
                paths.push(PathBuf::from(arg));
            }
        }

        let (glb, compiled_root, output_vpk) = if source_root.is_some() {
            if paths.len() != 2 {
                print_usage();
                bail!("expected compiled root and output VPK path with --source-root");
            }
            (None, paths.remove(0), paths.remove(0))
        } else {
            if paths.len() != 3 {
                print_usage();
                bail!("expected input GLB, compiled root, and output VPK path");
            }
            (Some(paths.remove(0)), paths.remove(0), paths.remove(0))
        };

        if source_root.is_some() && glb.is_some() {
            print_usage();
            bail!("use either an input GLB or --source-root, not both");
        }
        Ok(Self {
            glb,
            source_root,
            compiled_root,
            output_vpk,
            oracle,
            force,
        })
    }
}

#[derive(Debug)]
struct GeneratedMaterial {
    entry: String,
    material_name: String,
    shader: String,
    texture_params: BTreeMap<String, String>,
    int_params: BTreeMap<String, i64>,
    vector_params: BTreeMap<String, [f32; 4]>,
}

#[derive(Debug)]
struct GeneratedTexture {
    entry: String,
    format: &'static str,
    width: u16,
    height: u16,
    mip_count: u8,
    flags: u16,
}

#[derive(Debug)]
struct GeneratedReport {
    materials: Vec<GeneratedMaterial>,
    textures: Vec<GeneratedTexture>,
}

fn inspect_generated(output_vpk: &Path) -> Result<GeneratedReport> {
    let vpk =
        valve_pak::open(output_vpk).with_context(|| format!("opening {}", output_vpk.display()))?;
    let mut entries: Vec<String> = vpk.file_paths().cloned().collect();
    entries.sort();
    println!("entries:");
    for entry in &entries {
        println!("  {entry}");
    }

    let mut materials = Vec::new();
    let mut textures = Vec::new();
    for entry in entries {
        if entry.ends_with(".vmat_c") {
            let bytes = vpk
                .get_file(&entry)
                .with_context(|| format!("reading {entry}"))?
                .read_all()?;
            let mat = morphic::material::parse(&bytes)
                .with_context(|| format!("parsing generated material {entry}"))?;
            println!(
                "material {} shader={} textures={}",
                mat.name,
                mat.shader_name,
                mat.texture_params.len()
            );
            materials.push(GeneratedMaterial {
                entry,
                material_name: mat.name,
                shader: mat.shader_name,
                texture_params: mat.texture_params,
                int_params: mat.int_params,
                vector_params: mat.vector_params,
            });
        } else if entry.ends_with(".vtex_c") {
            let bytes = vpk
                .get_file(&entry)
                .with_context(|| format!("reading {entry}"))?
                .read_all()?;
            let info = morphic::inspect(&bytes)
                .with_context(|| format!("inspecting generated texture {entry}"))?;
            println!(
                "texture {entry} {}x{} {} mips={} flags=0x{:04x}",
                info.width,
                info.height,
                info.format.name(),
                info.mip_count,
                info.flags.bits()
            );
            textures.push(GeneratedTexture {
                entry,
                format: info.format.name(),
                width: info.width,
                height: info.height,
                mip_count: info.mip_count,
                flags: info.flags.bits(),
            });
        }
    }

    if materials.is_empty() || textures.is_empty() {
        bail!(
            "expected generated materials and textures, found {} materials and {} textures",
            materials.len(),
            textures.len()
        );
    }
    Ok(GeneratedReport {
        materials,
        textures,
    })
}

fn compare_oracle(oracle: Option<&Path>, generated: &GeneratedReport) -> Result<()> {
    let Some(oracle) = oracle else {
        return Ok(());
    };
    let bytes = std::fs::read(oracle).with_context(|| format!("reading {}", oracle.display()))?;
    let json: Json =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", oracle.display()))?;
    let oracle_materials = json
        .pointer("/materials/entries")
        .and_then(Json::as_array)
        .context("oracle has no /materials/entries array")?;
    let mut by_name = BTreeMap::new();
    for material in oracle_materials {
        let name = material
            .get("material_name")
            .and_then(Json::as_str)
            .context("oracle material without material_name")?;
        by_name.insert(name.to_string(), material);
    }

    println!("oracle compare:");
    for generated_material in &generated.materials {
        let oracle_material = by_name
            .get(&generated_material.material_name)
            .with_context(|| format!("oracle missing {}", generated_material.material_name))?;
        let oracle_shader = oracle_material
            .get("shader")
            .and_then(Json::as_str)
            .context("oracle material without shader")?;
        if generated_material.shader != oracle_shader {
            bail!(
                "{} shader mismatch: pure={} oracle={}",
                generated_material.material_name,
                generated_material.shader,
                oracle_shader
            );
        }

        compare_object_keys(
            &generated_material.material_name,
            "texture params",
            generated_material.texture_params.keys(),
            oracle_material.pointer("/texture_params"),
        )?;
        compare_object_keys(
            &generated_material.material_name,
            "int params",
            generated_material.int_params.keys(),
            oracle_material.pointer("/int_params"),
        )?;
        compare_object_keys(
            &generated_material.material_name,
            "vector params",
            generated_material.vector_params.keys(),
            oracle_material.pointer("/vector_params"),
        )?;

        let pure_color = generated_material
            .texture_params
            .get("g_tColor")
            .map(String::as_str)
            .unwrap_or("<missing>");
        let oracle_color = oracle_material
            .pointer("/texture_params/g_tColor")
            .and_then(Json::as_str)
            .unwrap_or("<missing>");
        println!(
            "  {} ({}) ok; g_tColor pure={} oracle={}",
            generated_material.material_name, generated_material.entry, pure_color, oracle_color
        );
    }
    Ok(())
}

fn compare_object_keys<'a>(
    material_name: &str,
    label: &str,
    pure_keys: impl Iterator<Item = &'a String>,
    oracle_value: Option<&Json>,
) -> Result<()> {
    let pure: BTreeSet<_> = pure_keys.map(String::as_str).collect();
    let oracle = oracle_value
        .and_then(Json::as_object)
        .with_context(|| format!("oracle {material_name} has no {label} object"))?
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if pure != oracle {
        bail!("{material_name} {label} key mismatch: pure={pure:?} oracle={oracle:?}");
    }
    Ok(())
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

fn guard_compiled_root(compiled_root: &Path) -> Result<()> {
    for guarded in GUARDED_INSTALLED_VPKS {
        let guarded = Path::new(guarded);
        if compiled_root == guarded {
            bail!("refusing to use installed VPK path as compiled root");
        }
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
        "usage: soul_container_pure_probe <input.glb> <compiled_root> <out_dir.vpk> \
         [--oracle oracle.json] [--force]\n       soul_container_pure_probe --source-root \
         <source_root> <compiled_root> <out_dir.vpk> \
         [--oracle oracle.json] [--force]"
    );
}
