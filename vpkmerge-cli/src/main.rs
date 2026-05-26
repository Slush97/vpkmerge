use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use std::path::{Path, PathBuf};
use vpkmerge_core::{
    extract_portraits, merge, split, CollisionPolicy, MergeOptions, OverlapPolicy, PathPredicate,
    PortraitInfo, SoundEvents, SplitOptions, SplitOutput,
};

#[derive(Parser)]
#[command(
    name = "vpkmerge",
    version,
    about = "Combine multiple VPK files into one (or split one into many).",
    long_about = "Combine multiple Valve Pak (VPK) files into one. Built for Deadlock \
                  modding so players can pre-merge mods and bypass the ~100-mod mount limit.\n\
                  \n\
                  By default, later inputs win on path collision. Use --strict to refuse to \
                  merge when paths collide.\n\
                  \n\
                  See `vpkmerge split --help` for the inverse operation (one VPK to many).",
    subcommand_negates_reqs = true,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Command>,

    /// Output VPK path. Parent directory is created if missing.
    output: Option<PathBuf>,

    /// Input VPK paths. At least 2 required (validated by the engine).
    #[arg(num_args = 0..)]
    inputs: Vec<PathBuf>,

    /// Error out on any path collision instead of letting later inputs win.
    #[arg(long)]
    strict: bool,

    /// Print each path that gets overridden by a later input.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Route entries from one VPK into N output VPKs by path predicate.
    Split(SplitCmd),

    /// Extract and decode hero portrait/card art from a VPK to PNG.
    Portrait(PortraitCmd),

    /// Decode a soundevents (`.vsndevts_c`) file to JSON, optionally edit clip
    /// paths / params and re-emit an uncompressed (loadable) file.
    Soundevents(SoundeventsCmd),
}

#[derive(Args)]
struct SoundeventsCmd {
    /// Path to a `.vsndevts_c` file, or (with --from-vpk) an entry path inside a VPK.
    input: PathBuf,

    /// Read INPUT as an entry path inside this VPK instead of a file on disk
    /// (e.g. `--from-vpk pak01_dir.vpk soundevents/hero/gigawatt.vsndevts_c`).
    #[arg(long, value_name = "VPK")]
    from_vpk: Option<PathBuf>,

    /// After applying edits, re-encode (uncompressed v4) to this path.
    #[arg(long, value_name = "FILE")]
    encode: Option<PathBuf>,

    /// Replace a clip path everywhere in the tree: --swap-vsnd OLD=NEW (repeatable).
    #[arg(long = "swap-vsnd", value_name = "OLD=NEW")]
    swap_vsnd: Vec<String>,

    /// Set a numeric field on one event: --set EVENT/FIELD=NUMBER (repeatable),
    /// e.g. --set "Seven.Wpn.Fire/volume=0.25".
    #[arg(long = "set", value_name = "EVENT/FIELD=NUMBER")]
    set: Vec<String>,
}

#[derive(Args)]
struct PortraitCmd {
    /// Input VPK to read portraits from.
    input: PathBuf,

    /// Directory to write decoded PNGs into (created if missing).
    #[arg(long, value_name = "DIR")]
    out: PathBuf,

    /// Only extract portraits for this hero codename (e.g. "hornet").
    /// Omit to extract every hero in the VPK.
    #[arg(long, value_name = "CODENAME")]
    hero: Option<String>,

    /// Write the JSON manifest to this file instead of stdout.
    #[arg(long, value_name = "FILE")]
    manifest: Option<PathBuf>,
}

#[derive(Args)]
struct SplitCmd {
    /// Input VPK to split.
    input: PathBuf,

    /// JSON plan describing outputs and prefix predicates.
    #[arg(long, value_name = "FILE")]
    plan: PathBuf,

    /// Refuse to split if any path matches more than one output.
    #[arg(long, conflicts_with = "all_matches")]
    strict: bool,

    /// Route each path to EVERY matching output (entries can land in multiple outputs).
    #[arg(long)]
    all_matches: bool,

    /// Optional residual VPK path. Overrides "residual" in the plan if both are set.
    #[arg(long, value_name = "FILE")]
    residual: Option<PathBuf>,

    /// Log each path routed to each output.
    #[arg(short, long)]
    verbose: bool,
}

#[derive(serde::Deserialize)]
struct PlanFile {
    outputs: Vec<PlanOutput>,
    #[serde(default)]
    residual: Option<PathBuf>,
}

#[derive(serde::Deserialize)]
struct PlanOutput {
    path: PathBuf,
    prefixes: Vec<String>,
}

#[derive(serde::Serialize)]
struct Manifest {
    input: String,
    portraits: Vec<ManifestEntry>,
}

