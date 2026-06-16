//! UV-island extraction and mask/atlas rasterization for per-region reskinning.
//!
//! Blender's role in per-part skin masking is purely mechanical: parse the mesh,
//! let you select faces in 3D, and bake that selection into a UV-space mask. This
//! module does the same three steps headlessly. [`segments`] partitions a decoded
//! [`Model`]'s triangles into regions (one per mesh part, one per material, or one
//! per connected UV island), and [`atlas_png`] / [`mask_png`] rasterize those
//! regions into PNGs:
//!
//! - an **atlas** colors every region a distinct hue so you can pick the index of
//!   the region you want by eye (the headless stand-in for Blender's viewport
//!   face-picker), and
//! - a **mask** bakes the selected regions to white-on-black, which the reskin
//!   builders consume as a region selector in place of the AO-contrast heuristic.
//!
//! UV islands are found by union-find over each vertex buffer's index graph:
//! triangles that share a vertex index share that vertex's UV exactly, so a
//! connected component in the index graph is exactly a UV island (a seam is where
//! the exporter split a vertex, which severs the component). Pure geometry, no
//! .NET and no Blender, matching this project's runtime-free ethos.

use std::collections::BTreeMap;
use std::io::Cursor;

use crate::error::DecodeError;
use crate::model::{MeshPart, Model, Primitive};

/// How to partition a model's triangles into regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentBy {
    /// One region per renderable mesh part ([`MeshPart::name`]).
    Part,
    /// One region per material path.
    Material,
    /// One region per connected component in UV space (a UV island).
    Island,
}

/// A texture-space triangle: the UV coordinates of its three corners.
#[derive(Debug, Clone, Copy)]
struct UvTri {
    uv: [[f32; 2]; 3],
}

/// One region of a model's texture space: a label plus the texture-space
/// triangles that fill it.
#[derive(Debug, Clone)]
pub struct Segment {
    /// Stable 0-based index into the slice returned by [`segments`]. Segments are
    /// sorted largest-first by UV area, so id 0 is the biggest region.
    pub id: usize,
    /// Human label: the mesh part name, material file stem, or `"<part> island N"`.
    pub label: String,
    /// Source mesh part this region came from.
    pub mesh: String,
    tris: Vec<UvTri>,
}

impl Segment {
    /// Number of texture-space triangles in this region.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// UV-space bounding box `[min_u, min_v, max_u, max_v]` (all-zero when empty).
    #[must_use]
    pub fn uv_bounds(&self) -> [f32; 4] {
        if self.tris.is_empty() {
            return [0.0; 4];
        }
        let mut b = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for t in &self.tris {
            for c in t.uv {
                b[0] = b[0].min(c[0]);
                b[1] = b[1].min(c[1]);
                b[2] = b[2].max(c[0]);
                b[3] = b[3].max(c[1]);
            }
        }
        b
    }

    /// Summed UV-space triangle area: a rough "how much of the texture this region
    /// owns". May exceed 1.0 when UVs overlap or tile outside the unit square.
    #[must_use]
    pub fn uv_area(&self) -> f32 {
        self.tris
            .iter()
            .map(|t| {
                let [p0, p1, p2] = t.uv;
                0.5 * ((p1[0] - p0[0]) * (p2[1] - p0[1]) - (p2[0] - p0[0]) * (p1[1] - p0[1])).abs()
            })
            .sum()
    }
}

/// Distinct picking color for a segment id (golden-angle hue, fixed S+V). The
/// atlas paints with these and a legend prints them, so an id maps to a swatch.
#[must_use]
pub fn segment_color(id: usize) -> [u8; 3] {
    #[allow(clippy::cast_precision_loss)]
    let hue = ((id as f32) * 0.618_034).fract() * 360.0;
    hsv_to_rgb(hue, 0.65, 0.95)
}

