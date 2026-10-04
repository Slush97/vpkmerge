//! Bunnydicta outfit variants: the bunnysuit Vindicta mod with its costume
//! stripped and one Blender-authored outfit skinned onto the same skeleton.
//! One self-contained addon VPK per outfit (the original mod merged underneath).
//!
//! Model edit, all on proven-in-game primitives:
//! - `ears` (both draw calls) and the costume bows (`clothes` draw call 1, on
//!   `vindicta_hair`) are neutralized (index count 0; the T1a path that removed
//!   Vindicta's dress in game).
//! - `clothes` draw call 0 (`clothingmaterial`) gets the outfit geometry, written
//!   uncompressed (`m_bMeshoptCompressed` off, never morphic's meshopt codec).
//!   Garment colours come from ONE 2048 atlas overriding the material's own
//!   albedo (BC7 donor splice, not inline PNG): each garment material gets a
//!   full-height column (flat colour, or its Blender UV-Y colour ramp), the
//!   tartan skirt gets its own tiled band.
//! - The mod's `outline` hull (inverted-hull NPR outline) wraps the bunnysuit,
//!   heels included; hull vertices that hug the costume are snapped onto the bare
//!   skin at the hull's usual offset (and take that skin vertex's skinning), so no
//!   dark heel/suit silhouette is left behind.
//!
//! Plus two texture fixes in every variant: fishnet-free skin
//! (`skinmaterialfishnet`) and the head albedo with the neck blended to the body
//! blue (stock `vindicta_headv2_color`).
//!
//! GLB -> model space was measured, not assumed (body_calibration.glb vs the
//! mod's own skin/head buffers, max error 2e-5): native = (z, x, y) / 0.0254,
//! UVs identical, joints matched by bone name.
//!
//! Usage:
//!   bunnydicta_outfits build <mod_dir.vpk> <pak01_dir.vpk> <export_dir> \
//!       <skin_base.png> <head_color_fixed.png> <out_dir> [outfit_no...]
//!   bunnydicta_outfits probe <vpk>
//!   bunnydicta_outfits ctrl <vpk> [entry-substring...]   (buffer registry: counts,
//!       strides, index width, meshopt flag per embedded mesh)

use anyhow::{bail, Context, Result};
use image::{Rgb, RgbImage};
use morphic::model::VertexBuffer;
use std::collections::HashMap;

const ENTRY: &str = "models/heroes_staging/hornet_v3/hornet.vmdl_c";
const SKIN_TEX: &str =
    "models/heroes_staging/hornet_v3/materials/skinmaterialfishnet_png_8f8f2eb3.vtex_c";
const HEAD_TEX: &str =
    "models/heroes_staging/hornet_v3/materials/vindicta_headv2_color_png_2b9a36b1.vtex_c";
const CLOTH_TEX: &str =
    "models/heroes_staging/hornet_v3/materials/clothingmaterials_png_2b69a76a.vtex_c";
const INCH: f32 = 1.0 / 0.0254;
const ATLAS: u32 = 2048;

const OUTFITS: [(u32, &str); 9] = [
    (1, "bandeau_pencil"),
    (2, "tennis"),
    (3, "goth"),
    (4, "y2k"),
    (5, "street"),
    (6, "micro"),
    (7, "triangle"),
    (8, "ruched"),
    (9, "harness"),
];

type Lin = [f32; 3];
const MINT: Lin = [0.30, 0.72, 0.58];
const WHITE: Lin = [0.86, 0.87, 0.9];
const STITCH: Lin = [0.62, 0.8, 0.72];
const GOTH_BLACK: Lin = [0.018, 0.016, 0.02];
const GOTH_RED: Lin = [0.2, 0.004, 0.018];

const Y2K_HOT: Lin = [0.95, 0.025, 0.32];
const Y2K_HOT_D: Lin = [0.34, 0.004, 0.11];
const Y2K_PIPE: Lin = [1.0, 0.62, 0.86];
const Y2K_LIL: Lin = [0.42, 0.16, 0.86];
const Y2K_LIL_D: Lin = [0.17, 0.05, 0.42];
const Y2K_WHT: Lin = [0.9, 0.88, 0.94];
const Y2K_WHT_D: Lin = [0.5, 0.42, 0.6];
const Y2K_CH_HI: Lin = [1.0, 0.92, 0.97];
const Y2K_CH_M: Lin = [1.0, 0.33, 0.66];
const Y2K_CH_D: Lin = [0.2, 0.004, 0.08];

