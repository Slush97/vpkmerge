//! Texture painting support: which textures a hero's model samples, and where on
//! each texture a body area lives.
//!
//! [`hero_textures`] lists every texture the hero's live model (the one
//! `scripts/heroes.vdata_c` names) samples, per material, with the facts a
//! painter needs before touching pixels: size and format, whether it is a uniform
//! placeholder, what its alpha holds, and whether vertex colors multiply it.
//!
//! [`map_texture_regions`] partitions one texture's UV space into regions (by skin
//! bone, UV island or mesh part), counting only the draw calls whose material
//! samples that texture, and writes the decoded texture, an overlay with each
//! region's id drawn on it, and on request a white-on-black mask of chosen
//! regions at the texture's own resolution. UV (0,0) is the top-left texel, the
//! same mapping the reskin builders paint with (in-game confirmed).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use morphic::model::{Model, Segment, SegmentBy};
use morphic::{DecodeOptions, Image, ImageData};

use crate::model::{compiled_resource_path, live_hero_model_from_vdata, open_vpks, read_entry};

/// Engine placeholders live here and are shared by every material in the game.
const ENGINE_DEFAULTS: &str = "materials/default/";
/// Longest edge [`TextureFacts`] are measured at.
const FACTS_EDGE: u32 = 256;
/// Longest edge of the region statistics raster and of the overlay PNG.
const ANALYSIS_EDGE: u32 = 1024;

/// Every texture a hero's live model samples, grouped by material.
#[derive(Debug, Clone)]
pub struct HeroTextures {
    pub codename: String,
    pub model_entry: String,
    /// Largest first (by vertex count).
    pub materials: Vec<MaterialTextures>,
}

#[derive(Debug, Clone)]
pub struct MaterialTextures {
    /// Compiled material entry (`.vmat_c`).
    pub material: String,
    pub shader: Option<String>,
    /// Shader feature flags set on the material (`F_*` != 0).
    pub flags: Vec<String>,
    pub meshes: Vec<String>,
    /// Triangles with a surface (zero-area stitch triangles left out).
    pub triangles: usize,
    pub body: bool,
    pub weapon: bool,
    /// The draw calls carry a non-white vertex `COLOR` stream, which the shader
    /// multiplies into the albedo.
    pub vertex_color: bool,
    /// Texture-coordinate params with a non-identity value.
    pub uv_params: Vec<(String, [f32; 4])>,
    pub textures: Vec<MaterialTexture>,
}

