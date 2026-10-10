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
//!
//! Only triangles with a non-zero 3D area count. Deadlock hero meshes carry many
//! zero-area stitch triangles (a third of Vindicta's dress): two corners share
//! one position while their UVs sit on opposite sides of a seam. They never
//! rasterize on screen, but in UV space they span half the sheet, which streaked
//! masks and joined unrelated islands into one.

use std::collections::BTreeMap;
use std::io::Cursor;

use crate::error::DecodeError;
use crate::model::{MeshPart, Model, Primitive, Skeleton, VertexBuffer};

/// How to partition a model's triangles into regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentBy {
    /// One region per renderable mesh part ([`MeshPart::name`]).
    Part,
    /// One region per material path.
    Material,
    /// One region per connected component in UV space (a UV island).
    Island,
    /// One region per skeleton bone: each triangle goes to the bone that carries
    /// most of its skin weight, and a bone whose region is under 1% of the total
    /// UV area folds into its parent (fingers into the hand). Labels are bone
    /// names, so regions read as body areas. Unskinned meshes form one region
    /// per mesh part.
    Bone,
}

/// A bone whose region holds less than this share of the segmented UV area is
/// folded into its parent in [`SegmentBy::Bone`].
const MIN_BONE_SHARE: f32 = 0.01;

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
    /// Skeleton bone index -> skin weight times 3D surface area, summed.
    bones: BTreeMap<usize, f32>,
    /// Triangles whose UV winding is negative.
    negative: usize,
}

impl Segment {
    fn new(label: String, mesh: String) -> Self {
        Self {
            id: 0,
            label,
            mesh,
            tris: Vec::new(),
            bones: BTreeMap::new(),
            negative: 0,
        }
    }

    /// Adds triangle `t` (vertex indices into `vb`), weighting its skin by its
    /// 3D area so large faces count for more than slivers.
    fn push(&mut self, vb: &VertexBuffer, uvs: &[[f32; 2]], t: [usize; 3]) {
        let uv = [uvs[t[0]], uvs[t[1]], uvs[t[2]]];
        if edge(uv[0], uv[1], uv[2]) < 0.0 {
            self.negative += 1;
        }
        self.tris.push(UvTri { uv });
        let area = surface_area(vb, t).unwrap_or(1.0) / 3.0;
        for (bone, w) in t.into_iter().flat_map(|v| vertex_influences(vb, v)) {
            *self.bones.entry(bone).or_default() += w * area;
        }
    }

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

    /// Skin influence per skeleton bone index, as shares of the region's total
    /// (summing to 1), largest first. Empty for an unskinned region.
    #[must_use]
    pub fn bone_weights(&self) -> Vec<(usize, f32)> {
        let total: f32 = self.bones.values().sum();
        if total <= 0.0 {
            return Vec::new();
        }
        let mut out: Vec<(usize, f32)> = self.bones.iter().map(|(&b, &w)| (b, w / total)).collect();
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    /// Share of triangles wound against the region's majority in UV space. A
    /// mesh's faces wind consistently in 3D, so a minority winding in UV means
    /// those faces are mirrored: near 0.5 is a left/right pair sharing texels.
    #[must_use]
    pub fn mirrored_fraction(&self) -> f32 {
        if self.tris.is_empty() {
            return 0.0;
        }
        let minority = self.negative.min(self.tris.len() - self.negative);
        #[allow(clippy::cast_precision_loss)]
        let share = minority as f32 / self.tris.len() as f32;
        share
    }

    /// Calls `f` with the buffer index (`y * width + x`) of every texel whose
    /// center lies inside one of the region's triangles, on a `width` x `height`
    /// texture (top-left origin). A texel under several triangles is visited
    /// once per triangle.
    pub fn for_each_texel(&self, width: u32, height: u32, mut f: impl FnMut(usize)) {
        for t in &self.tris {
            for_each_texel(width as usize, height as usize, &t.uv, &mut f);
        }
    }
}

/// The triangles of `prim` that can own texels: UV layer 0 present, indices in
/// range, and a non-zero 3D area (see the module docs on stitch triangles).
fn renderable_tris<'a>(
    vb: &'a VertexBuffer,
    prim: &'a Primitive,
) -> Option<(&'a [[f32; 2]], impl Iterator<Item = [usize; 3]> + 'a)> {
    let uvs = vb
        .texcoords
        .first()
        .filter(|uvs| uvs.len() == vb.element_count)?;
    let n = vb.element_count;
    let tris = prim
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| [t[0] as usize, t[1] as usize, t[2] as usize])
        .filter(move |t| t.iter().all(|&i| i < n))
        .filter(move |&t| surface_area(vb, t).is_none_or(|a| a > 0.0));
    Some((uvs.as_slice(), tris))
}

