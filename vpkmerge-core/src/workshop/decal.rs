//! Decals: a picture, still or animated, on a model as a draw call of its own.
//!
//! A [flipbook](super::flipbook) repaints a whole part once per frame, so one
//! small picture costs the part's textures times the frame count. A decal
//! copies only the surface under the picture into a new draw call
//! ([`morphic::model::append_projected_decal`]) with a material of its own, so
//! its texture holds the picture's frames and nothing else, and the part keeps
//! its textures and UVs.
//!
//! The picture is placed by projection: it is fitted into a box on the model's
//! outline as seen from the front, back or a side, and lands on whatever
//! surface that view shows there. The material is a copy of the one it lands
//! on most, so the picture is lit like the part underneath.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use image::{imageops, Rgba, RgbaImage};
use morphic::model::{DecalProjection, DepthMap};

use super::flipbook::{is_flat, scroll_edits, FlipbookOptions, FlipbookRows, Layout};
use super::{bind_finish_textures, compiled, open, read};
use crate::vmat_style::{patch_vmat_params, VmatEdit};

/// Where decal materials and textures live. A draw call that renders a
/// material from here is a decal, and a later decal does not land on it.
const FOLDER: &str = "materials/grimoire/decal";

/// Texels around each frame that repeat its edge, so filtering at a distance
/// does not pull in the neighbouring frame.
const GUTTER: u32 = 2;

/// The side of the model a picture is projected from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecalView {
    Front,
    Back,
    Left,
    Right,
}

impl DecalView {
    /// The way the picture travels. Models face +X and stand along +Z.
    fn direction(self) -> [f32; 3] {
        match self {
            Self::Front => [-1.0, 0.0, 0.0],
            Self::Back => [1.0, 0.0, 0.0],
            Self::Left => [0.0, -1.0, 0.0],
            Self::Right => [0.0, 1.0, 0.0],
        }
    }
}

/// Where a decal goes and how its frames play.
#[derive(Clone, Debug, PartialEq)]
pub struct DecalOptions {
    /// Names the decal's material and texture: lower-case letters, digits and
    /// underscores. Two decals on one model need different names.
    pub name: String,
    pub view: DecalView,
    /// The box the picture is fitted into, as fractions of the model's outline
    /// from `view`: `[left, top, right, bottom]`, from the top-left corner.
    /// The picture keeps its shape and sits in the middle of the box.
    pub rect: [f32; 4],
    /// Frames across the texture: a power of two up to 16.
    pub columns: u32,
    /// Frames down the texture: a power of two up to 16.
    pub rows: u32,
    /// Time frames per second, 0.1 to 60. Unused for a single frame.
    pub fps: f64,
    pub rows_follow: FlipbookRows,
    /// Largest edge of the texture: a power of two from 64 to 8192.
    pub max_texture: u32,
    /// How far the picture sits above the surface, in model units. A decal
    /// added to a model that already has some sits half of this higher for
    /// each of them.
    pub offset: f32,
}

/// What [`decal_picture`] wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct DecalReport {
    /// Draw calls added, over every level of detail.
    pub draw_calls: usize,
    /// Triangles added at the highest level of detail.
    pub triangles: usize,
    /// Share of the picture that found a surface.
    pub coverage: f32,
    /// Materials the picture landed on, each with the share of the picture it
    /// holds, largest first. The first one lends the decal its material.
    pub hosts: Vec<(String, f32)>,
    /// The decal's material (`.vmat`).
    pub material: String,
    /// The decal's texture (`.vtex`) and its size.
    pub texture: String,
    pub width: u32,
    pub height: u32,
    /// The frames have transparent parts, cut out by alpha test.
    pub cutout: bool,
}

/// The model's outline from one view.
struct Outline {
    right: [f32; 3],
    down: [f32; 3],
    direction: [f32; 3],
    /// Extent along `right`, `down` and `direction`.
    min: [f32; 3],
    max: [f32; 3],
}