#[derive(Debug, Clone)]
pub struct MaterialTexture {
    pub slot: String,
    /// Compiled texture entry (`.vtex_c`).
    pub entry: String,
    /// Under `materials/default/`: an engine placeholder every material shares.
    pub engine_default: bool,
    /// Header and pixel facts; `None` when the entry is not in the pak.
    pub facts: Option<TextureFacts>,
    /// Other materials of this model that sample the same texture (file stems).
    pub shared_with: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TextureFacts {
    /// Real mip-0 size (non-power-of-two padding excluded).
    pub width: u32,
    pub height: u32,
    /// VRF format name (`Bc7`, `Dxt5`, ...).
    pub format: String,
    pub mips: u8,
    /// DXT5 carrying `YCoCg`: decodes fine, but replacing it is unsupported.
    pub ycocg: bool,
    /// Every texel holds this RGBA value, measured at a mip of at most 256 px.
    pub uniform: Option<[u8; 4]>,
    /// Lowest and highest alpha, measured at the same mip.
    pub alpha: (u8, u8),
}

/// Lists every texture `codename`'s live model samples, per material.
pub fn hero_textures(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    codename: &str,
) -> Result<HeroTextures> {
    let live = crate::model::live_hero_materials(vpk.as_ref(), base, codename)?;
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let (model_entry, _) = live_hero_model_from_vdata(&vpks, codename)?;
    let model = decode_model(&vpks, &model_entry)?;

    let mut materials: Vec<MaterialTextures> = live
        .into_iter()
        .map(|m| {
            let compiled = compiled_resource_path(&m.material);
            let uv_params = read_entry(&vpks, &compiled)
                .and_then(|bytes| morphic::material::parse(&bytes).ok())
                .map(|mat| uv_params(&mat))
                .unwrap_or_default();
            let textures = m
                .textures
                .iter()
                .map(|t| MaterialTexture {
                    slot: t.slot.clone(),
                    engine_default: t.compiled_path.starts_with(ENGINE_DEFAULTS),
                    facts: read_entry(&vpks, &t.compiled_path)
                        .and_then(|bytes| texture_facts(&bytes).ok()),
                    entry: t.compiled_path.clone(),
                    shared_with: Vec::new(),
                })
                .collect();
            MaterialTextures {
                triangles: renderable_triangles(&model, &m.material),
                vertex_color: has_vertex_color(&model, &m.material),
                material: compiled,
                shader: m.shader_name,
                flags: m.feature_flags.into_iter().map(|(name, _)| name).collect(),
                meshes: m.mesh_names,
                body: m.body,
                weapon: m.weapon,
                uv_params,
                textures,
            }
        })
        .collect();

    let users: Vec<(String, Vec<String>)> = materials
        .iter()
        .map(|m| {
            (
                stem(&m.material),
                m.textures.iter().map(|t| t.entry.clone()).collect(),
            )
        })
        .collect();
    for m in &mut materials {
        let me = stem(&m.material);
        for t in m.textures.iter_mut().filter(|t| !t.engine_default) {
            t.shared_with = users
                .iter()
                .filter(|(name, entries)| *name != me && entries.contains(&t.entry))
                .map(|(name, _)| name.clone())
                .collect();
        }
    }

    Ok(HeroTextures {
        codename: codename.to_owned(),
        model_entry,
        materials,
    })
}

fn decode_model(vpks: &[valve_pak::VPK], entry: &str) -> Result<Model> {
    let bytes =
        read_entry(vpks, entry).with_context(|| format!("model entry {entry} not found"))?;
    morphic::model::decode(&bytes).with_context(|| format!("decoding {entry}"))
}

/// A path's file name without its extension (`.../vindicta_dress.vmat_c` ->
/// `vindicta_dress`).
fn stem(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.split('.').next().unwrap_or(file).to_owned()
}

fn same_material(a: &str, b: &str) -> bool {
    compiled_resource_path(a).eq_ignore_ascii_case(&compiled_resource_path(b))
}

fn renderable_triangles(model: &Model, material: &str) -> usize {
    morphic::model::segments_where(model, SegmentBy::Material, &|_, p| {
        same_material(&p.material, material)
    })
    .iter()
    .map(Segment::triangle_count)
    .sum()
}

fn has_vertex_color(model: &Model, material: &str) -> bool {
    model.meshes.iter().any(|mesh| {
        mesh.primitives
            .iter()
            .filter(|p| same_material(&p.material, material))
            .any(|p| {
                let Some(colors) = mesh
                    .vertex_buffers
                    .get(p.vertex_buffer)
                    .and_then(|vb| vb.colors.first())
                else {
                    return false;
                };
                p.indices.iter().any(|&i| {
                    colors
                        .get(i as usize)
                        .is_some_and(|c| c[..3].iter().any(|&v| v < 0.98))
                })
            })
    })
}

/// Texture-coordinate vector params that move the mapping away from identity.
fn uv_params(mat: &morphic::material::Material) -> Vec<(String, [f32; 4])> {
    mat.vector_params
        .iter()
        .filter(|(name, _)| name.to_ascii_lowercase().contains("texcoord"))
        .filter(|(name, v)| {
            let lower = name.to_ascii_lowercase();
            let identity = if lower.contains("scale") {
                1.0
            } else if lower.contains("center") {
                0.5
            } else {
                0.0
            };
            (v[0] - identity).abs() > 1e-6 || (v[1] - identity).abs() > 1e-6
        })
        .map(|(name, v)| (name.clone(), *v))
        .collect()
}

/// Header facts plus uniformity and alpha range, measured at a small mip.
pub fn texture_facts(bytes: &[u8]) -> Result<TextureFacts> {
    let info = morphic::inspect(bytes).context("reading texture header")?;
    let (width, height) = (u32::from(info.actual_width), u32::from(info.actual_height));
    let mut mip = 0u8;
    while mip + 1 < info.mip_count {
        let (w, h) = info.actual_mip_dims(mip + 1);
        if w.max(h) < FACTS_EDGE {
            break;
        }
        mip += 1;
    }
    let image = morphic::decode_at(
        bytes,
        &DecodeOptions {
            mip,
            slice: 0,
            face: 0,
        },
    )
    .context("decoding texture")?;
    let (w, h) = info.actual_mip_dims(mip);
    let rgba = rgba8(&morphic::crop_to_actual(&image, w, h));
    let mut lo = [u8::MAX; 4];
    let mut hi = [0u8; 4];
    for px in rgba.chunks_exact(4) {
        for c in 0..4 {
            lo[c] = lo[c].min(px[c]);
            hi[c] = hi[c].max(px[c]);
        }
    }
    let uniform = (0..4).all(|c| hi[c] - lo[c] <= 2).then_some(lo);
    Ok(TextureFacts {
        width,
        height,
        format: format!("{:?}", info.format),
        mips: info.mip_count,
        ycocg: info.ycocg,
        uniform,
        alpha: (lo[3], hi[3]),
    })
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rgba8(image: &Image) -> Vec<u8> {
    match &image.data {
        ImageData::Rgba8(px) => px.clone(),
        ImageData::Rgba16F(px) => px
            .iter()
            .map(|h| (h.to_f32().clamp(0.0, 1.0) * 255.0).round() as u8)
            .collect(),
    }
}

// ------------------------------------------------------------- region maps

/// What [`map_texture_regions`] segments by and what it bakes.
#[derive(Debug, Clone)]
pub struct RegionMapOptions {
    pub by: SegmentBy,
    /// Region ids to bake into a mask, from a call with the same `by`.
    pub select: Vec<usize>,
    /// Texels the mask grows into texture space no region samples, so mips and
    /// filtering near seams pick up the new paint instead of the old.
    pub padding: u32,
    /// How many regions, largest first, get their id drawn on the overlay.
    pub labels: usize,
}

#[derive(Debug, Clone)]
pub struct TextureRegionMap {
    /// Compiled texture entry.
    pub texture: String,
    pub model_entry: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
    /// `(material, slot)` pairs in the model that sample this texture.
    pub bindings: Vec<(String, String)>,
    /// Share of the texture any region samples; the rest never shows in game.
    pub used: f32,
    /// Largest coverage first.
    pub regions: Vec<TextureRegion>,
    /// The decoded texture's RGB at full size: the base to paint on.
    pub texture_png: PathBuf,
    /// Its alpha channel as grayscale, when not fully opaque.
    pub alpha_png: Option<PathBuf>,
    /// The texture under region colors, outlines and ids.
    pub overlay_png: PathBuf,
    pub mask: Option<RegionMask>,
}

#[derive(Debug, Clone)]
pub struct TextureRegion {
    pub id: usize,
    pub label: String,
    pub mesh: String,
    pub triangles: usize,
    /// Share of the texture's texels the region samples.
    pub coverage: f32,
    /// `[x0, y0, x1, y1]` texel bounds at full size, max exclusive.
    pub bounds: [u32; 4],
    /// Summed UV area over covered texels: 1 when every texel is sampled once,
    /// about 2 when mirrored halves sample the same texels.
    pub uv_reuse: f32,
    /// Share of the region's texels that other regions also sample.
    pub shared: f32,
    /// Share of the region's triangles that are mirrored in UV.
    pub mirrored: f32,
    /// Separate patches the region shows as on the overlay (tiny specks left
    /// out). 0 when smaller regions cover all of it there.
    pub pieces: usize,
    /// Skin bones carrying the region, by share of its weight (largest first,
    /// at most three).
    pub bones: Vec<(String, f32)>,
    /// The id is drawn on the overlay.
    pub labeled: bool,
}

#[derive(Debug, Clone)]
pub struct RegionMask {
    pub png: PathBuf,
    pub regions: Vec<usize>,
    /// Share of the texture the mask covers, before padding.
    pub coverage: f32,
    /// Share of the masked texels that unselected regions also sample: painting
    /// there changes those regions too.
    pub shared_with_unselected: f32,
}

/// Segments the UV space of `texture` on `codename`'s live model and writes
/// `texture.png`, `overlay-<by>.png` and, when `opts.select` is not empty, a
/// mask PNG into `out_dir`.
#[allow(clippy::too_many_lines)]
pub fn map_texture_regions(
    vpk: impl AsRef<Path>,
    base: Option<&Path>,
    codename: &str,
    texture: &str,
    opts: &RegionMapOptions,
    out_dir: &Path,
) -> Result<TextureRegionMap> {
    let vpks = open_vpks(vpk.as_ref(), base)?;
    let (model_entry, _) = live_hero_model_from_vdata(&vpks, codename)?;
    let model = decode_model(&vpks, &model_entry)?;
    let texture = compiled_resource_path(texture);

    let bindings = texture_bindings(&vpks, &model, &texture);
    if bindings.is_empty() {
        bail!("{texture} is not sampled by any material of {model_entry}");
    }
    let bound: BTreeSet<String> = bindings
        .iter()
        .map(|(m, _)| m.to_ascii_lowercase())
        .collect();
    let segs = morphic::model::segments_where(&model, opts.by, &|_, p| {
        bound.contains(&compiled_resource_path(&p.material).to_ascii_lowercase())
    });

    let bytes =
        read_entry(&vpks, &texture).with_context(|| format!("texture {texture} not found"))?;
    let info = morphic::inspect(&bytes).context("reading texture header")?;
    if matches!(
        info.format,
        morphic::TextureFormat::Bc6h | morphic::TextureFormat::Rgba16161616F
    ) {
        bail!(
            "{texture} is HDR ({:?}); region maps cover 8-bit textures",
            info.format
        );
    }
    let (width, height) = (u32::from(info.actual_width), u32::from(info.actual_height));
    let full = morphic::decode(&bytes).context("decoding texture")?;
    let full = morphic::crop_to_actual(&full, width, height);
    let full = image::RgbaImage::from_raw(width, height, rgba8(&full))
        .context("texture buffer size mismatch")?;

    // Statistics and the overlay share one raster no larger than ANALYSIS_EDGE.
    let (aw, ah) = fit_within(width, height, ANALYSIS_EDGE);
    let stats = region_stats(&segs, aw, ah);
    let total = f64::from(aw) * f64::from(ah);
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let share = |n: usize| (n as f64 / total) as f32;

    let mut regions: Vec<TextureRegion> = segs
        .iter()
        .map(|s| {
            let unique = stats.unique[s.id];
            #[allow(clippy::cast_precision_loss)]
            let shared = if unique == 0 {
                0.0
            } else {
                stats.shared[s.id] as f32 / unique as f32
            };
            let coverage = share(unique);
            TextureRegion {
                id: s.id,
                label: s.label.clone(),
                mesh: s.mesh.clone(),
                triangles: s.triangle_count(),
                coverage,
                bounds: texel_bounds(s.uv_bounds(), width, height),
                uv_reuse: if coverage > 0.0 {
                    s.uv_area() / coverage
                } else {
                    0.0
                },
                shared,
                mirrored: s.mirrored_fraction(),
                bones: s
                    .bone_weights()
                    .into_iter()
                    .take(3)
                    .filter_map(|(b, w)| Some((model.skeleton.bones.get(b)?.name.clone(), w)))
                    .collect(),
                pieces: 0,
                labeled: false,
            }
        })
        .collect();
    regions.sort_by(|a, b| b.coverage.total_cmp(&a.coverage).then(a.id.cmp(&b.id)));
    let label_ids: Vec<usize> = regions.iter().take(opts.labels).map(|r| r.id).collect();

    // Alpha is a data channel on material textures (Vindicta's dress albedo holds
    // ~0 everywhere), so the paintable image is RGB and alpha gets its own file.
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;
    let texture_png = out_dir.join("texture.png");
    image::DynamicImage::ImageRgba8(full.clone())
        .to_rgb8()
        .save(&texture_png)
        .with_context(|| format!("writing {}", texture_png.display()))?;
    let alpha_png = if full.pixels().all(|p| p.0[3] == u8::MAX) {
        None
    } else {
        let path = out_dir.join("texture-alpha.png");
        let alpha: Vec<u8> = full.pixels().map(|p| p.0[3]).collect();
        image::GrayImage::from_raw(width, height, alpha)
            .context("alpha buffer size mismatch")?
            .save(&path)
            .with_context(|| format!("writing {}", path.display()))?;
        Some(path)
    };

    let small = image::imageops::resize(&full, aw, ah, image::imageops::FilterType::Triangle);
    let ids = id_raster(&segs, aw, ah);
    let pieces = count_pieces(&ids, aw, ah, segs.len());
    let (overlay, drawn) = render_overlay(&ids, &small, &label_ids);
    for r in &mut regions {
        r.labeled = drawn.contains(&r.id);
        r.pieces = pieces[r.id];
    }
    let overlay_png = out_dir.join(format!("overlay-{}.png", by_name(opts.by)));
    overlay
        .save(&overlay_png)
        .with_context(|| format!("writing {}", overlay_png.display()))?;

    let mask = if opts.select.is_empty() {
        None
    } else {
        Some(bake_mask(&segs, opts, width, height, out_dir)?)
    };

    Ok(TextureRegionMap {
        texture,
        model_entry,
        width,
        height,
        format: format!("{:?}", info.format),
        bindings,
        used: share(stats.used),
        regions,
        texture_png,
        alpha_png,
        overlay_png,
        mask,
    })
}

/// Every `(material, slot)` of the model whose material samples `texture`.
fn texture_bindings(
    vpks: &[valve_pak::VPK],
    model: &Model,
    texture: &str,
) -> Vec<(String, String)> {
    let materials: BTreeSet<String> = model
        .meshes
        .iter()
        .flat_map(|m| &m.primitives)
        .map(|p| compiled_resource_path(&p.material))
        .collect();
    let mut out = Vec::new();
    for material in materials {
        let Some(mat) = read_entry(vpks, &material).and_then(|b| morphic::material::parse(&b).ok())
        else {
            continue;
        };
        for (slot, path) in &mat.texture_params {
            if compiled_resource_path(path).eq_ignore_ascii_case(texture) {
                out.push((material.clone(), slot.clone()));
            }
        }
    }
    out
}

#[must_use]
pub fn by_name(by: SegmentBy) -> &'static str {
    match by {
        SegmentBy::Island => "island",
        SegmentBy::Part => "part",
        SegmentBy::Material => "material",
        SegmentBy::Bone => "bone",
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn fit_within(w: u32, h: u32, edge: u32) -> (u32, u32) {
    let longer = w.max(h);
    if longer <= edge {
        return (w, h);
    }
    let scale = f64::from(edge) / f64::from(longer);
    (
        ((f64::from(w) * scale).round() as u32).max(1),
        ((f64::from(h) * scale).round() as u32).max(1),
    )
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn texel_bounds(uv: [f32; 4], width: u32, height: u32) -> [u32; 4] {
    let x = |u: f32| (u.clamp(0.0, 1.0) * width as f32) as u32;
    let y = |v: f32| (v.clamp(0.0, 1.0) * height as f32) as u32;
    let ceil_x = |u: f32| ((u.clamp(0.0, 1.0) * width as f32).ceil() as u32).min(width);
    let ceil_y = |v: f32| ((v.clamp(0.0, 1.0) * height as f32).ceil() as u32).min(height);
    [x(uv[0]), y(uv[1]), ceil_x(uv[2]), ceil_y(uv[3])]
}

struct RegionStats {
    /// Texels each region samples, indexed by region id.
    unique: Vec<usize>,
    /// Of those, texels another region samples too.
    shared: Vec<usize>,
    /// Texels any region samples.
    used: usize,
}

fn region_stats(segs: &[Segment], w: u32, h: u32) -> RegionStats {
    let n = w as usize * h as usize;
    let mut claims = vec![0u16; n];
    let mut stamp = vec![usize::MAX; n];
    let mut unique = vec![0usize; segs.len()];
    for s in segs {
        s.for_each_texel(w, h, |t| {
            if stamp[t] != s.id {
                stamp[t] = s.id;
                claims[t] = claims[t].saturating_add(1);
                unique[s.id] += 1;
            }
        });
    }
    stamp.fill(usize::MAX);
    let mut shared = vec![0usize; segs.len()];
    for s in segs {
        s.for_each_texel(w, h, |t| {
            if stamp[t] != s.id {
                stamp[t] = s.id;
                if claims[t] > 1 {
                    shared[s.id] += 1;
                }
            }
        });
    }
    RegionStats {
        unique,
        shared,
        used: claims.iter().filter(|&&c| c > 0).count(),
    }
}

/// Per-texel region id (`-1` where none samples). Ids are sorted largest UV area
/// first, so painting in id order leaves small regions on top where UVs overlap.
fn id_raster(segs: &[Segment], w: u32, h: u32) -> Vec<i32> {
    let mut ids = vec![-1i32; w as usize * h as usize];
    for s in segs {
        let id = i32::try_from(s.id).unwrap_or(i32::MAX);
        s.for_each_texel(w, h, |t| ids[t] = id);
    }
    ids
}

/// 4-connected patches per region id in `ids`, ignoring specks under 1/16384 of
/// the raster.
#[allow(clippy::cast_sign_loss)]
fn count_pieces(ids: &[i32], w: u32, h: u32, regions: usize) -> Vec<usize> {
    let (w, h) = (w as usize, h as usize);
    let min = (w * h / 16_384).max(4);
    let mut seen = vec![false; w * h];
    let mut pieces = vec![0usize; regions];
    let mut stack = Vec::new();
    for start in 0..w * h {
        let id = ids[start];
        if id < 0 || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut size = 0;
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = (i % w, i / w);
            let neighbors = [
                (x > 0).then(|| i - 1),
                (x + 1 < w).then(|| i + 1),
                (y > 0).then(|| i - w),
                (y + 1 < h).then(|| i + w),
            ];
            for n in neighbors.into_iter().flatten() {
                if !seen[n] && ids[n] == id {
                    seen[n] = true;
                    stack.push(n);
                }
            }
        }
        if size >= min {
            pieces[id as usize] += 1;
        }
    }
    pieces
}

/// The texture with each region tinted its picking color and outlined, unused
/// texels darkened, and the ids in `labels` drawn at their regions. Returns the
/// image and the ids that got a label (a region fully overdrawn by smaller ones
/// has no visible texel to put it on).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn render_overlay(
    ids: &[i32],
    texture: &image::RgbaImage,
    labels: &[usize],
) -> (image::RgbImage, BTreeSet<usize>) {
    let (w, h) = texture.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let src = texture.as_raw();
    let mut out = image::RgbImage::new(w, h);
    for (i, px) in out.pixels_mut().enumerate() {
        let base = [src[i * 4], src[i * 4 + 1], src[i * 4 + 2]];
        let id = ids[i];
        let outline = id >= 0
            && ((i % wu + 1 < wu && ids[i + 1] != id) || (i / wu + 1 < hu && ids[i + wu] != id));
        px.0 = if outline {
            [0, 0, 0]
        } else if id < 0 {
            base.map(|c| c / 4)
        } else {
            let tint = morphic::model::segment_color(id as usize);
            [0, 1, 2].map(|c| base[c].midpoint(tint[c]))
        };
    }

    // Each label goes on its region's deepest texel (farthest from any other
    // region), which lands it inside the region's thickest patch even when the
    // region is scattered across the sheet.
    let depth = interior_depth(ids, wu, hu);
    let mut best: BTreeMap<usize, (u32, usize)> =
        labels.iter().map(|&id| (id, (0, usize::MAX))).collect();
    for (i, &id) in ids.iter().enumerate() {
        if id < 0 {
            continue;
        }
        if let Some(b) = best.get_mut(&(id as usize)) {
            if b.1 == usize::MAX || depth[i] > b.0 {
                *b = (depth[i], i);
            }
        }
    }
    let scale = (w.max(h) / 256).max(2);
    let mut drawn = BTreeSet::new();
    for (id, (_, i)) in best {
        if i != usize::MAX {
            draw_label(
                &mut out,
                &id.to_string(),
                (i % wu) as u32,
                (i / wu) as u32,
                scale,
            );
            drawn.insert(id);
        }
    }
    (out, drawn)
}

/// City-block distance from each texel to the nearest texel of another region
/// (or the image edge); 0 outside every region.
#[allow(clippy::many_single_char_names)]
fn interior_depth(ids: &[i32], w: usize, h: usize) -> Vec<u32> {
    let mut d = vec![0u32; w * h];
    let step = |d: &[u32], ids: &[i32], i: usize, n: Option<usize>| {
        n.filter(|&n| ids[n] == ids[i]).map_or(1, |n| d[n] + 1)
    };
    for i in 0..w * h {
        if ids[i] >= 0 {
            let (x, y) = (i % w, i / w);
            d[i] = step(&d, ids, i, (x > 0).then(|| i - 1)).min(step(
                &d,
                ids,
                i,
                (y > 0).then(|| i - w),
            ));
        }
    }
    for i in (0..w * h).rev() {
        if ids[i] >= 0 {
            let (x, y) = (i % w, i / w);
            let back = step(&d, ids, i, (x + 1 < w).then(|| i + 1)).min(step(
                &d,
                ids,
                i,
                (y + 1 < h).then(|| i + w),
            ));
            d[i] = d[i].min(back);
        }
    }
    d
}

/// 3x5 bitmap digits, one row per byte (bit 2 = left column).
const DIGITS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 1, 1, 1],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
];

