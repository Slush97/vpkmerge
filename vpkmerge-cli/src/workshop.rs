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
    /// Recolor current-game dash, heavy-melee or parry effects, optionally replacing dash trails or the
    /// parry shield and stun icons with image stamps.
    Movement {
        #[arg(long)]
        vpk: PathBuf,
        #[arg(long, value_parser = ["dash", "heavy-melee", "parry"])]
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
        /// Parry only: the icon over a parried enemy.
        #[arg(long)]
        stun_image: Option<PathBuf>,
        /// Parry only: one part's colors, `PART=SPEC` with PART shield, catch or stun and SPEC
        /// `original`, `rainbow`, `#RRGGBB` or `#RRGGBB:#RRGGBB`. Parts not given take --color.
        #[arg(long = "part")]
        parts: Vec<String>,
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
        /// Pulse and rainbow speed, 1 is about a two second pulse.
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        /// Glow in the part's own texture colors instead of --color.
        #[arg(long)]
        paint_colors: bool,
        #[arg(long)]
        encode_vpk: PathBuf,
    },
    /// Play a grid of frames on one part of a model, like a GIF.
    ///
    /// The frames are PNGs in the part's UV space, laid over its color texture by their alpha.
    /// The addon patches the model (the part's UVs move onto one cell of the grid), the material
    /// and the part's textures. For a second part of the same model, pass the first addon ahead
    /// of the base game with --vpk and merge the two addons.
    Flipbook {
        /// Source VPKs in highest-priority-first order, including base game fallback.
        #[arg(long, required = true)]
        vpk: Vec<PathBuf>,
        /// The model that draws the part, e.g. `models/heroes_wip/abrams/abrams.vmdl_c`.
        #[arg(long)]
        model: String,
        /// The part's material.
        #[arg(long)]
        entry: String,
        /// Folder of frame PNGs, played in file name order: columns x rows of them.
        #[arg(long)]
        frames: PathBuf,
        /// Cells across: 1, 2, 4, 8 or 16.
        #[arg(long)]
        columns: u32,
        /// Cells down: 1, 2, 4, 8 or 16.
        #[arg(long, default_value_t = 1)]
        rows: u32,
        /// Time frames per second.
        #[arg(long, default_value_t = 8.0)]
        fps: f64,
        /// What picks the row. `time`: every cell is a time frame. `health`: each row is a
        /// state, the first at full health and the last near death. `speed`: the first row
        /// standing still, the last at --full-speed.
        #[arg(long, value_parser = ["time", "health", "speed"], default_value = "time")]
        rows_follow: String,
        /// With `--rows-follow speed`: units per second that reach the last row.
        #[arg(long, default_value_t = 400.0)]
        full_speed: f64,
        /// Largest edge of a grid texture, a power of two from 256 to 8192.
        #[arg(long, default_value_t = 4096)]
        max_texture: u32,
        /// Keep the normal, ambient occlusion and glow textures as they are (smaller addon,
        /// sharper surface). Uses the shader's texture transforms.
        #[arg(long)]
        keep_detail_maps: bool,
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
        /// White-on-black UV mask PNG: only this area turns see-through (translucent mode only).
        #[arg(long)]
        mask: Option<PathBuf>,
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

#[allow(clippy::too_many_lines)]
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
            stun_image,
            parts,
            encode_vpk,
        } => vpkmerge_core::workshop::movement_fx(
            &vpk,
            &kind,
            &color,
            &vpkmerge_core::workshop::MovementFxOptions {
                parts: &parts
                    .iter()
                    .map(|spec| vpkmerge_core::workshop::parse_parry_part(spec))
                    .collect::<Result<Vec<_>>>()?,
                end_color: end_color.as_deref(),
                rainbow,
                image: image.as_deref(),
                brightness: Some(brightness),
                illuminated,
                outline_color: outline_color.as_deref(),
                stun_image: stun_image.as_deref(),
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
            speed,
            paint_colors,
            encode_vpk,
        } => {
            let mask = mask.map(std::fs::read).transpose()?;
            let style = vpkmerge_core::workshop::GlowStyle {
                color: rgb(&color)?,
                strength,
                animation: match animation.as_str() {
                    "pulse" => vpkmerge_core::workshop::GlowAnimation::Pulse,
                    "rainbow" => vpkmerge_core::workshop::GlowAnimation::Rainbow,
                    _ => vpkmerge_core::workshop::GlowAnimation::Steady,
                },
                speed,
                paint_colors,
            };
            vpkmerge_core::workshop::glow_material(
                &vpk,
                &entry,
                mask.as_deref(),
                &style,
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
        WorkshopCommand::Flipbook {
            vpk,
            model,
            entry,
            frames,
            columns,
            rows,
            fps,
            rows_follow,
            full_speed,
            max_texture,
            keep_detail_maps,
            encode_vpk,
        } => {
            use vpkmerge_core::workshop::flipbook::{
                flipbook_material, FlipbookOptions, FlipbookRows,
            };
            let mut paths: Vec<PathBuf> = std::fs::read_dir(&frames)?
                .filter_map(|file| file.ok().map(|file| file.path()))
                .filter(|path| {
                    path.extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
                })
                .collect();
            paths.sort();
            let frames = paths
                .iter()
                .map(std::fs::read)
                .collect::<std::io::Result<Vec<_>>>()?;
            let report = flipbook_material(
                &vpk,
                &model,
                &entry,
                &frames,
                &FlipbookOptions {
                    columns,
                    rows,
                    fps,
                    rows_follow: match rows_follow.as_str() {
                        "health" => FlipbookRows::Health,
                        "speed" => FlipbookRows::Speed { full: full_speed },
                        _ => FlipbookRows::Time,
                    },
                    max_texture,
                    keep_detail_maps,
                },
                &encode_vpk,
            )?;
            println!(
                "{} vertices moved onto one of {columns} x {rows} cells ({:.2} x {:.2} tiles per cell)",
                report.vertices, report.cell_extent[0], report.cell_extent[1]
            );
            for texture in &report.textures {
                println!(
                    "  {} {}x{}  {}",
                    texture.slots.join("+"),
                    texture.width,
                    texture.height,
                    texture.path
                );
            }
            Ok(())
        }
        WorkshopCommand::Surface {
            vpk,
            entry,
            mode,
            opacity,
            mask,
            encode_vpk,
        } => match mask {
            Some(mask) => {
                anyhow::ensure!(
                    mode == "translucent",
                    "--mask only works with --mode translucent"
                );
                vpkmerge_core::workshop::surface_area_material(
                    &vpk,
                    &entry,
                    opacity,
                    &std::fs::read(mask)?,
                    &encode_vpk,
                )
            }
            None => {
                vpkmerge_core::workshop::surface_material(&vpk, &entry, &mode, opacity, &encode_vpk)
            }
        },
    }
}