/// Partition a decoded model into regions by the chosen scheme. When `part` is
/// `Some`, only mesh parts whose name contains it (case-insensitive) are
/// considered, e.g. `Some("body")` to ignore weapon meshes. The result is sorted
/// largest-first by UV area and assigned contiguous ids `0..n`.
#[must_use]
pub fn segments(model: &Model, by: SegmentBy, part: Option<&str>) -> Vec<Segment> {
    let filtered;
    let meshes: &[MeshPart] = if let Some(want) = part {
        let want = want.to_lowercase();
        filtered = model
            .meshes
            .iter()
            .filter(|m| m.name.to_lowercase().contains(&want))
            .cloned()
            .collect::<Vec<_>>();
        &filtered
    } else {
        &model.meshes
    };
    let mut segs = match by {
        SegmentBy::Part => by_part(meshes),
        SegmentBy::Material => by_material(meshes),
        SegmentBy::Island => by_island(meshes),
    };
    segs.retain(|s| !s.tris.is_empty());
    segs.sort_by(|a, b| {
        b.uv_area()
            .partial_cmp(&a.uv_area())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (i, s) in segs.iter_mut().enumerate() {
        s.id = i;
    }
    segs
}

/// Fraction of the `res`x`res` texture each segment actually covers (unique
/// texels rasterized / total texels), in segment order. Unlike [`Segment::uv_area`]
/// this is bounded to `[0, 1]` and not inflated by tiling/overlapping UVs, so it
/// is the honest "how much of the texture this region owns" for sorting/picking.
#[must_use]
pub fn segment_coverage(segs: &[Segment], res: u32) -> Vec<f32> {
    let r = res as usize;
    #[allow(clippy::cast_precision_loss)]
    let total = (r * r) as f32;
    let mut stamp = vec![0u32; r * r];
    let mut generation = 0u32;
    let mut out = Vec::with_capacity(segs.len());
    for seg in segs {
        generation += 1;
        let mut count = 0usize;
        for t in &seg.tris {
            stamp_tri(&mut stamp, r, generation, &t.uv, &mut count);
        }
        #[allow(clippy::cast_precision_loss)]
        out.push(count as f32 / total);
    }
    out
}

/// Render every segment to a distinct-hue atlas PNG (RGBA8, `res`x`res`) for
/// visual region picking. Larger regions are painted first so small islands stay
/// visible on top; empty texels are dark gray.
pub fn atlas_png(segs: &[Segment], res: u32) -> Result<Vec<u8>, DecodeError> {
    let paint: Vec<usize> = (0..segs.len()).collect();
    let ids = rasterize_ids(segs, &paint, res, 2);
    let r = res as usize;
    let mut rgba = vec![0u8; r * r * 4];
    for (i, &id) in ids.iter().enumerate() {
        let [cr, cg, cb] = if id < 0 {
            [18, 18, 22]
        } else {
            segment_color(usize::try_from(id).unwrap_or(0))
        };
        rgba[i * 4] = cr;
        rgba[i * 4 + 1] = cg;
        rgba[i * 4 + 2] = cb;
        rgba[i * 4 + 3] = 255;
    }
    encode_png(res, res, rgba)
}

/// Bake the chosen segments (by id) to a white-on-black mask PNG (RGBA8,
/// `res`x`res`) the reskin builders sample as a region selector. Only the
/// selected segments are rasterized, so a region overlapped in UV by an
/// unselected one is not erased. Out-of-range ids are ignored.
pub fn mask_png(segs: &[Segment], selected: &[usize], res: u32) -> Result<Vec<u8>, DecodeError> {
    let paint: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|&id| id < segs.len())
        .collect();
    let ids = rasterize_ids(segs, &paint, res, 2);
    let r = res as usize;
    let mut rgba = vec![0u8; r * r * 4];
    for (i, &id) in ids.iter().enumerate() {
        let v = if id < 0 { 0 } else { 255 };
        rgba[i * 4] = v;
        rgba[i * 4 + 1] = v;
        rgba[i * 4 + 2] = v;
        rgba[i * 4 + 3] = 255;
    }
    encode_png(res, res, rgba)
}

// --- segment extraction ----------------------------------------------------

/// Texture-space triangles of one primitive (UV layer 0). Empty when the
/// primitive has no UVs or its buffer is out of range.
fn prim_uv_tris(mesh: &MeshPart, prim: &Primitive) -> Vec<UvTri> {
    let Some(vb) = mesh.vertex_buffers.get(prim.vertex_buffer) else {
        return Vec::new();
    };
    let Some(uvs) = vb.texcoords.first() else {
        return Vec::new();
    };
    if uvs.len() != vb.element_count {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(prim.indices.len() / 3);
    for t in prim.indices.chunks_exact(3) {
        let (i0, i1, i2) = (t[0] as usize, t[1] as usize, t[2] as usize);
        if i0 >= uvs.len() || i1 >= uvs.len() || i2 >= uvs.len() {
            continue;
        }
        out.push(UvTri {
            uv: [uvs[i0], uvs[i1], uvs[i2]],
        });
    }
    out
}

fn by_part(meshes: &[MeshPart]) -> Vec<Segment> {
    meshes
        .iter()
        .enumerate()
        .map(|(mi, mesh)| {
            let name = mesh_name(mesh, mi);
            let mut tris = Vec::new();
            for p in &mesh.primitives {
                tris.extend(prim_uv_tris(mesh, p));
            }
            Segment {
                id: 0,
                label: name.clone(),
                mesh: name,
                tris,
            }
        })
        .collect()
}

fn by_material(meshes: &[MeshPart]) -> Vec<Segment> {
    // Preserve first-seen mesh per material; key on the material path.
    let mut map: BTreeMap<String, (String, Vec<UvTri>)> = BTreeMap::new();
    for (mi, mesh) in meshes.iter().enumerate() {
        let name = mesh_name(mesh, mi);
        for p in &mesh.primitives {
            let entry = map
                .entry(p.material.clone())
                .or_insert_with(|| (name.clone(), Vec::new()));
            entry.1.extend(prim_uv_tris(mesh, p));
        }
    }
    map.into_iter()
        .map(|(mat, (mesh, tris))| {
            let label = mat.rsplit('/').next().unwrap_or(mat.as_str()).to_string();
            let label = label.strip_suffix("_c").unwrap_or(&label).to_string();
            Segment {
                id: 0,
                label,
                mesh,
                tris,
            }
        })
        .collect()
}

fn by_island(meshes: &[MeshPart]) -> Vec<Segment> {
    let mut out = Vec::new();
    for (mi, mesh) in meshes.iter().enumerate() {
        let name = mesh_name(mesh, mi);
        // Group primitives by the vertex buffer they draw from: the index graph
        // (and thus island connectivity) only joins within a single buffer.
        let mut by_buf: BTreeMap<usize, Vec<&Primitive>> = BTreeMap::new();
        for p in &mesh.primitives {
            by_buf.entry(p.vertex_buffer).or_default().push(p);
        }
        let mut island_no = 0usize;
        for (bi, prims) in by_buf {
            let Some(vb) = mesh.vertex_buffers.get(bi) else {
                continue;
            };
            let Some(uvs) = vb.texcoords.first() else {
                continue;
            };
            if uvs.len() != vb.element_count {
                continue;
            }
            let n = vb.element_count;
            let mut uf = UnionFind::new(n);
            for p in &prims {
                for t in p.indices.chunks_exact(3) {
                    let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                    if a < n && b < n && c < n {
                        uf.union(a, b);
                        uf.union(a, c);
                    }
                }
            }
            let mut groups: BTreeMap<usize, Vec<UvTri>> = BTreeMap::new();
            for p in &prims {
                for t in p.indices.chunks_exact(3) {
                    let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                    if a >= n || b >= n || c >= n {
                        continue;
                    }
                    groups.entry(uf.find(a)).or_default().push(UvTri {
                        uv: [uvs[a], uvs[b], uvs[c]],
                    });
                }
            }
            for tris in groups.into_values() {
                out.push(Segment {
                    id: 0,
                    label: format!("{name} island {island_no}"),
                    mesh: name.clone(),
                    tris,
                });
                island_no += 1;
            }
        }
    }
    out
}

fn mesh_name(mesh: &MeshPart, index: usize) -> String {
    if mesh.name.is_empty() {
        format!("mesh{index}")
    } else {
        mesh.name.clone()
    }
}

// --- rasterization ---------------------------------------------------------

/// Rasterize the listed segments into a per-texel id map (`-1` = empty), then
/// grow each region `dilate` texels into empty neighbors to close UV-seam cracks.
/// `paint` is the draw order: earlier entries can be overdrawn by later ones.
fn rasterize_ids(segs: &[Segment], paint: &[usize], res: u32, dilate: u32) -> Vec<i32> {
    let r = res as usize;
    let mut ids = vec![-1i32; r * r];
    for &si in paint {
        let Some(seg) = segs.get(si) else { continue };
        let id = i32::try_from(seg.id).unwrap_or(i32::MAX);
        for t in &seg.tris {
            fill_tri(&mut ids, r, id, &t.uv);
        }
    }
    for _ in 0..dilate {
        dilate_once(&mut ids, r);
    }
    ids
}

fn fill_tri(ids: &mut [i32], r: usize, id: i32, uv: &[[f32; 2]; 3]) {
    for_each_texel(r, uv, |idx| ids[idx] = id);
}

/// Marks every texel of `uv` with `generation` in `stamp`, counting each texel
/// only the first time this generation touches it (so overlapping triangles in
/// one segment do not double-count its coverage).
fn stamp_tri(stamp: &mut [u32], r: usize, generation: u32, uv: &[[f32; 2]; 3], count: &mut usize) {
    for_each_texel(r, uv, |idx| {
        if stamp[idx] != generation {
            stamp[idx] = generation;
            *count += 1;
        }
    });
}

/// Walks the texel centers covered by a UV-space triangle (top-left origin, UV
/// mapped straight to `[0, r)` to match how the reskin builders sample textures),
/// invoking `f` with each covered buffer index.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn for_each_texel(r: usize, uv: &[[f32; 2]; 3], mut f: impl FnMut(usize)) {
    let res = r as f32;
    let p = [
        [uv[0][0] * res, uv[0][1] * res],
        [uv[1][0] * res, uv[1][1] * res],
        [uv[2][0] * res, uv[2][1] * res],
    ];
    let area = edge(p[0], p[1], p[2]);
    if area.abs() < 1e-9 {
        return;
    }
    let min_x = p.iter().map(|q| q[0]).fold(f32::INFINITY, f32::min).floor();
    let max_x = p
        .iter()
        .map(|q| q[0])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil();
    let min_y = p.iter().map(|q| q[1]).fold(f32::INFINITY, f32::min).floor();
    let max_y = p
        .iter()
        .map(|q| q[1])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil();
    let x0 = min_x.max(0.0) as usize;
    let x1 = (max_x.min(res) as usize).min(r);
    let y0 = min_y.max(0.0) as usize;
    let y1 = (max_y.min(res) as usize).min(r);
    for y in y0..y1 {
        for x in x0..x1 {
            let px = [x as f32 + 0.5, y as f32 + 0.5];
            let w0 = edge(p[1], p[2], px);
            let w1 = edge(p[2], p[0], px);
            let w2 = edge(p[0], p[1], px);
            let inside = if area > 0.0 {
                w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0
            } else {
                w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0
            };
            if inside {
                f(y * r + x);
            }
        }
    }
}

/// Signed area of the triangle (a, b, c) times two.
fn edge(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// One pass of 4-neighbor dilation into empty texels (reads a snapshot so the
/// growth front does not feed itself within a pass).
fn dilate_once(ids: &mut [i32], r: usize) {
    let snap = ids.to_vec();
    for y in 0..r {
        for x in 0..r {
            if snap[y * r + x] != -1 {
                continue;
            }
            let neighbors = [
                (x > 0).then(|| (x - 1, y)),
                (x + 1 < r).then(|| (x + 1, y)),
                (y > 0).then(|| (x, y - 1)),
                (y + 1 < r).then(|| (x, y + 1)),
            ];
            for (nx, ny) in neighbors.into_iter().flatten() {
                let v = snap[ny * r + nx];
                if v != -1 {
                    ids[y * r + x] = v;
                    break;
                }
            }
        }
    }
}

fn encode_png(w: u32, h: u32, rgba: Vec<u8>) -> Result<Vec<u8>, DecodeError> {
    let img: image::RgbaImage = image::ImageBuffer::from_raw(w, h, rgba)
        .ok_or(DecodeError::Model("mask buffer size mismatch"))?;
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|_| DecodeError::Model("mask PNG encode failed"))?;
    Ok(out)
}

#[allow(
    clippy::many_single_char_names,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let h6 = (h / 60.0).rem_euclid(6.0);
    let x = c * (1.0 - (h6 % 2.0 - 1.0).abs());
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let q = |t: f32| ((t + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [q(r), q(g), q(b)]
}

/// Disjoint-set forest with path halving + union by attaching to the second root.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::VertexBuffer;

    /// A clean two-island mesh: a lower-left 0.4x0.4 UV quad and an upper-right
    /// one, each two triangles, sharing no vertex indices (so two UV islands).
    fn two_quad_mesh() -> MeshPart {
        let vb = VertexBuffer {
            element_count: 8,
            texcoords: vec![vec![
                [0.0, 0.0],
                [0.4, 0.0],
                [0.4, 0.4],
                [0.0, 0.4],
                [0.6, 0.6],
                [1.0, 0.6],
                [1.0, 1.0],
                [0.6, 1.0],
            ]],
            ..Default::default()
        };
        let prim = Primitive {
            vertex_buffer: 0,
            vertex_buffers: vec![0],
            material: "models/heroes/body.vmat_c".into(),
            vertex_count: 8,
            indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
        };
        MeshPart {
            name: "body".into(),
            mesh_index: 0,
            vertex_buffers: vec![vb],
            primitives: vec![prim],
            min_bounds: [0.0; 3],
            max_bounds: [0.0; 3],
            bone_weight_count: 0,
        }
    }

    /// Mirror `segments()`' sort + id assignment without needing a full `Model`.
    fn finalize(mut segs: Vec<Segment>) -> Vec<Segment> {
        segs.retain(|s| !s.tris.is_empty());
        segs.sort_by(|a, b| {
            b.uv_area()
                .partial_cmp(&a.uv_area())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for (i, s) in segs.iter_mut().enumerate() {
            s.id = i;
        }
        segs
    }

    #[test]
    fn island_mode_separates_the_two_quads() {
        let meshes = [two_quad_mesh()];
        let segs = finalize(by_island(&meshes));
        assert_eq!(segs.len(), 2, "two disjoint UV charts are two islands");
        assert!(segs.iter().all(|s| s.triangle_count() == 2));
    }

    #[test]
    fn part_and_material_modes_collapse_to_one_region() {
        let meshes = [two_quad_mesh()];
        assert_eq!(by_part(&meshes).len(), 1);
        assert_eq!(by_material(&meshes).len(), 1);
    }

    #[test]
    fn coverage_tracks_each_quad_area() {
        let meshes = [two_quad_mesh()];
        let segs = finalize(by_island(&meshes));
        // Each 0.4 x 0.4 quad is 0.16 of the unit texture.
        for c in segment_coverage(&segs, 256) {
            assert!((c - 0.16).abs() < 0.02, "coverage {c} not ~0.16");
        }
    }

    #[test]
    fn mask_paints_only_the_selected_island() {
        let meshes = [two_quad_mesh()];
        let segs = finalize(by_island(&meshes));
        // The lower-left quad is the island whose UV bbox starts near the origin.
        let lower_left = segs
            .iter()
            .position(|s| s.uv_bounds()[0] < 0.5)
            .expect("a lower-left island");
        let png = mask_png(&segs, &[lower_left], 64).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        let sample = |x: u32, y: u32| img.get_pixel(x, y)[0];
        assert_eq!(sample(13, 13), 255, "selected quad (~0.2,0.2) is white");
        assert_eq!(sample(51, 51), 0, "unselected quad (~0.8,0.8) stays black");
    }
}