impl Outline {
    fn of(model: &morphic::model::Model, view: DecalView) -> Result<Self> {
        let direction = view.direction();
        let up = [0.0, 0.0, 1.0];
        let mut outline = Self {
            right: cross(direction, up),
            down: [0.0, 0.0, -1.0],
            direction,
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        };
        let axes = [outline.right, outline.down, outline.direction];
        for position in model
            .meshes
            .iter()
            .flat_map(|mesh| &mesh.vertex_buffers)
            .flat_map(|buffer| &buffer.positions)
        {
            for (axis, along) in axes.iter().enumerate() {
                let at = dot(*position, *along);
                outline.min[axis] = outline.min[axis].min(at);
                outline.max[axis] = outline.max[axis].max(at);
            }
        }
        ensure!(
            outline.min.iter().all(|v| v.is_finite()),
            "The model has no geometry"
        );
        Ok(outline)
    }

    /// The projection that covers the part of the outline from `(left, top)`,
    /// `width` by `height` model units.
    fn window(&self, left: f32, top: f32, width: f32, height: f32) -> DecalProjection {
        let origin = [0, 1, 2].map(|axis| {
            self.right[axis] * left + self.down[axis] * top + self.direction[axis] * self.min[2]
        });
        DecalProjection {
            origin,
            right: self.right.map(|v| v * width),
            down: self.down.map(|v| v * height),
            direction: self.direction,
        }
    }