/// A Blender LINEAR colour ramp as dense constant steps (one per atlas texel
/// row is plenty), so the column painter below stays a plain step lookup.
fn linear(stops: &[(f32, Lin)]) -> Vec<(f32, Lin)> {
    const STEPS: u16 = 2048;
    (0..STEPS)
        .map(|i| {
            let y = f32::from(i) / f32::from(STEPS);
            let k = stops.iter().rposition(|s| s.0 <= y).unwrap_or(0);
            let (p0, c0) = stops[k];
            let c = stops.get(k + 1).map_or(c0, |&(p1, c1)| {
                let t = if p1 > p0 { (y - p0) / (p1 - p0) } else { 0.0 };
                std::array::from_fn(|j| c0[j] + (c1[j] - c0[j]) * t)
            });
            (y, c)
        })
        .collect()
}

/// Blender colour ramps keyed on UV Y (= 1 - glTF v), transcribed from the
/// outfit material scripts (the GLB exporter drops them). CONSTANT
/// interpolation unless wrapped in `linear` (the Y2K ramps, which paint the
/// shine the flat-albedo atlas can't otherwise carry).
fn ramp_for(material: &str) -> Option<Vec<(f32, Lin)>> {
    Some(match material {
        "o02_bottom_mat" => vec![
            (0.0, MINT),
            (0.03, WHITE),
            (0.048, MINT),
            (0.062, WHITE),
            (0.074, MINT),
            (0.889, WHITE),
        ],
        "o02_visor_brim_mat" => vec![
            (0.0, WHITE),
            (0.70, STITCH),
            (0.712, WHITE),
            (0.79, STITCH),
            (0.802, WHITE),
            (0.88, STITCH),
            (0.892, WHITE),
        ],
        "o03_top_mat" => vec![(0.0, GOTH_BLACK), (0.965, GOTH_RED)],
        "o03_bottom_mat" => vec![(0.0, GOTH_RED), (0.045, GOTH_BLACK)],
        "o04_top_mat" => linear(&[
            (0.0, Y2K_PIPE),
            (0.075, Y2K_PIPE),
            (0.085, Y2K_HOT_D),
            (0.13, Y2K_HOT),
            (0.35, Y2K_HOT),
            (1.0, [1.0, 0.14, 0.5]),
        ]),
        "o04_skirt_mat" => linear(&[
            (0.0, Y2K_HOT),
            (0.018, Y2K_HOT),
            (0.024, Y2K_LIL_D),
            (0.06, Y2K_LIL),
            (0.5, Y2K_LIL),
            (0.64, [0.53, 0.27, 0.93]),
            (0.78, Y2K_LIL),
            (0.97, Y2K_LIL),
            (0.985, Y2K_HOT),
            (1.0, Y2K_HOT),
        ]),
        "o04_belt_mat" => linear(&[
            (0.0, Y2K_WHT_D),
            (0.1, Y2K_WHT_D),
            (0.13, Y2K_HOT),
            (0.2, Y2K_HOT),
            (0.23, Y2K_WHT),
            (0.55, [1.0, 1.0, 1.0]),
            (0.77, Y2K_WHT),
            (0.8, Y2K_HOT),
            (0.87, Y2K_HOT),
            (0.9, Y2K_WHT_D),
            (1.0, Y2K_WHT_D),
        ]),
        "o04_chrome_mat" => linear(&[
            (0.0, Y2K_CH_M),
            (0.12, Y2K_CH_HI),
            (0.2, Y2K_CH_HI),
            (0.3, Y2K_CH_M),
            (0.42, Y2K_CH_D),
            (0.55, Y2K_CH_M),
            (0.75, Y2K_CH_D),
            (1.0, Y2K_CH_M),
        ]),
        "o07_skirt" => vec![(0.0, [0.22, 0.008, 0.012]), (0.872, [0.012, 0.012, 0.014])],
        _ => return None,
    })
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("probe") if a.len() == 2 => probe(&a[1]),
        Some("ctrl") if a.len() >= 2 => ctrl(&a[1], &a[2..]),
        Some("build") if a.len() >= 7 => {
            let only: Vec<u32> = a[7..].iter().map(|s| s.parse()).collect::<Result<_, _>>()?;
            build(&a[1], &a[2], &a[3], &a[4], &a[5], &a[6], &only)
        }
        _ => bail!(
            "usage: bunnydicta_outfits build <mod_dir.vpk> <pak01_dir.vpk> <export_dir> \
             <skin_base.png> <head_color_fixed.png> <out_dir> [outfit_no...]\n       \
             bunnydicta_outfits probe <vpk>\n       \
             bunnydicta_outfits ctrl <vpk> [entry-substring...]"
        ),
    }
}