/// White digits on a black box centered at `(cx, cy)`, kept inside the image.
fn draw_label(img: &mut image::RgbImage, text: &str, cx: u32, cy: u32, scale: u32) {
    let digits: Vec<usize> = text.bytes().map(|b| usize::from(b - b'0')).collect();
    let (w, h) = img.dimensions();
    let count = u32::try_from(digits.len()).unwrap_or(1);
    let box_w = (count * 4 + 1) * scale;
    let box_h = 7 * scale;
    let x0 = cx.saturating_sub(box_w / 2).min(w.saturating_sub(box_w));
    let y0 = cy.saturating_sub(box_h / 2).min(h.saturating_sub(box_h));
    for y in y0..(y0 + box_h).min(h) {
        for x in x0..(x0 + box_w).min(w) {
            img.put_pixel(x, y, image::Rgb([0, 0, 0]));
        }
    }
    for (k, &d) in digits.iter().enumerate() {
        let left = x0 + (u32::try_from(k).unwrap_or(0) * 4 + 1) * scale;
        for (row, bits) in DIGITS[d].iter().enumerate() {
            for col in 0..3u32 {
                if bits & (4 >> col) == 0 {
                    continue;
                }
                let top = y0 + (u32::try_from(row).unwrap_or(0) + 1) * scale;
                for y in top..(top + scale).min(h) {
                    for x in (left + col * scale)..(left + (col + 1) * scale).min(w) {
                        img.put_pixel(x, y, image::Rgb([255, 255, 255]));
                    }
                }
            }
        }
    }
}

