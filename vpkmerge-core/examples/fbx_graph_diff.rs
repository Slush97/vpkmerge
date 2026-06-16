//! Diff two binary FBX node/property graphs.
//!
//! This is intended for the soul-container resourcecompiler path: compare a
//! known-good Blender FBX against the Rust-authored FBX and keep closing graph
//! shape gaps until Valve's importer materializes meshes and materials.
//!
//! Usage:
//!   cargo run -p vpkmerge-core --example fbx_graph_diff -- \
//!     <known-good-blender.fbx> <rust.fbx> [--max-diffs 120] [--dump-focus]

use anyhow::{bail, Context, Result};
use fbxcel::low::v7400::AttributeValue;
use fbxcel::tree::{any::AnyTree, v7400::NodeHandle};
use fbxcel_dom::any::AnyDocument;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

const FOCUS_PATHS: &[&str] = &[
    "FileId",
    "CreationTime",
    "Creator",
    "GlobalSettings",
    "Definitions",
    "Objects/Geometry",
    "Objects/Model",
    "Objects/Material",
    "Objects/Texture",
    "Objects/Video",
    "Objects/NodeAttribute",
    "Connections",
    "Takes",
];

fn main() -> Result<()> {
    let args = Args::parse()?;
    let left = LoadedFbx::load(&args.left).with_context(|| {
        format!(
            "loading left/known-good FBX as binary tree: {}",
            args.left.display()
        )
    })?;
    let right = LoadedFbx::load(&args.right).with_context(|| {
        format!(
            "loading right/candidate FBX as binary tree: {}",
            args.right.display()
        )
    })?;

    print_header("Files");
    print_file_summary("left", &left);
    print_file_summary("right", &right);

    print_header("DOM Objects");
    print_dom_summary("left", &left.dom);
    print_dom_summary("right", &right.dom);
    diff_count_map(
        "object class counts",
        "left",
        "right",
        &left.graph.dom_class_counts,
        &right.graph.dom_class_counts,
        args.max_diffs,
    );
    diff_count_map(
        "connection kind counts",
        "left",
        "right",
        &left.graph.dom_connection_counts,
        &right.graph.dom_connection_counts,
        args.max_diffs,
    );

    print_header("Node Graph");
    diff_count_map(
        "root child counts",
        "left",
        "right",
        &left.graph.root_children,
        &right.graph.root_children,
        args.max_diffs,
    );
    diff_count_map(
        "node path counts",
        "left",
        "right",
        &left.graph.path_counts,
        &right.graph.path_counts,
        args.max_diffs,
    );
    diff_nested_count_map(
        "attribute shape signatures by path",
        "left",
        "right",
        &left.graph.attr_shapes,
        &right.graph.attr_shapes,
        args.max_diffs,
    );

    print_header("Focused Value Signatures");
    for path in FOCUS_PATHS {
        print_value_signatures(path, "left", &left.graph.attr_values);
        print_value_signatures(path, "right", &right.graph.attr_values);
    }

    if args.dump_focus {
        print_header("Focused Trees");
        for path in FOCUS_PATHS {
            dump_focus_path("left", path, left.tree.root(), 5);
            dump_focus_path("right", path, right.tree.root(), 5);
        }
    }

    Ok(())
}

#[derive(Debug)]
struct Args {
    left: PathBuf,
    right: PathBuf,
    max_diffs: usize,
    dump_focus: bool,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut paths = Vec::new();
        let mut max_diffs = 120_usize;
        let mut dump_focus = false;
        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--help" || arg == "-h" {
                print_usage();
                std::process::exit(0);
            } else if arg == "--dump-focus" {
                dump_focus = true;
            } else if arg == "--max-diffs" {
                let value = args.next().context("--max-diffs requires a value")?;
                max_diffs = value
                    .to_string_lossy()
                    .parse()
                    .context("parsing --max-diffs")?;
            } else if arg.to_string_lossy().starts_with("--") {
                bail!("unknown option: {}", arg.to_string_lossy());
            } else {
                paths.push(PathBuf::from(arg));
            }
        }
        if paths.len() != 2 {
            print_usage();
            bail!("expected exactly two FBX paths");
        }
        Ok(Self {
            left: paths.remove(0),
            right: paths.remove(0),
            max_diffs,
            dump_focus,
        })
    }
}