fn build(
    mod_vpk: &str,
    pak: &str,
    export_dir: &str,
    skin_png: &str,
    head_png: &str,
    out_dir: &str,
    only: &[u32],
) -> Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let model = vpkmerge_core::read_vpk_entry(mod_vpk, ENTRY).context("read model")?;
    let skel = morphic::model::decode_skeleton(&model)?;
    let bone_ix: HashMap<String, u16> = skel
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.to_ascii_lowercase(), u16::try_from(i).unwrap()))
        .collect();

    // Shared by every variant: costume draw calls off, skin + head texture
    // fixes. The outline hull is re-seated per variant (a variant may keep part
    // of the costume, which keeps its outline).
    let mut base = morphic::model::remove_part_draw_calls(&model, "ears", &[0, 1])?;
    base = morphic::model::remove_part_draw_calls(&base, "clothes", &[1])?;
    if let Ok(delta) = std::env::var("BUNNYDICTA_ANAT_DELTA") {
        base = patch_body_skin(&base, &delta)?;
    }
    let skin = overlay_rgb(mod_vpk, SKIN_TEX, &image::open(skin_png)?.to_rgb8())?;
    let head = overlay_rgb(pak, HEAD_TEX, &image::open(head_png)?.to_rgb8())?;

    for (no, style) in OUTFITS {
        if !only.is_empty() && !only.contains(&no) {
            continue;
        }
        let glb = format!("{export_dir}/outfit_{no:02}_{style}.glb");
        let prefix = format!("o{no:02}_");
        let pieces = read_pieces(&glb, &prefix, &bone_ix)?;
        // weight transfer from the whole body grabs the braid behind the back,
        // and the garment then tears open when the braid swings
        for p in pieces.iter().filter(|_| no >= 6) {
            for (j, w) in p.vb.joints.iter().zip(&p.vb.weights) {
                if let Some(k) = (0..4).find(|&k| {
                    w[k] > 1e-4
                        && skel.bones[usize::from(j[k])]
                            .name
                            .to_ascii_lowercase()
                            .contains("hair")
                }) {
                    bail!(
                        "{}: weighted to hair bone {}",
                        p.material,
                        skel.bones[usize::from(j[k])].name
                    );
                }
            }
        }
        let kept: Vec<[f32; 3]> = pieces
            .iter()
            .filter(|p| p.material.contains(KEPT_COSTUME))
            .flat_map(|p| p.vb.positions.iter().copied())
            .collect();
        let (vb, indices, atlas) = assemble(&pieces)?;
        atlas.save(format!("{out_dir}/bunnydicta_o{no:02}_{style}_atlas.png"))?;

        let seated = reseat_outline(&base, &kept)?;
        let (edited, rep) =
            morphic::model::replace_draw_call_uncompressed(&seated, "clothes", 0, &vb, &indices)?;
        println!(
            "o{no:02} {style}: {} pieces, clothes dc0 {} -> {} verts, {} idx ({}-byte), model {} bytes",
            pieces.len(),
            rep.old_vertex_count,
            rep.new_vertex_count,
            rep.new_index_count,
            rep.index_size,
            edited.len()
        );
        let cloth = overlay_rgb(mod_vpk, CLOTH_TEX, &atlas)?;

        let overlay = format!("{out_dir}/.o{no:02}_overlay_dir.vpk");
        vpkmerge_core::pack(
            &[
                (ENTRY, edited.as_slice()),
                (SKIN_TEX, skin.as_slice()),
                (HEAD_TEX, head.as_slice()),
                (CLOTH_TEX, cloth.as_slice()),
            ],
            &overlay,
        )?;
        let out = format!("{out_dir}/bunnydicta_o{no:02}_{style}_dir.vpk");
        let _ = std::fs::remove_file(&out);
        vpkmerge_core::merge(
            &[mod_vpk, overlay.as_str()],
            &out,
            &vpkmerge_core::MergeOptions::default(),
        )?;
        std::fs::remove_file(&overlay)?;
        println!("  wrote {out}");
    }
    Ok(())
}

