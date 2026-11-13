//! Flipbook materials: one part of a model plays a grid of frames, like a GIF.
//!
//! `pbr.vfx` adds `fract(g_vAlbedoScrollSpeed1 * t)` to the UV of its colour
//! textures, where `t` is the game time, rounded to steps of
//! `g_flAlbedoScrollQuantize1` seconds when that is not zero. So a texture
//! that holds a grid of frames, a mesh whose UVs cover one cell, and a scroll
//! of one cell per step make the part step through the frames.
//!
//! Three things have to change together, and [`flipbook_material`] writes all
//! of them into one addon VPK:
//!
//! - the model: the part's UVs shrink onto one cell (other parts are untouched);
//! - the textures: each becomes a grid, written at a part-scoped path so a
//!   texture shared with another material keeps its original;
//! - the material: the scroll speed and step, and the new texture paths.
//!
//! The shader scrolls the colour, tint mask and alt colour textures together,
//! so the frames are whole-part overlays: a picture that changes in one spot
//! is the same part painted `columns * rows` times.

use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use image::{imageops, RgbaImage};

use super::{bind_finish_textures, compiled, open, read};
use crate::vmat_style::{patch_vmat_params, VmatEdit};

/// The clock `pbr.vfx` scrolls by, as a material expression can read it.
/// `time()` is a different clock (seconds since the engine started), so an
/// expression that has to cancel the shader's own multiplication uses this one.
const GAME_TIME: &str = "$ent_gametime";

/// Texture slots `pbr.vfx` samples through a transform of their own once
/// `F_ENABLE_TEXTURE_TRANSFORMS` is on, with the transform's parameter stem and
/// the switch that moves the slot to the second UV set.
const DETAIL_LAYERS: &[(&str, &str, Option<&str>)] = &[
    ("g_tNormalRoughness", "Normal", None),
    (
        "g_tAmbientOcclusion",
        "AmbientOcclusion",
        Some("g_bUseSecondaryUvForAmbientOcclusion"),
    ),
    (
        "g_tSelfIllumMask",
        "SelfIllum",
        Some("g_bUseSecondaryUvForSelfIllum1"),
    ),
];

/// What picks the row of the frame grid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlipbookRows {
    /// Every cell is a time frame: the part plays `columns * rows` frames in a
    /// loop. Static material values only.
    Time,
    /// Columns are time frames; the row follows the hero's health, the first
    /// row at full health and the last near death.
    Health,
    /// Columns are time frames; the row follows the hero's speed, the first
    /// row standing still and the last at `full` units per second or faster.
    Speed { full: f64 },
}

/// How a flipbook is laid out and driven.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlipbookOptions {
    /// Cells across. A power of two up to 16.
    pub columns: u32,
    /// Cells down. A power of two up to 16.
    pub rows: u32,
    /// Time frames per second, 0.1 to 60.
    pub fps: f64,
    pub rows_follow: FlipbookRows,
    /// Largest edge of a grid texture: a power of two from 256 to 8192. Each
    /// cell gets this divided by the cell count on that axis, or the source
    /// texture's size if that is smaller.
    pub max_texture: u32,
    /// Leave the normal, ambient occlusion and self-illumination textures as
    /// they are, and scale their lookups back up through the shader's texture
    /// transforms. The addon is smaller and the part keeps its surface detail.
    /// Needs the material's shader variant with texture transforms.
    pub keep_detail_maps: bool,
}

/// One grid texture written for the part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlipbookTexture {
    /// The material's texture slots bound to it.
    pub slots: Vec<String>,
    /// Its resource path (`.vtex`).
    pub path: String,
    pub width: u32,
    pub height: u32,
}

/// What [`flipbook_material`] wrote.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FlipbookReport {
    /// Vertices of the part whose UVs moved onto a cell.
    pub vertices: usize,
    /// How many texture tiles a cell spans on each axis. 1 unless a UV island
    /// of the part runs past the unit square, which costs that much resolution.
    pub cell_extent: [f32; 2],
    pub textures: Vec<FlipbookTexture>,
}