fn print_usage() {
    eprintln!(
        "usage: fbx_graph_diff <known-good-blender.fbx> <candidate-rust.fbx> \
         [--max-diffs N] [--dump-focus]"
    );
}

#[derive(Debug)]
struct LoadedFbx {
    path: PathBuf,
    version: String,
    footer_status: String,
    tree: fbxcel::tree::v7400::Tree,
    graph: GraphSummary,
    dom: DomSummary,
}

impl LoadedFbx {
    fn load(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let reader = BufReader::new(file);
        let (version, tree, footer_status) = match AnyTree::from_seekable_reader(reader)
            .with_context(|| format!("parsing binary FBX {}", path.display()))?
        {
            AnyTree::V7400(version, tree, footer) => {
                let footer_status = match footer {
                    Ok(_) => "ok".to_string(),
                    Err(err) => format!("parse warning: {err}"),
                };
                (
                    format!("{}.{}", version.major(), version.minor()),
                    tree,
                    footer_status,
                )
            }
            tree => bail!("unsupported FBX tree version: {:?}", tree.fbx_version()),
        };
        let dom = DomSummary::load(path);
        let graph = GraphSummary::from_tree(&tree, &dom);
        Ok(Self {
            path: path.to_path_buf(),
            version,
            footer_status,
            tree,
            graph,
            dom,
        })
    }
}

#[derive(Debug, Default)]
struct GraphSummary {
    node_count: usize,
    max_depth: usize,
    root_children: BTreeMap<String, usize>,
    path_counts: BTreeMap<String, usize>,
    attr_shapes: BTreeMap<String, BTreeMap<String, usize>>,
    attr_values: BTreeMap<String, BTreeMap<String, usize>>,
    dom_class_counts: BTreeMap<String, usize>,
    dom_connection_counts: BTreeMap<String, usize>,
}

impl GraphSummary {
    fn from_tree(tree: &fbxcel::tree::v7400::Tree, dom: &DomSummary) -> Self {
        let mut out = Self {
            dom_class_counts: dom.class_counts.clone(),
            dom_connection_counts: dom.connection_counts.clone(),
            ..Self::default()
        };
        for child in tree.root().children() {
            *out.root_children
                .entry(child.name().to_string())
                .or_default() += 1;
            walk_node(child, child.name().to_string(), 1, &mut out);
        }
        out
    }
}

fn walk_node(node: NodeHandle<'_>, path: String, depth: usize, out: &mut GraphSummary) {
    out.node_count += 1;
    out.max_depth = out.max_depth.max(depth);
    *out.path_counts.entry(path.clone()).or_default() += 1;
    *out.attr_shapes
        .entry(path.clone())
        .or_default()
        .entry(attr_shape(node.attributes()))
        .or_default() += 1;
    *out.attr_values
        .entry(path.clone())
        .or_default()
        .entry(attr_value_signature(node.attributes()))
        .or_default() += 1;

    for child in node.children() {
        walk_node(child, format!("{path}/{}", child.name()), depth + 1, out);
    }
}

#[derive(Debug, Default)]
struct DomSummary {
    status: String,
    object_count: usize,
    class_counts: BTreeMap<String, usize>,
    connection_counts: BTreeMap<String, usize>,
    sample_objects: Vec<String>,
}

impl DomSummary {
    fn load(path: &Path) -> Self {
        match load_dom(path) {
            Ok(summary) => summary,
            Err(err) => Self {
                status: format!("load failed: {err:#}"),
                ..Self::default()
            },
        }
    }
}