/// Applies the Blender anatomy shape keys (chest + groin) to the skin draw call.
/// `delta` rows are 10 LE f32: stock native position, reshaped native position,
/// reshaped normal, Blender material index (2 = skin). Buffer vertices match
/// rows by stock position; the draw call owns its buffers, so it is rewritten
/// uncompressed like the garments.
fn patch_body_skin(bytes: &[u8], delta: &str) -> Result<Vec<u8>> {
    let raw = std::fs::read(delta).with_context(|| format!("read {delta}"))?;
    let rows: Vec<[f32; 10]> = raw
        .chunks_exact(40)
        .map(|c| {
            std::array::from_fn(|k| f32::from_le_bytes(c[k * 4..k * 4 + 4].try_into().unwrap()))
        })
        .filter(|r: &[f32; 10]| r[9] == 2.0)
        .collect();
    let cell = |p: [f32; 3]| p.map(|x| (x / 0.05).floor() as i32);
    let mut grid: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        grid.entry(cell([r[0], r[1], r[2]])).or_default().push(i);
    }

    let model = morphic::model::decode(bytes)?;
    let body = model
        .meshes
        .iter()
        .find(|m| m.name == "body")
        .context("no body")?;
    let (prim_ix, prim) = body
        .primitives
        .iter()
        .enumerate()
        .find(|(_, p)| p.material.contains("skinmaterial"))
        .context("no skin draw call")?;
    let mut vb = body.vertex_buffers[prim.vertex_buffer].clone();
    let (mut worst, mut moved) = (0f32, 0);
    for i in 0..vb.element_count {
        let p = vb.positions[i];
        let c = cell(p);
        let mut best = (f32::MAX, 0);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &j in grid
                        .get(&[c[0] + dx, c[1] + dy, c[2] + dz])
                        .into_iter()
                        .flatten()
                    {
                        let r = &rows[j];
                        let d = (0..3).map(|k| (r[k] - p[k]).powi(2)).sum::<f32>();
                        if d < best.0 {
                            best = (d, j);
                        }
                    }
                }
            }
        }
        if best.0 == f32::MAX {
            bail!("skin vertex {i} at {p:?} has no Blender match");
        }
        worst = worst.max(best.0.sqrt());
        let r = &rows[best.1];
        if (0..3).any(|k| (r[3 + k] - r[k]).abs() > 1e-4) {
            vb.positions[i] = [r[3], r[4], r[5]];
            vb.normals[i] = [r[6], r[7], r[8]];
            moved += 1;
        }
    }
    if worst > 0.01 {
        bail!("skin match error {worst} in: wrong axis mapping?");
    }
    vb.tangents.clear();
    println!(
        "body skin: {moved}/{} verts reshaped (match error {worst:.1e} in)",
        vb.element_count
    );
    let (out, _) =
        morphic::model::replace_draw_call_uncompressed(bytes, "body", prim_ix, &vb, &prim.indices)?;
    Ok(out)
}

/// Replaces a texture's RGB with `img` (alpha plane kept: Deadlock albedo alpha
/// is a mask), re-encoding the full mip chain in the texture's own BCn format.
fn overlay_rgb(vpk: &str, entry: &str, img: &RgbImage) -> Result<Vec<u8>> {
    let bytes =
        vpkmerge_core::read_vpk_entry(vpk, entry).with_context(|| format!("read {entry}"))?;
    let mut tex = morphic::decode(&bytes)?;
    if (tex.width, tex.height) != img.dimensions() {
        bail!(
            "{entry} is {}x{}, overlay {:?}",
            tex.width,
            tex.height,
            img.dimensions()
        );
    }
    let morphic::ImageData::Rgba8(ref mut px) = tex.data else {
        bail!("{entry}: expected an LDR texture");
    };
    for (dst, src) in px.chunks_exact_mut(4).zip(img.pixels()) {
        dst[..3].copy_from_slice(&src.0);
    }
    Ok(morphic::replace_mip_chain(&bytes, &tex)?)
}

enum Paint {
    Ramp(Vec<(f32, Lin)>),
    Tiled(RgbImage),
}

struct Piece {
    material: String,
    paint: Paint,
    vb: VertexBuffer,
    indices: Vec<u32>,
}