/// Where each frame sits in a grid texture.
#[derive(Clone, Copy, Debug)]
pub(super) struct Layout {
    pub(super) columns: u32,
    pub(super) rows: u32,
    pub(super) cell: [u32; 2],
    /// One texture tile at cell resolution; a cell is this, repeated over
    /// `cell_extent` tiles.
    pub(super) tile: [u32; 2],
    /// Step `k` of a single clock moves the UV by `(k / columns, k / cells)`,
    /// so each column sits lower than the one before by a fraction of a cell.
    /// A grid whose column and row are set separately is not staggered.
    pub(super) staggered: bool,
}

impl Layout {
    pub(super) fn total(&self) -> [u32; 2] {
        [self.cell[0] * self.columns, self.cell[1] * self.rows]
    }

    /// Top-left texel of frame `k`. A staggered frame may run past the bottom
    /// edge and continue from the top.
    pub(super) fn origin(&self, k: u32) -> [u32; 2] {
        let x = (k % self.columns) * self.cell[0];
        let y = if self.staggered {
            (k * self.cell[1] / self.columns) % self.total()[1]
        } else {
            (k / self.columns) * self.cell[1]
        };
        [x, y]
    }
}

/// The scroll speed and step that walk the grid.
pub(super) fn scroll_edits(options: &FlipbookOptions) -> Result<Vec<VmatEdit>> {
    let (columns, rows) = (f64::from(options.columns), f64::from(options.rows));
    let row = match options.rows_follow {
        FlipbookRows::Time => {
            return Ok(vec![
                VmatEdit::Vector {
                    name: "g_vAlbedoScrollSpeed1".into(),
                    value: [
                        options.fps / columns,
                        options.fps / (columns * rows),
                        0.0,
                        0.0,
                    ],
                },
                VmatEdit::Float {
                    name: "g_flAlbedoScrollQuantize1".into(),
                    value: 1.0 / options.fps,
                },
            ]);
        }
        FlipbookRows::Health => format!("floor(clamp(1-$ent_health,0,.999)*{rows})"),
        FlipbookRows::Speed { full } => {
            format!("floor(clamp(length($ent_abs_velocity)/{full:.3},0,.999)*{rows})")
        }
    };
    // The shader multiplies the speed by the game time, so dividing the wanted
    // offset by the same clock leaves exactly the offset.
    let x = if options.columns > 1 {
        let column = format!(
            "floor(frac({GAME_TIME}*{:.6})*{columns})",
            options.fps / columns
        );
        format!("{column}/{columns}/{GAME_TIME}")
    } else {
        "0".to_string()
    };
    let y = format!("{row}/{rows}/{GAME_TIME}");
    Ok(vec![
        VmatEdit::expr("g_vAlbedoScrollSpeed1", &format!("float2({x},{y})"))?,
        VmatEdit::Float {
            name: "g_flAlbedoScrollQuantize1".into(),
            value: 0.0,
        },
    ])
}

/// With texture transforms on, the shader samples a detail layer at
/// `scale * (uv - 0.5) + 0.5 + offset`. The mesh UVs were divided by `scale`,
/// so this brings the layer back to the original lookup.
fn detail_edits(material: &morphic::material::Material, scale: [f64; 2]) -> Vec<VmatEdit> {
    let offset = [
        (0.5 * scale[0] - 0.5).rem_euclid(1.0),
        (0.5 * scale[1] - 0.5).rem_euclid(1.0),
    ];
    let mut edits = vec![VmatEdit::Int {
        name: "F_ENABLE_TEXTURE_TRANSFORMS".into(),
        value: 1,
    }];
    for (_, stem, _) in DETAIL_LAYERS
        .iter()
        .filter(|layer| !on_second_uv(material, layer.2))
    {
        edits.push(VmatEdit::Vector {
            name: format!("g_v{stem}TexcoordScale1"),
            value: [scale[0], scale[1], 0.0, 0.0],
        });
        edits.push(VmatEdit::Vector {
            name: format!("g_v{stem}TexcoordOffset1"),
            value: [offset[0], offset[1], 0.0, 0.0],
        });
    }
    edits
}

