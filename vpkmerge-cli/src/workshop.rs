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
    /// Recolor current-game dash or heavy-melee effects, optionally replacing dash trails with image stamps.
    Movement {
        #[arg(long)]
        vpk: PathBuf,
        #[arg(long, value_parser = ["dash", "heavy-melee"])]
        kind: String,
        #[arg(long)]
        color: String,
        #[arg(long)]
        image: Option<PathBuf>,
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
        WorkshopCommand::Movement {
            vpk,
            kind,
            color,
            image,
            encode_vpk,
        } => {
            vpkmerge_core::workshop::movement_fx(&vpk, &kind, &color, image.as_deref(), &encode_vpk)
        }
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
