//! Compile controlled FBX mutations through resourcecompiler.
//!
//! This starts from the known-good Blender source tree, rewrites one FBX feature
//! at a time toward a candidate Rust FBX, and reports whether Valve's importer
//! produced embedded model geometry/materials or only a shell model.
//!
//! Usage:
//!   cargo run -p vpkmerge-core --example soul_container_fbx_mutation -- \
//!     /tmp/vpkmerge-rust-prep/models/props_gameplay/soul_container/model.fbx

use anyhow::{anyhow, bail, Context, Result};
use fbxcel::low::{
    v7400::{ArrayAttributeEncoding, AttributeValue},
    FbxVersion,
};
use fbxcel::tree::{
    any::AnyTree,
    v7400::{NodeHandle, NodeId, Tree},
};
use fbxcel::writer::v7400::binary::{AttributesWriter, Error as FbxWriteError, FbxFooter, Writer};
use std::collections::BTreeSet;
use std::fmt;
use std::fs::File;
use std::io::{BufReader, Cursor, Seek, Write};
use std::path::{Path, PathBuf};

const INSTALLED_PROOF_VPK: &str =
    "/home/esoc/.steam/steam/steamapps/common/Deadlock/game/citadel/addons/pak06_dir.vpk";
const DEFAULT_BLENDER_SOURCE_DIR: &str =
    "/home/esoc/csdk12/Reduced_CSDK_12/content/citadel_addons/test/models/props_gameplay/soul_container_glbprobe";
const DEFAULT_MODEL_REL: &str = "models/props_gameplay/soul_container_glbprobe";
const DEFAULT_CANDIDATE_MODEL_REL: &str = "models/props_gameplay/soul_container";
const DEFAULT_CSDK_ROOT: &str = "/home/esoc/csdk12/Reduced_CSDK_12";
const DEFAULT_PROTON: &str =
    "/home/esoc/.local/share/Steam/steamapps/common/Proton - Experimental/proton";
const DEFAULT_STEAM_ROOT: &str = "/home/esoc/.local/share/Steam";
const DEFAULT_PROTON_PREFIX: &str = "/tmp/proton-vpkmerge-rc";
const BLENDER_FOOTER_MAGIC_A: [u8; 16] = [
    0xfa, 0xbc, 0xab, 0x09, 0xd0, 0xc8, 0xd4, 0x66, 0xb1, 0x76, 0xfb, 0x83, 0x1c, 0xf7, 0x26, 0x7e,
];
const FBX_FOOTER_MAGIC_B: [u8; 16] = [
    0xf8, 0x5a, 0x8c, 0x6a, 0xde, 0xf5, 0xd9, 0x7e, 0xec, 0xe9, 0x0c, 0xe3, 0x75, 0x8f, 0x29, 0x0b,
];

const DEFAULT_FEATURES: &[Feature] = &[
    Feature::Header,
    Feature::GlobalSettings,
    Feature::Documents,
    Feature::References,
    Feature::Takes,
    Feature::Definitions,
    Feature::MaterialProperties,
    Feature::ModelProperties,
    Feature::GeometryEdges,
    Feature::GeometryNormals,
    Feature::GeometryUvs,
    Feature::GeometryMaterialLayer,
    Feature::GeometryLayer,
    Feature::GeometryTopology,
    Feature::GeometryAll,
    Feature::FullObjectsConnections,
];