fn on_second_uv(material: &morphic::material::Material, switch: Option<&str>) -> bool {
    switch.is_some_and(|name| material.int_params.get(name).copied().unwrap_or(0) != 0)
}

/// Whether the slot keeps its original texture: it reads the second UV set,
/// which the flipbook does not move, or its lookup is scaled back by
/// [`detail_edits`].
fn keeps_original(
    material: &morphic::material::Material,
    slot: &str,
    keep_detail_maps: bool,
) -> bool {
    DETAIL_LAYERS
        .iter()
        .find(|layer| layer.0 == slot)
        .is_some_and(|layer| keep_detail_maps || on_second_uv(material, layer.2))
}

pub(super) fn is_flat(image: &RgbaImage) -> bool {
    let first = image.get_pixel(0, 0);
    image.pixels().all(|pixel| pixel == first)
}

/// A grid of `source`, one cell per frame, with `overlays[k]` laid over frame
/// `k` when overlays are given.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn build_grid(source: &RgbaImage, overlays: Option<&[RgbaImage]>, layout: &Layout) -> RgbaImage {
    let [tile_w, tile_h] = layout.tile;
    let [total_w, total_h] = layout.total();
    let tile = imageops::resize(source, tile_w, tile_h, imageops::FilterType::Lanczos3);
    let cell_of = |tile: &RgbaImage| {
        RgbaImage::from_fn(layout.cell[0], layout.cell[1], |x, y| {
            *tile.get_pixel(x % tile_w, y % tile_h)
        })
    };
    let plain = cell_of(&tile);
    let mut grid = RgbaImage::new(total_w, total_h);
    for k in 0..layout.columns * layout.rows {
        let painted = overlays.map(|overlays| {
            let overlay = imageops::resize(
                &overlays[k as usize],
                tile_w,
                tile_h,
                imageops::FilterType::Lanczos3,
            );
            let mut frame = tile.clone();
            for (base, over) in frame.pixels_mut().zip(overlay.pixels()) {
                let alpha = f32::from(over[3]) / 255.0;
                for c in 0..3 {
                    base[c] = (f32::from(over[c]) * alpha + f32::from(base[c]) * (1.0 - alpha))
                        .round() as u8;
                }
            }
            cell_of(&frame)
        });
        let cell = painted.as_ref().unwrap_or(&plain);
        let [x0, y0] = layout.origin(k);
        for (x, y, pixel) in cell.enumerate_pixels() {
            grid.put_pixel(x0 + x, (y0 + y) % total_h, *pixel);
        }
    }
    grid
}

fn validate(options: &FlipbookOptions, frames: usize) -> Result<()> {
    for (name, cells) in [("columns", options.columns), ("rows", options.rows)] {
        ensure!(
            cells.is_power_of_two() && cells <= 16,
            "Flipbook {name} must be 1, 2, 4, 8 or 16"
        );
    }
    ensure!(
        options.columns * options.rows > 1,
        "A flipbook needs more than one frame"
    );
    ensure!(
        frames == (options.columns * options.rows) as usize,
        "Expected {} frames ({} columns x {} rows), got {frames}",
        options.columns * options.rows,
        options.columns,
        options.rows
    );
    ensure!(
        options.fps.is_finite() && (0.1..=60.0).contains(&options.fps),
        "Flipbook speed must be between 0.1 and 60 frames per second"
    );
    ensure!(
        options.max_texture.is_power_of_two() && (256..=8192).contains(&options.max_texture),
        "Largest texture edge must be a power of two from 256 to 8192"
    );
    if let FlipbookRows::Speed { full } = options.rows_follow {
        ensure!(
            full.is_finite() && full > 0.0,
            "Full speed must be above zero"
        );
    }
    Ok(())
}