/// 3D area of triangle `t`, or `None` when the buffer has no positions. Exactly
/// coincident corners (and float noise around them) read as zero.
fn surface_area(vb: &VertexBuffer, t: [usize; 3]) -> Option<f32> {
    if vb.positions.len() != vb.element_count {
        return None;
    }
    let [a, b, c] = t.map(|i| vb.positions[i]);
    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let cross = [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ];
    let twice = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
    let longest = [a, b, c]
        .iter()
        .zip([b, c, a])
        .map(|(p, q)| (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2))
        .fold(0.0f32, f32::max);
    Some(if twice <= 1e-6 * longest {
        0.0
    } else {
        0.5 * twice
    })
}

/// The (bone, weight) lanes of vertex `v` that carry weight. A rigid buffer
/// (joints but no weights) binds lane 0 fully.
fn vertex_influences(vb: &VertexBuffer, v: usize) -> impl Iterator<Item = (usize, f32)> + '_ {
    let joints = vb
        .joints
        .get(v)
        .filter(|_| vb.joints.len() == vb.element_count);
    let weights = vb
        .weights
        .get(v)
        .filter(|_| vb.weights.len() == vb.element_count);
    joints.into_iter().flat_map(move |j| {
        (0..4).filter_map(move |lane| {
            let w = weights.map_or(if lane == 0 { 1.0 } else { 0.0 }, |w| w[lane]);
            (w > 0.0).then(|| (usize::from(j[lane]), w))
        })
    })
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
    let want = part.map(str::to_lowercase);
    segments_where(model, by, &|mesh, _| {
        want.as_deref()
            .is_none_or(|w| mesh.name.to_lowercase().contains(w))
    })
}

