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
        /// White-on-black UV mask PNG limiting the finish to an area; omit for the whole part.
        #[arg(long)]
        mask: Option<PathBuf>,
        #[arg(long)]
        encode_vpk: PathBuf,
    },
    /// Make a part glow, optionally only inside a mask, steady, pulsing or cycling through colors.
    Glow {
        /// Source VPKs in highest-priority-first order, including base game fallback.
        #[arg(long, required = true)]
        vpk: Vec<PathBuf>,
        #[arg(long)]
        entry: String,
        /// White-on-black UV mask PNG limiting the glow; omit to glow the whole part.
        #[arg(long)]
        mask: Option<PathBuf>,
        /// Glow color as RRGGBB.
        #[arg(long, default_value = "33ccff")]
        color: String,
        #[arg(long, default_value_t = 4.0)]
        strength: f64,
        #[arg(long, value_parser = ["steady", "pulse", "rainbow"], default_value = "steady")]
        animation: String,
        #[arg(long)]
        encode_vpk: PathBuf,
    },
    /// Set transparency while separating opacity from an opaque part's packed metalness.
    ///
    /// Ghost is an experimental research recipe and is not supported by the skin editor.
    Surface {
        #[arg(long, required = true)]
        vpk: Vec<PathBuf>,
        #[arg(long)]
        entry: String,
        #[arg(long, value_parser = ["translucent", "ghost"])]
        mode: String,
        #[arg(long)]
        opacity: f64,
        #[arg(long)]
        encode_vpk: PathBuf,
    },
}

/// Parse `RRGGBB` (optionally `#`-prefixed) into 0..1 channels.
fn rgb(color: &str) -> Result<[f64; 3]> {
    let hex = color.trim_start_matches('#');
    anyhow::ensure!(
        hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()),
        "--color must be RRGGBB"
    );
    let channel =
        |at: usize| f64::from(u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(0)) / 255.0;
    Ok([channel(0), channel(2), channel(4)])
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
        WorkshopCommand::Glow {
            vpk,
            entry,
            mask,
            color,
            strength,
            animation,
            encode_vpk,
        } => {
            let mask = mask.map(std::fs::read).transpose()?;
            vpkmerge_core::workshop::glow_material(
                &vpk,
                &entry,
                mask.as_deref(),
                rgb(&color)?,
                strength,
                match animation.as_str() {
                    "pulse" => vpkmerge_core::workshop::GlowAnimation::Pulse,
                    "rainbow" => vpkmerge_core::workshop::GlowAnimation::Rainbow,
                    _ => vpkmerge_core::workshop::GlowAnimation::Steady,
                },
                &encode_vpk,
            )
        }
        WorkshopCommand::Finish {
            vpk,
            entry,
            roughness,
            metalness,
            mask,
            encode_vpk,
        } => {
            let mask = mask.map(std::fs::read).transpose()?;
            vpkmerge_core::workshop::finish_material(
                &vpk,
                &entry,
                roughness,
                metalness,
                mask.as_deref(),
                &encode_vpk,
            )
        }
        WorkshopCommand::Surface {
            vpk,
            entry,
            mode,
            opacity,
            encode_vpk,
        } => vpkmerge_core::workshop::surface_material(&vpk, &entry, &mode, opacity, &encode_vpk),
    }
}