fn main() -> Result<()> {
    let args = Args::parse()?;
    args.validate()?;

    std::fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("creating {}", args.out_dir.display()))?;

    let baseline_fbx = args
        .baseline_fbx
        .clone()
        .unwrap_or_else(|| args.blender_source_dir.join("model.fbx"));
    require_file(&baseline_fbx, "baseline Blender FBX")?;
    require_file(&args.candidate_fbx, "candidate Rust FBX")?;
    require_dir(
        &args.blender_source_dir,
        "known-good Blender source model directory",
    )?;

    let (version, baseline_tree) = load_fbx_tree(&baseline_fbx)
        .with_context(|| format!("loading baseline FBX {}", baseline_fbx.display()))?;
    let (candidate_version, mut candidate_tree) = load_fbx_tree(&args.candidate_fbx)
        .with_context(|| format!("loading candidate FBX {}", args.candidate_fbx.display()))?;
    if candidate_version != version {
        eprintln!(
            "warning: baseline FBX version {:?} differs from candidate {:?}; writing mutations as baseline version",
            version, candidate_version
        );
    }
    let rewrites = rewrite_tree_strings(
        &mut candidate_tree,
        &args.candidate_model_rel,
        &args.model_rel,
    );
    if rewrites > 0 {
        println!(
            "rewrote {rewrites} candidate FBX string attrs from {} to {}",
            args.candidate_model_rel, args.model_rel
        );
    }

    let source_vmat_count = count_source_vmats(&args.blender_source_dir)?;
    let expected_materials = count_child_nodes(&baseline_tree, &["Objects"], "Material")?;
    let mut features = args
        .features
        .clone()
        .unwrap_or_else(|| DEFAULT_FEATURES.to_vec());
    if args.features.is_none() {
        let compatibility = Compatibility::from_trees(&baseline_tree, &candidate_tree)?;
        features.retain(|feature| {
            if compatibility.supports(*feature) {
                true
            } else {
                println!(
                    "skipping default feature {}: {}",
                    feature.name(),
                    compatibility.reason(*feature)
                );
                false
            }
        });
    }
    let mut cases = build_cases(args.mode, &features);
    if !args.include_candidate {
        cases.retain(|case| case.kind != CaseKind::Candidate);
    }

    println!("output_dir {}", args.out_dir.display());
    println!("baseline_fbx {}", baseline_fbx.display());
    println!("candidate_fbx {}", args.candidate_fbx.display());
    println!("source_vmats {source_vmat_count}");
    println!("fbx_materials {expected_materials}");
    println!(
        "{:<34} {:<12} {:>6} {:>6} {:>6} {:>6} {:>6} {:>10}  notes",
        "case", "status", "vmat", "vtex", "parts", "draws", "mats", "largest"
    );

    for case in cases {
        let result = match write_case_fbx(
            &case,
            version,
            &baseline_fbx,
            &baseline_tree,
            &candidate_tree,
            &args,
        ) {
            Ok(case_fbx) if args.write_only => {
                println!(
                    "{:<34} {:<12} {:>6} {:>6} {:>6} {:>6} {:>6} {:>10}  {}",
                    case.name,
                    "written",
                    "-",
                    "-",
                    "-",
                    "-",
                    "-",
                    "-",
                    case_fbx.display()
                );
                continue;
            }
            Ok(case_fbx) => run_case_compile(&case.name, &case_fbx, &args, expected_materials),
            Err(err) => Err(err),
        };

        match result {
            Ok(outcome) => print_outcome(&case.name, &outcome),
            Err(err) => {
                println!(
                    "{:<34} {:<12} {:>6} {:>6} {:>6} {:>6} {:>6} {:>10}  {:#}",
                    case.name, "error", "-", "-", "-", "-", "-", "-", err
                );
                if args.stop_on_failure {
                    bail!("stopping after failed case {}", case.name);
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct Args {
    candidate_fbx: PathBuf,
    blender_source_dir: PathBuf,
    baseline_fbx: Option<PathBuf>,
    model_rel: String,
    candidate_model_rel: String,
    out_dir: PathBuf,
    addon_prefix: String,
    csdk_root: PathBuf,
    proton: PathBuf,
    steam_root: PathBuf,
    proton_prefix: PathBuf,
    keep_staging: bool,
    write_only: bool,
    stop_on_failure: bool,
    include_candidate: bool,
    mode: Mode,
    features: Option<Vec<Feature>>,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut candidate_fbx = None;
        let mut blender_source_dir = PathBuf::from(DEFAULT_BLENDER_SOURCE_DIR);
        let mut baseline_fbx = None;
        let mut model_rel = DEFAULT_MODEL_REL.to_string();
        let mut candidate_model_rel = DEFAULT_CANDIDATE_MODEL_REL.to_string();
        let mut out_dir =
            PathBuf::from(format!("/tmp/vpkmerge-fbx-mutation-{}", std::process::id()));
        let mut addon_prefix = "vpkmerge_fbxmut".to_string();
        let mut csdk_root = PathBuf::from(DEFAULT_CSDK_ROOT);
        let mut proton = PathBuf::from(DEFAULT_PROTON);
        let mut steam_root = PathBuf::from(DEFAULT_STEAM_ROOT);
        let mut proton_prefix = PathBuf::from(DEFAULT_PROTON_PREFIX);
        let mut keep_staging = false;
        let mut write_only = false;
        let mut stop_on_failure = false;
        let mut include_candidate = true;
        let mut mode = Mode::Cumulative;
        let mut features = None;

        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--help" || arg == "-h" {
                print_usage();
                std::process::exit(0);
            } else if arg == "--blender-source-dir" {
                blender_source_dir = PathBuf::from(next_value(&mut args, "--blender-source-dir")?);
            } else if arg == "--baseline-fbx" {
                baseline_fbx = Some(PathBuf::from(next_value(&mut args, "--baseline-fbx")?));
            } else if arg == "--model-rel" {
                model_rel = next_value(&mut args, "--model-rel")?
                    .to_string_lossy()
                    .into_owned();
            } else if arg == "--candidate-model-rel" {
                candidate_model_rel = next_value(&mut args, "--candidate-model-rel")?
                    .to_string_lossy()
                    .into_owned();
            } else if arg == "--out-dir" {
                out_dir = PathBuf::from(next_value(&mut args, "--out-dir")?);
            } else if arg == "--addon-prefix" {
                addon_prefix = next_value(&mut args, "--addon-prefix")?
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
            } else if arg == "--mode" {
                mode = next_value(&mut args, "--mode")?.to_string_lossy().parse()?;
            } else if arg == "--features" {
                let value = next_value(&mut args, "--features")?
                    .to_string_lossy()
                    .into_owned();
                features = Some(parse_features(&value)?);
            } else if arg == "--keep-staging" {
                keep_staging = true;
            } else if arg == "--write-only" {
                write_only = true;
            } else if arg == "--stop-on-failure" {
                stop_on_failure = true;
            } else if arg == "--no-candidate" {
                include_candidate = false;
            } else if arg.to_string_lossy().starts_with("--") {
                bail!("unknown option: {}", arg.to_string_lossy());
            } else if candidate_fbx.is_none() {
                candidate_fbx = Some(PathBuf::from(arg));
            } else {
                bail!("unexpected positional argument: {}", arg.to_string_lossy());
            }
        }

        let Some(candidate_fbx) = candidate_fbx else {
            print_usage();
            bail!("candidate Rust FBX path is required");
        };

        Ok(Self {
            candidate_fbx,
            blender_source_dir,
            baseline_fbx,
            model_rel,
            candidate_model_rel,
            out_dir,
            addon_prefix,
            csdk_root,
            proton,
            steam_root,
            proton_prefix,
            keep_staging,
            write_only,
            stop_on_failure,
            include_candidate,
            mode,
            features,
        })
    }

    fn validate(&self) -> Result<()> {
        validate_addon_name(&self.addon_prefix)?;
        require_file(&self.proton, "Proton executable")?;
        require_dir(
            &self.csdk_root.join("game/bin_tools/win64"),
            "resourcecompiler directory",
        )?;
        if self.model_rel.starts_with('/') || self.model_rel.contains('\\') {
            bail!("--model-rel must be Source-relative with forward slashes");
        }
        if self.candidate_model_rel.is_empty() {
            bail!("--candidate-model-rel must not be empty");
        }
        Ok(())
    }
}

fn print_usage() {
    eprintln!(
        "usage: soul_container_fbx_mutation <candidate-rust-model.fbx> \
         [--blender-source-dir PATH] [--baseline-fbx PATH] [--model-rel REL] \
         [--candidate-model-rel REL] [--out-dir PATH] [--mode cumulative|isolated|both] \
         [--features a,b,c] [--write-only] [--keep-staging] [--stop-on-failure]"
    );
    eprintln!("features: {}", Feature::all_names().join(","));
}

fn next_value(
    args: &mut impl Iterator<Item = std::ffi::OsString>,
    name: &str,
) -> Result<std::ffi::OsString> {
    args.next()
        .with_context(|| format!("{name} requires a value"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Cumulative,
    Isolated,
    Both,
}

impl std::str::FromStr for Mode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "cumulative" => Ok(Self::Cumulative),
            "isolated" => Ok(Self::Isolated),
            "both" => Ok(Self::Both),
            _ => bail!("unknown mode {value:?}; expected cumulative, isolated, or both"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Feature {
    Header,
    GlobalSettings,
    Documents,
    References,
    Takes,
    Definitions,
    MaterialProperties,
    ModelProperties,
    GeometryEdges,
    GeometryNormals,
    GeometryUvs,
    GeometryMaterialLayer,
    GeometryLayer,
    GeometryTopology,
    GeometryAll,
    FullObjectsConnections,
}

impl Feature {
    fn all_names() -> &'static [&'static str] {
        &[
            "header",
            "global_settings",
            "documents",
            "references",
            "takes",
            "definitions",
            "material_props",
            "model_props",
            "geometry_edges",
            "geometry_normals",
            "geometry_uvs",
            "geometry_material_layer",
            "geometry_layer",
            "geometry_topology",
            "geometry_all",
            "full_objects_connections",
        ]
    }

    fn name(self) -> &'static str {
        match self {
            Self::Header => "header",
            Self::GlobalSettings => "global_settings",
            Self::Documents => "documents",
            Self::References => "references",
            Self::Takes => "takes",
            Self::Definitions => "definitions",
            Self::MaterialProperties => "material_props",
            Self::ModelProperties => "model_props",
            Self::GeometryEdges => "geometry_edges",
            Self::GeometryNormals => "geometry_normals",
            Self::GeometryUvs => "geometry_uvs",
            Self::GeometryMaterialLayer => "geometry_material_layer",
            Self::GeometryLayer => "geometry_layer",
            Self::GeometryTopology => "geometry_topology",
            Self::GeometryAll => "geometry_all",
            Self::FullObjectsConnections => "full_objects_connections",
        }
    }
}

impl std::str::FromStr for Feature {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "header" => Ok(Self::Header),
            "global_settings" => Ok(Self::GlobalSettings),
            "documents" => Ok(Self::Documents),
            "references" => Ok(Self::References),
            "takes" => Ok(Self::Takes),
            "definitions" => Ok(Self::Definitions),
            "material_props" => Ok(Self::MaterialProperties),
            "model_props" => Ok(Self::ModelProperties),
            "geometry_edges" => Ok(Self::GeometryEdges),
            "geometry_normals" => Ok(Self::GeometryNormals),
            "geometry_uvs" => Ok(Self::GeometryUvs),
            "geometry_material_layer" => Ok(Self::GeometryMaterialLayer),
            "geometry_layer" => Ok(Self::GeometryLayer),
            "geometry_topology" => Ok(Self::GeometryTopology),
            "geometry_all" => Ok(Self::GeometryAll),
            "full_objects_connections" => Ok(Self::FullObjectsConnections),
            _ => bail!("unknown feature {value:?}"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Compatibility {
    dst_geometry: usize,
    src_geometry: usize,
    dst_model: usize,
    src_model: usize,
    dst_material: usize,
    src_material: usize,
}

impl Compatibility {
    fn from_trees(dst: &Tree, src: &Tree) -> Result<Self> {
        Ok(Self {
            dst_geometry: count_child_nodes(dst, &["Objects"], "Geometry")?,
            src_geometry: count_child_nodes(src, &["Objects"], "Geometry")?,
            dst_model: count_child_nodes(dst, &["Objects"], "Model")?,
            src_model: count_child_nodes(src, &["Objects"], "Model")?,
            dst_material: count_child_nodes(dst, &["Objects"], "Material")?,
            src_material: count_child_nodes(src, &["Objects"], "Material")?,
        })
    }

    fn supports(self, feature: Feature) -> bool {
        match feature {
            Feature::MaterialProperties => self.dst_material == self.src_material,
            Feature::ModelProperties => self.dst_model == self.src_model,
            Feature::GeometryEdges
            | Feature::GeometryNormals
            | Feature::GeometryUvs
            | Feature::GeometryMaterialLayer
            | Feature::GeometryLayer
            | Feature::GeometryTopology
            | Feature::GeometryAll => self.dst_geometry == self.src_geometry,
            _ => true,
        }
    }

    fn reason(self, feature: Feature) -> String {
        match feature {
            Feature::MaterialProperties => format!(
                "Objects/Material count mismatch: baseline={} candidate={}",
                self.dst_material, self.src_material
            ),
            Feature::ModelProperties => format!(
                "Objects/Model count mismatch: baseline={} candidate={}",
                self.dst_model, self.src_model
            ),
            Feature::GeometryEdges
            | Feature::GeometryNormals
            | Feature::GeometryUvs
            | Feature::GeometryMaterialLayer
            | Feature::GeometryLayer
            | Feature::GeometryTopology
            | Feature::GeometryAll => format!(
                "Objects/Geometry count mismatch: baseline={} candidate={}",
                self.dst_geometry, self.src_geometry
            ),
            _ => "compatible".to_string(),
        }
    }
}

fn parse_features(value: &str) -> Result<Vec<Feature>> {
    let mut out = Vec::new();
    for raw in value.split(',') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        out.push(raw.parse()?);
    }
    if out.is_empty() {
        bail!("--features did not contain any feature names");
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CaseKind {
    BaselineRaw,
    BaselineRoundtrip,
    Candidate,
    Isolated(Feature),
    Cumulative(Vec<Feature>),
}

#[derive(Debug, Clone)]
struct Case {
    name: String,
    kind: CaseKind,
}

fn build_cases(mode: Mode, features: &[Feature]) -> Vec<Case> {
    let mut cases = vec![
        Case {
            name: "baseline_raw".to_string(),
            kind: CaseKind::BaselineRaw,
        },
        Case {
            name: "baseline_roundtrip".to_string(),
            kind: CaseKind::BaselineRoundtrip,
        },
    ];

    if matches!(mode, Mode::Isolated | Mode::Both) {
        cases.extend(features.iter().copied().map(|feature| Case {
            name: format!("iso_{}", feature.name()),
            kind: CaseKind::Isolated(feature),
        }));
    }

    if matches!(mode, Mode::Cumulative | Mode::Both) {
        let mut cumulative = Vec::new();
        for (idx, feature) in features.iter().copied().enumerate() {
            cumulative.push(feature);
            cases.push(Case {
                name: format!("cum_{:02}_{}", idx + 1, feature.name()),
                kind: CaseKind::Cumulative(cumulative.clone()),
            });
        }
    }

    cases.push(Case {
        name: "candidate_rewritten".to_string(),
        kind: CaseKind::Candidate,
    });
    cases
}

fn write_case_fbx(
    case: &Case,
    version: FbxVersion,
    baseline_fbx: &Path,
    baseline_tree: &Tree,
    candidate_tree: &Tree,
    args: &Args,
) -> Result<PathBuf> {
    let case_dir = args.out_dir.join(&case.name);
    std::fs::create_dir_all(&case_dir)
        .with_context(|| format!("creating {}", case_dir.display()))?;
    let out_fbx = case_dir.join("model.fbx");

    match &case.kind {
        CaseKind::BaselineRaw => {
            std::fs::copy(baseline_fbx, &out_fbx)
                .with_context(|| format!("copying baseline FBX to {}", out_fbx.display()))?;
        }
        CaseKind::BaselineRoundtrip => {
            write_fbx_tree(&out_fbx, version, baseline_tree)?;
        }
        CaseKind::Candidate => {
            write_fbx_tree(&out_fbx, version, candidate_tree)?;
        }
        CaseKind::Isolated(feature) => {
            let mut tree = baseline_tree.clone();
            apply_feature(&mut tree, candidate_tree, *feature)
                .with_context(|| format!("applying feature {}", feature.name()))?;
            write_fbx_tree(&out_fbx, version, &tree)?;
        }
        CaseKind::Cumulative(features) => {
            let mut tree = baseline_tree.clone();
            for feature in features {
                apply_feature(&mut tree, candidate_tree, *feature)
                    .with_context(|| format!("applying feature {}", feature.name()))?;
            }
            write_fbx_tree(&out_fbx, version, &tree)?;
        }
    }

    Ok(out_fbx)
}

fn apply_feature(dst: &mut Tree, src: &Tree, feature: Feature) -> Result<()> {
    match feature {
        Feature::Header => replace_root_node(dst, src, "FBXHeaderExtension"),
        Feature::GlobalSettings => replace_root_node(dst, src, "GlobalSettings"),
        Feature::Documents => replace_root_node(dst, src, "Documents"),
        Feature::References => replace_root_node(dst, src, "References"),
        Feature::Takes => replace_root_node(dst, src, "Takes"),
        Feature::Definitions => replace_root_node(dst, src, "Definitions"),
        Feature::MaterialProperties => {
            replace_indexed_node_children(dst, src, &["Objects"], "Material")
        }
        Feature::ModelProperties => replace_indexed_node_children(dst, src, &["Objects"], "Model"),
        Feature::GeometryEdges => replace_indexed_geometry_children(dst, src, &["Edges"]),
        Feature::GeometryNormals => {
            replace_indexed_geometry_children(dst, src, &["LayerElementNormal"])
        }
        Feature::GeometryUvs => replace_indexed_geometry_children(dst, src, &["LayerElementUV"]),
        Feature::GeometryMaterialLayer => {
            replace_indexed_geometry_children(dst, src, &["LayerElementMaterial"])
        }
        Feature::GeometryLayer => replace_indexed_geometry_children(dst, src, &["Layer"]),
        Feature::GeometryTopology => {
            replace_indexed_geometry_children(dst, src, &["Vertices", "PolygonVertexIndex"])
        }
        Feature::GeometryAll => replace_indexed_node_children(dst, src, &["Objects"], "Geometry"),
        Feature::FullObjectsConnections => {
            replace_root_node(dst, src, "Definitions")?;
            replace_root_node(dst, src, "Objects")?;
            replace_root_node(dst, src, "Connections")
        }
    }
}

fn replace_root_node(dst: &mut Tree, src: &Tree, name: &str) -> Result<()> {
    let dst_id = first_id_by_path(dst, &[name]).with_context(|| format!("missing dst {name}"))?;
    let src_id = first_id_by_path(src, &[name]).with_context(|| format!("missing src {name}"))?;
    replace_node_from_source(dst, dst_id, src, src_id);
    Ok(())
}

fn replace_indexed_node_children(
    dst: &mut Tree,
    src: &Tree,
    parent_path: &[&str],
    node_name: &str,
) -> Result<()> {
    let dst_parent = first_id_by_path(dst, parent_path)
        .with_context(|| format!("missing dst path {}", parent_path.join("/")))?;
    let src_parent = first_id_by_path(src, parent_path)
        .with_context(|| format!("missing src path {}", parent_path.join("/")))?;
    let dst_ids = child_ids_by_name(dst, dst_parent, node_name);
    let src_ids = child_ids_by_name(src, src_parent, node_name);
    if dst_ids.len() != src_ids.len() {
        bail!(
            "{}/{} count mismatch: dst={} src={}",
            parent_path.join("/"),
            node_name,
            dst_ids.len(),
            src_ids.len()
        );
    }
    for (dst_id, src_id) in dst_ids.into_iter().zip(src_ids) {
        replace_node_children_from_source(dst, dst_id, src, src_id);
    }
    Ok(())
}

fn replace_indexed_geometry_children(
    dst: &mut Tree,
    src: &Tree,
    child_names: &[&str],
) -> Result<()> {
    let dst_parent = first_id_by_path(dst, &["Objects"]).context("missing dst Objects")?;
    let src_parent = first_id_by_path(src, &["Objects"]).context("missing src Objects")?;
    let dst_geometries = child_ids_by_name(dst, dst_parent, "Geometry");
    let src_geometries = child_ids_by_name(src, src_parent, "Geometry");
    if dst_geometries.len() != src_geometries.len() {
        bail!(
            "Objects/Geometry count mismatch: dst={} src={}",
            dst_geometries.len(),
            src_geometries.len()
        );
    }

    for (index, (dst_geometry, src_geometry)) in
        dst_geometries.into_iter().zip(src_geometries).enumerate()
    {
        for child_name in child_names {
            replace_child_set_by_name(dst, dst_geometry, src, src_geometry, child_name)
                .with_context(|| format!("geometry {index} child {child_name}"))?;
        }
    }
    Ok(())
}

fn replace_child_set_by_name(
    dst: &mut Tree,
    dst_parent: NodeId,
    src: &Tree,
    src_parent: NodeId,
    child_name: &str,
) -> Result<()> {
    let dst_ids = child_ids_by_name(dst, dst_parent, child_name);
    let src_ids = child_ids_by_name(src, src_parent, child_name);
    if dst_ids.len() != src_ids.len() {
        bail!(
            "{child_name} count mismatch: dst={} src={}",
            dst_ids.len(),
            src_ids.len()
        );
    }
    for (dst_id, src_id) in dst_ids.into_iter().zip(src_ids) {
        replace_node_from_source(dst, dst_id, src, src_id);
    }
    Ok(())
}

fn replace_node_from_source(dst: &mut Tree, dst_id: NodeId, src: &Tree, src_id: NodeId) {
    let new_id = clone_subtree_into(dst, src, src_id);
    dst.insert_before(new_id, dst_id);
    dst.detach(dst_id);
}

fn replace_node_children_from_source(dst: &mut Tree, dst_id: NodeId, src: &Tree, src_id: NodeId) {
    let old_children: Vec<_> = dst_id
        .to_handle(dst)
        .children()
        .map(|child| child.node_id())
        .collect();
    for child in old_children {
        dst.detach(child);
    }
    let new_children: Vec<_> = src_id
        .to_handle(src)
        .children()
        .map(|child| clone_subtree_into(dst, src, child.node_id()))
        .collect();
    for child in new_children {
        dst.append(child, dst_id);
    }
}

fn clone_subtree_into(dst: &mut Tree, src: &Tree, src_id: NodeId) -> NodeId {
    let src_handle = src_id.to_handle(src);
    let new_id = dst.create_node(src_handle.name());
    dst.set_attributes_vec(new_id, src_handle.attributes().to_vec());
    let child_ids: Vec<_> = src_handle
        .children()
        .map(|child| clone_subtree_into(dst, src, child.node_id()))
        .collect();
    for child_id in child_ids {
        dst.append(child_id, new_id);
    }
    new_id
}

fn first_id_by_path(tree: &Tree, path: &[&str]) -> Option<NodeId> {
    ids_by_path(tree, path).into_iter().next()
}

fn ids_by_path(tree: &Tree, path: &[&str]) -> Vec<NodeId> {
    let mut current = vec![tree.root().node_id()];
    for name in path {
        let mut next = Vec::new();
        for parent in current {
            next.extend(child_ids_by_name(tree, parent, name));
        }
        current = next;
    }
    current
}

fn child_ids_by_name(tree: &Tree, parent: NodeId, name: &str) -> Vec<NodeId> {
    parent
        .to_handle(tree)
        .children_by_name(name)
        .map(|child| child.node_id())
        .collect()
}

fn count_child_nodes(tree: &Tree, parent_path: &[&str], node_name: &str) -> Result<usize> {
    let parent = first_id_by_path(tree, parent_path)
        .with_context(|| format!("missing path {}", parent_path.join("/")))?;
    Ok(child_ids_by_name(tree, parent, node_name).len())
}

fn rewrite_tree_strings(tree: &mut Tree, from: &str, to: &str) -> usize {
    if from == to {
        return 0;
    }
    let mut ids = Vec::new();
    collect_ids(tree.root(), &mut ids);
    let mut changed = 0;
    for id in ids {
        let attrs = tree.take_attributes_vec(id);
        let rewritten = attrs
            .into_iter()
            .map(|attr| match attr {
                AttributeValue::String(value) if value.contains(from) => {
                    changed += 1;
                    AttributeValue::String(value.replace(from, to))
                }
                other => other,
            })
            .collect();
        tree.set_attributes_vec(id, rewritten);
    }
    changed
}

fn collect_ids(node: NodeHandle<'_>, out: &mut Vec<NodeId>) {
    if node.parent().is_some() {
        out.push(node.node_id());
    }
    for child in node.children() {
        collect_ids(child, out);
    }
}

fn load_fbx_tree(path: &Path) -> Result<(FbxVersion, Tree)> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(file);
    match AnyTree::from_seekable_reader(reader)
        .with_context(|| format!("parsing binary FBX {}", path.display()))?
    {
        AnyTree::V7400(version, tree, footer) => {
            if let Err(err) = footer {
                eprintln!("warning: {} footer parse warning: {err}", path.display());
            }
            Ok((version, tree))
        }
        tree => bail!("unsupported FBX tree version: {:?}", tree.fbx_version()),
    }
}

fn write_fbx_tree(path: &Path, version: FbxVersion, tree: &Tree) -> Result<()> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = Writer::new(cursor, version).map_err(|err| anyhow!("{err}"))?;
    write_tree_zlib_arrays(&mut writer, tree).map_err(|err| anyhow!("{err}"))?;
    let bytes = writer
        .finalize_and_flush(&FbxFooter {
            unknown1: Some(&BLENDER_FOOTER_MAGIC_A),
            unknown2: Some([0; 4]),
            unknown3: Some(&FBX_FOOTER_MAGIC_B),
            ..FbxFooter::default()
        })
        .map_err(|err| anyhow!("{err}"))?
        .into_inner();
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn write_tree_zlib_arrays<W: Write + Seek>(
    writer: &mut Writer<W>,
    tree: &Tree,
) -> std::result::Result<(), FbxWriteError> {
    let Some(mut current) = tree.root().first_child() else {
        return Ok(());
    };

    'all: loop {
        let mut attrs_writer = writer.new_node(current.name())?;
        for attr in current.attributes() {
            append_attr_zlib_arrays(&mut attrs_writer, attr)?;
        }

        let mut visit_child = true;
        current = 'next: loop {
            if visit_child {
                if let Some(child) = current.first_child() {
                    break 'next child;
                }
                visit_child = false;
            }
            writer.close_node()?;
            if let Some(sib) = current.next_sibling() {
                break 'next sib;
            }
            let parent = current
                .parent()
                .expect("current node should have a parent until the implicit root");
            if parent.node_id() == tree.root().node_id() {
                break 'all;
            }
            current = parent;
        };
    }

    Ok(())
}

fn append_attr_zlib_arrays<W: Write + Seek>(
    attrs_writer: &mut AttributesWriter<'_, W>,
    attr: &AttributeValue,
) -> std::result::Result<(), FbxWriteError> {
    match attr {
        AttributeValue::Bool(value) => attrs_writer.append_bool(*value),
        AttributeValue::I16(value) => attrs_writer.append_i16(*value),
        AttributeValue::I32(value) => attrs_writer.append_i32(*value),
        AttributeValue::I64(value) => attrs_writer.append_i64(*value),
        AttributeValue::F32(value) => attrs_writer.append_f32(*value),
        AttributeValue::F64(value) => attrs_writer.append_f64(*value),
        AttributeValue::ArrBool(values) => attrs_writer
            .append_arr_bool_from_iter(ArrayAttributeEncoding::Zlib, values.iter().copied()),
        AttributeValue::ArrI32(values) => attrs_writer
            .append_arr_i32_from_iter(ArrayAttributeEncoding::Zlib, values.iter().copied()),
        AttributeValue::ArrI64(values) => attrs_writer
            .append_arr_i64_from_iter(ArrayAttributeEncoding::Zlib, values.iter().copied()),
        AttributeValue::ArrF32(values) => attrs_writer
            .append_arr_f32_from_iter(ArrayAttributeEncoding::Zlib, values.iter().copied()),
        AttributeValue::ArrF64(values) => attrs_writer
            .append_arr_f64_from_iter(ArrayAttributeEncoding::Zlib, values.iter().copied()),
        AttributeValue::Binary(value) => attrs_writer.append_binary_direct(value),
        AttributeValue::String(value) => attrs_writer.append_string_direct(value),
    }
}

fn run_case_compile(
    case_name: &str,
    case_fbx: &Path,
    args: &Args,
    expected_vmat_sources: usize,
) -> Result<Outcome> {
    let case_dir = args.out_dir.join(case_name);
    let source_root = case_dir.join("source");
    let source_model_dir = source_root.join(&args.model_rel);
    copy_dir(&args.blender_source_dir, &source_model_dir)
        .with_context(|| format!("staging source tree {}", source_root.display()))?;
    std::fs::copy(case_fbx, source_model_dir.join("model.fbx"))
        .with_context(|| format!("installing case FBX into {}", source_model_dir.display()))?;

    let output_vpk = case_dir.join(format!("{case_name}_dir.vpk"));
    guard_output_path(&output_vpk)?;
    let addon = format!(
        "{}_{}_{}",
        args.addon_prefix,
        std::process::id(),
        sanitize_addon_fragment(case_name)
    );
    validate_addon_name(&addon)?;

    let options = vpkmerge_core::SoulContainerImportOptions {
        model_rel: args.model_rel.clone(),
        ..vpkmerge_core::SoulContainerImportOptions::default()
    };
    let backend = vpkmerge_core::SoulContainerCompileBackend::ResourceCompiler(
        vpkmerge_core::ResourceCompilerBackend {
            addon,
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
        &source_root,
        &options,
        &backend,
        &output_vpk,
    )?;

    inspect_compiled_vpk(
        &report.output_vpk,
        &args.model_rel,
        expected_vmat_sources,
        &report.addon,
    )
}

#[derive(Debug)]
struct Outcome {
    status: String,
    vmat_count: usize,
    vtex_count: usize,
    mesh_parts: Option<usize>,
    draw_calls: Option<usize>,
    material_refs: Option<usize>,
    largest_axis: Option<f32>,
    notes: String,
}

fn inspect_compiled_vpk(
    output_vpk: &Path,
    model_rel: &str,
    expected_vmat_sources: usize,
    addon: &str,
) -> Result<Outcome> {
    let info = vpkmerge_core::inspect(output_vpk)
        .with_context(|| format!("inspecting {}", output_vpk.display()))?;
    let entries: BTreeSet<_> = info.file_paths.into_iter().collect();
    let model_entry = format!("{model_rel}/soul_container.vmdl_c");
    let vmat_count = entries
        .iter()
        .filter(|entry| entry.starts_with(&format!("{model_rel}/materials/")))
        .filter(|entry| entry.ends_with(".vmat_c"))
        .count();
    let vtex_count = entries
        .iter()
        .filter(|entry| entry.ends_with(".vtex_c"))
        .count();

    if !entries.contains(&model_entry) {
        return Ok(Outcome {
            status: "missing".to_string(),
            vmat_count,
            vtex_count,
            mesh_parts: None,
            draw_calls: None,
            material_refs: None,
            largest_axis: None,
            notes: format!(
                "addon={addon} vpk={} missing {model_entry}",
                output_vpk.display()
            ),
        });
    }

    let vpk = valve_pak::open(output_vpk)?;
    let model_bytes = vpk
        .get_file(&model_entry)
        .with_context(|| format!("reading {model_entry} from {}", output_vpk.display()))?
        .read_all()?;
    let model_info = morphic::model::inspect(&model_bytes)?;
    if !model_info.has_embedded_geometry {
        return Ok(Outcome {
            status: "shell".to_string(),
            vmat_count,
            vtex_count,
            mesh_parts: Some(model_info.mesh_parts),
            draw_calls: None,
            material_refs: None,
            largest_axis: None,
            notes: format!("addon={addon} vpk={}", output_vpk.display()),
        });
    }

    let model = morphic::model::decode(&model_bytes).context("decoding compiled model")?;
    let draw_calls = model.meshes.iter().map(|mesh| mesh.primitives.len()).sum();
    let material_refs = model.materials().len();
    let largest_axis = model.position_bounds().map(|bounds| {
        [
            bounds.max[0] - bounds.min[0],
            bounds.max[1] - bounds.min[1],
            bounds.max[2] - bounds.min[2],
        ]
        .into_iter()
        .fold(0.0_f32, f32::max)
    });
    let status = if vmat_count == 0 {
        "embedded_no_vmat"
    } else if vmat_count < expected_vmat_sources {
        "embedded_partial"
    } else {
        "embedded"
    };
    Ok(Outcome {
        status: status.to_string(),
        vmat_count,
        vtex_count,
        mesh_parts: Some(model_info.mesh_parts),
        draw_calls: Some(draw_calls),
        material_refs: Some(material_refs),
        largest_axis,
        notes: format!("addon={addon} vpk={}", output_vpk.display()),
    })
}

fn print_outcome(case_name: &str, outcome: &Outcome) {
    println!(
        "{:<34} {:<12} {:>6} {:>6} {:>6} {:>6} {:>6} {:>10}  {}",
        case_name,
        outcome.status,
        outcome.vmat_count,
        outcome.vtex_count,
        display_opt(outcome.mesh_parts),
        display_opt(outcome.draw_calls),
        display_opt(outcome.material_refs),
        outcome
            .largest_axis
            .map(|value| format!("{value:.4}"))
            .unwrap_or_else(|| "-".to_string()),
        outcome.notes
    );
}

fn display_opt<T: fmt::Display>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn count_source_vmats(model_dir: &Path) -> Result<usize> {
    let materials = model_dir.join("materials");
    if !materials.is_dir() {
        return Ok(0);
    }
    let mut count = 0;
    for entry in
        std::fs::read_dir(&materials).with_context(|| format!("reading {}", materials.display()))?
    {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|ext| ext == "vmat")
        {
            count += 1;
        }
    }
    Ok(count)
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir(&src_path, &dst_path)?;
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

fn sanitize_addon_fragment(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "case".to_string()
    } else {
        out
    }
}

fn validate_addon_name(addon: &str) -> Result<()> {
    if addon == "." || addon == ".." || addon.is_empty() {
        bail!("invalid addon name: {addon:?}");
    }
    if !addon
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
    {
        bail!("addon name must be file-name safe, got: {addon:?}");
    }
    Ok(())
}
