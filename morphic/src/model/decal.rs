//! Decals: a picture projected onto a model as draw calls of its own.
//!
//! A decal copies the triangles a picture lands on, trims the copies to the
//! picture's rectangle and lifts them a little off the surface. Every copied
//! vertex keeps the stored bytes of the vertex it came from, apart from its
//! position and UV, so it is skinned the same way and follows the animation.
//! The copies read a material of their own, so the picture needs no room in
//! the textures of the part underneath.
//!
//! The copies go at the end of the vertex and index buffers the part already
//! draws from, as one more draw call per part. Existing vertices and indices
//! keep their bytes and their numbers.

use std::collections::HashMap;

use crate::error::DecodeError;
use crate::kv3::{self, Seg, ValueFlags};
use crate::resource::Resource;

use super::edit::{parse_embedded, uv_field};
use super::mesh::{lod_group_masks, EmbeddedMesh, SceneObject};
use super::topology::{bake_draw_call, locate_draw_calls, DrawCallGroup};
use super::vbib::OnDiskBuffer;

/// Texels along each edge of the depth map that decides what the picture can
/// see.
const DEPTH_MAP_SIZE: usize = 512;

/// A flat picture in model space and the way it travels onto the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecalProjection {
    /// The picture's top-left corner.
    pub origin: [f32; 3],
    /// From the picture's left edge to its right edge.
    pub right: [f32; 3],
    /// From the picture's top edge to its bottom edge.
    pub down: [f32; 3],
    /// Unit vector from the viewer into the model.
    pub direction: [f32; 3],
}

impl DecalProjection {
    /// Where a point falls: across the picture, down the picture (both 0 to 1
    /// inside it) and how far along the projection.
    #[must_use]
    pub fn project(&self, point: [f32; 3]) -> [f32; 3] {
        let rel = sub(point, self.origin);
        [
            dot(rel, self.right) / dot(self.right, self.right),
            dot(rel, self.down) / dot(self.down, self.down),
            dot(rel, self.direction),
        ]
    }
}

/// How a decal sits on the surface and in its texture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecalOptions {
    /// How far the copies sit above the surface, in model units.
    pub offset: f32,
    /// The part of the texture the picture fills: `[u0, v0, u1, v1]`.
    pub uv_rect: [f32; 4],
    /// How far behind the nearest surface a triangle may sit and still take
    /// the picture, in model units.
    pub depth_tolerance: f32,
    /// The picture stops where the surface turns further from the viewer than
    /// this: the cosine of the angle between the surface normal and the way
    /// back to the viewer. Past it a projected picture smears.
    pub min_facing: f32,
}

impl Default for DecalOptions {
    fn default() -> Self {
        Self {
            offset: 0.1,
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            depth_tolerance: 0.5,
            min_facing: 0.4,
        }
    }
}

/// What [`append_projected_decal`] added.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecalReport {
    /// Draw calls added, over every level of detail.
    pub draw_calls: usize,
    /// Triangles added at the highest level of detail.
    pub triangles: usize,
    /// Share of the picture that found a surface at the highest level of
    /// detail.
    pub coverage: f32,
    /// Materials the picture landed on at the highest level of detail, each
    /// with the share of the picture it holds, largest first.
    pub hosts: Vec<(String, f32)>,
}

/// The nearest surface along a projection, over the picture's rectangle.
#[derive(Clone, Debug)]
pub struct DepthMap {
    width: usize,
    height: usize,
    depth: Vec<f32>,
    shade: Vec<f32>,
}