fn load_dom(path: &Path) -> Result<DomSummary> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(file);
    let doc = match AnyDocument::from_seekable_reader(reader)
        .with_context(|| format!("loading DOM {}", path.display()))?
    {
        AnyDocument::V7400(_version, doc) => doc,
        doc => bail!("unsupported FBX DOM version: {:?}", doc.fbx_version()),
    };

    let mut out = DomSummary {
        status: "ok".to_string(),
        ..DomSummary::default()
    };
    for object in doc.objects() {
        out.object_count += 1;
        let class = object_class(&object);
        *out.class_counts.entry(class.clone()).or_default() += 1;
        if out.sample_objects.len() < 12 {
            out.sample_objects.push(format!(
                "{} id={} name={}",
                class,
                object.object_id().raw(),
                object.name().unwrap_or("")
            ));
        }
        for destination in object.destination_objects() {
            let destination_class = destination
                .object_handle()
                .map(|handle| object_class(&handle))
                .unwrap_or_else(|| "RawObjectId".to_string());
            let label = destination.label().unwrap_or("OO");
            *out.connection_counts
                .entry(format!("{class} -> {destination_class} [{label}]"))
                .or_default() += 1;
        }
    }
    Ok(out)
}

fn object_class(object: &fbxcel_dom::v7400::object::ObjectHandle<'_>) -> String {
    let subclass = object.subclass();
    if subclass.is_empty() {
        object.class().to_string()
    } else {
        format!("{}/{}", object.class(), subclass)
    }
}

fn print_header(title: &str) {
    println!("\n== {title} ==");
}

fn print_file_summary(label: &str, loaded: &LoadedFbx) {
    println!(
        "{label}: {}  fbx={} footer={} nodes={} max_depth={}",
        loaded.path.display(),
        loaded.version,
        loaded.footer_status,
        loaded.graph.node_count,
        loaded.graph.max_depth
    );
}

fn print_dom_summary(label: &str, dom: &DomSummary) {
    println!(
        "{label}: status={} objects={}",
        dom.status, dom.object_count
    );
    if !dom.sample_objects.is_empty() {
        println!("{label}: sample objects");
        for sample in &dom.sample_objects {
            println!("  {sample}");
        }
    }
}

fn diff_count_map(
    title: &str,
    left_label: &str,
    right_label: &str,
    left: &BTreeMap<String, usize>,
    right: &BTreeMap<String, usize>,
    max_diffs: usize,
) {
    println!("\n-- {title} --");
    let mut keys = BTreeSet::new();
    keys.extend(left.keys().cloned());
    keys.extend(right.keys().cloned());
    let mut shown = 0_usize;
    let mut total = 0_usize;
    for key in keys {
        let l = left.get(&key).copied().unwrap_or_default();
        let r = right.get(&key).copied().unwrap_or_default();
        if l == r {
            continue;
        }
        total += 1;
        if shown < max_diffs {
            println!("  {key}: {left_label}={l} {right_label}={r}");
            shown += 1;
        }
    }
    print_diff_footer(total, shown);
}

fn diff_nested_count_map(
    title: &str,
    left_label: &str,
    right_label: &str,
    left: &BTreeMap<String, BTreeMap<String, usize>>,
    right: &BTreeMap<String, BTreeMap<String, usize>>,
    max_diffs: usize,
) {
    println!("\n-- {title} --");
    let mut paths = BTreeSet::new();
    paths.extend(left.keys().cloned());
    paths.extend(right.keys().cloned());
    let mut shown = 0_usize;
    let mut total = 0_usize;
    for path in paths {
        let left_signatures = left.get(&path).cloned().unwrap_or_default();
        let right_signatures = right.get(&path).cloned().unwrap_or_default();
        let mut signatures = BTreeSet::new();
        signatures.extend(left_signatures.keys().cloned());
        signatures.extend(right_signatures.keys().cloned());
        for signature in signatures {
            let l = left_signatures.get(&signature).copied().unwrap_or_default();
            let r = right_signatures
                .get(&signature)
                .copied()
                .unwrap_or_default();
            if l == r {
                continue;
            }
            total += 1;
            if shown < max_diffs {
                println!("  {path} attrs={signature}: {left_label}={l} {right_label}={r}");
                shown += 1;
            }
        }
    }
    print_diff_footer(total, shown);
}