fn read_pieces(glb: &str, prefix: &str, bone_ix: &HashMap<String, u16>) -> Result<Vec<Piece>> {
    let bytes = std::fs::read(glb).with_context(|| format!("read {glb}"))?;
    let (doc, buffers) = open_glb(&bytes).with_context(|| format!("parse {glb}"))?;
    let mut out = Vec::new();
    for node in doc.nodes() {
        let (Some(mesh), Some(name)) = (node.mesh(), node.name()) else {
            continue;
        };
        if !name.starts_with(prefix) {
            continue;
        }
        let skin = node
            .skin()
            .with_context(|| format!("{name} is not skinned"))?;
        let joint_map: Vec<u16> = skin
            .joints()
            .map(|j| {
                let n = j.name().unwrap_or("").to_ascii_lowercase();
                bone_ix
                    .get(&n)
                    .copied()
                    .with_context(|| format!("{name}: joint {n:?} not in skeleton"))
            })
            .collect::<Result<_>>()?;
        for prim in mesh.primitives() {
            let r = prim.reader(|b| buffers.get(b.index()).map(|d| d.0.as_slice()));
            let positions: Vec<[f32; 3]> = r
                .read_positions()
                .context("no POSITION")?
                .map(|p| [p[2] * INCH, p[0] * INCH, p[1] * INCH])
                .collect();
            let normals: Vec<[f32; 3]> = r
                .read_normals()
                .context("no NORMAL")?
                .map(|n| [n[2], n[0], n[1]])
                .collect();
            let n = positions.len();
            let uv: Vec<[f32; 2]> = r
                .read_tex_coords(0)
                .map_or_else(|| vec![[0.5, 0.5]; n], |t| t.into_f32().collect());
            let weights: Vec<[f32; 4]> = r
                .read_weights(0)
                .context("no WEIGHTS_0")?
                .into_f32()
                .collect();
            let joints: Vec<[u16; 4]> = r
                .read_joints(0)
                .context("no JOINTS_0")?
                .into_u16()
                .zip(&weights)
                .map(|(j, w)| {
                    // Unweighted slots point at the vertex's main bone so the
                    // on-disk palette index is always a real, in-use entry.
                    let main = joint_map[usize::from(j[0])];
                    std::array::from_fn(|k| {
                        if w[k] > 0.0 {
                            joint_map[usize::from(j[k])]
                        } else {
                            main
                        }
                    })
                })
                .collect();
            if name == "o02_skirt" {
                let allowed: Vec<u16> = [
                    "pelvis",
                    "spine_0",
                    "spine_1",
                    "spine_2",
                    "leg_upper_l",
                    "leg_upper_r",
                ]
                .iter()
                .filter_map(|n| bone_ix.get(*n).copied())
                .collect();
                for (j, w) in joints.iter().zip(&weights) {
                    for k in 0..4 {
                        if w[k] > 0.0001 && !allowed.contains(&j[k]) {
                            bail!("{name}: skirt has non-torso/upper-leg bone {}", j[k]);
                        }
                    }
                }
            }
            let indices: Vec<u32> = r.read_indices().context("no indices")?.into_u32().collect();
            let mat = prim.material();
            let material = mat.name().unwrap_or("").to_owned();
            let pbr = mat.pbr_metallic_roughness();
            let paint = if let Some(tex) = pbr.base_color_texture() {
                let gltf::image::Source::View { view, .. } = tex.texture().source().source() else {
                    bail!("{material}: external image");
                };
                let buf = &buffers[view.buffer().index()].0;
                let png = &buf[view.offset()..view.offset() + view.length()];
                Paint::Tiled(image::load_from_memory(png)?.to_rgb8())
            } else if let Some(r) = ramp_for(&material) {
                Paint::Ramp(r)
            } else {
                let c = pbr.base_color_factor();
                Paint::Ramp(vec![(0.0, [c[0], c[1], c[2]])])
            };
            out.push(Piece {
                material,
                paint,
                vb: VertexBuffer {
                    element_count: n,
                    positions,
                    normals,
                    texcoords: vec![uv],
                    joints,
                    weights,
                    ..VertexBuffer::default()
                },
                indices,
            });
        }
    }
    if out.is_empty() {
        bail!("{glb}: no meshes named {prefix}*");
    }
    Ok(out)
}

/// The exports carry leftover empty scenes without a `nodes` field, which the
/// strict `gltf` deserializer rejects; patch the JSON chunk before parsing.
fn open_glb(bytes: &[u8]) -> Result<(gltf::Document, Vec<gltf::buffer::Data>)> {
    let glb = gltf::Glb::from_slice(bytes)?;
    let mut json: serde_json::Value = serde_json::from_slice(&glb.json)?;
    if let Some(scenes) = json.get_mut("scenes").and_then(|s| s.as_array_mut()) {
        for sc in scenes {
            sc.as_object_mut()
                .context("scene is not an object")?
                .entry("nodes")
                .or_insert_with(|| serde_json::json!([]));
        }
    }
    let root: gltf::json::Root = serde_json::from_value(json)?;
    let doc = gltf::Document::from_json_without_validation(root);
    let bin = glb.bin.context("glb has no BIN chunk")?.into_owned();
    Ok((doc, vec![gltf::buffer::Data(bin)]))
}