/// [`segments`] over only the primitives `keep` accepts, e.g. the draw calls
/// whose material samples one texture, so the regions describe that texture's
/// UV space alone.
#[must_use]
pub fn segments_where(
    model: &Model,
    by: SegmentBy,
    keep: &dyn Fn(&MeshPart, &Primitive) -> bool,
) -> Vec<Segment> {
    let meshes: Vec<(String, &MeshPart, Vec<&Primitive>)> = model
        .meshes
        .iter()
        .enumerate()
        .map(|(mi, mesh)| {
            let prims = mesh.primitives.iter().filter(|p| keep(mesh, p)).collect();
            (mesh_name(mesh, mi), mesh, prims)
        })
        .collect();
    let mut segs = match by {
        SegmentBy::Part => by_part(&meshes),
        SegmentBy::Material => by_material(&meshes),
        SegmentBy::Island => by_island(&meshes),
        SegmentBy::Bone => by_bone(&meshes, &model.skeleton),
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

/// A mesh part's display name and the primitives being segmented.
type MeshPrims<'a> = (String, &'a MeshPart, Vec<&'a Primitive>);

/// Every renderable triangle of `prims`, with the buffer it indexes.
fn each_tri<'a>(
    mesh: &'a MeshPart,
    prims: &'a [&'a Primitive],
) -> impl Iterator<Item = (&'a Primitive, &'a VertexBuffer, &'a [[f32; 2]], [usize; 3])> + 'a {
    prims.iter().flat_map(move |p| {
        let vb = mesh.vertex_buffers.get(p.vertex_buffer);
        vb.and_then(|vb| renderable_tris(vb, p).map(|(uvs, tris)| (vb, uvs, tris)))
            .into_iter()
            .flat_map(move |(vb, uvs, tris)| tris.map(move |t| (*p, vb, uvs, t)))
    })
}

fn by_part(meshes: &[MeshPrims]) -> Vec<Segment> {
    meshes
        .iter()
        .map(|(name, mesh, prims)| {
            let mut seg = Segment::new(name.clone(), name.clone());
            for (_, vb, uvs, t) in each_tri(mesh, prims) {
                seg.push(vb, uvs, t);
            }
            seg
        })
        .collect()
}

fn by_material(meshes: &[MeshPrims]) -> Vec<Segment> {
    // Preserve first-seen mesh per material; key on the material path.
    let mut map: BTreeMap<&str, Segment> = BTreeMap::new();
    for (name, mesh, prims) in meshes {
        for (p, vb, uvs, t) in each_tri(mesh, prims) {
            map.entry(p.material.as_str())
                .or_insert_with(|| {
                    let label = p.material.rsplit('/').next().unwrap_or(&p.material);
                    let label = label.strip_suffix("_c").unwrap_or(label);
                    Segment::new(label.to_owned(), name.clone())
                })
                .push(vb, uvs, t);
        }
    }
    map.into_values().collect()
}

fn by_island(meshes: &[MeshPrims]) -> Vec<Segment> {
    let mut out = Vec::new();
    for (name, mesh, prims) in meshes {
        // Group primitives by the vertex buffer they draw from: the index graph
        // (and thus island connectivity) only joins within a single buffer.
        let mut by_buf: BTreeMap<usize, Vec<&Primitive>> = BTreeMap::new();
        for p in prims {
            by_buf.entry(p.vertex_buffer).or_default().push(p);
        }
        let mut island_no = 0usize;
        for (bi, prims) in by_buf {
            let Some(vb) = mesh.vertex_buffers.get(bi) else {
                continue;
            };
            let mut uf = UnionFind::new(vb.element_count);
            for (_, _, _, [a, b, c]) in each_tri(mesh, &prims) {
                uf.union(a, b);
                uf.union(a, c);
            }
            let mut groups: BTreeMap<usize, Segment> = BTreeMap::new();
            for (_, vb, uvs, t) in each_tri(mesh, &prims) {
                groups
                    .entry(uf.find(t[0]))
                    .or_insert_with(|| {
                        let seg = Segment::new(format!("{name} island {island_no}"), name.clone());
                        island_no += 1;
                        seg
                    })
                    .push(vb, uvs, t);
            }
            out.extend(groups.into_values());
        }
    }
    out
}

fn by_bone(meshes: &[MeshPrims], skeleton: &Skeleton) -> Vec<Segment> {
    // Each triangle's dominant bone, or None when its buffer carries no skin.
    let dominant = |vb: &VertexBuffer, t: [usize; 3]| {
        let mut sum: BTreeMap<usize, f32> = BTreeMap::new();
        for (bone, w) in t.into_iter().flat_map(|v| vertex_influences(vb, v)) {
            *sum.entry(bone).or_default() += w;
        }
        sum.into_iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(bone, _)| bone)
    };
    let uv_area =
        |uvs: &[[f32; 2]], t: [usize; 3]| 0.5 * edge(uvs[t[0]], uvs[t[1]], uvs[t[2]]).abs();

    let bones = skeleton.bones.len();
    let mut area = vec![0.0f32; bones];
    let mut total = 0.0f32;
    for (_, mesh, prims) in meshes {
        for (_, vb, uvs, t) in each_tri(mesh, prims) {
            let a = uv_area(uvs, t);
            total += a;
            if let Some(b) = dominant(vb, t).filter(|&b| b < bones) {
                area[b] += a;
            }
        }
    }

    // Fold small regions into their parents, deepest bones first so a chain of
    // small bones (finger joints) collapses all the way up in one pass.
    let depth = |mut b: usize| {
        let mut d = 0;
        while let Some(p) = skeleton.bones[b].parent.filter(|&p| p < bones && p != b) {
            b = p;
            d += 1;
            if d > bones {
                break;
            }
        }
        d
    };
    let mut order: Vec<usize> = (0..bones).collect();
    order.sort_by_key(|&b| std::cmp::Reverse(depth(b)));
    let mut owner: Vec<usize> = (0..bones).collect();
    for b in order {
        if area[b] > 0.0 && area[b] < MIN_BONE_SHARE * total {
            if let Some(p) = skeleton.bones[b].parent.filter(|&p| p < bones && p != b) {
                area[p] += area[b];
                area[b] = 0.0;
                owner[b] = p;
            }
        }
    }
    let resolve = |mut b: usize| {
        for _ in 0..bones {
            if owner[b] == b {
                break;
            }
            b = owner[b];
        }
        b
    };

    let mut by_key: BTreeMap<Result<usize, String>, Segment> = BTreeMap::new();
    for (name, mesh, prims) in meshes {
        for (_, vb, uvs, t) in each_tri(mesh, prims) {
            let key = dominant(vb, t)
                .filter(|&b| b < bones)
                .map(resolve)
                .ok_or_else(|| name.clone());
            by_key
                .entry(key.clone())
                .or_insert_with(|| {
                    let label = key.map_or_else(|mesh| mesh, |b| skeleton.bones[b].name.clone());
                    Segment::new(label, name.clone())
                })
                .push(vb, uvs, t);
        }
    }
    by_key.into_values().collect()
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
    for_each_texel(r, r, uv, |idx| ids[idx] = id);
}