    /// The projection of a picture with the given width over height, fitted
    /// into `rect` and centred there.
    fn picture(&self, rect: [f32; 4], aspect: f32) -> DecalProjection {
        let size = [self.max[0] - self.min[0], self.max[1] - self.min[1]];
        let (box_w, box_h) = ((rect[2] - rect[0]) * size[0], (rect[3] - rect[1]) * size[1]);
        let (width, height) = if box_w / box_h > aspect {
            (box_h * aspect, box_h)
        } else {
            (box_w, box_w / aspect)
        };
        self.window(
            self.min[0] + rect[0] * size[0] + (box_w - width) / 2.0,
            self.min[1] + rect[1] * size[1] + (box_h - height) / 2.0,
            width,
            height,
        )
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn decode_pictures(frames: &[Vec<u8>]) -> Result<Vec<RgbaImage>> {
    frames
        .iter()
        .map(|png| {
            Ok(
                image::load_from_memory_with_format(png, image::ImageFormat::Png)
                    .context("Decoding decal frame PNG")?
                    .to_rgba8(),
            )
        })
        .collect()
}

fn check_rect(rect: [f32; 4]) -> Result<()> {
    ensure!(
        rect.iter().all(|v| v.is_finite())
            && rect[0] < rect[2]
            && rect[1] < rect[3]
            && rect[0] >= -1.0
            && rect[1] >= -1.0
            && rect[2] <= 2.0
            && rect[3] <= 2.0,
        "The picture's box must be left,top,right,bottom with left < right and top < bottom"
    );
    Ok(())
}

fn validate(options: &DecalOptions, frames: usize) -> Result<()> {
    ensure!(
        !options.name.is_empty()
            && options.name.len() <= 48
            && options
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
        "A decal name is lower-case letters, digits and underscores"
    );
    check_rect(options.rect)?;
    for (name, cells) in [("columns", options.columns), ("rows", options.rows)] {
        ensure!(
            cells.is_power_of_two() && cells <= 16,
            "Decal {name} must be 1, 2, 4, 8 or 16"
        );
    }
    ensure!(
        frames == (options.columns * options.rows) as usize,
        "Expected {} frames ({} columns x {} rows), got {frames}",
        options.columns * options.rows,
        options.columns,
        options.rows
    );
    ensure!(
        options.fps.is_finite() && (0.1..=60.0).contains(&options.fps),
        "Decal speed must be between 0.1 and 60 frames per second"
    );
    ensure!(
        options.max_texture.is_power_of_two() && (64..=8192).contains(&options.max_texture),
        "Largest texture edge must be a power of two from 64 to 8192"
    );
    ensure!(
        options.offset.is_finite() && (0.0..=4.0).contains(&options.offset),
        "The decal's lift off the surface must be between 0 and 4 units"
    );
    if let FlipbookRows::Speed { full } = options.rows_follow {
        ensure!(
            full.is_finite() && full > 0.0,
            "Full speed must be above zero"
        );
    }
    Ok(())
}

/// One frame at cell size: the picture inside a gutter that repeats its edge.
/// `alpha` replaces the picture's alpha when the decal is not cut out.
fn cell_of(picture: &RgbaImage, cell: [u32; 2], gutter: u32, alpha: Option<u8>) -> RgbaImage {
    let inner = imageops::resize(
        picture,
        cell[0] - 2 * gutter,
        cell[1] - 2 * gutter,
        imageops::FilterType::Lanczos3,
    );
    RgbaImage::from_fn(cell[0], cell[1], |x, y| {
        let mut pixel = *inner.get_pixel(
            x.saturating_sub(gutter).min(inner.width() - 1),
            y.saturating_sub(gutter).min(inner.height() - 1),
        );
        if let Some(alpha) = alpha {
            pixel[3] = alpha;
        }
        pixel
    })
}

/// The frames laid out the way the scroll walks them.
fn build_sheet(
    pictures: &[RgbaImage],
    layout: &Layout,
    gutter: u32,
    alpha: Option<u8>,
) -> RgbaImage {
    let [total_w, total_h] = layout.total();
    let mut sheet = RgbaImage::new(total_w, total_h);
    for (k, picture) in (0u32..).zip(pictures) {
        let cell = cell_of(picture, layout.cell, gutter, alpha);
        let [x0, y0] = layout.origin(k);
        for (x, y, pixel) in cell.enumerate_pixels() {
            sheet.put_pixel(x0 + x, (y0 + y) % total_h, *pixel);
        }
    }
    sheet
}

fn decode_texture(bytes: &[u8]) -> Result<Option<RgbaImage>> {
    let info = morphic::inspect(bytes)?;
    let decoded = morphic::crop_to_actual(
        &morphic::decode(bytes)?,
        u32::from(info.actual_width),
        u32::from(info.actual_height),
    );
    let morphic::ImageData::Rgba8(pixels) = decoded.data else {
        return Ok(None);
    };
    Ok(Some(
        RgbaImage::from_raw(decoded.width, decoded.height, pixels)
            .context("Texture size does not match its pixels")?,
    ))
}

#[allow(clippy::cast_possible_truncation)]
fn mean(image: &RgbaImage) -> Rgba<u8> {
    let mut sum = [0u64; 4];
    for pixel in image.pixels() {
        for (total, value) in sum.iter_mut().zip(pixel.0) {
            *total += u64::from(value);
        }
    }
    let count = (u64::from(image.width()) * u64::from(image.height())).max(1);
    Rgba(sum.map(|total| ((total + count / 2) / count) as u8))
}

/// Put a picture on a model as its own draw call: `frames` play in the box
/// `options.rect` of the view `options.view`, on the model `model`, read from
/// `paths` (highest priority first, the base game last).
///
/// `frames` are PNGs of the picture itself, in row-major order: one frame for
/// a still picture, all `columns * rows` time frames for
/// [`FlipbookRows::Time`], otherwise one row of `columns` time frames per
/// state. Transparent parts of the frames are cut out.
///
/// The addon holds the model with the added draw calls, the decal's material
/// and its textures. For a second decal on the same model, pass the first addon
/// ahead of the base game in `paths` and merge the two addons, later one
/// winning.
///
/// # Errors
/// Fails when the picture lands on nothing, when the material it lands on most
/// is not `pbr.vfx` or already scrolls its texture, on a cut-out over a colour
/// texture without alpha, and on out-of-range options.
#[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
pub fn decal_picture(
    paths: &[PathBuf],
    model: &str,
    frames: &[Vec<u8>],
    options: &DecalOptions,
    output: &Path,
) -> Result<DecalReport> {
    validate(options, frames.len())?;
    ensure!(
        !model.contains("..") && !model.contains('\\') && !model.contains(':'),
        "Invalid resource path"
    );
    let pictures = decode_pictures(frames)?;
    let (picture_w, picture_h) = pictures[0].dimensions();
    ensure!(picture_w > 0 && picture_h > 0, "The first frame is empty");

    let paks = open(paths)?;
    let model = compiled(model);
    let source_model = read(&paks, &model)?;
    let decoded = morphic::model::decode(&source_model)?;
    let outline = Outline::of(&decoded, options.view)?;
    let projection = outline.picture(options.rect, picture_w as f32 / picture_h as f32);
    // Each decal sits a little above the ones the model already has, so where
    // two overlap the later one shows and they do not flicker.
    let earlier: BTreeSet<&str> = decoded
        .meshes
        .iter()
        .flat_map(|mesh| &mesh.primitives)
        .map(|primitive| primitive.material.as_str())
        .filter(|material| material.starts_with(FOLDER))
        .collect();
    let offset = options.offset * (1.0 + 0.5 * earlier.len() as f32);

    // A staggered column sits `cell height / columns` lower than the one
    // before, so the height has to divide by the column count.
    let cell = [
        picture_w
            .next_power_of_two()
            .clamp(8, options.max_texture / options.columns),
        picture_h
            .next_power_of_two()
            .clamp(options.columns.max(8), options.max_texture / options.rows),
    ];
    let layout = Layout {
        columns: options.columns,
        rows: options.rows,
        cell,
        tile: cell,
        staggered: options.rows_follow == FlipbookRows::Time,
    };
    let gutter = if cell[0].min(cell[1]) >= 32 {
        GUTTER
    } else {
        0
    };
    let [total_w, total_h] = layout.total();
    let uv_rect = [
        gutter as f32 / total_w as f32,
        gutter as f32 / total_h as f32,
        (cell[0] - gutter) as f32 / total_w as f32,
        (cell[1] - gutter) as f32 / total_h as f32,
    ];

    // The model: the surface under the picture, as draw calls of the decal's
    // material.
    let folder = format!("{FOLDER}/{}", model.trim_end_matches(".vmdl_c")).to_ascii_lowercase();
    let material_path = format!("{folder}/{}.vmat", options.name);
    let (patched_model, placed) = morphic::model::append_projected_decal(
        &source_model,
        &projection,
        &material_path,
        &morphic::model::DecalOptions {
            offset,
            uv_rect,
            ..morphic::model::DecalOptions::default()
        },
        |material| !material.starts_with(FOLDER),
    )?;
    let Some((host, _)) = placed.hosts.first() else {
        bail!("The picture does not land on the model from this view");
    };

    // The material: a copy of the one the picture lands on most.
    let original = read(&paks, &compiled(host))?;
    let material = morphic::material::parse(&original)?;
    ensure!(
        material.shader_name == "pbr.vfx",
        "Only pbr.vfx parts can take a decal"
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
        "The part under the picture already scrolls its texture"
    );
    let cutout = pictures
        .iter()
        .any(|picture| picture.pixels().any(|pixel| pixel[3] < 250));
    let mut edits = Vec::new();
    if pictures.len() > 1 {
        edits.extend(scroll_edits(&FlipbookOptions {
            columns: options.columns,
            rows: options.rows,
            fps: options.fps,
            rows_follow: options.rows_follow,
            max_texture: options.max_texture,
            keep_detail_maps: false,
        })?);
    }
    if cutout {
        edits.push(VmatEdit::Int {
            name: "F_ALPHA_TEST".into(),
            value: 1,
        });
        edits.push(VmatEdit::Float {
            name: "g_flAlphaTestReference1".into(),
            value: 0.5,
        });
    }
    let mut vmat = if edits.is_empty() {
        original.clone()
    } else {
        let (vmat, stats) = patch_vmat_params(&original, &edits)?;
        ensure!(
            stats.failed.is_empty(),
            "Decal parameters could not be patched: {:?}",
            stats.failed
        );
        vmat
    };

    // The textures: the frames for colour, and for every other texture the
    // part reads through its UVs, its average. The decal has UVs of its own,
    // so the part's detail would land in the wrong place.
    let color_path = material
        .texture_params
        .get("g_tColor")
        .context("The part under the picture has no colour texture")?;
    let color_bytes = read(&paks, &compiled(color_path))?;
    let color_info = morphic::inspect(&color_bytes)?;
    ensure!(
        !cutout || color_info.format != morphic::TextureFormat::Dxt1,
        "The part's colour texture has no alpha, so a picture with transparent parts cannot be cut out"
    );
    // An opaque part keeps a mask in its colour alpha; the picture takes the
    // part's usual value.
    let alpha = if cutout {
        None
    } else {
        Some(decode_texture(&color_bytes)?.map_or(0, |image| mean(&image)[3]))
    };
    let sheet = build_sheet(&pictures, &layout, gutter, alpha);
    let texture_path = format!("{folder}/{}_color.vtex", options.name);
    let mut files: Vec<(String, Vec<u8>)> = vec![(
        compiled(&texture_path),
        crate::trippy::encode_part_texture(
            &color_bytes,
            &morphic::Image {
                width: total_w,
                height: total_h,
                data: morphic::ImageData::Rgba8(sheet.into_raw()),
            },
        )
        .context("Encoding the decal's frames")?,
    )];
    let mut bindings = vec![("g_tColor".to_string(), texture_path.clone())];
    let mut averaged: BTreeMap<&str, Option<String>> = BTreeMap::new();
    for (slot, path) in &material.texture_params {
        if slot == "g_tColor" || path.starts_with("materials/default/") {
            continue;
        }
        if !averaged.contains_key(path.as_str()) {
            let bytes = read(&paks, &compiled(path))?;
            let flat = match decode_texture(&bytes)? {
                Some(image) if !is_flat(&image) => {
                    let new_path = format!(
                        "{folder}/{}_{}.vtex",
                        options.name,
                        slot.to_ascii_lowercase()
                    );
                    let texture = crate::trippy::encode_part_texture(
                        &bytes,
                        &morphic::Image {
                            width: 4,
                            height: 4,
                            data: morphic::ImageData::Rgba8(
                                RgbaImage::from_pixel(4, 4, mean(&image)).into_raw(),
                            ),
                        },
                    )
                    .with_context(|| format!("Encoding the average of {path}"))?;
                    files.push((compiled(&new_path), texture));
                    Some(new_path)
                }
                _ => None,
            };
            averaged.insert(path.as_str(), flat);
        }
        if let Some(Some(new_path)) = averaged.get(path.as_str()) {
            bindings.push((slot.clone(), new_path.clone()));
        }
    }
    vmat = bind_finish_textures(&vmat, &material_path, &bindings)?;

    files.push((model, patched_model));
    files.push((compiled(&material_path), vmat));
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    crate::pack(&refs, output)?;
    Ok(DecalReport {
        draw_calls: placed.draw_calls,
        triangles: placed.triangles,
        coverage: placed.coverage,
        hosts: placed.hosts,
        material: material_path,
        texture: texture_path,
        width: total_w,
        height: total_h,
        cutout,
    })
}

/// A PNG of the model from `view`, `height` pixels tall, with `frame` drawn
/// where [`decal_picture`] would put it for the same view and box. For
/// choosing the box before building the addon.
///
/// # Errors
/// Fails on a model without geometry, a frame that is not a PNG and an invalid
/// box.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn decal_preview(
    paths: &[PathBuf],
    model: &str,
    frame: &[u8],
    view: DecalView,
    rect: [f32; 4],
    height: u32,
) -> Result<Vec<u8>> {
    check_rect(rect)?;
    ensure!(
        (64..=4096).contains(&height),
        "Preview height must be between 64 and 4096 pixels"
    );
    let picture = image::load_from_memory_with_format(frame, image::ImageFormat::Png)
        .context("Decoding decal frame PNG")?
        .to_rgba8();
    ensure!(
        picture.width() > 0 && picture.height() > 0,
        "The frame is empty"
    );
    let paks = open(paths)?;
    let decoded = morphic::model::decode(&read(&paks, &compiled(model))?)?;
    let outline = Outline::of(&decoded, view)?;
    let placed = outline.picture(rect, picture.width() as f32 / picture.height() as f32);

    let size = [
        outline.max[0] - outline.min[0],
        outline.max[1] - outline.min[1],
    ];
    let margin = size[1] * 0.04;
    let window = outline.window(
        outline.min[0] - margin,
        outline.min[1] - margin,
        size[0] + 2.0 * margin,
        size[1] + 2.0 * margin,
    );
    let width = ((size[0] + 2.0 * margin) / (size[1] + 2.0 * margin) * height as f32)
        .round()
        .max(1.0) as u32;
    let mut map = DepthMap::new(width as usize, height as usize);
    for mesh in &decoded.meshes {
        for primitive in &mesh.primitives {
            let Some(buffer) = mesh.vertex_buffers.get(primitive.vertex_buffer) else {
                continue;
            };
            for triangle in primitive.indices.as_chunks::<3>().0 {
                let corner = |index: u32| buffer.positions.get(index as usize).copied();
                let (Some(a), Some(b), Some(c)) = (
                    corner(triangle[0]),
                    corner(triangle[1]),
                    corner(triangle[2]),
                ) else {
                    continue;
                };
                let normal = triangle
                    .iter()
                    .filter_map(|&index| buffer.normals.get(index as usize))
                    .fold([0.0; 3], |sum, n| {
                        [sum[0] + n[0], sum[1] + n[1], sum[2] + n[2]]
                    });
                let length = dot(normal, normal).sqrt();
                let facing = if length > 1e-12 {
                    -dot(normal, outline.direction) / length
                } else {
                    1.0
                };
                if facing > 0.0 {
                    map.draw([a, b, c].map(|p| window.project(p)), facing);
                }
            }
        }
    }

    let mut image = RgbaImage::from_pixel(width, height, Rgba([24, 24, 28, 255]));
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let across = (x as f32 + 0.5) / width as f32;
        let down = (y as f32 + 0.5) / height as f32;
        let Some((depth, shade)) = map.at(across, down) else {
            continue;
        };
        let grey = (70.0 + 150.0 * shade).round() as u8;
        *pixel = Rgba([grey, grey, grey, 255]);
        let point = [0, 1, 2].map(|axis| {
            window.origin[axis]
                + window.right[axis] * across
                + window.down[axis] * down
                + window.direction[axis] * depth
        });
        let [s, t, _] = placed.project(point);
        if (0.0..1.0).contains(&s) && (0.0..1.0).contains(&t) {
            let texel = picture.get_pixel(
                ((s * picture.width() as f32) as u32).min(picture.width() - 1),
                ((t * picture.height() as f32) as u32).min(picture.height() - 1),
            );
            let alpha = f32::from(texel[3]) / 255.0;
            for channel in 0..3 {
                pixel[channel] = (f32::from(texel[channel]) * alpha
                    + f32::from(pixel[channel]) * (1.0 - alpha))
                    .round() as u8;
            }
        }
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .context("Encoding the preview")?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline() -> Outline {
        // A model 40 wide and 80 tall seen from behind, its feet at height 0.
        Outline {
            right: [0.0, -1.0, 0.0],
            down: [0.0, 0.0, -1.0],
            direction: [1.0, 0.0, 0.0],
            min: [-20.0, -80.0, -10.0],
            max: [20.0, 0.0, 10.0],
        }
    }

    fn options() -> DecalOptions {
        DecalOptions {
            name: "cat".into(),
            view: DecalView::Back,
            rect: [0.25, 0.25, 0.75, 0.75],
            columns: 4,
            rows: 2,
            fps: 8.0,
            rows_follow: FlipbookRows::Time,
            max_texture: 2048,
            offset: 0.1,
        }
    }

    /// A wide picture in a tall box keeps its shape, fills the box's width and
    /// sits in the middle of its height.
    #[test]
    fn a_picture_is_fitted_into_its_box_without_stretching() {
        let placed = outline().picture([0.25, 0.25, 0.75, 0.75], 2.0);
        // The box is 20 wide and 40 tall; the picture is 20 by 10.
        assert!((dot(placed.right, placed.right).sqrt() - 20.0).abs() < 1e-4);
        assert!((dot(placed.down, placed.down).sqrt() - 10.0).abs() < 1e-4);
        // Its centre is the box's centre: the middle of the model.
        let centre = [0, 1, 2]
            .map(|axis| placed.origin[axis] + placed.right[axis] / 2.0 + placed.down[axis] / 2.0);
        assert!(centre[1].abs() < 1e-4);
        assert!((centre[2] - 40.0).abs() < 1e-4);
        // And it starts in front of the nearest surface.
        assert!((centre[0] - -10.0).abs() < 1e-4);
    }

    #[test]
    fn the_four_views_look_at_the_model_from_outside() {
        for view in [
            DecalView::Front,
            DecalView::Back,
            DecalView::Left,
            DecalView::Right,
        ] {
            let direction = view.direction();
            let right = cross(direction, [0.0, 0.0, 1.0]);
            // Right, down and direction form the viewer's frame.
            assert_eq!(cross(right, [0.0, 0.0, -1.0]), direction);
        }
    }

    /// Each frame's edge repeats into its gutter, and the frames land where the
    /// scroll will look for them.
    #[test]
    fn frames_keep_a_gutter_of_their_own_edge() {
        let pictures: Vec<RgbaImage> = (0u8..4)
            .map(|k| RgbaImage::from_pixel(8, 8, Rgba([k * 60, 0, 0, 255])))
            .collect();
        let layout = Layout {
            columns: 4,
            rows: 1,
            cell: [32, 32],
            tile: [32, 32],
            staggered: false,
        };
        let sheet = build_sheet(&pictures, &layout, 2, Some(7));
        assert_eq!(sheet.dimensions(), (128, 32));
        for k in 0u8..4 {
            let x0 = u32::from(k) * 32;
            for (x, y) in [(0, 0), (31, 31), (16, 16), (1, 30)] {
                assert_eq!(*sheet.get_pixel(x0 + x, y), Rgba([k * 60, 0, 0, 7]));
            }
        }
    }

    #[test]
    fn a_cut_out_frame_keeps_its_own_alpha() {
        let mut picture = RgbaImage::from_pixel(8, 8, Rgba([200, 10, 10, 255]));
        picture.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
        let cell = cell_of(&picture, [8, 8], 0, None);
        assert_eq!(cell.get_pixel(0, 0)[3], 0);
        assert_eq!(cell.get_pixel(7, 7)[3], 255);
    }

    #[test]
    fn options_are_checked() {
        assert!(validate(&options(), 8).is_ok());
        assert!(validate(&options(), 7).is_err());
        let still = DecalOptions {
            columns: 1,
            rows: 1,
            ..options()
        };
        assert!(validate(&still, 1).is_ok());
        for bad in [
            DecalOptions {
                name: "Bad Name".into(),
                ..options()
            },
            DecalOptions {
                rect: [0.5, 0.2, 0.4, 0.8],
                ..options()
            },
            DecalOptions {
                columns: 3,
                ..options()
            },
            DecalOptions {
                max_texture: 1000,
                ..options()
            },
            DecalOptions {
                offset: -1.0,
                ..options()
            },
        ] {
            assert!(validate(&bad, 8).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_mean_of_a_texture_is_its_average_colour() {
        let mut image = RgbaImage::from_pixel(2, 1, Rgba([0, 100, 255, 0]));
        image.put_pixel(1, 0, Rgba([100, 200, 255, 255]));
        assert_eq!(mean(&image), Rgba([50, 150, 255, 128]));
    }
}
