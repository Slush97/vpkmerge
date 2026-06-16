//! Soul-container GLB import pipeline.
//!
//! This module is the dependency-light replacement for the Blender *prep* stage:
//! it reads a user GLB, normalizes its static mesh to the stock soul-container
//! bounds, writes Source 2 source materials/textures/modeldoc, and leaves the
//! compiled-resource step behind an explicit backend boundary.

// Pixel/geometry quantization and FBX index bookkeeping convert between float
// and fixed-width integer lanes with clamped, bounded inputs; the truncation,
// sign loss, and wrap on these casts are intentional.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use anyhow::{anyhow, bail, Context, Result};
use gltf::mesh::Mode;
use image::{ColorType, ImageFormat};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Stock soul-container source/compiled model folder.
pub const DEFAULT_SOUL_CONTAINER_MODEL_REL: &str = "models/props_gameplay/soul_container";

/// Largest stock soul-container axis in Source units.
pub const DEFAULT_SOUL_CONTAINER_TARGET_LARGEST_AXIS: f32 = 12.65;

/// Empirical FBX-to-Source multiplier observed in the resourcecompiler proof.
pub const DEFAULT_SOURCE_UNITS_PER_BLENDER: f32 = 100.0;

/// Stock soul-container sphere collider radius.
pub const DEFAULT_SOUL_CONTAINER_PHYSICS_RADIUS: f32 = 7.0;

/// Controls how a GLB is converted to compiler source content.
#[derive(Debug, Clone)]
pub struct SoulContainerImportOptions {
    /// Source-relative model folder, without a leading slash.
    pub model_rel: String,
    /// Desired largest compiled Source-unit axis.
    pub target_largest_axis: f32,
    /// Scale factor resourcecompiler applies to Blender/FBX-space positions.
    pub source_units_per_blender: f32,
    /// Radius used by the generated source `.vmdl` sphere collider.
    pub physics_radius: f32,
}

impl Default for SoulContainerImportOptions {
    fn default() -> Self {
        Self {
            model_rel: DEFAULT_SOUL_CONTAINER_MODEL_REL.to_string(),
            target_largest_axis: DEFAULT_SOUL_CONTAINER_TARGET_LARGEST_AXIS,
            source_units_per_blender: DEFAULT_SOURCE_UNITS_PER_BLENDER,
            physics_radius: DEFAULT_SOUL_CONTAINER_PHYSICS_RADIUS,
        }
    }
}

/// Axis-aligned bounds for an import stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoulContainerBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub span: [f32; 3],
}

