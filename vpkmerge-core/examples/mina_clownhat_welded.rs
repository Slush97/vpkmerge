// Clown hat via the PROVEN no-new-draw-call path: weld the hat into Mina's
// existing `mina_head` draw call (the box approach that loaded + tracked the
// head), and color it by painting clown swatches into UNUSED gutter texels of
// her face albedo, with the hat verts UV'd to those swatches. Zero KV3 array
// growth, zero new material; her face texels stay untouched.
//
// usage: cargo run --release --example mina_clownhat_welded -- <pak01_dir.vpk> <hat.glb> <out_dir.vpk>
//   HAT_WIDTH, HAT_RAISE, HAT_YAW env vars tune fit.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use morphic::model::{
    replace_mesh_group_uncompressed, EditedPrimitive, PrimitiveSelection, VertexBuffer,
};
use morphic::{Image, ImageData};
use vpkmerge_core::read_vpk_entry;
use vpkmerge_core::soul_import_clone::{
    base_color_factor, glb_json, material_index_by_name, read_glb_primitives,
};
use vpkmerge_core::SoulOrient;

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";
const ALBEDO_C: &str =
    "models/heroes_wip/vampirebat/materials/mina_head_material__color_png_77d536c8.vtex_c";
const HEAD_BONE: u16 = 16;
const GRID: usize = 256; // UV coverage grid for gutter finding