fn srgb(l: Lin) -> Rgb<u8> {
    Rgb(l.map(|c| {
        let c = c.clamp(0.0, 1.0);
        let s = if c <= 0.003_130_8 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round() as u8
    }))
}

/// Merges the pieces into one buffer and paints the atlas their UVs are
/// remapped into: a tiled band across the top for a textured material (the
/// band spans exactly its whole-tile u range so the horizontal wrap is
/// seamless), then one full-height column per flat/ramp material, the ramp laid
/// along v so the Blender UV-Y ramp survives verbatim.
fn assemble(pieces: &[Piece]) -> Result<(VertexBuffer, Vec<u32>, RgbImage)> {
    let mut atlas = RgbImage::new(ATLAS, ATLAS);
    let a = ATLAS as f32;
    let mut band_bottom = 0u32;
    let mut uv_map: HashMap<&str, Box<dyn Fn([f32; 2]) -> [f32; 2]>> = HashMap::new();

    for p in pieces {
        let Paint::Tiled(tile) = &p.paint else {
            continue;
        };
        if uv_map.contains_key(p.material.as_str()) {
            continue;
        }
        let uvs = pieces
            .iter()
            .filter(|q| q.material == p.material)
            .flat_map(|q| q.vb.texcoords[0].iter().copied());
        let (mut u0, mut u1, mut v0, mut v1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for [u, v] in uvs {
            u0 = u0.min(u);
            u1 = u1.max(u);
            v0 = v0.min(v);
            v1 = v1.max(v);
        }
        let (u0, u1) = (u0.floor(), u1.ceil());
        let px_per_tile = a / (u1 - u0);
        let h = ((v1 - v0) * px_per_tile).ceil() as u32 + 8;
        let (tw, th) = tile.dimensions();
        let top = band_bottom + 4;
        for y in band_bottom..band_bottom + h {
            let v = v0 + (y as f32 - top as f32 + 0.5) / px_per_tile;
            for x in 0..ATLAS {
                let u = u0 + (x as f32 + 0.5) / px_per_tile;
                let tx = ((u.rem_euclid(1.0) * tw as f32) as u32).min(tw - 1);
                let ty = ((v.rem_euclid(1.0) * th as f32) as u32).min(th - 1);
                atlas.put_pixel(x, y, *tile.get_pixel(tx, ty));
            }
        }
        let topf = top as f32;
        uv_map.insert(
            &p.material,
            Box::new(move |[u, v]| [(u - u0) / (u1 - u0), (topf + (v - v0) * px_per_tile) / a]),
        );
        band_bottom += h;
    }

    let mut cols: Vec<(&str, &Vec<(f32, Lin)>)> = Vec::new();
    for p in pieces {
        if let Paint::Ramp(r) = &p.paint {
            if !cols.iter().any(|(m, _)| *m == p.material) {
                cols.push((&p.material, r));
            }
        }
    }
    if !cols.is_empty() {
        let col_w = ATLAS / cols.len() as u32;
        let y0 = band_bottom + 8;
        let y1 = ATLAS - 8;
        let span = (y1 - y0) as f32;
        for (ci, (mat, ramp)) in cols.iter().enumerate() {
            let x0 = ci as u32 * col_w;
            let x_end = if ci + 1 == cols.len() {
                ATLAS
            } else {
                x0 + col_w
            };
            for y in band_bottom..ATLAS {
                let v = ((y as f32 + 0.5 - y0 as f32) / span).clamp(0.0, 1.0);
                let blender_y = 1.0 - v;
                let c = ramp
                    .iter()
                    .rev()
                    .find(|(pos, _)| blender_y >= *pos)
                    .map_or(ramp[0].1, |s| s.1);
                for x in x0..x_end {
                    atlas.put_pixel(x, y, srgb(c));
                }
            }
            let cx = (x0 as f32 + col_w as f32 / 2.0) / a;
            let (y0f, spanf) = (y0 as f32, span);
            uv_map.insert(
                mat,
                Box::new(move |[_, v]| [cx, (y0f + v.clamp(0.0, 1.0) * spanf) / a]),
            );
        }
    }

    let mut vb = VertexBuffer::default();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    for p in pieces {
        let base = u32::try_from(vb.positions.len())?;
        let f = &uv_map[p.material.as_str()];
        vb.positions.extend_from_slice(&p.vb.positions);
        vb.normals.extend_from_slice(&p.vb.normals);
        vb.joints.extend_from_slice(&p.vb.joints);
        vb.weights.extend_from_slice(&p.vb.weights);
        uvs.extend(p.vb.texcoords[0].iter().map(|&t| f(t)));
        indices.extend(p.indices.iter().map(|i| i + base));
    }
    vb.element_count = vb.positions.len();
    vb.texcoords = vec![uvs];
    Ok((vb, indices, atlas))
}

/// Material-name marker for a garment that IS part of the original costume
/// (tennis briefs = the bunnysuit's own leotard, cut down). The hull was
/// modelled around it, so hull vertices wrapping it stay where they are.
const KEPT_COSTUME: &str = "briefs";

/// Snaps the outline-hull vertices that wrap the removed costume onto the bare
/// skin, at the hull's median offset from skin elsewhere, and gives them that
/// skin vertex's skinning. Topology and every other vertex are untouched.
/// `kept` is costume geometry the variant keeps; the hull
/// around it is left in place so its leg-opening folds stay on the garment
/// edge instead of drawing lines across bare skin.
fn reseat_outline(bytes: &[u8], kept: &[[f32; 3]]) -> Result<Vec<u8>> {
    let model = morphic::model::decode(bytes)?;
    let part = |n: &str| {
        model
            .meshes
            .iter()
            .find(|m| m.name == n)
            .with_context(|| format!("no {n}"))
    };
    let outline = part("outline")?;
    let clothes = part("clothes")?;
    let ears = part("ears")?;
    let body = part("body")?;

    let mut costume: Vec<[f32; 3]> = Vec::new();
    for m in [clothes, ears] {
        for vb in &m.vertex_buffers {
            costume.extend_from_slice(&vb.positions);
        }
    }
    // Skin surface = body vertices drawn by the head + skin materials (body vb0
    // also carries hair and the body's own outline shell, which must not count).
    // Everything that stays (skin, hair, gun) decides whether a hull vertex
    // belongs to the costume: it does only when the costume is its closest part.
    let mut skin: Vec<([f32; 3], [f32; 3], [u16; 4], [f32; 4])> = Vec::new();
    let mut keep: Vec<[f32; 3]> = part("gun")?.vertex_buffers[0].positions.clone();
    keep.extend_from_slice(kept);
    for prim in &body.primitives {
        if prim.material.contains("outline") {
            continue;
        }
        let is_skin = prim.material.contains("headv2") || prim.material.contains("skinmaterial");
        let vb = &body.vertex_buffers[prim.vertex_buffer];
        let mut used: Vec<u32> = prim.indices.clone();
        used.sort_unstable();
        used.dedup();
        for i in used {
            let i = i as usize;
            keep.push(vb.positions[i]);
            if is_skin {
                skin.push((vb.positions[i], vb.normals[i], vb.joints[i], vb.weights[i]));
            }
        }
    }

    let d2 = |a: [f32; 3], b: [f32; 3]| (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>();
    let mut vb = outline.vertex_buffers[0].clone();
    // Hair shell vertices must keep their positions and braid weights. Snapping
    // individual hair-shell vertices to skin stretches adjoining triangles.
    let skeleton = morphic::model::decode_skeleton(bytes)?;
    let follows_hair = |i: usize| {
        (0..4).any(|k| {
            vb.weights[i][k] > 0.001
                && skeleton.bones[vb.joints[i][k] as usize]
                    .name
                    .to_ascii_lowercase()
                    .contains("hair")
        })
    };

    let nearest_skin: Vec<(usize, f32)> = vb
        .positions
        .iter()
        .map(|&p| {
            skin.iter()
                .enumerate()
                .map(|(i, s)| (i, d2(p, s.0)))
                .min_by(|x, y| x.1.total_cmp(&y.1))
                .unwrap()
        })
        .collect();
    let on_costume: Vec<bool> = vb
        .positions
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            if follows_hair(i) {
                return false;
            }
            let dk = keep.iter().map(|&k| d2(p, k)).fold(f32::MAX, f32::min);
            // kept pieces went through a GLB round trip (~2e-5 drift), so a
            // costume point must be clearly nearer than them to count
            costume.iter().any(|&c| d2(p, c).sqrt() < dk.sqrt() - 1e-3)
        })
        .collect();
    let mut offs: Vec<f32> = nearest_skin
        .iter()
        .zip(&on_costume)
        .filter(|(_, c)| !**c)
        .map(|(n, _)| n.1.sqrt())
        .collect();
    offs.sort_by(f32::total_cmp);
    let off = offs[offs.len() / 2];
    let mut moved = 0;
    for i in 0..vb.element_count {
        if !on_costume[i] {
            continue;
        }
        let (p, n, j, w) = skin[nearest_skin[i].0];
        vb.positions[i] = [p[0] + n[0] * off, p[1] + n[1] * off, p[2] + n[2] * off];
        vb.normals[i] = n;
        vb.joints[i] = j;
        vb.weights[i] = w;
        moved += 1;
    }
    vb.tangents.clear();
    println!(
        "outline: reseated {moved}/{} hull verts onto skin at offset {off:.3}",
        vb.element_count
    );
    let indices = outline.primitives[0].indices.clone();
    let (out, _) =
        morphic::model::replace_draw_call_uncompressed(bytes, "outline", 0, &vb, &indices)?;
    Ok(out)
}