impl SoulContainerBounds {
    fn from_positions(positions: &[[f32; 3]]) -> Result<Self> {
        if positions.is_empty() {
            bail!("no mesh positions");
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for p in positions {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        let span = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        Ok(Self { min, max, span })
    }

    fn center(self) -> [f32; 3] {
        [
            midpoint(self.min[0], self.max[0]),
            midpoint(self.min[1], self.max[1]),
            midpoint(self.min[2], self.max[2]),
        ]
    }

    fn largest_axis(self) -> f32 {
        self.span.into_iter().fold(0.0_f32, f32::max)
    }

    fn scaled(self, scale: f32) -> Self {
        Self {
            min: [
                self.min[0] * scale,
                self.min[1] * scale,
                self.min[2] * scale,
            ],
            max: [
                self.max[0] * scale,
                self.max[1] * scale,
                self.max[2] * scale,
            ],
            span: [
                self.span[0] * scale,
                self.span[1] * scale,
                self.span[2] * scale,
            ],
        }
    }
}

/// One generated source material.
#[derive(Debug, Clone, PartialEq)]
pub struct SoulContainerPreparedMaterial {
    /// Sanitized material stem used for source files.
    pub name: String,
    /// Source-relative material path without extension. This exact value is also
    /// written as the FBX material name.
    pub source_material: String,
    /// Path to the source `.vmat` file.
    pub vmat_path: PathBuf,
    /// Path to the source PNG color texture.
    pub color_texture_path: PathBuf,
}

/// Files and measurements produced by [`prepare_soul_container_import`].
#[derive(Debug, Clone)]
pub struct SoulContainerPreparedSource {
    /// Source tree root passed to the prepare function.
    pub source_root: PathBuf,
    /// Source-relative model folder.
    pub model_rel: String,
    pub model_dir: PathBuf,
    pub fbx_path: PathBuf,
    pub vmdl_path: PathBuf,
    pub materials: Vec<SoulContainerPreparedMaterial>,
    /// GLB node-world bounds after glTF Y-up -> Source/FBX Z-up axis conversion,
    /// before center/scale normalization.
    pub imported_bounds: SoulContainerBounds,
    /// Written FBX-space bounds after normalization.
    pub fbx_bounds: SoulContainerBounds,
    /// Expected compiled Source-unit bounds if the backend uses
    /// `source_units_per_blender`.
    pub expected_source_bounds: SoulContainerBounds,
    /// Uniform scale applied to imported positions before FBX emission.
    pub scale: f32,
    pub vertex_count: usize,
    pub triangle_count: usize,
}

/// Prepare a Source 2 source tree for a soul-container GLB import.
///
/// The output layout is:
///
/// ```text
/// <source_root>/models/props_gameplay/soul_container/
///   soul_container.vmdl
///   model.fbx
///   materials/
///     <material>.vmat
///     <material>_color.png
/// ```
pub fn prepare_soul_container_import(
    glb: impl AsRef<Path>,
    source_root: impl AsRef<Path>,
    options: &SoulContainerImportOptions,
) -> Result<SoulContainerPreparedSource> {
    validate_options(options)?;
    let glb = glb.as_ref();
    let source_root = source_root.as_ref();
    let model_dir = source_root.join(&options.model_rel);
    let materials_dir = model_dir.join("materials");

    let (doc, buffers, images) =
        gltf::import(glb).with_context(|| format!("importing GLB {}", glb.display()))?;
    let mut mesh = extract_static_mesh(&doc, &buffers, options).context("extracting GLB mesh")?;
    if mesh.positions.is_empty() {
        bail!("GLB contains no mesh positions");
    }

    let imported_bounds = SoulContainerBounds::from_positions(&mesh.positions)?;
    let imported_largest_axis = imported_bounds.largest_axis();
    if imported_largest_axis <= 0.0 {
        bail!(
            "invalid imported model bounds: min={:?} max={:?}",
            imported_bounds.min,
            imported_bounds.max
        );
    }
    let scale =
        options.target_largest_axis / (imported_largest_axis * options.source_units_per_blender);
    let center = imported_bounds.center();
    for p in &mut mesh.positions {
        for k in 0..3 {
            p[k] = (p[k] - center[k]) * scale;
        }
    }
    let fbx_bounds = SoulContainerBounds::from_positions(&mesh.positions)?;
    let expected_source_bounds = fbx_bounds.scaled(options.source_units_per_blender);

    std::fs::create_dir_all(&materials_dir)
        .with_context(|| format!("creating {}", materials_dir.display()))?;

    let material_reports =
        write_material_sources(&mesh.materials, &images, &materials_dir, options)
            .context("writing material sources")?;

    let fbx_path = model_dir.join("model.fbx");
    write_fbx(&fbx_path, &mesh).with_context(|| format!("writing {}", fbx_path.display()))?;

    let vmdl_path = model_dir.join("soul_container.vmdl");
    write_vmdl(&vmdl_path, options).with_context(|| format!("writing {}", vmdl_path.display()))?;

    Ok(SoulContainerPreparedSource {
        source_root: source_root.to_path_buf(),
        model_rel: options.model_rel.clone(),
        model_dir,
        fbx_path,
        vmdl_path,
        materials: material_reports,
        imported_bounds,
        fbx_bounds,
        expected_source_bounds,
        scale,
        vertex_count: mesh.positions.len(),
        triangle_count: mesh.triangles.len(),
    })
}

/// Compiler backend for prepared soul-container source.
#[derive(Debug, Clone)]
pub enum SoulContainerCompileBackend {
    /// Valve's Source 2 `resourcecompiler.exe` launched through Proton.
    ResourceCompiler(ResourceCompilerBackend),
    /// Partial pure Rust writer for generated `.vmat_c` and `.vtex_c`.
    ///
    /// This intentionally does not emit `.vmdl_c` yet, so its packed output is
    /// useful for material/texture probing but not as a complete soul-container
    /// replacement.
    PureRust,
}

/// Configuration for the current external resourcecompiler backend.
#[derive(Debug, Clone)]
pub struct ResourceCompilerBackend {
    pub addon: String,
    pub csdk_root: PathBuf,
    pub proton: PathBuf,
    pub steam_root: PathBuf,
    pub proton_prefix: PathBuf,
    /// Remove existing CSDK content/game staging directories for this addon.
    pub force: bool,
    /// Keep CSDK staging directories after a successful compile.
    pub keep_staging: bool,
    /// Additional resourcecompiler arguments appended before `-filelist`.
    pub extra_args: Vec<String>,
}

/// Result from compiling a prepared source tree.
#[derive(Debug, Clone)]
pub struct SoulContainerCompileReport {
    pub output_vpk: PathBuf,
    pub addon: String,
    pub compiled_root: PathBuf,
    pub packed_entries: usize,
}

/// Compile a prepared source tree with a chosen backend and pack the result.
pub fn compile_soul_container_source(
    source_root: impl AsRef<Path>,
    options: &SoulContainerImportOptions,
    backend: &SoulContainerCompileBackend,
    output_vpk: impl AsRef<Path>,
) -> Result<SoulContainerCompileReport> {
    match backend {
        SoulContainerCompileBackend::ResourceCompiler(rc) => {
            compile_with_resourcecompiler(source_root.as_ref(), options, rc, output_vpk.as_ref())
        }
        SoulContainerCompileBackend::PureRust => {
            compile_with_pure_rust(source_root.as_ref(), options, output_vpk.as_ref())
        }
    }
}

/// Compile the prepared soul-container materials/textures with the partial pure
/// Rust backend and pack them into a VPK.
///
/// The resulting tree contains generated `.vmat_c` and `.vtex_c` files only.
/// It deliberately does not include `soul_container.vmdl_c`; use this for
/// material/texture probes until a pure model writer lands.
pub fn compile_soul_container_prepared_pure_rust(
    prepared: &SoulContainerPreparedSource,
    compiled_root: impl AsRef<Path>,
    output_vpk: impl AsRef<Path>,
    force: bool,
) -> Result<SoulContainerCompileReport> {
    validate_source_rel(&prepared.model_rel)?;
    require_dir(&prepared.source_root, "prepared source root")?;
    require_file(&prepared.fbx_path, "prepared FBX")?;
    require_file(&prepared.vmdl_path, "prepared VMDL")?;
    let materials = pure_materials_from_prepared(prepared);
    compile_pure_rust_materials(
        &materials,
        &prepared.model_rel,
        compiled_root.as_ref(),
        output_vpk.as_ref(),
        force,
    )
}

/// Compile an existing prepared source tree with the partial pure Rust backend
/// and pack generated `.vmat_c`/`.vtex_c` resources into a VPK.
///
/// This scans `<source_root>/<model_rel>/materials/*.vmat` and expects each
/// material to have a sibling `<stem>_color.png`.
pub fn compile_soul_container_source_pure_rust(
    source_root: impl AsRef<Path>,
    options: &SoulContainerImportOptions,
    compiled_root: impl AsRef<Path>,
    output_vpk: impl AsRef<Path>,
    force: bool,
) -> Result<SoulContainerCompileReport> {
    let source_root = source_root.as_ref();
    validate_options(options)?;
    require_dir(source_root, "source root")?;
    let materials = scan_pure_source_materials(source_root, options)?;
    compile_pure_rust_materials(
        &materials,
        &options.model_rel,
        compiled_root.as_ref(),
        output_vpk.as_ref(),
        force,
    )
}

#[derive(Debug, Clone)]
#[allow(clippy::struct_field_names)]
struct SourceMaterial {
    name: String,
    source_material: String,
    color: [f32; 4],
    image_index: Option<usize>,
}

#[derive(Debug, Clone)]
struct StaticMesh {
    positions: Vec<[f32; 3]>,
    normals: Vec<Option<[f32; 3]>>,
    texcoords: Vec<[f32; 2]>,
    triangles: Vec<[u32; 3]>,
    triangle_materials: Vec<usize>,
    materials: Vec<SourceMaterial>,
}

#[derive(Debug, Clone)]
struct PureSourceMaterial {
    source_material: String,
    vmat_path: PathBuf,
    color_texture_path: PathBuf,
}

#[derive(Default)]
struct MaterialRegistry {
    by_gltf_index: HashMap<Option<usize>, usize>,
    used_names: HashSet<String>,
    materials: Vec<SourceMaterial>,
}

impl MaterialRegistry {
    fn material_index(
        &mut self,
        material: &gltf::Material<'_>,
        options: &SoulContainerImportOptions,
    ) -> usize {
        let key = material.index();
        if let Some(&existing) = self.by_gltf_index.get(&key) {
            return existing;
        }

        let raw_name = material.name().map_or_else(
            || format!("material_{:02}", self.materials.len()),
            str::to_string,
        );
        let name = unique_safe_name(
            &raw_name,
            &format!("material_{:02}", self.materials.len()),
            &mut self.used_names,
        );
        let source_material = format!("{}/materials/{}", options.model_rel, name);
        let pbr = material.pbr_metallic_roughness();
        let image_index = pbr
            .base_color_texture()
            .map(|info| info.texture().source().index());
        let index = self.materials.len();
        self.materials.push(SourceMaterial {
            name,
            source_material,
            color: pbr.base_color_factor(),
            image_index,
        });
        self.by_gltf_index.insert(key, index);
        index
    }

    fn ensure_default(&mut self, options: &SoulContainerImportOptions) -> usize {
        let key = None;
        if let Some(&existing) = self.by_gltf_index.get(&key) {
            return existing;
        }
        let name = unique_safe_name("default", "material_00", &mut self.used_names);
        let source_material = format!("{}/materials/{}", options.model_rel, name);
        let index = self.materials.len();
        self.materials.push(SourceMaterial {
            name,
            source_material,
            color: [1.0, 1.0, 1.0, 1.0],
            image_index: None,
        });
        self.by_gltf_index.insert(key, index);
        index
    }
}

fn extract_static_mesh(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    options: &SoulContainerImportOptions,
) -> Result<StaticMesh> {
    let world = node_world_transforms(doc);
    let mut registry = MaterialRegistry::default();
    let mut mesh = StaticMesh {
        positions: Vec::new(),
        normals: Vec::new(),
        texcoords: Vec::new(),
        triangles: Vec::new(),
        triangle_materials: Vec::new(),
        materials: Vec::new(),
    };

    for node in doc.nodes() {
        let Some(node_mesh) = node.mesh() else {
            continue;
        };
        let node_world = world[node.index()];
        for primitive in node_mesh.primitives() {
            if primitive.mode() != Mode::Triangles {
                bail!(
                    "unsupported GLB primitive mode {:?}; only triangles are supported",
                    primitive.mode()
                );
            }
            let reader = primitive.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
            let local_positions: Vec<[f32; 3]> = reader
                .read_positions()
                .ok_or_else(|| anyhow!("mesh primitive has no POSITION"))?
                .collect();
            let local_normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map(Iterator::collect)
                .unwrap_or_default();
            let local_uvs: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|u| u.into_f32().collect())
                .unwrap_or_default();
            let indices: Vec<u32> = reader.read_indices().map_or_else(
                || (0..u32::try_from(local_positions.len()).unwrap_or(0)).collect(),
                |i| i.into_u32().collect(),
            );
            if !indices.len().is_multiple_of(3) {
                bail!(
                    "triangle primitive has {} indices, not a multiple of 3",
                    indices.len()
                );
            }

            let material_index = if primitive.material().index().is_some() {
                registry.material_index(&primitive.material(), options)
            } else {
                registry.ensure_default(options)
            };
            let base = u32::try_from(mesh.positions.len())
                .context("too many vertices for u32-indexed FBX mesh")?;
            for (i, p) in local_positions.iter().enumerate() {
                let p = transform_point(&node_world, *p);
                mesh.positions.push(gltf_to_source_point(p));
                let normal = local_normals
                    .get(i)
                    .map(|n| normalize(gltf_to_source_vector(transform_vector(&node_world, *n))));
                mesh.normals.push(normal);
                mesh.texcoords
                    .push(local_uvs.get(i).copied().unwrap_or([0.0, 0.0]));
            }
            for tri in indices.chunks_exact(3) {
                mesh.triangles
                    .push([base + tri[0], base + tri[1], base + tri[2]]);
                mesh.triangle_materials.push(material_index);
            }
        }
    }
    if registry.materials.is_empty() && !mesh.triangles.is_empty() {
        let default = registry.ensure_default(options);
        mesh.triangle_materials.fill(default);
    }
    mesh.materials = registry.materials;
    Ok(mesh)
}

fn write_material_sources(
    materials: &[SourceMaterial],
    images: &[gltf::image::Data],
    materials_dir: &Path,
    options: &SoulContainerImportOptions,
) -> Result<Vec<SoulContainerPreparedMaterial>> {
    let mut reports = Vec::with_capacity(materials.len());
    for material in materials {
        let color_texture_path = materials_dir.join(format!("{}_color.png", material.name));
        write_material_color_png(material, images, &color_texture_path)?;
        let vmat_path = materials_dir.join(format!("{}.vmat", material.name));
        write_vmat(&vmat_path, material, options)?;
        reports.push(SoulContainerPreparedMaterial {
            name: material.name.clone(),
            source_material: material.source_material.clone(),
            vmat_path,
            color_texture_path,
        });
    }
    Ok(reports)
}

fn write_material_color_png(
    material: &SourceMaterial,
    images: &[gltf::image::Data],
    out: &Path,
) -> Result<()> {
    if let Some(image_index) = material.image_index {
        if let Some(image) = images.get(image_index) {
            let rgba = image_to_rgba8(image)
                .with_context(|| format!("converting GLB image {image_index} to RGBA8"))?;
            image::save_buffer_with_format(
                out,
                &rgba,
                image.width,
                image.height,
                ColorType::Rgba8,
                ImageFormat::Png,
            )
            .with_context(|| format!("saving {}", out.display()))?;
            return Ok(());
        }
    }

    let px = [
        linear_to_srgb_u8(material.color[0]),
        linear_to_srgb_u8(material.color[1]),
        linear_to_srgb_u8(material.color[2]),
        (material.color[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    ];
    let mut rgba = Vec::with_capacity(2 * 2 * 4);
    for _ in 0..4 {
        rgba.extend_from_slice(&px);
    }
    image::save_buffer_with_format(out, &rgba, 2, 2, ColorType::Rgba8, ImageFormat::Png)
        .with_context(|| format!("saving {}", out.display()))
}

#[allow(clippy::unnecessary_wraps)]
fn image_to_rgba8(image: &gltf::image::Data) -> Result<Vec<u8>> {
    use gltf::image::Format;
    let pixels = &image.pixels;
    let out = match image.format {
        Format::R8 => pixels.iter().flat_map(|&r| [r, r, r, 255]).collect(),
        Format::R8G8 => pixels
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        Format::R8G8B8 => pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        Format::R8G8B8A8 => pixels.clone(),
        Format::R16 => pixels
            .chunks_exact(2)
            .flat_map(|p| {
                let r = u16::from_le_bytes([p[0], p[1]]);
                let r = (r >> 8) as u8;
                [r, r, r, 255]
            })
            .collect(),
        Format::R16G16 => pixels
            .chunks_exact(4)
            .flat_map(|p| {
                let r = (u16::from_le_bytes([p[0], p[1]]) >> 8) as u8;
                let a = (u16::from_le_bytes([p[2], p[3]]) >> 8) as u8;
                [r, r, r, a]
            })
            .collect(),
        Format::R16G16B16 => pixels
            .chunks_exact(6)
            .flat_map(|p| {
                [
                    (u16::from_le_bytes([p[0], p[1]]) >> 8) as u8,
                    (u16::from_le_bytes([p[2], p[3]]) >> 8) as u8,
                    (u16::from_le_bytes([p[4], p[5]]) >> 8) as u8,
                    255,
                ]
            })
            .collect(),
        Format::R16G16B16A16 => pixels
            .chunks_exact(8)
            .flat_map(|p| {
                [
                    (u16::from_le_bytes([p[0], p[1]]) >> 8) as u8,
                    (u16::from_le_bytes([p[2], p[3]]) >> 8) as u8,
                    (u16::from_le_bytes([p[4], p[5]]) >> 8) as u8,
                    (u16::from_le_bytes([p[6], p[7]]) >> 8) as u8,
                ]
            })
            .collect(),
        Format::R32G32B32FLOAT => pixels
            .chunks_exact(12)
            .flat_map(|p| {
                [
                    f32_to_unorm8(f32::from_le_bytes([p[0], p[1], p[2], p[3]])),
                    f32_to_unorm8(f32::from_le_bytes([p[4], p[5], p[6], p[7]])),
                    f32_to_unorm8(f32::from_le_bytes([p[8], p[9], p[10], p[11]])),
                    255,
                ]
            })
            .collect(),
        Format::R32G32B32A32FLOAT => pixels
            .chunks_exact(16)
            .flat_map(|p| {
                [
                    f32_to_unorm8(f32::from_le_bytes([p[0], p[1], p[2], p[3]])),
                    f32_to_unorm8(f32::from_le_bytes([p[4], p[5], p[6], p[7]])),
                    f32_to_unorm8(f32::from_le_bytes([p[8], p[9], p[10], p[11]])),
                    f32_to_unorm8(f32::from_le_bytes([p[12], p[13], p[14], p[15]])),
                ]
            })
            .collect(),
    };
    Ok(out)
}

fn write_vmat(
    path: &Path,
    material: &SourceMaterial,
    options: &SoulContainerImportOptions,
) -> Result<()> {
    let texture_rel = format!(
        "{}/materials/{}_color.png",
        options.model_rel, material.name
    );
    let text = format!(
        "\"Layer0\"\n\
         {{\n\
             \"shader\" \"pbr.vfx\"\n\n\
             \"F_SELF_ILLUM\" \"1\"\n\
             \"F_USE_NPR_LIGHTING\" \"1\"\n\
             \"F_USE_STATUS_EFFECTS_PROXY\" \"1\"\n\n\
             \"TextureColor\" \"{texture_rel}\"\n\
             \"TextureColor1\" \"{texture_rel}\"\n\n\
             \"g_bMaskColorTint1\" \"1\"\n\
             \"g_bMaskVertexColorTint1\" \"1\"\n\
             \"g_nTextureColorTintMode1\" \"0\"\n\
             \"g_vColorTint1\" \"[1 1 1 0]\"\n\
             \"g_fVertexColorStrength1\" \"1\"\n\n\
             \"g_flSelfIllumAlbedoFactor1\" \"1\"\n\
             \"g_flSelfIllumScale1\" \"0\"\n\
         }}\n"
    );
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

fn write_vmdl(path: &Path, options: &SoulContainerImportOptions) -> Result<()> {
    let text = format!(
        "<!-- kv3 encoding:text:version{{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d}} \
         format:modeldoc28:version{{fb63b6ca-f435-4aa0-a2c7-c66ddc651dca}} -->\n\
         {{\n\
         \trootNode = \n\
         \t{{\n\
         \t\t_class = \"RootNode\"\n\
         \t\tchildren = \n\
         \t\t[\n\
         \t\t\t{{\n\
         \t\t\t\t_class = \"BoneMarkupList\"\n\
         \t\t\t\tchildren = [ ]\n\
         \t\t\t\tbone_cull_type = \"None\"\n\
         \t\t\t}},\n\
         \t\t\t{{\n\
         \t\t\t\t_class = \"RenderMeshList\"\n\
         \t\t\t\tchildren = \n\
         \t\t\t\t[\n\
         \t\t\t\t\t{{\n\
         \t\t\t\t\t\t_class = \"RenderMeshFile\"\n\
         \t\t\t\t\t\tname = \"soul_container\"\n\
         \t\t\t\t\t\tfilename = \"{}/model.fbx\"\n\
         \t\t\t\t\t}},\n\
         \t\t\t\t]\n\
         \t\t\t}},\n\
         \t\t\t{{\n\
         \t\t\t\t_class = \"Skeleton\"\n\
         \t\t\t\tchildren = \n\
         \t\t\t\t[\n\
         \t\t\t\t\t{{\n\
         \t\t\t\t\t\t_class = \"Bone\"\n\
         \t\t\t\t\t\tname = \"joint1\"\n\
         \t\t\t\t\t\torigin = [ 0.0, 0.0, 0.0 ]\n\
         \t\t\t\t\t\tangles = [ 0.0, 90.0, 90.0 ]\n\
         \t\t\t\t\t\tdo_not_discard = true\n\
         \t\t\t\t\t}},\n\
         \t\t\t\t]\n\
         \t\t\t}},\n\
         \t\t\t{{\n\
         \t\t\t\t_class = \"PhysicsShapeList\"\n\
         \t\t\t\tchildren = \n\
         \t\t\t\t[\n\
         \t\t\t\t\t{{\n\
         \t\t\t\t\t\t_class = \"PhysicsShapeSphere\"\n\
         \t\t\t\t\t\tparent_bone = \"joint1\"\n\
         \t\t\t\t\t\tsurface_prop = \"hideout_ball\"\n\
         \t\t\t\t\t\tcollision_tags = \"\"\n\
         \t\t\t\t\t\tradius = {}\n\
         \t\t\t\t\t\tcenter = [ 0.0, 0.0, 0.0 ]\n\
         \t\t\t\t\t\tname = \"\"\n\
         \t\t\t\t\t}},\n\
         \t\t\t\t]\n\
         \t\t\t}},\n\
         \t\t]\n\
         \t}}\n\
         }}\n",
        options.model_rel, options.physics_radius
    );
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

#[allow(clippy::too_many_lines)]
fn write_fbx(path: &Path, mesh: &StaticMesh) -> Result<()> {
    let geometry_id = 100_000_i64;
    let model_id = 110_000_i64;
    let material_base_id = 120_000_i64;
    let node_attribute_base_id = 130_000_i64;
    if mesh.triangles.is_empty() {
        bail!("cannot write FBX with no triangles");
    }

    let parts = build_fbx_mesh_parts(mesh, geometry_id, model_id)?;
    let mesh_root_model_id = model_id + i64::try_from(parts.len())?;
    let scene_root_model_id = mesh_root_model_id + 1;
    let mesh_root_attribute_id = node_attribute_base_id;
    let scene_root_attribute_id = node_attribute_base_id + 1;

    let mut material_nodes = Vec::with_capacity(mesh.materials.len());
    for (i, material) in mesh.materials.iter().enumerate() {
        let id = material_base_id + i as i64;
        material_nodes.push(FbxNode::new(
            "Material",
            vec![
                FbxProp::L(id),
                FbxProp::s(format!("{}\0\x01Material", material.source_material)),
                FbxProp::s(""),
            ],
            vec![
                FbxNode::new("Version", vec![FbxProp::I(102)], Vec::new()),
                FbxNode::new("ShadingModel", vec![FbxProp::s("Phong")], Vec::new()),
                FbxNode::new("MultiLayer", vec![FbxProp::I(0)], Vec::new()),
                FbxNode::new(
                    "Properties70",
                    Vec::new(),
                    vec![
                        fbx_p_node(
                            "DiffuseColor",
                            "Color",
                            "",
                            "A",
                            vec![FbxProp::D(1.0), FbxProp::D(1.0), FbxProp::D(1.0)],
                        ),
                        fbx_p_node("DiffuseFactor", "Number", "", "A", vec![FbxProp::D(1.0)]),
                        fbx_p_node("ShadingModel", "KString", "", "", vec![FbxProp::s("phong")]),
                    ],
                ),
            ],
        ));
    }

    let mut object_nodes = Vec::with_capacity(parts.len() * 2 + material_nodes.len());
    for part in &parts {
        object_nodes.push(FbxNode::new(
            "Geometry",
            vec![
                FbxProp::L(part.geometry_id),
                FbxProp::s(format!("{}\0\x01Geometry", part.name)),
                FbxProp::s("Mesh"),
            ],
            vec![
                FbxNode::new("Properties70", Vec::new(), Vec::new()),
                FbxNode::new("GeometryVersion", vec![FbxProp::I(124)], Vec::new()),
                FbxNode::new(
                    "Vertices",
                    vec![FbxProp::DoubleArray(part.vertices.clone())],
                    Vec::new(),
                ),
                FbxNode::new(
                    "PolygonVertexIndex",
                    vec![FbxProp::IntArray(part.polygon_indices.clone())],
                    Vec::new(),
                ),
                FbxNode::new(
                    "Edges",
                    vec![FbxProp::IntArray(part.edges.clone())],
                    Vec::new(),
                ),
                FbxNode::new(
                    "LayerElementNormal",
                    vec![FbxProp::I(0)],
                    vec![
                        FbxNode::new("Version", vec![FbxProp::I(101)], Vec::new()),
                        FbxNode::new("Name", vec![FbxProp::s("")], Vec::new()),
                        FbxNode::new(
                            "MappingInformationType",
                            vec![FbxProp::s("ByPolygonVertex")],
                            Vec::new(),
                        ),
                        FbxNode::new(
                            "ReferenceInformationType",
                            vec![FbxProp::s("IndexToDirect")],
                            Vec::new(),
                        ),
                        FbxNode::new(
                            "Normals",
                            vec![FbxProp::DoubleArray(part.normals.clone())],
                            Vec::new(),
                        ),
                        FbxNode::new(
                            "NormalsIndex",
                            vec![FbxProp::IntArray(part.normal_indices.clone())],
                            Vec::new(),
                        ),
                    ],
                ),
                FbxNode::new(
                    "LayerElementUV",
                    vec![FbxProp::I(0)],
                    vec![
                        FbxNode::new("Version", vec![FbxProp::I(101)], Vec::new()),
                        FbxNode::new("Name", vec![FbxProp::s("UVMap")], Vec::new()),
                        FbxNode::new(
                            "MappingInformationType",
                            vec![FbxProp::s("ByPolygonVertex")],
                            Vec::new(),
                        ),
                        FbxNode::new(
                            "ReferenceInformationType",
                            vec![FbxProp::s("IndexToDirect")],
                            Vec::new(),
                        ),
                        FbxNode::new(
                            "UV",
                            vec![FbxProp::DoubleArray(part.uvs.clone())],
                            Vec::new(),
                        ),
                        FbxNode::new(
                            "UVIndex",
                            vec![FbxProp::IntArray(part.uv_indices.clone())],
                            Vec::new(),
                        ),
                    ],
                ),
                FbxNode::new(
                    "LayerElementMaterial",
                    vec![FbxProp::I(0)],
                    vec![
                        FbxNode::new("Version", vec![FbxProp::I(101)], Vec::new()),
                        FbxNode::new("Name", vec![FbxProp::s("")], Vec::new()),
                        FbxNode::new(
                            "MappingInformationType",
                            vec![FbxProp::s("AllSame")],
                            Vec::new(),
                        ),
                        FbxNode::new(
                            "ReferenceInformationType",
                            vec![FbxProp::s("IndexToDirect")],
                            Vec::new(),
                        ),
                        FbxNode::new("Materials", vec![FbxProp::IntArray(vec![0])], Vec::new()),
                    ],
                ),
                FbxNode::new(
                    "Layer",
                    vec![FbxProp::I(0)],
                    vec![
                        FbxNode::new("Version", vec![FbxProp::I(100)], Vec::new()),
                        fbx_layer_element("LayerElementNormal"),
                        fbx_layer_element("LayerElementMaterial"),
                        fbx_layer_element("LayerElementUV"),
                    ],
                ),
            ],
        ));
    }
    for part in &parts {
        object_nodes.push(FbxNode::new(
            "Model",
            vec![
                FbxProp::L(part.model_id),
                FbxProp::s(format!("{}\0\x01Model", part.name)),
                FbxProp::s("Mesh"),
            ],
            vec![
                FbxNode::new("Version", vec![FbxProp::I(232)], Vec::new()),
                FbxNode::new(
                    "Properties70",
                    Vec::new(),
                    vec![
                        fbx_p_node(
                            "DefaultAttributeIndex",
                            "int",
                            "Integer",
                            "",
                            vec![FbxProp::I(0)],
                        ),
                        fbx_p_node("InheritType", "enum", "", "", vec![FbxProp::I(1)]),
                    ],
                ),
                FbxNode::new("Shading", vec![FbxProp::C(true)], Vec::new()),
                FbxNode::new("Culling", vec![FbxProp::s("CullingOff")], Vec::new()),
                FbxNode::new("MultiLayer", vec![FbxProp::I(0)], Vec::new()),
                FbxNode::new("MultiTake", vec![FbxProp::I(0)], Vec::new()),
            ],
        ));
    }
    object_nodes.push(fbx_null_model(
        mesh_root_model_id,
        "vpkmerge_soul_container",
        false,
    ));
    object_nodes.push(fbx_null_model(scene_root_model_id, "Sketchfab_model", true));
    object_nodes.push(fbx_null_node_attribute(
        mesh_root_attribute_id,
        "vpkmerge_soul_container",
    ));
    object_nodes.push(fbx_null_node_attribute(
        scene_root_attribute_id,
        "Sketchfab_model",
    ));
    object_nodes.extend(material_nodes);

    let mut connection_nodes = Vec::with_capacity(parts.len() * 3);
    for part in &parts {
        connection_nodes.push(FbxNode::new(
            "C",
            vec![
                FbxProp::s("OO"),
                FbxProp::L(part.geometry_id),
                FbxProp::L(part.model_id),
            ],
            Vec::new(),
        ));
        connection_nodes.push(FbxNode::new(
            "C",
            vec![
                FbxProp::s("OO"),
                FbxProp::L(part.model_id),
                FbxProp::L(mesh_root_model_id),
            ],
            Vec::new(),
        ));
        connection_nodes.push(FbxNode::new(
            "C",
            vec![
                FbxProp::s("OO"),
                FbxProp::L(material_base_id + part.material_index as i64),
                FbxProp::L(part.model_id),
            ],
            Vec::new(),
        ));
    }
    connection_nodes.push(FbxNode::new(
        "C",
        vec![
            FbxProp::s("OO"),
            FbxProp::L(mesh_root_model_id),
            FbxProp::L(scene_root_model_id),
        ],
        Vec::new(),
    ));
    connection_nodes.push(FbxNode::new(
        "C",
        vec![
            FbxProp::s("OO"),
            FbxProp::L(scene_root_model_id),
            FbxProp::L(0),
        ],
        Vec::new(),
    ));
    connection_nodes.push(FbxNode::new(
        "C",
        vec![
            FbxProp::s("OO"),
            FbxProp::L(mesh_root_attribute_id),
            FbxProp::L(mesh_root_model_id),
        ],
        Vec::new(),
    ));
    connection_nodes.push(FbxNode::new(
        "C",
        vec![
            FbxProp::s("OO"),
            FbxProp::L(scene_root_attribute_id),
            FbxProp::L(scene_root_model_id),
        ],
        Vec::new(),
    ));

    let root_nodes = vec![
        FbxNode::new(
            "FBXHeaderExtension",
            Vec::new(),
            vec![
                FbxNode::new("FBXHeaderVersion", vec![FbxProp::I(1003)], Vec::new()),
                FbxNode::new("FBXVersion", vec![FbxProp::I(7400)], Vec::new()),
                FbxNode::new("Creator", vec![FbxProp::s("vpkmerge-core")], Vec::new()),
            ],
        ),
        FbxNode::new(
            "FileId",
            vec![FbxProp::Binary(vec![
                0x28, 0xb3, 0x2a, 0xeb, 0xb6, 0x24, 0xcc, 0xc2, 0xbf, 0xc8, 0xb0, 0x2a, 0xa9, 0x2b,
                0xfc, 0xf1,
            ])],
            Vec::new(),
        ),
        FbxNode::new(
            "CreationTime",
            vec![FbxProp::s("1970-01-01 10:00:00:000")],
            Vec::new(),
        ),
        FbxNode::new(
            "Creator",
            vec![FbxProp::s("Blender (stable FBX IO) - 5.1.1 - 5.15.0")],
            Vec::new(),
        ),
        FbxNode::new(
            "GlobalSettings",
            Vec::new(),
            vec![
                FbxNode::new("Version", vec![FbxProp::I(1000)], Vec::new()),
                FbxNode::new(
                    "Properties70",
                    Vec::new(),
                    vec![
                        fbx_p_node("UpAxis", "int", "Integer", "", vec![FbxProp::I(1)]),
                        fbx_p_node("UpAxisSign", "int", "Integer", "", vec![FbxProp::I(1)]),
                        fbx_p_node("FrontAxis", "int", "Integer", "", vec![FbxProp::I(2)]),
                        fbx_p_node("FrontAxisSign", "int", "Integer", "", vec![FbxProp::I(1)]),
                        fbx_p_node("CoordAxis", "int", "Integer", "", vec![FbxProp::I(0)]),
                        fbx_p_node("CoordAxisSign", "int", "Integer", "", vec![FbxProp::I(1)]),
                        fbx_p_node("OriginalUpAxis", "int", "Integer", "", vec![FbxProp::I(-1)]),
                        fbx_p_node(
                            "OriginalUpAxisSign",
                            "int",
                            "Integer",
                            "",
                            vec![FbxProp::I(1)],
                        ),
                        fbx_p_node(
                            "UnitScaleFactor",
                            "double",
                            "Number",
                            "",
                            vec![FbxProp::D(1.0)],
                        ),
                        fbx_p_node(
                            "OriginalUnitScaleFactor",
                            "double",
                            "Number",
                            "",
                            vec![FbxProp::D(1.0)],
                        ),
                        fbx_p_node(
                            "AmbientColor",
                            "ColorRGB",
                            "Color",
                            "",
                            vec![FbxProp::D(0.0), FbxProp::D(0.0), FbxProp::D(0.0)],
                        ),
                        fbx_p_node(
                            "DefaultCamera",
                            "KString",
                            "",
                            "",
                            vec![FbxProp::s("Producer Perspective")],
                        ),
                        fbx_p_node("TimeMode", "enum", "", "", vec![FbxProp::I(11)]),
                        fbx_p_node("TimeSpanStart", "KTime", "Time", "", vec![FbxProp::L(0)]),
                        fbx_p_node(
                            "TimeSpanStop",
                            "KTime",
                            "Time",
                            "",
                            vec![FbxProp::L(46_186_158_000)],
                        ),
                        fbx_p_node(
                            "CustomFrameRate",
                            "double",
                            "Number",
                            "",
                            vec![FbxProp::D(24.0)],
                        ),
                    ],
                ),
            ],
        ),
        FbxNode::new(
            "Documents",
            Vec::new(),
            vec![
                FbxNode::new("Count", vec![FbxProp::I(1)], Vec::new()),
                FbxNode::new(
                    "Document",
                    vec![FbxProp::L(1), FbxProp::s(""), FbxProp::s("Scene")],
                    vec![FbxNode::new("RootNode", vec![FbxProp::L(0)], Vec::new())],
                ),
            ],
        ),
        FbxNode::new("References", Vec::new(), Vec::new()),
        FbxNode::new(
            "Definitions",
            Vec::new(),
            vec![
                FbxNode::new("Version", vec![FbxProp::I(100)], Vec::new()),
                FbxNode::new(
                    "Count",
                    vec![FbxProp::I(
                        1 + i32::try_from(parts.len())?
                            + i32::try_from(parts.len() + 2)?
                            + i32::try_from(mesh.materials.len())?
                            + 2,
                    )],
                    Vec::new(),
                ),
                fbx_object_type("GlobalSettings", 1),
                fbx_object_type_with_template(
                    "NodeAttribute",
                    2,
                    fbx_property_template("FbxNull", fbx_null_template_properties()),
                ),
                fbx_object_type_with_template(
                    "Geometry",
                    i32::try_from(parts.len())?,
                    fbx_property_template("FbxMesh", fbx_mesh_template_properties()),
                ),
                fbx_object_type_with_template(
                    "Model",
                    i32::try_from(parts.len() + 2)?,
                    fbx_property_template("FbxNode", fbx_node_template_properties()),
                ),
                fbx_object_type_with_template(
                    "Material",
                    i32::try_from(mesh.materials.len())?,
                    fbx_property_template("FbxSurfacePhong", fbx_phong_template_properties()),
                ),
            ],
        ),
        FbxNode::new("Objects", Vec::new(), object_nodes),
        FbxNode::new("Connections", Vec::new(), connection_nodes),
        FbxNode::new(
            "Takes",
            Vec::new(),
            vec![FbxNode::new("Current", vec![FbxProp::s("")], Vec::new())],
        ),
    ];

    let bytes = write_binary_fbx(&root_nodes)?;
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

#[derive(Debug, Clone)]
struct FbxMeshPart {
    material_index: usize,
    geometry_id: i64,
    model_id: i64,
    name: String,
    vertices: Vec<f64>,
    polygon_indices: Vec<i32>,
    edges: Vec<i32>,
    normals: Vec<f64>,
    normal_indices: Vec<i32>,
    uvs: Vec<f64>,
    uv_indices: Vec<i32>,
}

fn build_fbx_mesh_parts(
    mesh: &StaticMesh,
    geometry_base_id: i64,
    model_base_id: i64,
) -> Result<Vec<FbxMeshPart>> {
    if mesh.triangle_materials.len() != mesh.triangles.len() {
        bail!(
            "triangle/material mismatch: {} triangles, {} material refs",
            mesh.triangles.len(),
            mesh.triangle_materials.len()
        );
    }

    let mut groups = vec![Vec::new(); mesh.materials.len()];
    for (triangle_index, &material_index) in mesh.triangle_materials.iter().enumerate() {
        let Some(group) = groups.get_mut(material_index) else {
            bail!("triangle {triangle_index} references missing material index {material_index}");
        };
        group.push(triangle_index);
    }

    let mut parts = Vec::new();
    for (material_index, triangle_indices) in groups.into_iter().enumerate() {
        if triangle_indices.is_empty() {
            continue;
        }
        let part_index = parts.len();
        let mut part = FbxMeshPart {
            material_index,
            geometry_id: geometry_base_id + i64::try_from(part_index)?,
            model_id: model_base_id + i64::try_from(part_index)?,
            name: format!("mesh_{}", mesh.materials[material_index].name),
            vertices: Vec::new(),
            polygon_indices: Vec::with_capacity(triangle_indices.len() * 3),
            edges: Vec::new(),
            normals: Vec::new(),
            normal_indices: Vec::with_capacity(triangle_indices.len() * 3),
            uvs: Vec::new(),
            uv_indices: Vec::with_capacity(triangle_indices.len() * 3),
        };
        let mut remap = HashMap::<u32, i32>::new();
        let mut edge_map = HashSet::<(i32, i32)>::new();
        for triangle_index in triangle_indices {
            let tri = mesh.triangles[triangle_index];
            let fallback = face_normal(
                mesh.positions[tri[0] as usize],
                mesh.positions[tri[1] as usize],
                mesh.positions[tri[2] as usize],
            );
            let a = remap_fbx_vertex(mesh, &mut part, &mut remap, tri[0], fallback)?;
            let b = remap_fbx_vertex(mesh, &mut part, &mut remap, tri[1], fallback)?;
            let c = remap_fbx_vertex(mesh, &mut part, &mut remap, tri[2], fallback)?;
            let polygon_base = i32::try_from(part.polygon_indices.len())?;
            push_fbx_edge(&mut edge_map, &mut part.edges, a, b, polygon_base);
            push_fbx_edge(&mut edge_map, &mut part.edges, b, c, polygon_base + 1);
            push_fbx_edge(&mut edge_map, &mut part.edges, c, a, polygon_base + 2);
            part.polygon_indices.extend([a, b, -c - 1]);
            part.normal_indices.extend([a, b, c]);
            part.uv_indices.extend([a, b, c]);
        }
        parts.push(part);
    }
    if parts.is_empty() {
        bail!("cannot write FBX with no materialized mesh parts");
    }
    Ok(parts)
}

fn push_fbx_edge(
    seen: &mut HashSet<(i32, i32)>,
    edges: &mut Vec<i32>,
    a: i32,
    b: i32,
    polygon_vertex_offset: i32,
) {
    let key = if a <= b { (a, b) } else { (b, a) };
    if seen.insert(key) {
        edges.push(polygon_vertex_offset);
    }
}

fn remap_fbx_vertex(
    mesh: &StaticMesh,
    part: &mut FbxMeshPart,
    remap: &mut HashMap<u32, i32>,
    source_index: u32,
    fallback_normal: [f32; 3],
) -> Result<i32> {
    if let Some(&existing) = remap.get(&source_index) {
        return Ok(existing);
    }
    let vertex_index = i32::try_from(remap.len()).context("too many vertices for FBX i32 index")?;
    let position = mesh
        .positions
        .get(source_index as usize)
        .copied()
        .with_context(|| format!("missing vertex position {source_index}"))?;
    part.vertices
        .extend(position.iter().map(|&value| f64::from(value)));

    let normal = mesh
        .normals
        .get(source_index as usize)
        .and_then(|normal| *normal)
        .unwrap_or(fallback_normal);
    part.normals
        .extend(normal.iter().map(|&value| f64::from(value)));

    let uv = mesh
        .texcoords
        .get(source_index as usize)
        .copied()
        .unwrap_or([0.0, 0.0]);
    part.uvs.extend([f64::from(uv[0]), f64::from(uv[1])]);
    remap.insert(source_index, vertex_index);
    Ok(vertex_index)
}

#[derive(Debug, Clone)]
struct FbxNode {
    name: &'static str,
    props: Vec<FbxProp>,
    children: Vec<FbxNode>,
}

impl FbxNode {
    fn new(name: &'static str, props: Vec<FbxProp>, children: Vec<FbxNode>) -> Self {
        Self {
            name,
            props,
            children,
        }
    }
}

#[derive(Debug, Clone)]
enum FbxProp {
    C(bool),
    I(i32),
    D(f64),
    L(i64),
    S(String),
    Binary(Vec<u8>),
    DoubleArray(Vec<f64>),
    IntArray(Vec<i32>),
}

impl FbxProp {
    fn s(value: impl Into<String>) -> Self {
        Self::S(value.into())
    }
}

fn fbx_p_node(
    name: impl Into<String>,
    type_name: impl Into<String>,
    label: impl Into<String>,
    flags: impl Into<String>,
    values: Vec<FbxProp>,
) -> FbxNode {
    let mut props = vec![
        FbxProp::s(name),
        FbxProp::s(type_name),
        FbxProp::s(label),
        FbxProp::s(flags),
    ];
    props.extend(values);
    FbxNode::new("P", props, Vec::new())
}

fn fbx_layer_element(type_name: &str) -> FbxNode {
    FbxNode::new(
        "LayerElement",
        Vec::new(),
        vec![
            FbxNode::new("Type", vec![FbxProp::s(type_name)], Vec::new()),
            FbxNode::new("TypedIndex", vec![FbxProp::I(0)], Vec::new()),
        ],
    )
}

fn fbx_null_model(id: i64, name: &str, blender_root_transform: bool) -> FbxNode {
    let mut properties = Vec::new();
    if blender_root_transform {
        properties.push(fbx_p_node(
            "Lcl Rotation",
            "Lcl Rotation",
            "",
            "A",
            vec![FbxProp::D(179.999_991), FbxProp::D(-0.0), FbxProp::D(0.0)],
        ));
        properties.push(fbx_p_node(
            "Lcl Scaling",
            "Lcl Scaling",
            "",
            "A",
            vec![
                FbxProp::D(100.0),
                FbxProp::D(100.000_015),
                FbxProp::D(100.000_015),
            ],
        ));
    }
    properties.push(fbx_p_node(
        "DefaultAttributeIndex",
        "int",
        "Integer",
        "",
        vec![FbxProp::I(0)],
    ));
    properties.push(fbx_p_node(
        "InheritType",
        "enum",
        "",
        "",
        vec![FbxProp::I(1)],
    ));

    FbxNode::new(
        "Model",
        vec![
            FbxProp::L(id),
            FbxProp::s(format!("{name}\0\x01Model")),
            FbxProp::s("Null"),
        ],
        vec![
            FbxNode::new("Version", vec![FbxProp::I(232)], Vec::new()),
            FbxNode::new("Properties70", Vec::new(), properties),
            FbxNode::new("Shading", vec![FbxProp::C(true)], Vec::new()),
            FbxNode::new("Culling", vec![FbxProp::s("CullingOff")], Vec::new()),
            FbxNode::new("MultiLayer", vec![FbxProp::I(0)], Vec::new()),
            FbxNode::new("MultiTake", vec![FbxProp::I(0)], Vec::new()),
        ],
    )
}

fn fbx_null_node_attribute(id: i64, name: &str) -> FbxNode {
    FbxNode::new(
        "NodeAttribute",
        vec![
            FbxProp::L(id),
            FbxProp::s(format!("{name}\0\x01NodeAttribute")),
            FbxProp::s("Null"),
        ],
        vec![
            FbxNode::new("TypeFlags", vec![FbxProp::s("Null")], Vec::new()),
            FbxNode::new("Properties70", Vec::new(), Vec::new()),
        ],
    )
}

fn fbx_object_type(name: impl Into<String>, count: i32) -> FbxNode {
    FbxNode::new(
        "ObjectType",
        vec![FbxProp::s(name)],
        vec![FbxNode::new("Count", vec![FbxProp::I(count)], Vec::new())],
    )
}

fn fbx_object_type_with_template(
    name: impl Into<String>,
    count: i32,
    template: FbxNode,
) -> FbxNode {
    FbxNode::new(
        "ObjectType",
        vec![FbxProp::s(name)],
        vec![
            FbxNode::new("Count", vec![FbxProp::I(count)], Vec::new()),
            template,
        ],
    )
}

fn fbx_property_template(native_type: &str, properties: Vec<FbxNode>) -> FbxNode {
    FbxNode::new(
        "PropertyTemplate",
        vec![FbxProp::s(native_type)],
        vec![FbxNode::new("Properties70", Vec::new(), properties)],
    )
}

fn fbx_null_template_properties() -> Vec<FbxNode> {
    vec![
        fbx_p_node(
            "Color",
            "ColorRGB",
            "Color",
            "",
            vec![FbxProp::D(0.8), FbxProp::D(0.8), FbxProp::D(0.8)],
        ),
        fbx_p_node("Size", "double", "Number", "", vec![FbxProp::D(100.0)]),
        fbx_p_node("Look", "enum", "", "", vec![FbxProp::I(1)]),
    ]
}

fn fbx_mesh_template_properties() -> Vec<FbxNode> {
    vec![
        fbx_p_node(
            "Color",
            "ColorRGB",
            "Color",
            "",
            vec![FbxProp::D(0.8), FbxProp::D(0.8), FbxProp::D(0.8)],
        ),
        fbx_p_node(
            "BBoxMin",
            "Vector3D",
            "Vector",
            "",
            vec![FbxProp::D(0.0), FbxProp::D(0.0), FbxProp::D(0.0)],
        ),
        fbx_p_node(
            "BBoxMax",
            "Vector3D",
            "Vector",
            "",
            vec![FbxProp::D(0.0), FbxProp::D(0.0), FbxProp::D(0.0)],
        ),
        fbx_p_node("Primary Visibility", "bool", "", "", vec![FbxProp::I(1)]),
        fbx_p_node("Casts Shadows", "bool", "", "", vec![FbxProp::I(1)]),
        fbx_p_node("Receive Shadows", "bool", "", "", vec![FbxProp::I(1)]),
    ]
}

fn fbx_node_template_properties() -> Vec<FbxNode> {
    vec![
        fbx_p_node("QuaternionInterpolate", "enum", "", "", vec![FbxProp::I(0)]),
        fbx_p_node(
            "RotationOffset",
            "Vector3D",
            "Vector",
            "",
            vec![FbxProp::D(0.0), FbxProp::D(0.0), FbxProp::D(0.0)],
        ),
        fbx_p_node(
            "RotationPivot",
            "Vector3D",
            "Vector",
            "",
            vec![FbxProp::D(0.0), FbxProp::D(0.0), FbxProp::D(0.0)],
        ),
        fbx_p_node(
            "ScalingOffset",
            "Vector3D",
            "Vector",
            "",
            vec![FbxProp::D(0.0), FbxProp::D(0.0), FbxProp::D(0.0)],
        ),
        fbx_p_node(
            "ScalingPivot",
            "Vector3D",
            "Vector",
            "",
            vec![FbxProp::D(0.0), FbxProp::D(0.0), FbxProp::D(0.0)],
        ),
        fbx_p_node("InheritType", "enum", "", "", vec![FbxProp::I(0)]),
        fbx_p_node(
            "Lcl Translation",
            "Lcl Translation",
            "",
            "A",
            vec![FbxProp::D(0.0), FbxProp::D(0.0), FbxProp::D(0.0)],
        ),
        fbx_p_node(
            "Lcl Rotation",
            "Lcl Rotation",
            "",
            "A",
            vec![FbxProp::D(0.0), FbxProp::D(0.0), FbxProp::D(0.0)],
        ),
        fbx_p_node(
            "Lcl Scaling",
            "Lcl Scaling",
            "",
            "A",
            vec![FbxProp::D(1.0), FbxProp::D(1.0), FbxProp::D(1.0)],
        ),
        fbx_p_node("Visibility", "Visibility", "", "A", vec![FbxProp::D(1.0)]),
        fbx_p_node(
            "Visibility Inheritance",
            "Visibility Inheritance",
            "",
            "",
            vec![FbxProp::I(1)],
        ),
        fbx_p_node(
            "DefaultAttributeIndex",
            "int",
            "Integer",
            "",
            vec![FbxProp::I(-1)],
        ),
    ]
}

fn fbx_phong_template_properties() -> Vec<FbxNode> {
    let color = |name| {
        fbx_p_node(
            name,
            "Color",
            "",
            "A",
            vec![FbxProp::D(0.0), FbxProp::D(0.0), FbxProp::D(0.0)],
        )
    };
    vec![
        fbx_p_node("ShadingModel", "KString", "", "", vec![FbxProp::s("Phong")]),
        fbx_p_node("MultiLayer", "bool", "", "", vec![FbxProp::I(0)]),
        color("EmissiveColor"),
        fbx_p_node("EmissiveFactor", "Number", "", "A", vec![FbxProp::D(1.0)]),
        fbx_p_node(
            "AmbientColor",
            "Color",
            "",
            "A",
            vec![FbxProp::D(0.2), FbxProp::D(0.2), FbxProp::D(0.2)],
        ),
        fbx_p_node("AmbientFactor", "Number", "", "A", vec![FbxProp::D(1.0)]),
        fbx_p_node(
            "DiffuseColor",
            "Color",
            "",
            "A",
            vec![FbxProp::D(0.8), FbxProp::D(0.8), FbxProp::D(0.8)],
        ),
        fbx_p_node("DiffuseFactor", "Number", "", "A", vec![FbxProp::D(1.0)]),
        color("TransparentColor"),
        fbx_p_node(
            "TransparencyFactor",
            "Number",
            "",
            "A",
            vec![FbxProp::D(0.0)],
        ),
        fbx_p_node("Opacity", "Number", "", "A", vec![FbxProp::D(1.0)]),
        fbx_p_node(
            "NormalMap",
            "Vector3D",
            "Vector",
            "",
            vec![FbxProp::D(0.0), FbxProp::D(0.0), FbxProp::D(0.0)],
        ),
        fbx_p_node(
            "Bump",
            "Vector3D",
            "Vector",
            "",
            vec![FbxProp::D(0.0), FbxProp::D(0.0), FbxProp::D(0.0)],
        ),
        fbx_p_node("BumpFactor", "double", "Number", "", vec![FbxProp::D(1.0)]),
        fbx_p_node(
            "SpecularColor",
            "Color",
            "",
            "A",
            vec![FbxProp::D(0.2), FbxProp::D(0.2), FbxProp::D(0.2)],
        ),
        fbx_p_node("SpecularFactor", "Number", "", "A", vec![FbxProp::D(1.0)]),
        fbx_p_node("Shininess", "Number", "", "A", vec![FbxProp::D(20.0)]),
        fbx_p_node(
            "ShininessExponent",
            "Number",
            "",
            "A",
            vec![FbxProp::D(20.0)],
        ),
        color("ReflectionColor"),
        fbx_p_node("ReflectionFactor", "Number", "", "A", vec![FbxProp::D(1.0)]),
    ]
}

fn write_binary_fbx(nodes: &[FbxNode]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(b"Kaydara FBX Binary  \0\x1a\0");
    out.extend_from_slice(&7400_u32.to_le_bytes());
    for node in nodes {
        write_fbx_node(&mut out, node)?;
    }
    write_fbx_null_record(&mut out);
    write_fbx_footer(&mut out);
    Ok(out)
}

fn write_fbx_node(out: &mut Vec<u8>, node: &FbxNode) -> Result<()> {
    let start = out.len();
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.push(
        u8::try_from(node.name.len())
            .with_context(|| format!("FBX node name too long: {}", node.name))?,
    );
    out.extend_from_slice(node.name.as_bytes());
    let props_start = out.len();
    for prop in &node.props {
        write_fbx_prop(out, prop)?;
    }
    let props_end = out.len();
    for child in &node.children {
        write_fbx_node(out, child)?;
    }
    if !node.children.is_empty() {
        write_fbx_null_record(out);
    }
    let end = u32::try_from(out.len()).context("FBX exceeds 32-bit offset range")?;
    let prop_count = u32::try_from(node.props.len())?;
    let prop_len = u32::try_from(props_end - props_start)?;
    out[start..start + 4].copy_from_slice(&end.to_le_bytes());
    out[start + 4..start + 8].copy_from_slice(&prop_count.to_le_bytes());
    out[start + 8..start + 12].copy_from_slice(&prop_len.to_le_bytes());
    Ok(())
}

fn write_fbx_null_record(out: &mut Vec<u8>) {
    out.extend_from_slice(&[0_u8; 13]);
}

fn write_fbx_footer(out: &mut Vec<u8>) {
    // Footer bytes produced by common FBX 7.x exporters. Some readers accept a
    // node table without this, but Autodesk/Valve tooling is stricter.
    const FOOTER_MAGIC_A: [u8; 16] = [
        0xfa, 0xbc, 0xab, 0x09, 0xd0, 0xc8, 0xd4, 0x66, 0xb1, 0x76, 0xfb, 0x83, 0x1c, 0xf7, 0x26,
        0x7e,
    ];
    const FOOTER_MAGIC_B: [u8; 16] = [
        0xf8, 0x5a, 0x8c, 0x6a, 0xde, 0xf5, 0xd9, 0x7e, 0xec, 0xe9, 0x0c, 0xe3, 0x75, 0x8f, 0x29,
        0x0b,
    ];
    out.extend_from_slice(&FOOTER_MAGIC_A);
    let padding_len = out.len().wrapping_neg() & 0x0f;
    out.extend(std::iter::repeat_n(0_u8, padding_len));
    out.extend_from_slice(&[0_u8; 4]);
    out.extend_from_slice(&7400_u32.to_le_bytes());
    out.extend_from_slice(&[0_u8; 120]);
    out.extend_from_slice(&FOOTER_MAGIC_B);
}

fn write_fbx_prop(out: &mut Vec<u8>, prop: &FbxProp) -> Result<()> {
    match prop {
        FbxProp::C(value) => {
            out.push(b'C');
            out.push(u8::from(*value));
        }
        FbxProp::I(value) => {
            out.push(b'I');
            out.extend_from_slice(&value.to_le_bytes());
        }
        FbxProp::D(value) => {
            out.push(b'D');
            out.extend_from_slice(&value.to_le_bytes());
        }
        FbxProp::L(value) => {
            out.push(b'L');
            out.extend_from_slice(&value.to_le_bytes());
        }
        FbxProp::S(value) => {
            out.push(b'S');
            out.extend_from_slice(&u32::try_from(value.len())?.to_le_bytes());
            out.extend_from_slice(value.as_bytes());
        }
        FbxProp::Binary(value) => {
            out.push(b'R');
            out.extend_from_slice(&u32::try_from(value.len())?.to_le_bytes());
            out.extend_from_slice(value);
        }
        FbxProp::DoubleArray(values) => {
            out.push(b'd');
            out.extend_from_slice(&u32::try_from(values.len())?.to_le_bytes());
            out.extend_from_slice(&0_u32.to_le_bytes());
            out.extend_from_slice(&u32::try_from(values.len() * 8)?.to_le_bytes());
            for value in values {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        FbxProp::IntArray(values) => {
            out.push(b'i');
            out.extend_from_slice(&u32::try_from(values.len())?.to_le_bytes());
            out.extend_from_slice(&0_u32.to_le_bytes());
            out.extend_from_slice(&u32::try_from(values.len() * 4)?.to_le_bytes());
            for value in values {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    Ok(())
}

fn compile_with_resourcecompiler(
    source_root: &Path,
    options: &SoulContainerImportOptions,
    backend: &ResourceCompilerBackend,
    output_vpk: &Path,
) -> Result<SoulContainerCompileReport> {
    validate_addon_name(&backend.addon)?;
    require_dir(source_root, "source root")?;
    require_file(&backend.proton, "Proton executable")?;

    let content_addon = backend
        .csdk_root
        .join("content")
        .join("citadel_addons")
        .join(&backend.addon);
    let game_addon = backend
        .csdk_root
        .join("game")
        .join("citadel_addons")
        .join(&backend.addon);
    let compiler_dir = backend.csdk_root.join("game/bin_tools/win64");
    require_dir(&compiler_dir, "resourcecompiler directory")?;

    remove_staging(&content_addon, backend.force)?;
    remove_staging(&game_addon, backend.force)?;
    copy_tree(source_root, &content_addon)?;

    let source_vmdl = content_addon
        .join(&options.model_rel)
        .join("soul_container.vmdl");
    require_file(&source_vmdl, "generated source model")?;

    let filelist = tempfile::NamedTempFile::new().context("creating resourcecompiler filelist")?;
    std::fs::write(filelist.path(), format!("{}\n", wine_z_path(&source_vmdl)))
        .context("writing resourcecompiler filelist")?;

    let mut cmd = Command::new(&backend.proton);
    cmd.arg("run")
        .arg("resourcecompiler.exe")
        .arg("-game")
        .arg("citadel")
        .arg("-addon")
        .arg(&backend.addon)
        .arg("-fshallow")
        .arg("-nop4")
        .arg("-v")
        .arg("-consoleapp")
        .arg("-consolelog")
        .arg("-condebug")
        .arg("-toconsole")
        .arg("-danger_mode_ignore_schema_mismatches");
    for arg in &backend.extra_args {
        cmd.arg(arg);
    }
    cmd.arg("-filelist")
        .arg(wine_z_path(filelist.path()))
        .current_dir(&compiler_dir)
        .env("STEAM_COMPAT_DATA_PATH", &backend.proton_prefix)
        .env("STEAM_COMPAT_CLIENT_INSTALL_PATH", &backend.steam_root)
        .env("SteamAppId", "1422450")
        .env("SteamGameId", "1422450")
        .env("VPROJECT", "1");

    let status = cmd
        .status()
        .context("running resourcecompiler through Proton")?;
    if !status.success() {
        bail!("resourcecompiler failed with status {status}");
    }

    let entries = pack_directory(&game_addon, output_vpk)?;
    if !backend.keep_staging {
        std::fs::remove_dir_all(&content_addon).ok();
        std::fs::remove_dir_all(&game_addon).ok();
    }
    Ok(SoulContainerCompileReport {
        output_vpk: output_vpk.to_path_buf(),
        addon: backend.addon.clone(),
        compiled_root: game_addon,
        packed_entries: entries,
    })
}

fn pack_directory(root: &Path, output_vpk: &Path) -> Result<usize> {
    require_dir(root, "compiled game addon")?;
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    if files.is_empty() {
        bail!("no files under {}", root.display());
    }
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    crate::pack(&refs, output_vpk)?;
    Ok(refs.len())
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            collect_files(root, &path, out)?;
        } else if ty.is_file() {
            let rel = path
                .strip_prefix(root)
                .with_context(|| format!("stripping root from {}", path.display()))?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            out.push((rel, bytes));
        }
    }
    Ok(())
}

fn compile_with_pure_rust(
    source_root: &Path,
    options: &SoulContainerImportOptions,
    output_vpk: &Path,
) -> Result<SoulContainerCompileReport> {
    let compiled_root = default_pure_compiled_root(output_vpk);
    compile_soul_container_source_pure_rust(source_root, options, &compiled_root, output_vpk, false)
}

fn compile_pure_rust_materials(
    materials: &[PureSourceMaterial],
    model_rel: &str,
    compiled_root: &Path,
    output_vpk: &Path,
    force: bool,
) -> Result<SoulContainerCompileReport> {
    validate_source_rel(model_rel)?;
    if materials.is_empty() {
        bail!("no prepared soul-container materials to compile");
    }
    prepare_pure_compiled_root(compiled_root, force)?;

    let mut files = Vec::with_capacity(materials.len() * 2);
    let material_prefix = format!("{model_rel}/materials/");
    for material in materials {
        validate_source_rel(&material.source_material)?;
        if !material.source_material.starts_with(&material_prefix) {
            bail!(
                "prepared material path {:?} is outside {material_prefix:?}",
                material.source_material
            );
        }
        require_file(&material.vmat_path, "prepared source VMAT")?;
        require_file(&material.color_texture_path, "prepared source color PNG")?;

        let png = std::fs::read(&material.color_texture_path)
            .with_context(|| format!("reading {}", material.color_texture_path.display()))?;
        let vtex = morphic::encode_vtex_png_rgba8888_from_png(&png, morphic::TextureFlags::empty())
            .with_context(|| format!("encoding {}", material.color_texture_path.display()))?;
        let texture_info = morphic::inspect(&vtex).with_context(|| {
            format!("inspecting generated VTEX for {}", material.source_material)
        })?;

        let color_texture = format!("{}_color.vtex", material.source_material);
        let vmat = morphic::encode_pbr_vmat_c(&morphic::PbrVmatParams {
            material_name: format!("{}.vmat", material.source_material),
            color_texture: color_texture.clone(),
            representative_width: texture_info.width,
            representative_height: texture_info.height,
        })
        .with_context(|| format!("encoding VMAT for {}", material.source_material))?;
        let parsed = morphic::material::parse(&vmat)
            .with_context(|| format!("parsing generated VMAT for {}", material.source_material))?;
        if parsed.texture_params.get("g_tColor").map(String::as_str) != Some(&color_texture) {
            bail!(
                "generated VMAT g_tColor mismatch for {}",
                material.source_material
            );
        }

        let vmat_entry = format!("{}.vmat_c", material.source_material);
        let vtex_entry = format!("{}_color.vtex_c", material.source_material);
        write_compiled_entry(compiled_root, &vmat_entry, &vmat)?;
        write_compiled_entry(compiled_root, &vtex_entry, &vtex)?;
        files.push((vmat_entry, vmat));
        files.push((vtex_entry, vtex));
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(entry, bytes)| (entry.as_str(), bytes.as_slice()))
        .collect();
    crate::pack(&refs, output_vpk)?;

    Ok(SoulContainerCompileReport {
        output_vpk: output_vpk.to_path_buf(),
        addon: "pure_rust_partial".to_string(),
        compiled_root: compiled_root.to_path_buf(),
        packed_entries: refs.len(),
    })
}

fn pure_materials_from_prepared(prepared: &SoulContainerPreparedSource) -> Vec<PureSourceMaterial> {
    let mut materials = Vec::with_capacity(prepared.materials.len());
    for material in &prepared.materials {
        materials.push(PureSourceMaterial {
            source_material: material.source_material.clone(),
            vmat_path: material.vmat_path.clone(),
            color_texture_path: material.color_texture_path.clone(),
        });
    }
    materials.sort_by(|a, b| a.source_material.cmp(&b.source_material));
    materials
}

fn scan_pure_source_materials(
    source_root: &Path,
    options: &SoulContainerImportOptions,
) -> Result<Vec<PureSourceMaterial>> {
    let materials_dir = source_root.join(&options.model_rel).join("materials");
    require_dir(&materials_dir, "prepared materials directory")?;
    let mut materials = Vec::new();
    for entry in std::fs::read_dir(&materials_dir)
        .with_context(|| format!("reading {}", materials_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() || path.extension() != Some(std::ffi::OsStr::new("vmat")) {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .with_context(|| format!("material file name is not UTF-8: {}", path.display()))?;
        let source_material = format!("{}/materials/{stem}", options.model_rel);
        let color_texture_path = materials_dir.join(format!("{stem}_color.png"));
        materials.push(PureSourceMaterial {
            source_material,
            vmat_path: path,
            color_texture_path,
        });
    }
    materials.sort_by(|a, b| a.source_material.cmp(&b.source_material));
    if materials.is_empty() {
        bail!("no .vmat files under {}", materials_dir.display());
    }
    Ok(materials)
}

fn prepare_pure_compiled_root(compiled_root: &Path, force: bool) -> Result<()> {
    if compiled_root.as_os_str().is_empty() || compiled_root == Path::new("/") {
        bail!(
            "refusing to use unsafe compiled root {}",
            compiled_root.display()
        );
    }
    if compiled_root.exists() {
        if !force {
            bail!(
                "pure compiled root exists; pass force=true to replace it: {}",
                compiled_root.display()
            );
        }
        if !compiled_root.is_dir() {
            bail!(
                "pure compiled root exists and is not a directory: {}",
                compiled_root.display()
            );
        }
        std::fs::remove_dir_all(compiled_root)
            .with_context(|| format!("removing {}", compiled_root.display()))?;
    }
    std::fs::create_dir_all(compiled_root)
        .with_context(|| format!("creating {}", compiled_root.display()))
}

fn write_compiled_entry(root: &Path, entry: &str, bytes: &[u8]) -> Result<()> {
    validate_source_rel(entry)?;
    let path = root.join(entry);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn default_pure_compiled_root(output_vpk: &Path) -> PathBuf {
    let parent = output_vpk.parent().unwrap_or_else(|| Path::new("."));
    let stem = output_vpk
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("soul_container_pure");
    let stem = stem.strip_suffix("_dir").unwrap_or(stem);
    parent.join(format!("{stem}_compiled_game"))
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    require_dir(src, "source root")?;
    std::fs::create_dir_all(dst).with_context(|| format!("creating {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_tree(&src_path, &dst_path)?;
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

fn remove_staging(path: &Path, force: bool) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if !force {
        bail!(
            "staging path exists; set ResourceCompilerBackend::force to remove it: {}",
            path.display()
        );
    }
    std::fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))
}

fn validate_options(options: &SoulContainerImportOptions) -> Result<()> {
    validate_source_rel(&options.model_rel)?;
    if options.target_largest_axis <= 0.0 {
        bail!("target_largest_axis must be positive");
    }
    if options.source_units_per_blender <= 0.0 {
        bail!("source_units_per_blender must be positive");
    }
    if options.physics_radius <= 0.0 {
        bail!("physics_radius must be positive");
    }
    Ok(())
}

fn validate_source_rel(path: &str) -> Result<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path
            .split(['/', '\\'])
            .any(|s| s.is_empty() || s == "." || s == ".." || s.contains(':'))
    {
        bail!("model_rel must be a clean Source-relative path, got {path:?}");
    }
    Ok(())
}

fn validate_addon_name(addon: &str) -> Result<()> {
    if addon.is_empty()
        || addon == "."
        || addon == ".."
        || !addon
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        bail!("addon name must be file-name safe, got {addon:?}");
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

fn wine_z_path(path: &Path) -> String {
    format!("Z:{}", path.to_string_lossy().replace('/', "\\"))
}

fn unique_safe_name(raw: &str, fallback: &str, used: &mut HashSet<String>) -> String {
    let base = safe_name(raw, fallback);
    let mut name = base.clone();
    let mut suffix = 1_u32;
    while used.contains(&name) {
        suffix += 1;
        name = format!("{base}_{suffix}");
    }
    used.insert(name.clone());
    name
}

fn safe_name(raw: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut prev_underscore = false;
    for c in raw.trim().chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

fn linear_to_srgb_u8(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

fn f32_to_unorm8(c: f32) -> u8 {
    (c.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn midpoint(a: f32, b: f32) -> f32 {
    (a + b) * 0.5
}

fn gltf_to_source_point(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[2], -p[1]]
}

fn gltf_to_source_vector(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[2], -v[1]]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len <= f32::EPSILON {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    normalize([
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ])
}

fn node_world_transforms(doc: &gltf::Document) -> Vec<[[f32; 4]; 4]> {
    let mut out = vec![IDENTITY4; doc.nodes().count()];
    for scene in doc.scenes() {
        for node in scene.nodes() {
            accumulate_node(&node, IDENTITY4, &mut out);
        }
    }
    out
}

fn accumulate_node(node: &gltf::Node<'_>, parent: [[f32; 4]; 4], out: &mut [[[f32; 4]; 4]]) {
    let world = mat_mul(parent, node.transform().matrix());
    out[node.index()] = world;
    for child in node.children() {
        accumulate_node(&child, world, out);
    }
}

const IDENTITY4: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

#[allow(clippy::many_single_char_names)]
fn mat_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut r = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k][row] * b[col][k];
            }
            r[col][row] = s;
        }
    }
    r
}

#[allow(clippy::many_single_char_names)]
fn transform_point(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    let v = [p[0], p[1], p[2], 1.0];
    let mut o = [0.0f32; 3];
    for (row, oo) in o.iter_mut().enumerate() {
        let mut s = 0.0;
        for (col, &vc) in v.iter().enumerate() {
            s += m[col][row] * vc;
        }
        *oo = s;
    }
    o
}

#[allow(clippy::many_single_char_names)]
fn transform_vector(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    let v = [p[0], p[1], p[2], 0.0];
    let mut o = [0.0f32; 3];
    for (row, oo) in o.iter_mut().enumerate() {
        let mut s = 0.0;
        for (col, &vc) in v.iter().enumerate() {
            s += m[col][row] * vc;
        }
        *oo = s;
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn safe_name_matches_material_path_needs() {
        assert_eq!(safe_name("Red Mat.001", "fallback"), "red_mat_001");
        assert_eq!(safe_name("  !!!  ", "fallback"), "fallback");
    }

    #[test]
    fn prepares_minimal_glb_source_tree() -> Result<()> {
        let tmp = tempdir()?;
        let glb_path = tmp.path().join("tri.glb");
        std::fs::write(&glb_path, minimal_triangle_glb()?)?;
        let out = tmp.path().join("source");

        let report =
            prepare_soul_container_import(&glb_path, &out, &SoulContainerImportOptions::default())?;

        assert_eq!(report.vertex_count, 3);
        assert_eq!(report.triangle_count, 1);
        assert_eq!(report.materials.len(), 1);
        assert_eq!(
            report.materials[0].source_material,
            "models/props_gameplay/soul_container/materials/red_mat"
        );
        assert!((report.expected_source_bounds.largest_axis() - 12.65).abs() < 0.001);

        let fbx = std::fs::read(&report.fbx_path)?;
        assert!(fbx.starts_with(b"Kaydara FBX Binary  \0\x1a\0"));
        assert!(fbx
            .windows(b"models/props_gameplay/soul_container/materials/red_mat\0\x01Material".len())
            .any(|w| w == b"models/props_gameplay/soul_container/materials/red_mat\0\x01Material"));
        let vmdl = std::fs::read_to_string(&report.vmdl_path)?;
        assert!(vmdl.contains("models/props_gameplay/soul_container/model.fbx"));
        assert!(report.materials[0].vmat_path.is_file());
        assert!(report.materials[0].color_texture_path.is_file());
        Ok(())
    }

    #[test]
    fn pure_rust_prepared_compile_writes_material_texture_pack() -> Result<()> {
        let tmp = tempdir()?;
        let glb_path = tmp.path().join("tri.glb");
        std::fs::write(&glb_path, minimal_triangle_glb()?)?;
        let source_root = tmp.path().join("source");
        let prepared = prepare_soul_container_import(
            &glb_path,
            &source_root,
            &SoulContainerImportOptions::default(),
        )?;
        let compiled_root = tmp.path().join("compiled_game");
        let output_vpk = tmp.path().join("pure_dir.vpk");

        let report = compile_soul_container_prepared_pure_rust(
            &prepared,
            &compiled_root,
            &output_vpk,
            true,
        )?;

        assert_eq!(report.packed_entries, 2);
        assert_eq!(report.compiled_root, compiled_root);
        let vmat_entry = "models/props_gameplay/soul_container/materials/red_mat.vmat_c";
        let vtex_entry = "models/props_gameplay/soul_container/materials/red_mat_color.vtex_c";
        assert!(report.compiled_root.join(vmat_entry).is_file());
        assert!(report.compiled_root.join(vtex_entry).is_file());

        let vpk = valve_pak::open(&output_vpk)?;
        let vmat = vpk.get_file(vmat_entry)?.read_all()?;
        let mat = morphic::material::parse(&vmat)?;
        assert_eq!(
            mat.name,
            "models/props_gameplay/soul_container/materials/red_mat.vmat"
        );
        assert_eq!(
            mat.texture_params.get("g_tColor").map(String::as_str),
            Some("models/props_gameplay/soul_container/materials/red_mat_color.vtex")
        );

        let vtex = vpk.get_file(vtex_entry)?.read_all()?;
        let info = morphic::inspect(&vtex)?;
        assert_eq!(info.format, morphic::TextureFormat::PngRgba8888);
        assert_eq!((info.width, info.height, info.mip_count), (2, 2, 1));
        Ok(())
    }

    fn minimal_triangle_glb() -> Result<Vec<u8>> {
        let mut bin = Vec::new();
        for value in [
            0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0,
            0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0,
        ] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0_u16, 1, 2] {
            bin.extend_from_slice(&value.to_le_bytes());
        }
        while bin.len() % 4 != 0 {
            bin.push(0);
        }

        let doc = json!({
            "asset": { "version": "2.0" },
            "scene": 0,
            "scenes": [{ "nodes": [0] }],
            "nodes": [{ "mesh": 0 }],
            "meshes": [{
                "primitives": [{
                    "attributes": { "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 },
                    "indices": 3,
                    "material": 0
                }]
            }],
            "materials": [{
                "name": "Red Mat",
                "pbrMetallicRoughness": {
                    "baseColorFactor": [1.0, 0.0, 0.0, 1.0]
                }
            }],
            "buffers": [{ "byteLength": bin.len() }],
            "bufferViews": [
                { "buffer": 0, "byteOffset": 0, "byteLength": 36, "target": 34962 },
                { "buffer": 0, "byteOffset": 36, "byteLength": 36, "target": 34962 },
                { "buffer": 0, "byteOffset": 72, "byteLength": 24, "target": 34962 },
                { "buffer": 0, "byteOffset": 96, "byteLength": 6, "target": 34963 }
            ],
            "accessors": [
                { "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0] },
                { "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" },
                { "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" },
                { "bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR" }
            ]
        });
        let mut json_bytes = serde_json::to_vec(&doc)?;
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }

        let total_len = 12 + 8 + json_bytes.len() + 8 + bin.len();
        let mut glb = Vec::with_capacity(total_len);
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2_u32.to_le_bytes());
        glb.extend_from_slice(&(u32::try_from(total_len)?).to_le_bytes());
        glb.extend_from_slice(&(u32::try_from(json_bytes.len())?).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json_bytes);
        glb.extend_from_slice(&(u32::try_from(bin.len())?).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&bin);
        Ok(glb)
    }
}