fn print_diff_footer(total: usize, shown: usize) {
    if total == 0 {
        println!("  no differences");
    } else if shown < total {
        println!("  ... shown {shown}/{total} differences");
    } else {
        println!("  shown {shown}/{total} differences");
    }
}

fn print_value_signatures(
    prefix: &str,
    label: &str,
    values: &BTreeMap<String, BTreeMap<String, usize>>,
) {
    let mut printed_header = false;
    for (path, signatures) in values {
        if path != prefix && !path.starts_with(&format!("{prefix}/")) {
            continue;
        }
        if !printed_header {
            println!("-- {label} {prefix} --");
            printed_header = true;
        }
        for (signature, count) in signatures {
            println!("  {path} x{count} attrs={signature}");
        }
    }
    if !printed_header {
        println!("-- {label} {prefix} --");
        println!("  <missing>");
    }
}

fn dump_focus_path(label: &str, target_path: &str, root: NodeHandle<'_>, max_depth: usize) {
    let mut matches = Vec::new();
    for child in root.children() {
        collect_focus_matches(child, child.name().to_string(), target_path, &mut matches);
    }
    println!("-- {label} {target_path} tree matches={} --", matches.len());
    if matches.is_empty() {
        println!("  <missing>");
        return;
    }
    for node in matches {
        dump_tree(node, 1, max_depth);
    }
}

fn collect_focus_matches<'a>(
    node: NodeHandle<'a>,
    path: String,
    target_path: &str,
    out: &mut Vec<NodeHandle<'a>>,
) {
    if path == target_path {
        out.push(node);
    }
    for child in node.children() {
        collect_focus_matches(child, format!("{path}/{}", child.name()), target_path, out);
    }
}

fn dump_tree(node: NodeHandle<'_>, indent: usize, max_depth: usize) {
    let pad = "  ".repeat(indent);
    println!(
        "{pad}{} attrs={}",
        node.name(),
        attr_value_signature(node.attributes())
    );
    if indent >= max_depth {
        let child_count = node.children().count();
        if child_count > 0 {
            println!("{pad}  ... {child_count} child nodes");
        }
        return;
    }
    for child in node.children() {
        dump_tree(child, indent + 1, max_depth);
    }
}

fn attr_shape(attrs: &[AttributeValue]) -> String {
    if attrs.is_empty() {
        return "-".to_string();
    }
    attrs
        .iter()
        .map(attr_shape_one)
        .collect::<Vec<_>>()
        .join(",")
}

fn attr_shape_one(attr: &AttributeValue) -> String {
    match attr {
        AttributeValue::Bool(_) => "bool".to_string(),
        AttributeValue::I16(_) => "i16".to_string(),
        AttributeValue::I32(_) => "i32".to_string(),
        AttributeValue::I64(_) => "i64".to_string(),
        AttributeValue::F32(_) => "f32".to_string(),
        AttributeValue::F64(_) => "f64".to_string(),
        AttributeValue::ArrBool(values) => format!("bool[{}]", values.len()),
        AttributeValue::ArrI32(values) => format!("i32[{}]", values.len()),
        AttributeValue::ArrI64(values) => format!("i64[{}]", values.len()),
        AttributeValue::ArrF32(values) => format!("f32[{}]", values.len()),
        AttributeValue::ArrF64(values) => format!("f64[{}]", values.len()),
        AttributeValue::String(_) => "string".to_string(),
        AttributeValue::Binary(values) => format!("binary[{}]", values.len()),
    }
}

fn attr_value_signature(attrs: &[AttributeValue]) -> String {
    if attrs.is_empty() {
        return "-".to_string();
    }
    attrs
        .iter()
        .map(attr_value_one)
        .collect::<Vec<_>>()
        .join(",")
}