/// Marks every texel of `uv` with `generation` in `stamp`, counting each texel
/// only the first time this generation touches it (so overlapping triangles in
/// one segment do not double-count its coverage).
fn stamp_tri(stamp: &mut [u32], r: usize, generation: u32, uv: &[[f32; 2]; 3], count: &mut usize) {
    for_each_texel(r, r, uv, |idx| {
        if stamp[idx] != generation {
            stamp[idx] = generation;
            *count += 1;
        }
    });
}

/// Walks the texel centers covered by a UV-space triangle (top-left origin, UV
/// mapped straight to `[0, w) x [0, h)` to match how the reskin builders sample
/// textures), invoking `f` with each covered buffer index (`y * w + x`).
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn for_each_texel(w: usize, h: usize, uv: &[[f32; 2]; 3], mut f: impl FnMut(usize)) {
    let (fw, fh) = (w as f32, h as f32);
    let p = [
        [uv[0][0] * fw, uv[0][1] * fh],
        [uv[1][0] * fw, uv[1][1] * fh],
        [uv[2][0] * fw, uv[2][1] * fh],
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
    let x1 = (max_x.min(fw) as usize).min(w);
    let y0 = min_y.max(0.0) as usize;
    let y1 = (max_y.min(fh) as usize).min(h);
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
                f(y * w + x);
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
    use crate::model::math::{Mat4, Quat, Vec3};
    use crate::model::Bone;

    /// A clean two-island mesh: a lower-left 0.4x0.4 UV quad and an upper-right
    /// one, each two triangles, sharing no vertex indices (so two UV islands).
    /// Each quad is a unit square in 3D, skinned fully to its own bone.
    fn two_quad_model() -> Model {
        let mut vb = VertexBuffer {
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
        vb.positions = vb.texcoords[0]
            .iter()
            .map(|uv| [uv[0], uv[1], 0.0])
            .collect();
        vb.joints = (0..8).map(|v| [u16::from(v >= 4) + 1, 0, 0, 0]).collect();
        vb.weights = vec![[1.0, 0.0, 0.0, 0.0]; 8];
        let prim = Primitive {
            vertex_buffer: 0,
            vertex_buffers: vec![0],
            material: "models/heroes/body.vmat_c".into(),
            vertex_count: 8,
            indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
        };
        let bone = |name: &str, parent| Bone {
            name: name.to_owned(),
            parent,
            flags: 0,
            position: Vec3::default(),
            rotation: Quat::default(),
            local_bind: Mat4::IDENTITY,
            global_bind: Mat4::IDENTITY,
            inverse_bind: Mat4::IDENTITY,
        };
        Model {
            skeleton: Skeleton {
                bones: vec![
                    bone("root", None),
                    bone("arm", Some(0)),
                    bone("hand", Some(1)),
                ],
            },
            meshes: vec![MeshPart {
                name: "body".into(),
                mesh_index: 0,
                vertex_buffers: vec![vb],
                primitives: vec![prim],
                min_bounds: [0.0; 3],
                max_bounds: [0.0; 3],
                bone_weight_count: 1,
            }],
            animations: Vec::new(),
            cloth: None,
        }
    }

    #[test]
    fn island_mode_separates_the_two_quads() {
        let segs = segments(&two_quad_model(), SegmentBy::Island, None);
        assert_eq!(segs.len(), 2, "two disjoint UV charts are two islands");
        assert!(segs.iter().all(|s| s.triangle_count() == 2));
    }

    #[test]
    fn part_and_material_modes_collapse_to_one_region() {
        let model = two_quad_model();
        assert_eq!(segments(&model, SegmentBy::Part, None).len(), 1);
        assert_eq!(segments(&model, SegmentBy::Material, None).len(), 1);
    }

    #[test]
    fn zero_area_stitch_triangles_neither_paint_nor_join_islands() {
        let mut model = two_quad_model();
        // A stitch: corners 2 and 4 moved onto one 3D point, so the triangle has
        // no surface while its UVs span from one quad to the other.
        let vb = &mut model.meshes[0].vertex_buffers[0];
        vb.positions[4] = vb.positions[2];
        model.meshes[0].primitives[0].indices.extend([2, 4, 3]);
        let segs = segments(&model, SegmentBy::Island, None);
        assert_eq!(segs.len(), 2, "the stitch must not merge the two islands");
        assert!(segs.iter().all(|s| s.triangle_count() == 2));
        let png = mask_png(&segs, &[0, 1], 64).unwrap();
        let img = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(
            img.get_pixel(32, 32)[0],
            0,
            "the gap between the quads stays black"
        );
    }

    #[test]
    fn bone_mode_labels_regions_by_bone_and_reports_weights() {
        let segs = segments(&two_quad_model(), SegmentBy::Bone, None);
        let mut labels: Vec<&str> = segs.iter().map(|s| s.label.as_str()).collect();
        labels.sort_unstable();
        assert_eq!(labels, ["arm", "hand"]);
        for s in &segs {
            let weights = s.bone_weights();
            assert_eq!(weights.len(), 1);
            assert!((weights[0].1 - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn bone_mode_folds_a_tiny_bone_into_its_parent() {
        let mut model = two_quad_model();
        // Shrink the "hand" quad to a sliver of UV space, under 1% of the total.
        let uvs = &mut model.meshes[0].vertex_buffers[0].texcoords[0];
        for uv in &mut uvs[4..] {
            *uv = [0.6 + (uv[0] - 0.6) * 0.05, 0.6 + (uv[1] - 0.6) * 0.05];
        }
        let segs = segments(&model, SegmentBy::Bone, None);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].label, "arm");
        assert_eq!(segs[0].triangle_count(), 4);
    }

    #[test]
    fn mirrored_fraction_counts_the_minority_winding() {
        let mut model = two_quad_model();
        // Mirror the second quad in U: its triangles now wind the other way.
        let uvs = &mut model.meshes[0].vertex_buffers[0].texcoords[0];
        for uv in &mut uvs[4..] {
            uv[0] = 1.6 - uv[0];
        }
        let segs = segments(&model, SegmentBy::Part, None);
        assert!((segs[0].mirrored_fraction() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn segments_where_keeps_only_accepted_primitives() {
        let mut model = two_quad_model();
        let mut other = model.meshes[0].primitives[0].clone();
        other.material = "models/heroes/gun.vmat_c".into();
        other.indices = vec![4, 5, 6, 4, 6, 7];
        model.meshes[0].primitives[0].indices.truncate(6);
        model.meshes[0].primitives.push(other);
        let segs = segments_where(&model, SegmentBy::Island, &|_, p| {
            p.material.contains("body")
        });
        assert_eq!(segs.len(), 1);
        assert!(segs[0].uv_bounds()[2] <= 0.4 + 1e-6);
    }

    #[test]
    fn for_each_texel_handles_non_square_textures() {
        let segs = segments(&two_quad_model(), SegmentBy::Island, None);
        let lower_left = segs.iter().find(|s| s.uv_bounds()[0] < 0.5).unwrap();
        let (w, h) = (100u32, 50u32);
        let mut hits = std::collections::HashSet::new();
        lower_left.for_each_texel(w, h, |i| {
            hits.insert(i);
        });
        // 0.4 x 0.4 of a 100 x 50 sheet is 40 x 20 texels.
        assert_eq!(hits.len(), 800);
        assert!(hits
            .iter()
            .all(|&i| i % (w as usize) < 40 && i / (w as usize) < 20));
    }

    #[test]
    fn coverage_tracks_each_quad_area() {
        let segs = segments(&two_quad_model(), SegmentBy::Island, None);
        // Each 0.4 x 0.4 quad is 0.16 of the unit texture.
        for c in segment_coverage(&segs, 256) {
            assert!((c - 0.16).abs() < 0.02, "coverage {c} not ~0.16");
        }
    }

    #[test]
    fn mask_paints_only_the_selected_island() {
        let segs = segments(&two_quad_model(), SegmentBy::Island, None);
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