#[derive(serde::Serialize)]
struct ManifestEntry {
    source_path: String,
    hero_codename: String,
    variant: String,
    width: u32,
    height: u32,
    format_name: String,
    /// Absolute path to the decoded PNG, or null if not decoded.
    output_path: Option<String>,
    /// Why the texture was skipped (null when decoded).
    skipped_reason: Option<String>,
}

impl From<PortraitInfo> for ManifestEntry {
    fn from(p: PortraitInfo) -> Self {
        Self {
            source_path: p.source_path,
            hero_codename: p.hero_codename,
            variant: p.variant.as_str().to_string(),
            width: p.width,
            height: p.height,
            format_name: p.format_name.to_string(),
            output_path: p.output_path.map(|p| p.display().to_string()),
            skipped_reason: p.skipped_reason,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Some(Command::Split(args)) => run_split(args),
        Some(Command::Portrait(args)) => run_portrait(args),
        Some(Command::Soundevents(args)) => run_soundevents(args),
        None => run_merge(cli),
    }
}

fn run_merge(cli: Cli) -> Result<()> {
    let Some(output) = cli.output else {
        anyhow::bail!("missing OUTPUT. Run `vpkmerge --help` for usage.");
    };

    if cli.verbose && !cli.strict {
        let conflicts = vpkmerge_core::detect_conflicts(&cli.inputs)?;
        for c in &conflicts {
            let winner_idx = *c.owner_indices.last().unwrap();
            let winner = cli.inputs[winner_idx].file_name().map_or_else(
                || cli.inputs[winner_idx].display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            println!("override: {} <- {}", c.path, winner);
        }
    }

    let policy = if cli.strict {
        CollisionPolicy::Error
    } else {
        CollisionPolicy::LastWins
    };

    let report = merge(
        &cli.inputs,
        &output,
        &MergeOptions {
            collision_policy: policy,
            ..Default::default()
        },
    )?;

    println!(
        "wrote {}: {} entries, {} paths overridden from {} inputs",
        report.output_path.display(),
        report.total_entries,
        report.overridden_paths,
        report.inputs,
    );
    Ok(())
}

fn run_split(args: SplitCmd) -> Result<()> {
    let plan = read_plan(&args.plan)?;
    let residual_path = args.residual.or(plan.residual);

    let outputs: Vec<SplitOutput> = plan
        .outputs
        .into_iter()
        .map(|o| SplitOutput {
            path: o.path,
            predicate: PathPredicate::AnyPrefix(o.prefixes),
        })
        .collect();

    let policy = if args.strict {
        OverlapPolicy::Error
    } else if args.all_matches {
        OverlapPolicy::AllMatches
    } else {
        OverlapPolicy::FirstMatch
    };

    if args.verbose {
        log_routing(&args.input, &outputs, policy, residual_path.as_deref())?;
    }

    let report = split(
        &args.input,
        &outputs,
        &SplitOptions {
            overlap_policy: policy,
            residual_path,
        },
    )?;

    println!(
        "split {}: {} input entries",
        args.input.display(),
        report.input_entries
    );
    for o in &report.outputs {
        println!("  {:<6} {}  {} entries", "out", o.path.display(), o.entries);
    }
    if let Some(r) = &report.residual {
        println!(
            "  {:<6} {}  {} entries",
            "resid",
            r.path.display(),
            r.entries
        );
    } else if report.unmatched > 0 {
        println!(
            "  {:<6} (dropped, no residual configured)  {} entries",
            "drop", report.unmatched
        );
    }
    Ok(())
}

fn run_portrait(args: PortraitCmd) -> Result<()> {
    let PortraitCmd {
        input,
        out,
        hero,
        manifest,
    } = args;
    let portraits = extract_portraits(&input, hero.as_deref(), &out)?;

    let decoded = portraits.iter().filter(|p| p.output_path.is_some()).count();
    let skipped = portraits.len() - decoded;
    eprintln!(
        "{}: {} portrait{} found, {decoded} decoded, {skipped} skipped",
        input.display(),
        portraits.len(),
        if portraits.len() == 1 { "" } else { "s" },
    );
    for p in &portraits {
        match (&p.output_path, &p.skipped_reason) {
            (Some(out), _) => eprintln!(
                "  {:<13} {:>4}x{:<4} {:<12} -> {}",
                p.hero_codename,
                p.width,
                p.height,
                p.variant.as_str(),
                out.display()
            ),
            (None, Some(reason)) => eprintln!(
                "  {:<13} {:<12} skipped: {reason}",
                p.hero_codename,
                p.variant.as_str()
            ),
            (None, None) => {}
        }
    }

    let manifest_data = Manifest {
        input: input.display().to_string(),
        portraits: portraits.into_iter().map(ManifestEntry::from).collect(),
    };
    let json = serde_json::to_string_pretty(&manifest_data).context("serializing manifest")?;
    if let Some(path) = &manifest {
        std::fs::write(path, &json).with_context(|| format!("writing {}", path.display()))?;
        eprintln!("manifest: {}", path.display());
    } else {
        println!("{json}");
    }
    Ok(())
}

fn run_soundevents(args: SoundeventsCmd) -> Result<()> {
    let SoundeventsCmd {
        input,
        from_vpk,
        encode,
        swap_vsnd,
        set,
    } = args;

    let label = match &from_vpk {
        Some(vpk) => format!("{} @ {}", input.display(), vpk.display()),
        None => input.display().to_string(),
    };

    let mut se = match &from_vpk {
        Some(vpk) => SoundEvents::from_vpk(vpk, &input.to_string_lossy())?,
        None => SoundEvents::from_file(&input)?,
    };

    // Apply edits (Phase 2). All edits log to stderr.
    for spec in &swap_vsnd {
        let (from, to) = spec
            .split_once('=')
            .with_context(|| format!("--swap-vsnd expects OLD=NEW, got {spec:?}"))?;
        let n = se.swap_vsnd(from, to);
        eprintln!("swap-vsnd: {n} path(s) rewritten {from} -> {to}");
    }
    for spec in &set {
        let (lhs, num) = spec
            .rsplit_once('=')
            .with_context(|| format!("--set expects EVENT/FIELD=NUMBER, got {spec:?}"))?;
        let (event, field) = lhs
            .rsplit_once('/')
            .with_context(|| format!("--set expects EVENT/FIELD=NUMBER, got {spec:?}"))?;
        let value: f64 = num
            .parse()
            .with_context(|| format!("--set value must be a number, got {num:?}"))?;
        if !se.set_event_field(event, field, value) {
            anyhow::bail!("--set: no event named {event:?} in {label}");
        }
        eprintln!("set: {event}/{field} = {value}");
    }

    // Human-readable summary to stderr.
    let summaries = se.summaries();
    eprintln!(
        "{label}: {} event{}",
        summaries.len(),
        if summaries.len() == 1 { "" } else { "s" }
    );
    for s in &summaries {
        let base = s
            .base
            .as_deref()
            .map_or(String::new(), |b| format!("  base={b}"));
        let vol = s.volume.map_or(String::new(), |v| format!("  volume={v}"));
        eprintln!(
            "  {:<34} {} sound{}{base}{vol}",
            s.name,
            s.vsnd_count,
            if s.vsnd_count == 1 { "" } else { "s" }
        );
    }

    // Either re-emit a file (encode path) or dump JSON to stdout.
    if let Some(out) = &encode {
        let bytes = se.encode()?;
        std::fs::write(out, &bytes).with_context(|| format!("writing {}", out.display()))?;
        eprintln!(
            "wrote {}: {} bytes uncompressed (original was {} bytes)",
            out.display(),
            bytes.len(),
            se.original_len()
        );
    } else {
        let json = serde_json::to_string_pretty(&se.to_json()).context("serializing JSON")?;
        println!("{json}");
    }
    Ok(())
}

fn read_plan(path: &Path) -> Result<PlanFile> {
    let bytes = std::fs::read(path).with_context(|| format!("reading plan {}", path.display()))?;
    let plan: PlanFile = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing plan {}", path.display()))?;
    Ok(plan)
}

fn log_routing(
    input: &Path,
    outputs: &[SplitOutput],
    policy: OverlapPolicy,
    residual: Option<&Path>,
) -> Result<()> {
    let info = vpkmerge_core::inspect(input)?;
    for path in &info.file_paths {
        let mut hits: Vec<&Path> = Vec::new();
        for o in outputs {
            if predicate_matches(&o.predicate, path) {
                hits.push(&o.path);
                if policy == OverlapPolicy::FirstMatch {
                    break;
                }
            }
        }
        if hits.is_empty() {
            if let Some(r) = residual {
                println!("residual: {path}  ->  {}", r.display());
            } else {
                println!("dropped : {path}");
            }
        } else {
            for h in hits {
                println!("route   : {path}  ->  {}", h.display());
            }
        }
    }
    Ok(())
}

fn predicate_matches(pred: &PathPredicate, path: &str) -> bool {
    match pred {
        PathPredicate::AnyPrefix(prefixes) => prefixes.iter().any(|p| path.starts_with(p)),
    }
}