fn attr_value_one(attr: &AttributeValue) -> String {
    match attr {
        AttributeValue::Bool(value) => format!("bool({value})"),
        AttributeValue::I16(value) => format!("i16({value})"),
        AttributeValue::I32(value) => format!("i32({value})"),
        AttributeValue::I64(value) => format!("i64({value})"),
        AttributeValue::F32(value) => format!("f32({value:.6})"),
        AttributeValue::F64(value) => format!("f64({value:.6})"),
        AttributeValue::ArrBool(values) => format!("bool[{}]", values.len()),
        AttributeValue::ArrI32(values) => array_i32_signature(values),
        AttributeValue::ArrI64(values) => array_i64_signature(values),
        AttributeValue::ArrF32(values) => array_f32_signature(values),
        AttributeValue::ArrF64(values) => array_f64_signature(values),
        AttributeValue::String(value) => format!("string({})", quote_short(value)),
        AttributeValue::Binary(values) => format!("binary[{}]", values.len()),
    }
}

fn array_i32_signature(values: &[i32]) -> String {
    let (min, max) = values.iter().fold((None, None), |(min, max), &value| {
        (
            Some(min.map_or(value, |m: i32| m.min(value))),
            Some(max.map_or(value, |m: i32| m.max(value))),
        )
    });
    format!(
        "i32[{}]{} first={}",
        values.len(),
        min_max_suffix(min, max),
        first_values(values, |value| value.to_string())
    )
}

fn array_i64_signature(values: &[i64]) -> String {
    let (min, max) = values.iter().fold((None, None), |(min, max), &value| {
        (
            Some(min.map_or(value, |m: i64| m.min(value))),
            Some(max.map_or(value, |m: i64| m.max(value))),
        )
    });
    format!(
        "i64[{}]{} first={}",
        values.len(),
        min_max_suffix(min, max),
        first_values(values, |value| value.to_string())
    )
}

fn array_f32_signature(values: &[f32]) -> String {
    let finite = values.iter().copied().filter(|value| value.is_finite());
    let (min, max) = finite.fold((None, None), |(min, max), value| {
        (
            Some(min.map_or(value, |m: f32| m.min(value))),
            Some(max.map_or(value, |m: f32| m.max(value))),
        )
    });
    format!(
        "f32[{}]{} first={}",
        values.len(),
        min_max_suffix_float(min, max),
        first_values(values, |value| format!("{value:.6}"))
    )
}

fn array_f64_signature(values: &[f64]) -> String {
    let finite = values.iter().copied().filter(|value| value.is_finite());
    let (min, max) = finite.fold((None, None), |(min, max), value| {
        (
            Some(min.map_or(value, |m: f64| m.min(value))),
            Some(max.map_or(value, |m: f64| m.max(value))),
        )
    });
    format!(
        "f64[{}]{} first={}",
        values.len(),
        min_max_suffix_float(min, max),
        first_values(values, |value| format!("{value:.6}"))
    )
}

fn min_max_suffix<T: std::fmt::Display>(min: Option<T>, max: Option<T>) -> String {
    match (min, max) {
        (Some(min), Some(max)) => format!(" min={min} max={max}"),
        _ => String::new(),
    }
}

fn min_max_suffix_float<T: std::fmt::Display>(min: Option<T>, max: Option<T>) -> String {
    min_max_suffix(min, max)
}

fn first_values<T>(values: &[T], format_one: impl Fn(&T) -> String) -> String {
    let mut out = values
        .iter()
        .take(6)
        .map(format_one)
        .collect::<Vec<_>>()
        .join("|");
    if values.len() > 6 {
        out.push_str("|...");
    }
    out
}

fn quote_short(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    if escaped.len() > 96 {
        format!("\"{}...\"", &escaped[..96])
    } else {
        format!("\"{escaped}\"")
    }
}