fn bake_mask(
    segs: &[Segment],
    opts: &RegionMapOptions,
    width: u32,
    height: u32,
    out_dir: &Path,
) -> Result<RegionMask> {
    if let Some(bad) = opts.select.iter().find(|&&id| id >= segs.len()) {
        bail!(
            "region {bad} does not exist: this texture has {} regions by {} (ids 0..{})",
            segs.len(),
            by_name(opts.by),
            segs.len().saturating_sub(1)
        );
    }
    let (w, h) = (width as usize, height as usize);
    let selected: BTreeSet<usize> = opts.select.iter().copied().collect();
    let mut mask = vec![0u8; w * h];
    let mut other = vec![false; w * h];
    for s in segs {
        if selected.contains(&s.id) {
            s.for_each_texel(width, height, |t| mask[t] = 255);
        } else {
            s.for_each_texel(width, height, |t| other[t] = true);
        }
    }
    let masked = mask.iter().filter(|&&m| m > 0).count();
    let overlap = mask
        .iter()
        .zip(&other)
        .filter(|(&m, &o)| m > 0 && o)
        .count();

    // Grow only into texels no region samples, never over a neighbor region.
    for _ in 0..opts.padding {
        let snap = mask.clone();
        for i in 0..w * h {
            if snap[i] > 0 || other[i] {
                continue;
            }
            let (x, y) = (i % w, i / w);
            let near = (x > 0 && snap[i - 1] > 0)
                || (x + 1 < w && snap[i + 1] > 0)
                || (y > 0 && snap[i - w] > 0)
                || (y + 1 < h && snap[i + w] > 0);
            if near {
                mask[i] = 255;
            }
        }
    }

    let ids: Vec<String> = selected.iter().map(ToString::to_string).collect();
    let name = if ids.len() <= 6 {
        format!("mask-{}-{}.png", by_name(opts.by), ids.join("-"))
    } else {
        let hash = selected.iter().fold(0xcbf2_9ce4_8422_2325_u64, |acc, &id| {
            (acc ^ id as u64).wrapping_mul(0x0100_0000_01b3)
        });
        format!(
            "mask-{}-{}regions-{:08x}.png",
            by_name(opts.by),
            ids.len(),
            hash & 0xffff_ffff
        )
    };
    let png = out_dir.join(name);
    image::GrayImage::from_raw(width, height, mask)
        .context("mask buffer size mismatch")?
        .save(&png)
        .with_context(|| format!("writing {}", png.display()))?;

    #[allow(clippy::cast_precision_loss)]
    Ok(RegionMask {
        png,
        regions: selected.into_iter().collect(),
        coverage: masked as f32 / (w * h) as f32,
        shared_with_unselected: if masked == 0 {
            0.0
        } else {
            overlap as f32 / masked as f32
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_draw_white_digits_inside_the_image() {
        let mut img = image::RgbImage::new(40, 20);
        draw_label(&mut img, "10", 39, 19, 2);
        let white = img.pixels().filter(|p| p.0 == [255, 255, 255]).count();
        // "1" lights 8 cells and "0" lights 12, each cell 2x2 texels.
        assert_eq!(white, (8 + 12) * 4);
    }

    #[test]
    fn uv_params_skip_identity_values() {
        let mut mat = morphic::material::Material::default();
        mat.vector_params
            .insert("g_vTexCoordScale".into(), [1.0, 1.0, 0.0, 0.0]);
        mat.vector_params
            .insert("g_vTexCoordOffset".into(), [0.0, 0.0, 0.0, 0.0]);
        mat.vector_params
            .insert("g_vTexCoordCenter".into(), [0.5, 0.5, 0.0, 0.0]);
        mat.vector_params
            .insert("g_vTexCoordScrollSpeed".into(), [0.0, 0.2, 0.0, 0.0]);
        mat.vector_params
            .insert("g_vColorTint".into(), [2.0, 2.0, 2.0, 0.0]);
        let found = uv_params(&mat);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "g_vTexCoordScrollSpeed");
    }

    #[test]
    fn texel_bounds_round_outward() {
        assert_eq!(
            texel_bounds([0.1, 0.25, 0.5, 1.2], 100, 40),
            [10, 10, 50, 40]
        );
    }
}