/// Turn one part of a model into a flipbook: `frames` play on the part named
/// by the material `entry`, on the model `model`, read from `paths` (highest
/// priority first, the base game last).
///
/// `frames` are PNGs in the part's UV space, laid over its colour texture by
/// their alpha, in row-major order: all `columns * rows` time frames for
/// [`FlipbookRows::Time`], otherwise one row of `columns` time frames per
/// state. They need not match the texture's size.
///
/// The addon holds the patched model, the material and the grid textures. To
/// give a second part of the same model a flipbook, pass the first addon ahead
/// of the base game in `paths` and merge the two addons, later one winning.
///
/// # Errors
/// Rejects other shaders than `pbr.vfx`, a material that already scrolls, a
/// material no draw call of the model uses, a part that shares vertices with
/// another material, HDR and `YCoCg` textures, and out-of-range options.
#[allow(clippy::too_many_lines)]
pub fn flipbook_material(
    paths: &[PathBuf],
    model: &str,
    entry: &str,
    frames: &[Vec<u8>],
    options: &FlipbookOptions,
    output: &Path,
) -> Result<FlipbookReport> {
    validate(options, frames.len())?;
    for path in [model, entry] {
        ensure!(
            !path.contains("..") && !path.contains('\\') && !path.contains(':'),
            "Invalid resource path"
        );
    }
    let overlays = frames
        .iter()
        .map(|png| {
            Ok(
                image::load_from_memory_with_format(png, image::ImageFormat::Png)
                    .context("Decoding flipbook frame PNG")?
                    .to_rgba8(),
            )
        })
        .collect::<Result<Vec<_>>>()?;

    let paks = open(paths)?;
    let (model, entry) = (compiled(model), compiled(entry));
    let original = read(&paks, &entry)?;
    let material = morphic::material::parse(&original)?;
    ensure!(
        material.shader_name == "pbr.vfx",
        "Only pbr.vfx supports workshop flipbooks"
    );
    ensure!(
        !material
            .dynamic_params
            .keys()
            .any(|name| name.starts_with("g_vAlbedoScrollSpeed")
                || name.starts_with("g_flAlbedoScrollQuantize"))
            && material
                .vector_params
                .get("g_vAlbedoScrollSpeed1")
                .is_none_or(|speed| speed[0] == 0.0 && speed[1] == 0.0),
        "This part already scrolls its texture"
    );

    // The model: move the part's UVs onto one cell. Islands outside the unit
    // square come back by whole tiles first; what still runs past 1 sets how
    // many tiles a cell has to hold.
    let draw_material = entry.trim_end_matches("_c");
    let source_model = read(&paks, &model)?;
    let (_, bounds) =
        morphic::model::remap_material_uvs(&source_model, draw_material, true, |uv| uv)?;
    ensure!(
        bounds.vertices > 0,
        "No part of {model} is drawn with {draw_material}"
    );
    let extent = [0, 1].map(|axis| {
        if bounds.uv_max[axis] > 1.0 {
            bounds.uv_max[axis] + 0.002
        } else {
            1.0
        }
    });
    #[allow(clippy::cast_precision_loss)]
    let scale = [
        options.columns as f32 * extent[0],
        options.rows as f32 * extent[1],
    ];
    let (patched_model, _) =
        morphic::model::remap_material_uvs(&source_model, draw_material, true, |uv| {
            [uv[0] / scale[0], uv[1] / scale[1]]
        })?;

    // The material: the scroll, and the detail layers' way back.
    let mut edits = scroll_edits(options)?;
    if options.keep_detail_maps {
        edits.extend(detail_edits(&material, scale.map(f64::from)));
    }
    let (mut vmat, stats) = patch_vmat_params(&original, &edits)?;
    ensure!(
        stats.failed.is_empty(),
        "Flipbook parameters could not be patched: {:?}",
        stats.failed
    );

    // The textures: one grid per texture the part samples through its own UVs.
    let mut by_path: std::collections::BTreeMap<&str, Vec<&str>> =
        std::collections::BTreeMap::new();
    for (slot, path) in &material.texture_params {
        by_path
            .entry(path.as_str())
            .or_default()
            .push(slot.as_str());
    }
    let folder = format!(
        "materials/grimoire/flipbook/{}",
        entry.trim_end_matches(".vmat_c")
    )
    .to_ascii_lowercase();
    let mut report = FlipbookReport {
        vertices: bounds.vertices,
        cell_extent: extent,
        textures: Vec::new(),
    };
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut bindings: Vec<(String, String)> = Vec::new();
    for (path, slots) in by_path {
        // Only the slots that follow the part's moved UVs take the grid; a
        // slot that keeps its original stays bound to it even when it shares
        // the texture with one that moves.
        let slots: Vec<&str> = slots
            .into_iter()
            .filter(|slot| !keeps_original(&material, slot, options.keep_detail_maps))
            .collect();
        if slots.is_empty() {
            continue;
        }
        let bytes = read(&paks, &compiled(path))?;
        let info = morphic::inspect(&bytes)?;
        let decoded = morphic::crop_to_actual(
            &morphic::decode(&bytes)?,
            u32::from(info.actual_width),
            u32::from(info.actual_height),
        );
        let morphic::ImageData::Rgba8(pixels) = decoded.data else {
            bail!("{path} is an HDR texture, which a flipbook cannot hold");
        };
        let source = RgbaImage::from_raw(decoded.width, decoded.height, pixels)
            .context("Texture size does not match its pixels")?;
        let animated = slots.contains(&"g_tColor");
        // A flat texture reads the same in every cell as it does whole.
        if !animated && is_flat(&source) {
            continue;
        }
        // A staggered column sits `cell height / columns` lower than the one
        // before, so the height has to divide by the column count.
        let cell = [
            (source.width().next_power_of_two()).clamp(4, options.max_texture / options.columns),
            (source.height().next_power_of_two())
                .clamp(options.columns.max(4), options.max_texture / options.rows),
        ];
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let tile = [0, 1].map(|axis| ((cell[axis] as f32 / extent[axis]).round() as u32).max(1));
        let layout = Layout {
            columns: options.columns,
            rows: options.rows,
            cell,
            tile,
            staggered: options.rows_follow == FlipbookRows::Time,
        };
        let grid = build_grid(&source, animated.then_some(overlays.as_slice()), &layout);
        let [width, height] = layout.total();
        let texture = crate::trippy::encode_part_texture(
            &bytes,
            &morphic::Image {
                width,
                height,
                data: morphic::ImageData::Rgba8(grid.into_raw()),
            },
        )
        .with_context(|| format!("Encoding the grid for {path}"))?;
        let new_path = format!("{folder}/{}.vtex", slots[0].to_ascii_lowercase());
        for slot in &slots {
            bindings.push(((*slot).to_string(), new_path.clone()));
        }
        files.push((compiled(&new_path), texture));
        report.textures.push(FlipbookTexture {
            slots: slots.iter().map(|slot| (*slot).to_string()).collect(),
            path: new_path,
            width,
            height,
        });
    }
    ensure!(
        report
            .textures
            .iter()
            .any(|t| t.slots.iter().any(|s| s == "g_tColor")),
        "This part has no colour texture to animate"
    );
    vmat = bind_finish_textures(&vmat, &material.name, &bindings)?;

    files.push((model, patched_model));
    files.push((entry, vmat));
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    crate::pack(&refs, output)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn options(columns: u32, rows: u32, rows_follow: FlipbookRows) -> FlipbookOptions {
        FlipbookOptions {
            columns,
            rows,
            fps: 8.0,
            rows_follow,
            max_texture: 4096,
            keep_detail_maps: false,
        }
    }

    fn layout(columns: u32, rows: u32, staggered: bool) -> Layout {
        Layout {
            columns,
            rows,
            cell: [8, 8],
            tile: [8, 8],
            staggered,
        }
    }

    /// The frames tile the texture exactly once, staggered or not.
    #[test]
    fn frames_cover_the_texture_without_overlap() {
        for staggered in [true, false] {
            let layout = layout(4, 2, staggered);
            let [width, height] = layout.total();
            let mut hits = vec![0u8; (width * height) as usize];
            for k in 0..8 {
                let [x0, y0] = layout.origin(k);
                for y in 0..layout.cell[1] {
                    for x in 0..layout.cell[0] {
                        hits[(((y0 + y) % height) * width + x0 + x) as usize] += 1;
                    }
                }
            }
            assert!(
                hits.iter().all(|&count| count == 1),
                "staggered={staggered}"
            );
        }
    }

    /// The static scroll lands step `k` on frame `k`: the shader's
    /// `fract(speed * k * step)` is that frame's corner.
    #[test]
    fn the_stepped_scroll_lands_on_each_frame_in_order() {
        let options = options(4, 2, FlipbookRows::Time);
        let edits = scroll_edits(&options).unwrap();
        let (VmatEdit::Vector { value: speed, .. }, VmatEdit::Float { value: step, .. }) =
            (&edits[0], &edits[1])
        else {
            panic!("a time flipbook uses static values");
        };
        let layout = layout(4, 2, true);
        let [width, height] = layout.total();
        for k in 0..8u32 {
            let t = f64::from(k) * step;
            let offset = [(speed[0] * t).fract(), (speed[1] * t).fract()];
            let [x0, y0] = layout.origin(k);
            assert!(
                (offset[0] * f64::from(width) - f64::from(x0)).abs() < 1e-6,
                "frame {k}"
            );
            assert!(
                (offset[1] * f64::from(height) - f64::from(y0)).abs() < 1e-6,
                "frame {k}"
            );
        }
    }

    /// Driven rows compile to expressions on the game clock, and leave the
    /// shader's own stepping off.
    #[test]
    fn driven_rows_compile_to_game_clock_expressions() {
        for rows_follow in [FlipbookRows::Health, FlipbookRows::Speed { full: 400.0 }] {
            let edits = scroll_edits(&options(8, 4, rows_follow)).unwrap();
            let VmatEdit::Expr {
                name, attributes, ..
            } = &edits[0]
            else {
                panic!("a driven flipbook uses an expression");
            };
            assert_eq!(name, "g_vAlbedoScrollSpeed1");
            assert!(attributes.iter().any(|a| a == GAME_TIME), "{attributes:?}");
            assert!(matches!(&edits[1], VmatEdit::Float { value, .. } if *value == 0.0));
        }
    }

    /// Each frame's overlay lands in its own cell, over the source where the
    /// overlay is opaque, and the source's alpha survives.
    #[test]
    fn overlays_land_in_their_own_cells() {
        let source = RgbaImage::from_pixel(8, 8, Rgba([10, 20, 30, 77]));
        let overlays: Vec<RgbaImage> = (0..4u8)
            .map(|k| {
                let mut overlay = RgbaImage::new(8, 8);
                for x in 0..4 {
                    for y in 0..8 {
                        overlay.put_pixel(x, y, Rgba([200 + k, 0, 0, 255]));
                    }
                }
                overlay
            })
            .collect();
        let layout = layout(2, 2, false);
        let grid = build_grid(&source, Some(&overlays), &layout);
        for k in 0..4u8 {
            let [x0, y0] = layout.origin(u32::from(k));
            assert_eq!(
                *grid.get_pixel(x0 + 1, y0 + 4),
                Rgba([200 + k, 0, 0, 77]),
                "frame {k}"
            );
            assert_eq!(
                *grid.get_pixel(x0 + 6, y0 + 4),
                Rgba([10, 20, 30, 77]),
                "frame {k}"
            );
        }
    }

    #[test]
    fn options_are_checked() {
        let good = options(4, 4, FlipbookRows::Time);
        assert!(validate(&good, 16).is_ok());
        assert!(validate(&good, 15).is_err());
        assert!(validate(&FlipbookOptions { columns: 3, ..good }, 12).is_err());
        assert!(validate(
            &FlipbookOptions {
                columns: 1,
                rows: 1,
                ..good
            },
            1
        )
        .is_err());
        assert!(validate(&FlipbookOptions { fps: 0.0, ..good }, 16).is_err());
        assert!(validate(
            &FlipbookOptions {
                max_texture: 3000,
                ..good
            },
            16
        )
        .is_err());
    }
}
