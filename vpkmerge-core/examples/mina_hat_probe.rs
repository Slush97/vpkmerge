// PROBE: attach a "hat" (a visible box) to Mina's head by baking it into her
// body `.vmdl_c`, rigid-skinned to the head bone, and packing an addon VPK that
// overrides the base model.
//
// This answers the load-bearing feasibility question for hero-hat imports: does
// overriding a hero body model with ADDED geometry skinned to a single bone load
// in-game and track that bone? The box is deliberately crude (skin-textured, sits
// atop the head); we only care whether it loads and follows the head.
//
// Mechanism:
//   - Mina's `head` mesh part (block 4) carries 3 draw calls (eyes/hair/head) over
//     one shared vertex+index buffer, and its bone palette includes `head` (model
//     bone 16). We compact the `mina_head` primitive into a standalone donor, weld
//     a 12-unit box onto it skinned 100% to bone 16, and feed it back through
//     `replace_mesh_group_uncompressed` (raw buffers, m_bMeshoptCompressed=false,
//     the engine-loadable path). Eyes/hair are preserved untouched.
//
// usage: cargo run --release --example mina_hat_probe -- <pak01_dir.vpk> <out_dir.vpk>
use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use morphic::model::{
    replace_mesh_group_uncompressed, EditedPrimitive, PrimitiveSelection, VertexBuffer,
};
use vpkmerge_core::read_vpk_entry;

const ENTRY: &str = "models/heroes_wip/vampirebat/vampirebat.vmdl_c";
const HEAD_BONE: u16 = 16; // `head`, confirmed by mina_bone_probe
const HEAD_MAT: &str = "mina_head"; // the body-head material (not mina_head_ui)

fn main() -> Result<()> {
    let pak = std::env::args().nth(1).context("arg1: pak01_dir.vpk")?;
    let out = std::env::args().nth(2).context("arg2: out_dir.vpk")?;

    let bytes = read_vpk_entry(&pak, ENTRY)?;
    let model = morphic::model::decode(&bytes)?;

    // Locate the head part + its mina_head primitive.
    let head = model
        .meshes
        .iter()
        .find(|m| m.name == "head")
        .ok_or_else(|| anyhow!("no 'head' mesh part"))?;
    if head.vertex_buffers.len() != 1 {
        return Err(anyhow!(
            "head part has {} vertex buffers; expected 1",
            head.vertex_buffers.len()
        ));
    }
    let shared = &head.vertex_buffers[0];
    let (prim_idx, prim) = head
        .primitives
        .iter()
        .enumerate()
        .find(|(_, p)| {
            p.material
                .rsplit('/')
                .next()
                .is_some_and(|f| f.starts_with(HEAD_MAT) && !f.contains("_ui"))
        })
        .ok_or_else(|| anyhow!("no {HEAD_MAT} primitive in head part"))?;
    eprintln!(
        "head part: mesh_index={}, {} draw calls; target primitive [{prim_idx}] = {}",
        head.mesh_index,
        head.primitives.len(),
        prim.material
    );

    // Compact the target primitive into a standalone vertex buffer (only the verts
    // its triangles reference), then weld the box on.
    let (mut donor_vb, mut donor_idx) = compact_primitive(shared, &prim.indices);
    let base = donor_vb.element_count as u32;
    let head_top = head_anchor(&model);
    eprintln!(
        "head anchor (model space): ({:.2}, {:.2}, {:.2}); welding box of 24 verts",
        head_top[0], head_top[1], head_top[2]
    );
    append_box(&mut donor_vb, &mut donor_idx, base, head_top);

    let donor = EditedPrimitive {
        mesh_name: Some("head".to_string()),
        material_name: Some(prim.material.clone()),
        vertex_buffer: donor_vb,
        indices: donor_idx,
    };
    let sel = PrimitiveSelection {
        mesh_index: head.mesh_index,
        primitive_index: prim_idx,
    };

    let (edited, report) = replace_mesh_group_uncompressed(&bytes, &[sel], &[donor])
        .map_err(|e| anyhow!("replace_mesh_group_uncompressed: {e}"))?;
    eprintln!(
        "rebuilt head part: {} -> {} verts, {} -> {} idx (uncompressed)",
        report.replaced_parts[0].old_vertex_count,
        report.replaced_parts[0].new_vertex_count,
        report.replaced_parts[0].old_index_count,
        report.replaced_parts[0].new_index_count,
    );

    // Structural sanity: the edited model must still decode and the head part must
    // have grown by exactly the box's 24 verts.
    let re = morphic::model::decode(&edited).context("edited model failed to re-decode")?;
    let re_head = re
        .meshes
        .iter()
        .find(|m| m.name == "head")
        .ok_or_else(|| anyhow!("head part vanished after edit"))?;
    let grew = re_head.vertex_buffers[0].element_count as i64 - shared.element_count as i64;
    eprintln!("re-decode OK: head verts grew by {grew} (expected 24)");

    vpkmerge_core::pack(&[(ENTRY, edited.as_slice())], &out)?;
    eprintln!("wrote {out}");
    Ok(())
}

/// Model-space anchor for the hat: above `head_end` so it reads as a hat sitting
/// on the crown rather than buried in the skull.
fn head_anchor(model: &morphic::model::Model) -> [f32; 3] {
    let bone = model
        .skeleton
        .bones
        .iter()
        .find(|b| b.name.eq_ignore_ascii_case("head_end"))
        .or_else(|| model.skeleton.bones.get(HEAD_BONE as usize));
    bone.map_or([-1.37, 0.0, 96.0], |b| {
        let t = b.global_bind.m;
        [t[12], t[13], t[14] + 4.0]
    })
}