fn probe(vpk: &str) -> Result<()> {
    let bytes = vpkmerge_core::read_vpk_entry(vpk, ENTRY).context("read model")?;
    let skel = morphic::model::decode_skeleton(&bytes)?;
    println!("skeleton: {} bones", skel.bones.len());
    let model = morphic::model::decode(&bytes)?;
    for m in &model.meshes {
        println!("mesh {} ({} vbs)", m.name, m.vertex_buffers.len());
        for (i, vb) in m.vertex_buffers.iter().enumerate() {
            let wmin = vb
                .weights
                .iter()
                .map(|w| w.iter().sum::<f32>())
                .fold(f32::MAX, f32::min);
            let jmax = vb.joints.iter().flatten().max().copied().unwrap_or(0);
            println!(
                "   vb{i}: {} verts, joint max {jmax} (< {} bones), min weight sum {wmin:.3}",
                vb.element_count,
                skel.bones.len()
            );
        }
    }
    for dc in morphic::model::draw_call_targets(&bytes)? {
        println!(
            "dc {:8} prim {} {:>6} verts {:>7} idx  {}",
            dc.mesh_name, dc.primitive_index, dc.vertex_count, dc.index_count, dc.material
        );
    }
    Ok(())
}

fn ctrl(vpk: &str, filters: &[String]) -> Result<()> {
    let entries: Vec<String> = vpkmerge_core::inspect(vpk)?
        .file_paths
        .into_iter()
        .filter(|p| p.ends_with(".vmdl_c"))
        .filter(|p| filters.is_empty() || filters.iter().any(|f| p.contains(f.as_str())))
        .collect();
    for e in entries {
        let bytes = vpkmerge_core::read_vpk_entry(vpk, &e)?;
        let Some(block) = ctrl_block(&bytes) else {
            continue;
        };
        let tree = morphic::kv3::decode(block)?;
        let Some(meshes) = tree.get("embedded_meshes").and_then(|m| m.as_array()) else {
            continue;
        };
        for m in meshes {
            let name = m.get("name").and_then(|n| n.as_str()).unwrap_or("?");
            for kind in ["m_vertexBuffers", "m_indexBuffers"] {
                for (i, b) in m
                    .get(kind)
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let int = |k: &str| b.get(k).and_then(|v| v.as_int()).unwrap_or(-1);
                    println!(
                        "{e}\t{name}\t{kind}[{i}]\tcount {}\tsize {}\tblock {}\tmeshopt {:?}",
                        int("m_nElementCount"),
                        int("m_nElementSizeInBytes"),
                        int("m_nBlockIndex"),
                        b.get("m_bMeshoptCompressed").and_then(|v| v.as_bool())
                    );
                }
            }
        }
    }
    Ok(())
}

fn ctrl_block(d: &[u8]) -> Option<&[u8]> {
    let rd = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize;
    let base = 8 + rd(8);
    (0..rd(12)).find_map(|i| {
        let off = base + i * 12;
        (&d[off..off + 4] == b"CTRL")
            .then(|| &d[off + 4 + rd(off + 4)..off + 4 + rd(off + 4) + rd(off + 8)])
    })
}
