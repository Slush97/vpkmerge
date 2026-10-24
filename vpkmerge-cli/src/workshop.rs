use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args)]
pub struct WorkshopArgs {
    #[command(subcommand)]
    command: WorkshopCommand,
}

#[derive(Subcommand)]
enum WorkshopCommand {
    /// Export current-game, non-additive NM actions on a selected hero rig.
    Motions {
        #[arg(long, required = true)]
        vpk: Vec<PathBuf>,
        #[arg(long)]
        entry: String,
        /// Named clip: `NAME=models/.../clip.vnmclip_c` or `NAME=legacy:clip_name` (repeatable).
        #[arg(long, required = true)]
        clip: Vec<String>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Recolor current-game dash or heavy-melee effects, optionally replacing dash trails with image stamps.
    Movement {
        #[arg(long)]
        vpk: PathBuf,
        #[arg(long, value_parser = ["dash", "heavy-melee"])]
        kind: String,
        #[arg(long)]
        color: String,
        #[arg(long, conflicts_with = "rainbow")]
        end_color: Option<String>,
        #[arg(long)]
        rainbow: bool,
        #[arg(long)]
        image: Option<PathBuf>,
        #[arg(long, default_value_t = 1.0)]
        brightness: f64,
        #[arg(long)]
        illuminated: bool,
        #[arg(long)]
        outline_color: Option<String>,
        #[arg(long)]
        encode_vpk: PathBuf,
    },
    /// Set a single part's roughness and metalness without changing shared masks.
    Finish {
        /// Source VPKs in highest-priority-first order, including base game fallback.
        #[arg(long, required = true)]
        vpk: Vec<PathBuf>,
        #[arg(long)]
        entry: String,
        #[arg(long)]
        roughness: f64,
        #[arg(long)]
        metalness: f64,
        #[arg(long)]
        encode_vpk: PathBuf,
    },
}

pub fn run(args: WorkshopArgs) -> Result<()> {
    match args.command {
        WorkshopCommand::Motions {
            vpk,
            entry,
            clip,
            out,
        } => {
            let clips = clip
                .iter()
                .map(|value| {
                    value
                        .split_once('=')
                        .map(|(name, path)| (name.to_string(), path.to_string()))
                        .ok_or_else(|| anyhow::anyhow!("Expected NAME=RESOURCE for --clip"))
                })
                .collect::<Result<Vec<_>>>()?;
            vpkmerge_core::workshop_motion::export_motions(&vpk, &entry, &clips, &out)
        }
        WorkshopCommand::Movement {
            vpk,
            kind,
            color,
            end_color,
            rainbow,
            image,
            brightness,
            illuminated,
            outline_color,
            encode_vpk,
        } => vpkmerge_core::workshop::movement_fx(
            &vpk,
            &kind,
            &color,
            &vpkmerge_core::workshop::MovementFxOptions {
                end_color: end_color.as_deref(),
                rainbow,
                image: image.as_deref(),
                brightness: Some(brightness),
                illuminated,
                outline_color: outline_color.as_deref(),
            },
            &encode_vpk,
        ),
        WorkshopCommand::Finish {
            vpk,
            entry,
            roughness,
            metalness,
            encode_vpk,
        } => vpkmerge_core::workshop::finish_material(
            &vpk,
            &entry,
            roughness,
            metalness,
            &encode_vpk,
        ),
    }
}