/// 24-vertex axis-aligned box (flat per-face normals) centered at `c`, skinned
/// 100% to the head bone. Appended to `vb`/`idx` starting at vertex `base`.
fn append_box(vb: &mut VertexBuffer, idx: &mut Vec<u32>, base: u32, c: [f32; 3]) {
    const HX: f32 = 6.0;
    const HY: f32 = 6.0;
    const HZ: f32 = 7.0;
    // 6 faces: (normal, 4 corner offsets CCW)
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        (
            [0.0, 0.0, 1.0],
            [[-HX, -HY, HZ], [HX, -HY, HZ], [HX, HY, HZ], [-HX, HY, HZ]],
        ),
        (
            [0.0, 0.0, -1.0],
            [
                [-HX, HY, -HZ],
                [HX, HY, -HZ],
                [HX, -HY, -HZ],
                [-HX, -HY, -HZ],
            ],
        ),
        (
            [1.0, 0.0, 0.0],
            [[HX, -HY, -HZ], [HX, HY, -HZ], [HX, HY, HZ], [HX, -HY, HZ]],
        ),
        (
            [-1.0, 0.0, 0.0],
            [
                [-HX, -HY, HZ],
                [-HX, HY, HZ],
                [-HX, HY, -HZ],
                [-HX, -HY, -HZ],
            ],
        ),
        (
            [0.0, 1.0, 0.0],
            [[-HX, HY, HZ], [HX, HY, HZ], [HX, HY, -HZ], [-HX, HY, -HZ]],
        ),
        (
            [0.0, -1.0, 0.0],
            [
                [-HX, -HY, -HZ],
                [HX, -HY, -HZ],
                [HX, -HY, HZ],
                [-HX, -HY, HZ],
            ],
        ),
    ];
    let has_tan = !vb.tangents.is_empty();
    let n_uv = vb.texcoords.len().max(1);
    if vb.texcoords.is_empty() {
        vb.texcoords.push(Vec::new());
    }
    let n_col = vb.colors.len();
    for (face, (normal, corners)) in faces.iter().enumerate() {
        for corner in corners {
            vb.positions
                .push([c[0] + corner[0], c[1] + corner[1], c[2] + corner[2]]);
            vb.normals.push(*normal);
            if has_tan {
                vb.tangents.push([1.0, 0.0, 0.0, 1.0]);
            }
            for ch in 0..n_uv {
                vb.texcoords[ch].push([0.5, 0.5]);
            }
            for ch in 0..n_col {
                vb.colors[ch].push([1.0, 1.0, 1.0, 1.0]);
            }
            vb.joints.push([HEAD_BONE, HEAD_BONE, HEAD_BONE, HEAD_BONE]);
            vb.weights.push([1.0, 0.0, 0.0, 0.0]);
        }
        let q = base + (face as u32) * 4;
        idx.extend_from_slice(&[q, q + 1, q + 2, q, q + 2, q + 3]);
        vb.element_count += 4;
    }
}

/// Builds a standalone vertex buffer holding only the vertices `indices`
/// references, with `indices` rebased to 0..n. Preserves every attribute channel
/// present on the shared buffer so the result stays self-consistent for
/// `combine_primitive_sources`.
fn compact_primitive(shared: &VertexBuffer, indices: &[u32]) -> (VertexBuffer, Vec<u32>) {
    let mut map: HashMap<u32, u32> = HashMap::new();
    let mut order: Vec<u32> = Vec::new();
    let mut rebased = Vec::with_capacity(indices.len());
    for &gi in indices {
        let n = *map.entry(gi).or_insert_with(|| {
            let id = order.len() as u32;
            order.push(gi);
            id
        });
        rebased.push(n);
    }
    let n = shared.element_count;
    let pull3 = |src: &[[f32; 3]]| -> Vec<[f32; 3]> {
        if src.len() == n {
            order.iter().map(|&gi| src[gi as usize]).collect()
        } else {
            Vec::new()
        }
    };
    let pull4 = |src: &[[f32; 4]]| -> Vec<[f32; 4]> {
        if src.len() == n {
            order.iter().map(|&gi| src[gi as usize]).collect()
        } else {
            Vec::new()
        }
    };
    let vb = VertexBuffer {
        element_count: order.len(),
        stride: shared.stride,
        positions: pull3(&shared.positions),
        normals: pull3(&shared.normals),
        tangents: pull4(&shared.tangents),
        texcoords: shared
            .texcoords
            .iter()
            .map(|ch| {
                if ch.len() == n {
                    order.iter().map(|&gi| ch[gi as usize]).collect()
                } else {
                    Vec::new()
                }
            })
            .collect(),
        colors: shared
            .colors
            .iter()
            .map(|ch| {
                if ch.len() == n {
                    order.iter().map(|&gi| ch[gi as usize]).collect()
                } else {
                    Vec::new()
                }
            })
            .collect(),
        joints: if shared.joints.len() == n {
            order.iter().map(|&gi| shared.joints[gi as usize]).collect()
        } else {
            Vec::new()
        },
        weights: if shared.weights.len() == n {
            order
                .iter()
                .map(|&gi| shared.weights[gi as usize])
                .collect()
        } else {
            Vec::new()
        },
        layout: shared.layout.clone(),
    };
    (vb, rebased)
}