impl DepthMap {
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            depth: vec![f32::INFINITY; width * height],
            shade: vec![0.0; width * height],
        }
    }

    /// Draw a triangle whose corners are `[across, down, depth]`, with across
    /// and down running 0 to 1 over the map. `shade` is kept for the texels
    /// where the triangle is the nearest surface.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    pub fn draw(&mut self, corners: [[f32; 3]; 3], shade: f32) {
        let (w, h) = (self.width as f32, self.height as f32);
        let p = corners.map(|c| [c[0] * w, c[1] * h]);
        let area = edge(p[0], p[1], p[2]);
        if area.abs() < 1e-12 {
            return;
        }
        let min_x = p.iter().map(|c| c[0]).fold(f32::INFINITY, f32::min);
        let max_x = p.iter().map(|c| c[0]).fold(f32::NEG_INFINITY, f32::max);
        let min_y = p.iter().map(|c| c[1]).fold(f32::INFINITY, f32::min);
        let max_y = p.iter().map(|c| c[1]).fold(f32::NEG_INFINITY, f32::max);
        if max_x < 0.0 || max_y < 0.0 || min_x > w || min_y > h {
            return;
        }
        let x0 = min_x.floor().max(0.0) as usize;
        let y0 = min_y.floor().max(0.0) as usize;
        let x1 = (max_x.ceil().max(0.0) as usize).min(self.width);
        let y1 = (max_y.ceil().max(0.0) as usize).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                let at = [x as f32 + 0.5, y as f32 + 0.5];
                let weights = [
                    edge(p[1], p[2], at) / area,
                    edge(p[2], p[0], at) / area,
                    edge(p[0], p[1], at) / area,
                ];
                if weights.iter().any(|&weight| weight < 0.0) {
                    continue;
                }
                let depth = weights[0] * corners[0][2]
                    + weights[1] * corners[1][2]
                    + weights[2] * corners[2][2];
                let slot = y * self.width + x;
                if depth < self.depth[slot] {
                    self.depth[slot] = depth;
                    self.shade[slot] = shade;
                }
            }
        }
    }

    /// Depth and shade of the nearest surface at a point of the map, if any
    /// surface was drawn there.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    pub fn at(&self, across: f32, down: f32) -> Option<(f32, f32)> {
        if !(0.0..=1.0).contains(&across) || !(0.0..=1.0).contains(&down) {
            return None;
        }
        let x = ((across * self.width as f32) as usize).min(self.width - 1);
        let y = ((down * self.height as f32) as usize).min(self.height - 1);
        let slot = y * self.width + x;
        self.depth[slot]
            .is_finite()
            .then(|| (self.depth[slot], self.shade[slot]))
    }
}

/// One vertex buffer of a mesh, as far as a decal needs it.
struct Surface {
    positions: Vec<[f32; 3]>,
    /// Empty when the buffer stores no normals.
    normals: Vec<[f32; 3]>,
    /// The buffer can take copies: it has normals and a UV set, and is not
    /// ZSTD-packed.
    can_host: bool,
}

struct Draw {
    scene_object: usize,
    /// Its place among the scene object's draw calls.
    call: usize,
    vertex_buffer: usize,
    index_buffer: usize,
    /// The draw reads one vertex stream, so a copy needs bytes in one buffer.
    single_stream: bool,
    material: String,
    indices: Vec<u32>,
}

struct MeshGeometry {
    lod_mask: u64,
    surfaces: Vec<Surface>,
    draws: Vec<Draw>,
    /// Width of each index buffer in bytes.
    index_sizes: Vec<usize>,
}

/// A polygon corner while a triangle is trimmed to the picture.
#[derive(Clone, Copy)]
struct Corner {
    position: [f32; 3],
    normal: [f32; 3],
    /// Across, down, depth.
    place: [f32; 3],
    /// How squarely the surface faces the viewer here: 1 head on, 0 edge on,
    /// negative turned away.
    facing: f32,
    /// The stored vertex this corner copies its other attributes from.
    parent: u32,
}

/// The copies one draw call gains.
struct Addition {
    mesh: usize,
    scene_object: usize,
    draw_call: usize,
    vertex_buffer: usize,
    index_buffer: usize,
    /// The material of the draw call the copies come from.
    material: String,
    parents: Vec<u32>,
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    /// Into the copies, not the whole buffer.
    indices: Vec<u32>,
    /// The share of the picture the copies hold.
    area: f32,
}