fn envf(k: &str, d: f32) -> f32 {
    std::env::var(k)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(d)
}

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let pak = a.next().context("arg1: pak01_dir.vpk")?;
    let glb_path = a.next().context("arg2: hat.glb")?;
    let out = a.next().context("arg3: out_dir.vpk")?;
    // Seat the brim at the head MESH crown (hair top, z~94), nestled 2 units in.
    // width ~= head width so the brim reads as a hat, not an oversized cone. The
    // old -7 raise buried the brim at face level (head-bone z), and width 18 was
    // double the head; both put the "hat" wrapping her face instead of on top.
    let width = envf("HAT_WIDTH", 13.0);
    let raise = envf("HAT_RAISE", -2.0);
    let yaw = envf("HAT_YAW", 0.0);

    let model_bytes = read_vpk_entry(&pak, ENTRY)?;
    let model = morphic::model::decode(&model_bytes)?;
    let anchor = bone_pos(&model, HEAD_BONE);

    // --- head part + mina_head primitive (face) ---
    let head = model.meshes.iter().find(|m| m.name == "head").unwrap();
    let shared = &head.vertex_buffers[0];
    // Crown = the actual top of the head MESH (hair pouf), not the head_end bone
    // (which sits inside the skull). The hat seats on the hair, not the scalp.
    let crown_z = shared
        .positions
        .iter()
        .map(|p| p[2])
        .fold(f32::NEG_INFINITY, f32::max);
    eprintln!(
        "head mesh top (hair) z = {crown_z:.1}; head bone z = {:.1}",
        anchor[2]
    );
    let (face_idx, face_prim) = head
        .primitives
        .iter()
        .enumerate()
        .find(|(_, p)| {
            p.material
                .rsplit('/')
                .next()
                .is_some_and(|f| f.starts_with("mina_head") && !f.contains("_ui"))
        })
        .ok_or_else(|| anyhow!("no mina_head primitive"))?;

    // --- read hat GLB, group by color ---
    let doc = glb_json(&std::fs::read(&glb_path)?)?;
    let glb = std::fs::read(&glb_path)?;
    let (prims, _) = read_glb_primitives(&glb, SoulOrient::YUp, None)?;
    let mat_index = material_index_by_name(&doc);
    let mut groups: Vec<(Option<String>, Vec<usize>, [f64; 4])> = Vec::new();
    for (pi, p) in prims.iter().enumerate() {
        if let Some(g) = groups.iter_mut().find(|g| g.0 == p.material_name) {
            g.1.push(pi);
        } else {
            let col = p
                .material_name
                .as_ref()
                .and_then(|m| mat_index.get(m))
                .map_or([0.8, 0.8, 0.8, 1.0], |&mi| base_color_factor(&doc, mi));
            groups.push((p.material_name.clone(), vec![pi], col));
        }
    }
    let n = groups.len();
    eprintln!("hat: {n} color group(s)");

    // --- merge geometry (Y-up GLB -> Source Z-up), track group per vertex ---
    let mut hat = VertexBuffer {
        texcoords: vec![Vec::new()],
        ..VertexBuffer::default()
    };
    let mut hat_idx: Vec<u32> = Vec::new();
    let mut vgroup: Vec<usize> = Vec::new();
    for (gi, g) in groups.iter().enumerate() {
        for &pi in &g.1 {
            let vb = &prims[pi].vertex_buffer;
            let base = hat.positions.len() as u32;
            for v in &vb.positions {
                hat.positions.push([v[0], v[2], -v[1]]);
            }
            for v in &vb.normals {
                hat.normals.push([v[0], v[2], -v[1]]);
            }
            for _ in 0..vb.positions.len() {
                vgroup.push(gi);
            }
            hat_idx.extend(prims[pi].indices.iter().map(|&i| base + i));
        }
    }
    hat.element_count = hat.positions.len();

    // --- fit: scale to width, center XY on bone, base at crown+raise ---
    let (mut mn, mut mx) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for p in &hat.positions {
        for k in 0..3 {
            mn[k] = mn[k].min(p[k]);
            mx[k] = mx[k].max(p[k]);
        }
    }
    let span = (mx[0] - mn[0]).max(mx[1] - mn[1]).max(1e-4);
    let scale = width / span;
    let c = [(mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5];
    for p in &mut hat.positions {
        p[0] = (p[0] - c[0]) * scale + anchor[0];
        p[1] = (p[1] - c[1]) * scale + anchor[1];
        p[2] *= scale;
    }
    // Orientation: pitch (about model X), roll (about model Y), yaw (about Z),
    // composed Rz*Ry*Rx, applied about the head anchor (positions) and origin
    // (normals). In bind pose the hat's up is model +Z, aligned with the head's
    // up, so a correct bind-pose orientation is correct in every animated pose.
    let pitch = envf("HAT_PITCH", 0.0);
    let roll = envf("HAT_ROLL", 0.0);
    let rot = euler_zyx(pitch, roll, yaw);
    for p in &mut hat.positions {
        let v = [p[0] - anchor[0], p[1] - anchor[1], p[2] - anchor[2]];
        let r = mat3_mul(&rot, &v);
        *p = [r[0] + anchor[0], r[1] + anchor[1], r[2] + anchor[2]];
    }
    for nv in &mut hat.normals {
        *nv = mat3_mul(&rot, nv);
    }
    // Re-center XY on the head and re-seat the base at crown+raise, so rotation
    // only reorients (never drifts the hat off the head).
    let (mut cx, mut cy, mut lo) = (0.0f32, 0.0f32, f32::INFINITY);
    for p in &hat.positions {
        cx += p[0];
        cy += p[1];
        lo = lo.min(p[2]);
    }
    cx /= hat.positions.len() as f32;
    cy /= hat.positions.len() as f32;
    let (sx, sy, sz) = (anchor[0] - cx, anchor[1] - cy, crown_z + raise - lo);
    for p in &mut hat.positions {
        p[0] += sx;
        p[1] += sy;
        p[2] += sz;
    }

    // --- find unused gutter cells in the face UV layout ---
    let mut used = vec![false; GRID * GRID];
    for tri in face_prim.indices.chunks_exact(3) {
        let uv: Vec<[f32; 2]> = tri
            .iter()
            .map(|&i| shared.texcoords[0][i as usize])
            .collect();
        rasterize_tri(&mut used, &uv);
    }
    dilate(&mut used, 2);
    let free = find_free_cells(&used, n)
        .ok_or_else(|| anyhow!("could not find {n} free gutter cells in the face UV layout"))?;
    eprintln!("free gutter cells (grid {GRID}): {:?}", free);

    // --- paint swatches into the face albedo (untouched elsewhere) ---
    let albedo_c = read_vpk_entry(&pak, ALBEDO_C)?;
    let img = morphic::decode(&albedo_c).map_err(|e| anyhow!("decode albedo: {e}"))?;
    let (w, h) = (img.width as usize, img.height as usize);
    let ImageData::Rgba8(mut px) = img.data else {
        return Err(anyhow!("albedo is HDR"));
    };
    let cell_w = w / GRID;
    let cell_h = h / GRID;
    for (gi, &(cx, cy)) in free.iter().enumerate() {
        let col = groups[gi].2;
        let rgb = [
            to_srgb_u8(col[0]),
            to_srgb_u8(col[1]),
            to_srgb_u8(col[2]),
            255,
        ];
        for ty in cy * cell_h..(cy + 1) * cell_h {
            for tx in cx * cell_w..(cx + 1) * cell_w {
                let o = (ty * w + tx) * 4;
                px[o..o + 4].copy_from_slice(&rgb);
            }
        }
    }
    let new_albedo = morphic::replace_mip_chain(
        &albedo_c,
        &Image {
            width: w as u32,
            height: h as u32,
            data: ImageData::Rgba8(px),
        },
    )
    .map_err(|e| anyhow!("re-encode albedo: {e}"))?;

    // --- assign hat UVs to each group's swatch-cell center ---
    let swatch_uv: Vec<[f32; 2]> = free
        .iter()
        .map(|&(cx, cy)| {
            [
                (cx as f32 + 0.5) / GRID as f32,
                (cy as f32 + 0.5) / GRID as f32,
            ]
        })
        .collect();
    hat.texcoords[0] = vgroup.iter().map(|&g| swatch_uv[g]).collect();
    let vc = hat.element_count;
    hat.joints = vec![[HEAD_BONE; 4]; vc];
    hat.weights = vec![[1.0, 0.0, 0.0, 0.0]; vc];

    // --- weld: compact the face primitive + append the hat, into mina_head ---
    let (mut donor_vb, mut donor_idx) = compact(shared, &face_prim.indices);
    let base = donor_vb.element_count as u32;
    append(&mut donor_vb, &hat, &hat_idx, base, &mut donor_idx);
    let donor = EditedPrimitive {
        mesh_name: Some("head".into()),
        material_name: Some(face_prim.material.clone()),
        vertex_buffer: donor_vb,
        indices: donor_idx,
    };
    let sel = PrimitiveSelection {
        mesh_index: head.mesh_index,
        primitive_index: face_idx,
    };
    let (edited, _) = replace_mesh_group_uncompressed(&model_bytes, &[sel], &[donor])
        .map_err(|e| anyhow!("weld: {e}"))?;
    morphic::model::decode(&edited).context("edited model failed to re-decode")?;

    vpkmerge_core::pack(
        &[
            (ENTRY, edited.as_slice()),
            (ALBEDO_C, new_albedo.as_slice()),
        ],
        &out,
    )?;
    eprintln!(
        "wrote {out}: model + overridden face albedo (hat welded into mina_head, {vc} hat verts)"
    );
    Ok(())
}