/// Project a picture onto a model and add the surface it lands on as new draw
/// calls that render `material`.
///
/// Every level of detail gets its own copy, on one mesh: the one that holds
/// most of the picture. So a picture on the body stays off a weapon that sits
/// in front of it in the bind pose, and is not hidden by it. A draw call takes
/// the picture, and can hide the surface behind it, only where `is_host`
/// accepts its material, which lets a caller keep a second decal off the first.
/// Surfaces hidden behind others of the same mesh, as seen along the
/// projection, are left out, as are surfaces turned away from it.
///
/// A model the picture misses comes back unchanged, with an empty report.
///
/// # Errors
/// Fails on legacy single-block meshes, on a ZSTD-packed index buffer, and when
/// a 16-bit index buffer cannot address the added vertices.
pub fn append_projected_decal(
    vmdl_bytes: &[u8],
    projection: &DecalProjection,
    material: &str,
    options: &DecalOptions,
    is_host: impl Fn(&str) -> bool,
) -> Result<(Vec<u8>, DecalReport), DecodeError> {
    let mut report = DecalReport::default();
    let mut additions: Vec<Addition> = Vec::new();
    {
        let (resource, embedded) = parse_embedded(vmdl_bytes)?;
        let masks = lod_group_masks(&kv3::decode(resource.data_block()?)?)?;
        let meshes = embedded
            .iter()
            .map(|em| {
                load(
                    &resource,
                    em,
                    masks.get(em.mesh_index).copied().unwrap_or(1),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        // What each mesh would take, with the share of the picture it holds.
        let mut candidates: Vec<(usize, Vec<Addition>)> = Vec::new();
        for (mesh_index, mesh) in meshes.iter().enumerate() {
            let depth = depth_map(mesh, projection, &is_host);
            let mut taken = Vec::new();
            for draw in &mesh.draws {
                let surface = &mesh.surfaces[draw.vertex_buffer];
                if !draw.single_stream || !surface.can_host || !is_host(&draw.material) {
                    continue;
                }
                let addition = cover(mesh_index, draw, surface, &depth, projection, options);
                if !addition.indices.is_empty() {
                    taken.push(addition);
                }
            }
            if !taken.is_empty() {
                candidates.push((mesh_index, taken));
            }
        }
        // One mesh per level of detail: the one that holds the most.
        let mut best: HashMap<u64, (usize, f32)> = HashMap::new();
        for (mesh_index, taken) in &candidates {
            let area = taken.iter().map(|addition| addition.area).sum();
            let slot = best
                .entry(meshes[*mesh_index].lod_mask)
                .or_insert((*mesh_index, area));
            if area > slot.1 {
                *slot = (*mesh_index, area);
            }
        }
        let mut hosts: HashMap<String, f32> = HashMap::new();
        for (mesh_index, taken) in candidates {
            let mesh = &meshes[mesh_index];
            if best.get(&mesh.lod_mask).map(|slot| slot.0) != Some(mesh_index) {
                continue;
            }
            for addition in taken {
                let vertices =
                    mesh.surfaces[addition.vertex_buffer].positions.len() + addition.parents.len();
                if mesh.index_sizes[addition.index_buffer] == 2 && vertices > usize::from(u16::MAX)
                {
                    return Err(DecodeError::Model(
                        "the decal needs more vertices than a 16-bit index buffer can address",
                    ));
                }
                if mesh.lod_mask & 1 != 0 {
                    report.triangles += addition.indices.len() / 3;
                    report.coverage += addition.area;
                    *hosts.entry(addition.material.clone()).or_default() += addition.area;
                }
                additions.push(addition);
            }
        }
        report.hosts = hosts.into_iter().collect();
        report
            .hosts
            .sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    }

    let mut bytes = vmdl_bytes.to_vec();
    for addition in &additions {
        bytes = apply(&bytes, addition, material)?;
        report.draw_calls += 1;
    }
    Ok((bytes, report))
}

fn load(
    resource: &Resource<'_>,
    em: &EmbeddedMesh,
    lod_mask: u64,
) -> Result<MeshGeometry, DecodeError> {
    if em.vbib_block.is_some() {
        return Err(DecodeError::Model("legacy VBIB meshes are not supported"));
    }
    let mdat = kv3::decode(
        resource
            .get_block_by_index(em.data_block)
            .ok_or(DecodeError::Model("MDAT block index out of range"))?,
    )?;
    let mut index_buffers = Vec::with_capacity(em.index_buffers.len());
    for desc in &em.index_buffers {
        let raw = resource
            .get_block_by_index(desc.block_index)
            .ok_or(DecodeError::Model("MIDX block index out of range"))?;
        index_buffers.push(desc.decode(raw, false)?);
    }
    let mut surfaces = Vec::with_capacity(em.vertex_buffers.len());
    for desc in &em.vertex_buffers {
        let raw = resource
            .get_block_by_index(desc.block_index)
            .ok_or(DecodeError::Model("MVTX block index out of range"))?;
        let on_disk = desc.decode(raw, true)?;
        let positions = on_disk.positions().unwrap_or_default();
        let normals = on_disk
            .fields
            .iter()
            .find(|f| f.semantic_name == "NORMAL")
            .and_then(|attr| on_disk.normal_tangent(attr).ok())
            .map_or_else(Vec::new, |(normals, _)| normals);
        let can_host = !positions.is_empty()
            && normals.len() == positions.len()
            && uv_field(&on_disk.fields).is_some()
            && !desc.zstd;
        surfaces.push(Surface {
            positions,
            normals,
            can_host,
        });
    }
    let mut draws = Vec::new();
    for (scene_object, so) in SceneObject::parse_all(&mdat)?.iter().enumerate() {
        for (call, dc) in so.draw_calls.iter().enumerate() {
            let Some(ib) = index_buffers.get(dc.index_buffer) else {
                continue;
            };
            if dc.vertex_buffer >= surfaces.len() {
                continue;
            }
            draws.push(Draw {
                scene_object,
                call,
                vertex_buffer: dc.vertex_buffer,
                index_buffer: dc.index_buffer,
                single_stream: dc.vertex_buffers.len() == 1,
                material: dc.material.clone(),
                indices: ib.read_indices(dc.start_index, dc.index_count, dc.base_vertex)?,
            });
        }
    }
    Ok(MeshGeometry {
        lod_mask,
        surfaces,
        draws,
        index_sizes: index_buffers.iter().map(|ib| ib.element_size).collect(),
    })
}

/// What the picture sees of one mesh. Other meshes do not count: a weapon or
/// prop sits wherever its bind pose left it, often at the model's origin, and
/// would hide the body behind it. Nor do draw calls that cannot take the
/// picture: an earlier decal lies on the surface and hides nothing.
fn depth_map(
    mesh: &MeshGeometry,
    projection: &DecalProjection,
    is_host: &impl Fn(&str) -> bool,
) -> DepthMap {
    let mut map = DepthMap::new(DEPTH_MAP_SIZE, DEPTH_MAP_SIZE);
    for draw in mesh.draws.iter().filter(|draw| is_host(&draw.material)) {
        let surface = &mesh.surfaces[draw.vertex_buffer];
        for triangle in draw.indices.as_chunks::<3>().0 {
            let Some(corners) = corners_of(surface, *triangle, projection) else {
                continue;
            };
            // A surface turned away cannot hide one that faces the picture.
            if !surface.normals.is_empty() && facing(&corners, projection) <= 0.0 {
                continue;
            }
            map.draw(corners.map(|corner| corner.place), 0.0);
        }
    }
    map
}

fn corners_of(
    surface: &Surface,
    triangle: [u32; 3],
    projection: &DecalProjection,
) -> Option<[Corner; 3]> {
    let mut corners = [Corner {
        position: [0.0; 3],
        normal: [0.0; 3],
        place: [0.0; 3],
        facing: 0.0,
        parent: 0,
    }; 3];
    for (corner, &index) in corners.iter_mut().zip(&triangle) {
        let position = *surface.positions.get(index as usize)?;
        let normal = surface
            .normals
            .get(index as usize)
            .copied()
            .unwrap_or([0.0; 3]);
        *corner = Corner {
            position,
            normal,
            place: projection.project(position),
            facing: -dot(normalize(normal), projection.direction),
            parent: index,
        };
    }
    Some(corners)
}

/// How squarely a whole triangle faces the viewer.
fn facing(corners: &[Corner; 3], projection: &DecalProjection) -> f32 {
    let sum = add(add(corners[0].normal, corners[1].normal), corners[2].normal);
    -dot(normalize(sum), projection.direction)
}

/// The copies of one draw call's triangles under the picture.
fn cover(
    mesh: usize,
    draw: &Draw,
    surface: &Surface,
    depth: &DepthMap,
    projection: &DecalProjection,
    options: &DecalOptions,
) -> Addition {
    let mut addition = Addition {
        mesh,
        scene_object: draw.scene_object,
        draw_call: draw.call,
        vertex_buffer: draw.vertex_buffer,
        index_buffer: draw.index_buffer,
        material: draw.material.clone(),
        parents: Vec::new(),
        positions: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::new(),
        area: 0.0,
    };
    let mut welded: HashMap<(u32, u32, u32), u32> = HashMap::new();
    let [u0, v0, u1, v1] = options.uv_rect;
    for triangle in draw.indices.as_chunks::<3>().0 {
        let Some(corners) = corners_of(surface, *triangle, projection) else {
            continue;
        };
        if corners.iter().all(|c| c.facing < options.min_facing) {
            continue;
        }
        let outside = |axis: usize| {
            corners.iter().all(|c| c.place[axis] < 0.0)
                || corners.iter().all(|c| c.place[axis] > 1.0)
        };
        if outside(0) || outside(1) {
            continue;
        }
        let polygon = trim(&corners, options.min_facing);
        if polygon.len() < 3 {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let count = polygon.len() as f32;
        let centre =
            [0, 1, 2].map(|axis| polygon.iter().map(|c| c.place[axis]).sum::<f32>() / count);
        // Each corner is tested a little way in, so a neighbour that overlaps
        // the triangle's own edge does not hide it.
        let seen = |place: [f32; 3]| {
            let at = [0, 1, 2].map(|axis| place[axis] + (centre[axis] - place[axis]) * 0.1);
            depth
                .at(at[0], at[1])
                .is_some_and(|(nearest, _)| at[2] <= nearest + options.depth_tolerance)
        };
        if !seen(centre) && !polygon.iter().any(|corner| seen(corner.place)) {
            continue;
        }

        let mut ids = Vec::with_capacity(polygon.len());
        for corner in &polygon {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let key = (
                corner.parent,
                (corner.place[0].clamp(0.0, 1.0) * 65535.0).round() as u32,
                (corner.place[1].clamp(0.0, 1.0) * 65535.0).round() as u32,
            );
            let next = u32::try_from(addition.parents.len()).unwrap_or(u32::MAX);
            let id = *welded.entry(key).or_insert_with(|| {
                addition.parents.push(corner.parent);
                addition.positions.push(add(
                    corner.position,
                    scale(normalize(corner.normal), options.offset),
                ));
                addition.uvs.push([
                    u0 + corner.place[0].clamp(0.0, 1.0) * (u1 - u0),
                    v0 + corner.place[1].clamp(0.0, 1.0) * (v1 - v0),
                ]);
                next
            });
            ids.push(id);
        }
        for i in 1..ids.len() - 1 {
            let fan = [ids[0], ids[i], ids[i + 1]];
            if fan[0] == fan[1] || fan[1] == fan[2] || fan[0] == fan[2] {
                continue;
            }
            addition.indices.extend_from_slice(&fan);
            addition.area += edge(
                [polygon[0].place[0], polygon[0].place[1]],
                [polygon[i].place[0], polygon[i].place[1]],
                [polygon[i + 1].place[0], polygon[i + 1].place[1]],
            )
            .abs()
                / 2.0;
        }
    }
    addition
}

/// Trim a triangle to the unit square of the picture, and to the part of it
/// that faces the viewer by at least `min_facing`. Facing runs smoothly over
/// the surface, corner to corner, so that edge is a curve across triangles and
/// not a row of whole ones.
fn trim(corners: &[Corner; 3], min_facing: f32) -> Vec<Corner> {
    let mut polygon = corners.to_vec();
    // Signed distance inside each of the five limits.
    let inside: [&dyn Fn(&Corner) -> f32; 5] = [
        &|c| c.place[0],
        &|c| 1.0 - c.place[0],
        &|c| c.place[1],
        &|c| 1.0 - c.place[1],
        &|c| c.facing - min_facing,
    ];
    for distance in inside {
        if polygon.is_empty() {
            break;
        }
        let mut kept = Vec::with_capacity(polygon.len() + 1);
        for (i, current) in polygon.iter().enumerate() {
            let next = &polygon[(i + 1) % polygon.len()];
            let (a, b) = (distance(current), distance(next));
            if a >= 0.0 {
                kept.push(*current);
            }
            if (a >= 0.0) != (b >= 0.0) {
                kept.push(between(current, next, a / (a - b)));
            }
        }
        polygon = kept;
    }
    polygon
}

/// The point a share `t` of the way from `a` to `b`. It copies its stored
/// attributes from the nearer end.
fn between(a: &Corner, b: &Corner, t: f32) -> Corner {
    let mix = |x: [f32; 3], y: [f32; 3]| [0, 1, 2].map(|axis| x[axis] + (y[axis] - x[axis]) * t);
    Corner {
        position: mix(a.position, b.position),
        normal: mix(a.normal, b.normal),
        place: mix(a.place, b.place),
        facing: a.facing + (b.facing - a.facing) * t,
        parent: if t < 0.5 { a.parent } else { b.parent },
    }
}

/// Append one draw call's copies to its buffers and register the new draw.
#[allow(clippy::too_many_lines)]
fn apply(vmdl_bytes: &[u8], addition: &Addition, material: &str) -> Result<Vec<u8>, DecodeError> {
    let (resource, embedded) = parse_embedded(vmdl_bytes)?;
    let em = embedded
        .get(addition.mesh)
        .ok_or(DecodeError::Model("embedded mesh out of range"))?;
    let vb_desc = em
        .vertex_buffers
        .get(addition.vertex_buffer)
        .ok_or(DecodeError::Model("vertex buffer out of range"))?;
    let ib_desc = em
        .index_buffers
        .get(addition.index_buffer)
        .ok_or(DecodeError::Model("index buffer out of range"))?;
    if vb_desc.zstd || ib_desc.zstd {
        return Err(DecodeError::Model("ZSTD buffers are not supported"));
    }

    // Vertices: each copy starts as its parent's stored bytes.
    let mut vertices = vb_desc.decode(
        resource
            .get_block_by_index(vb_desc.block_index)
            .ok_or(DecodeError::Model("MVTX block index out of range"))?,
        true,
    )?;
    let stride = vertices.element_size;
    let old_vertices = vertices.element_count;
    let mut copies = OnDiskBuffer {
        data: Vec::with_capacity(addition.parents.len() * stride),
        element_count: addition.parents.len(),
        element_size: stride,
        fields: vertices.fields.clone(),
    };
    for &parent in &addition.parents {
        let start = parent as usize * stride;
        copies.data.extend_from_slice(
            vertices
                .data
                .get(start..start + stride)
                .ok_or(DecodeError::Model("decal parent vertex out of range"))?,
        );
    }
    copies.write_positions(&addition.positions)?;
    let uv = uv_field(&copies.fields)
        .ok_or(DecodeError::Model("vertex buffer has no TEXCOORD"))?
        .clone();
    copies.write_vector2(&uv, &addition.uvs)?;
    vertices.data.extend_from_slice(&copies.data);
    let new_vertices = old_vertices + addition.parents.len();

    // Indices: the copies' triangles go after the existing ones.
    let mut indices = ib_desc
        .decode(
            resource
                .get_block_by_index(ib_desc.block_index)
                .ok_or(DecodeError::Model("MIDX block index out of range"))?,
            false,
        )?
        .data;
    let old_indices = ib_desc.element_count;
    for &index in &addition.indices {
        let absolute = old_vertices + index as usize;
        match ib_desc.element_size {
            2 => indices.extend_from_slice(
                &u16::try_from(absolute)
                    .map_err(|_| DecodeError::Model("decal vertex past a 16-bit index"))?
                    .to_le_bytes(),
            ),
            4 => indices.extend_from_slice(
                &u32::try_from(absolute)
                    .map_err(|_| DecodeError::Model("decal vertex past a 32-bit index"))?
                    .to_le_bytes(),
            ),
            _ => return Err(DecodeError::Model("unsupported index width")),
        }
    }
    let new_indices = old_indices + addition.indices.len();

    // CTRL: the new counts, and both buffers stored raw. A meshopt re-encode
    // is not byte-compatible with the engine's decoder.
    let ctrl_index = resource
        .blocks()
        .iter()
        .position(|block| &block.kind == b"CTRL")
        .ok_or(DecodeError::Model("model has no CTRL block"))?;
    let buffer_path = |list: &str, index: usize, key: &str| {
        vec![
            Seg::Key("embedded_meshes".into()),
            Seg::Index(addition.mesh),
            Seg::Key(list.into()),
            Seg::Index(index),
            Seg::Key(key.into()),
        ]
    };
    let vb_path = |key: &str| buffer_path("m_vertexBuffers", addition.vertex_buffer, key);
    let ib_path = |key: &str| buffer_path("m_indexBuffers", addition.index_buffer, key);
    let ctrl = kv3::set_scalars(
        resource
            .get_block_by_index(ctrl_index)
            .ok_or(DecodeError::Model("CTRL block out of range"))?,
        &[
            (vb_path("m_nElementCount"), as_i64(new_vertices)?),
            (ib_path("m_nElementCount"), as_i64(new_indices)?),
        ],
    )?;
    let ctrl = kv3::set_bools(
        &ctrl,
        &[
            (vb_path("m_bMeshoptCompressed"), false),
            (ib_path("m_bMeshoptCompressed"), false),
        ],
    )?;

    // MDAT: every draw that reads the vertex buffer learns its new size, then
    // a copy of the host draw call takes the new index range and material.
    let mdat_bytes = resource
        .get_block_by_index(em.data_block)
        .ok_or(DecodeError::Model("MDAT block index out of range"))?;
    let (mdat, mdat_flags) = kv3::decode_flagged(mdat_bytes)?;
    let scene_objects = SceneObject::parse_all(&mdat)?;
    let draw_path = |scene_object: usize, draw_call: usize, key: &str| {
        vec![
            Seg::Key("m_sceneObjects".into()),
            Seg::Index(scene_object),
            Seg::Key("m_drawCalls".into()),
            Seg::Index(draw_call),
            Seg::Key(key.into()),
        ]
    };
    let mut resized = Vec::new();
    for (so_index, so) in scene_objects.iter().enumerate() {
        for (dc_index, dc) in so.draw_calls.iter().enumerate() {
            if dc.vertex_buffers.contains(&addition.vertex_buffer)
                && dc.vertex_count != new_vertices
            {
                resized.push((
                    draw_path(so_index, dc_index, "m_nVertexCount"),
                    as_i64(new_vertices)?,
                ));
            }
        }
    }
    let mdat_resized = if resized.is_empty() {
        mdat_bytes.to_vec()
    } else {
        kv3::set_scalars(mdat_bytes, &resized)?
    };
    let draw_calls = locate_draw_calls(&mdat)?;
    let template = draw_calls
        .iter()
        .find(|(so, dc, _)| (*so, *dc) == (addition.scene_object, addition.draw_call))
        .map(|(_, _, value)| *value)
        .ok_or(DecodeError::Model("decal host draw call not found"))?;
    let mut clone = template.clone();
    let material_key = if clone.get("m_material").is_some() {
        "m_material"
    } else if clone.get("m_pMaterial").is_some() {
        "m_pMaterial"
    } else {
        return Err(DecodeError::Model("draw call has no material field"));
    };
    bake_draw_call(
        &mut clone,
        &DrawCallGroup {
            material: material.to_string(),
            start_index: old_indices,
            index_count: addition.indices.len(),
            vertex_start: 0,
            vertex_end: new_vertices,
        },
        material_key,
    );
    let draws_in_object = scene_objects
        .get(addition.scene_object)
        .map_or(0, |so| so.draw_calls.len());
    let draw_calls_path = [
        Seg::Key("m_sceneObjects".into()),
        Seg::Index(addition.scene_object),
        Seg::Key("m_drawCalls".into()),
    ];
    // The copy keeps the host's type flags: the engine refuses a material
    // reference stored as a plain string.
    let mut host_path = draw_calls_path.to_vec();
    host_path.push(Seg::Index(addition.draw_call));
    let clone_flags: ValueFlags = mdat_flags
        .iter()
        .filter_map(|(path, flag)| Some((path.strip_prefix(host_path.as_slice())?.to_vec(), *flag)))
        .collect();
    let mdat_new = kv3::insert_array_element_adding_flagged(
        &mdat_resized,
        &draw_calls_path,
        draws_in_object,
        &clone,
        &clone_flags,
    )?;

    // Block order and count are kept, so the indices stay valid across each
    // rebuild.
    let mut bytes = vmdl_bytes.to_vec();
    for (index, payload) in [
        (ctrl_index, ctrl),
        (em.data_block, mdat_new),
        (vb_desc.block_index, vertices.data),
        (ib_desc.block_index, indices),
    ] {
        bytes = Resource::parse(&bytes)?.rebuild_with_block(index, &payload)?;
    }
    Ok(bytes)
}

fn as_i64(n: usize) -> Result<i64, DecodeError> {
    i64::try_from(n).map_err(|_| DecodeError::Model("count exceeds i64 range"))
}

/// Twice the signed area of the triangle `a`, `b`, `c`.
fn edge(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: [f32; 3], by: f32) -> [f32; 3] {
    [a[0] * by, a[1] * by, a[2] * by]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let length = dot(a, a).sqrt();
    if length > 1e-12 {
        scale(a, 1.0 / length)
    } else {
        [0.0; 3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture one unit square at the origin, seen along +X: across is -Y,
    /// down is -Z.
    fn projection() -> DecalProjection {
        DecalProjection {
            origin: [0.0, 0.0, 0.0],
            right: [0.0, -1.0, 0.0],
            down: [0.0, 0.0, -1.0],
            direction: [1.0, 0.0, 0.0],
        }
    }

    fn corner(across: f32, down: f32, parent: u32) -> Corner {
        Corner {
            position: [0.0, -across, -down],
            normal: [-1.0, 0.0, 0.0],
            place: [across, down, 0.0],
            facing: 1.0,
            parent,
        }
    }

    fn area(polygon: &[Corner]) -> f32 {
        (1..polygon.len() - 1)
            .map(|i| {
                edge(
                    [polygon[0].place[0], polygon[0].place[1]],
                    [polygon[i].place[0], polygon[i].place[1]],
                    [polygon[i + 1].place[0], polygon[i + 1].place[1]],
                )
                .abs()
                    / 2.0
            })
            .sum()
    }

    #[test]
    fn projection_measures_across_down_and_depth() {
        let place = projection().project([3.0, -0.25, -0.75]);
        assert!((place[0] - 0.25).abs() < 1e-6);
        assert!((place[1] - 0.75).abs() < 1e-6);
        assert!((place[2] - 3.0).abs() < 1e-6);
    }

    #[test]
    fn a_triangle_inside_the_picture_is_kept_whole() {
        let corners = [
            corner(0.2, 0.2, 0),
            corner(0.8, 0.2, 1),
            corner(0.2, 0.8, 2),
        ];
        let polygon = trim(&corners, 0.4);
        assert_eq!(polygon.len(), 3);
        assert!((area(&polygon) - 0.18).abs() < 1e-6);
    }

    /// A triangle over the picture's corner keeps only the part inside, and
    /// each new corner copies the nearer end of the edge it sits on.
    #[test]
    fn a_triangle_across_the_edge_is_trimmed() {
        let corners = [
            corner(0.5, 0.5, 0),
            corner(1.5, 0.5, 1),
            corner(0.5, 1.5, 2),
        ];
        let polygon = trim(&corners, 0.4);
        // The inside part is the unit square's quarter from (0.5, 0.5).
        assert!((area(&polygon) - 0.25).abs() < 1e-6);
        assert!(polygon
            .iter()
            .all(|c| (0.0..=1.0).contains(&c.place[0]) && (0.0..=1.0).contains(&c.place[1])));
        let on_right_edge = polygon
            .iter()
            .find(|c| (c.place[0] - 1.0).abs() < 1e-6 && (c.place[1] - 0.5).abs() < 1e-6)
            .expect("a corner where the top edge leaves the picture");
        assert_eq!(on_right_edge.parent, 1);
    }

    #[test]
    fn a_triangle_outside_the_picture_is_dropped() {
        let corners = [
            corner(1.2, 0.2, 0),
            corner(1.8, 0.2, 1),
            corner(1.2, 0.8, 2),
        ];
        assert!(trim(&corners, 0.4).len() < 3);
    }

    /// Where the surface turns away the picture stops along a line between
    /// the corners, not at a triangle's edge.
    #[test]
    fn a_triangle_that_turns_away_is_cut_where_it_turns() {
        let mut corners = [
            corner(0.2, 0.2, 0),
            corner(0.8, 0.2, 1),
            corner(0.2, 0.8, 2),
        ];
        // Head on at the first corner, edge on at the other two.
        corners[1].facing = 0.0;
        corners[2].facing = 0.0;
        let polygon = trim(&corners, 0.5);
        // The half nearer the first corner by distance is a quarter by area.
        assert!((area(&polygon) - 0.18 / 4.0).abs() < 1e-6);
        assert!(polygon.iter().all(|c| c.facing >= 0.5 - 1e-6));
        corners[0].facing = 0.2;
        assert!(trim(&corners, 0.5).len() < 3);
    }

    #[test]
    fn the_depth_map_keeps_the_nearest_surface() {
        let mut map = DepthMap::new(8, 8);
        let full = |depth: f32| [[-1.0, -1.0, depth], [3.0, -1.0, depth], [-1.0, 3.0, depth]];
        map.draw(full(5.0), 0.5);
        map.draw(full(2.0), 0.9);
        map.draw(full(7.0), 0.1);
        let (depth, shade) = map.at(0.5, 0.5).expect("covered");
        assert!((depth - 2.0).abs() < 1e-6);
        assert!((shade - 0.9).abs() < 1e-6);
        assert!(map.at(1.5, 0.5).is_none());
        assert!(DepthMap::new(4, 4).at(0.5, 0.5).is_none());
    }
}