/// Rz(yaw) * Ry(roll) * Rx(pitch), degrees, as a row-major 3x3.
fn euler_zyx(pitch: f32, roll: f32, yaw: f32) -> [[f32; 3]; 3] {
    let (sx, cx) = pitch.to_radians().sin_cos();
    let (sy, cy) = roll.to_radians().sin_cos();
    let (sz, cz) = yaw.to_radians().sin_cos();
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}

fn mat3_mul(m: &[[f32; 3]; 3], v: &[f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn bone_pos(m: &morphic::model::Model, b: u16) -> [f32; 3] {
    let t = m.skeleton.bones[b as usize].global_bind.m;
    [t[12], t[13], t[14]]
}

fn to_srgb_u8(lin: f64) -> u8 {
    let s = if lin <= 0.003_130_8 {
        12.92 * lin
    } else {
        1.055 * lin.powf(1.0 / 2.4) - 0.055
    };
    (s.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Mark grid cells whose center falls inside the UV triangle (wrapped to [0,1)).
fn rasterize_tri(used: &mut [bool], uv: &[[f32; 2]]) {
    let w = |v: f32| v.rem_euclid(1.0);
    let p: Vec<[f32; 2]> = uv.iter().map(|t| [w(t[0]), w(t[1])]).collect();
    let (mut mn, mut mx) = ([1.0f32, 1.0], [0.0f32, 0.0]);
    for q in &p {
        for k in 0..2 {
            mn[k] = mn[k].min(q[k]);
            mx[k] = mx[k].max(q[k]);
        }
    }
    // Skip triangles that wrap across the seam (bbox > 0.5): mark their whole bbox.
    let span_wrap = (mx[0] - mn[0]) > 0.5 || (mx[1] - mn[1]) > 0.5;
    let x0 = (mn[0] * GRID as f32) as usize;
    let x1 = ((mx[0] * GRID as f32).ceil() as usize).min(GRID);
    let y0 = (mn[1] * GRID as f32) as usize;
    let y1 = ((mx[1] * GRID as f32).ceil() as usize).min(GRID);
    for gy in y0..y1 {
        for gx in x0..x1 {
            let c = [
                (gx as f32 + 0.5) / GRID as f32,
                (gy as f32 + 0.5) / GRID as f32,
            ];
            if span_wrap || point_in_tri(c, &p) {
                used[gy * GRID + gx] = true;
            }
        }
    }
}

fn point_in_tri(pt: [f32; 2], t: &[[f32; 2]]) -> bool {
    let d = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
        (a[0] - c[0]) * (b[1] - c[1]) - (b[0] - c[0]) * (a[1] - c[1])
    };
    let d1 = d(pt, t[0], t[1]);
    let d2 = d(pt, t[1], t[2]);
    let d3 = d(pt, t[2], t[0]);
    let neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
    let pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
    !(neg && pos)
}

fn dilate(used: &mut [bool], r: usize) {
    for _ in 0..r {
        let snap = used.to_vec();
        for gy in 0..GRID {
            for gx in 0..GRID {
                if snap[gy * GRID + gx] {
                    continue;
                }
                let near = [(0i32, 1i32), (0, -1), (1, 0), (-1, 0)]
                    .iter()
                    .any(|&(dx, dy)| {
                        let nx = gx as i32 + dx;
                        let ny = gy as i32 + dy;
                        nx >= 0
                            && nx < GRID as i32
                            && ny >= 0
                            && ny < GRID as i32
                            && snap[ny as usize * GRID + nx as usize]
                    });
                if near {
                    used[gy * GRID + gx] = true;
                }
            }
        }
    }
}

/// Pick `n` free cells, spaced out so each swatch is isolated from the others.
fn find_free_cells(used: &[bool], n: usize) -> Option<Vec<(usize, usize)>> {
    let mut frees: Vec<(usize, usize)> = Vec::new();
    for gy in 0..GRID {
        for gx in 0..GRID {
            if !used[gy * GRID + gx] {
                frees.push((gx, gy));
            }
        }
    }
    if frees.len() < n {
        return None;
    }
    // Greedy farthest-point spread for separation.
    let mut chosen = vec![frees[0]];
    while chosen.len() < n {
        let next = frees.iter().copied().max_by_key(|&(x, y)| {
            chosen
                .iter()
                .map(|&(cx, cy)| (x as i64 - cx as i64).pow(2) + (y as i64 - cy as i64).pow(2))
                .min()
                .unwrap_or(0)
        })?;
        chosen.push(next);
    }
    Some(chosen)
}

/// Compact a primitive into a standalone vertex buffer (its referenced verts
/// only), indices rebased to 0..n. Preserves all attribute channels.
fn compact(shared: &VertexBuffer, indices: &[u32]) -> (VertexBuffer, Vec<u32>) {
    let mut map: HashMap<u32, u32> = HashMap::new();
    let mut order = Vec::new();
    let mut reb = Vec::with_capacity(indices.len());
    for &gi in indices {
        let id = *map.entry(gi).or_insert_with(|| {
            let i = order.len() as u32;
            order.push(gi);
            i
        });
        reb.push(id);
    }
    let nn = shared.element_count;
    let p3 = |s: &[[f32; 3]]| {
        if s.len() == nn {
            order.iter().map(|&g| s[g as usize]).collect()
        } else {
            Vec::new()
        }
    };
    let p4 = |s: &[[f32; 4]]| {
        if s.len() == nn {
            order.iter().map(|&g| s[g as usize]).collect()
        } else {
            Vec::new()
        }
    };
    let vb = VertexBuffer {
        element_count: order.len(),
        stride: shared.stride,
        positions: p3(&shared.positions),
        normals: p3(&shared.normals),
        tangents: p4(&shared.tangents),
        texcoords: shared
            .texcoords
            .iter()
            .map(|ch| {
                if ch.len() == nn {
                    order.iter().map(|&g| ch[g as usize]).collect()
                } else {
                    Vec::new()
                }
            })
            .collect(),
        colors: shared
            .colors
            .iter()
            .map(|ch| {
                if ch.len() == nn {
                    order.iter().map(|&g| ch[g as usize]).collect()
                } else {
                    Vec::new()
                }
            })
            .collect(),
        joints: if shared.joints.len() == nn {
            order.iter().map(|&g| shared.joints[g as usize]).collect()
        } else {
            Vec::new()
        },
        weights: if shared.weights.len() == nn {
            order.iter().map(|&g| shared.weights[g as usize]).collect()
        } else {
            Vec::new()
        },
        layout: shared.layout.clone(),
    };
    (vb, reb)
}

/// Append the hat onto a compacted donor, matching channel counts.
fn append(
    vb: &mut VertexBuffer,
    hat: &VertexBuffer,
    hat_idx: &[u32],
    base: u32,
    idx: &mut Vec<u32>,
) {
    let has_tan = !vb.tangents.is_empty();
    let n_uv = vb.texcoords.len().max(1);
    let n_col = vb.colors.len();
    for i in 0..hat.element_count {
        vb.positions.push(hat.positions[i]);
        vb.normals.push(hat.normals[i]);
        if has_tan {
            vb.tangents.push([1.0, 0.0, 0.0, 1.0]);
        }
        for ch in 0..n_uv {
            vb.texcoords[ch].push(hat.texcoords.first().map_or([0.5, 0.5], |t| t[i]));
        }
        for ch in 0..n_col {
            vb.colors[ch].push([1.0; 4]);
        }
        vb.joints.push(hat.joints[i]);
        vb.weights.push(hat.weights[i]);
    }
    vb.element_count += hat.element_count;
    idx.extend(hat_idx.iter().map(|&i| base + i));
}
