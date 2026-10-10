//! Remove the geometry a set of bones owns, without touching topology.
//!
//! Selects every vertex whose skin weight lands (fully) on a bone whose name
//! starts with `<bone-prefix>`, then collapses those vertices to a single point
//! so their triangles become zero-area and stop rasterizing. Topology, index
//! buffers, draw calls, materials, and every other vertex attribute are
//! untouched, so this is a Tier 0 vertex-displacement edit (the proven in-place
//! path) rather than a container rebuild.
//!
//! Only safe when the flagged geometry is a separate connected component: if any
//! triangle mixes flagged and unflagged vertices the mesh would tear, so that is
//! checked and refused. Run `tail_analysis` first to see the split.
//!
//! Usage:
//!   strip_bone_geometry <in_dir.vpk> <entry> <bone-prefix> <out_dir.vpk>

use anyhow::{bail, Context, Result};
use std::path::Path;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 5 {
        eprintln!("usage: strip_bone_geometry <in_dir.vpk> <entry> <bone-prefix> <out_dir.vpk>");
        std::process::exit(2);
    }
    let (in_vpk, entry, prefix, out_vpk) = (&a[1], &a[2], &a[3], &a[4]);

    let vpk = valve_pak::open(Path::new(in_vpk)).context("open input vpk")?;
    let bytes = vpk
        .get_file(entry)
        .context("entry not found")?
        .read_all()
        .context("read entry")?;

    let skel = morphic::model::decode_skeleton(&bytes).context("decode skeleton")?;
    let targets: Vec<u16> = skel
        .bones
        .iter()
        .enumerate()
        .filter(|(_, b)| b.name.starts_with(prefix.as_str()))
        .map(|(i, _)| u16::try_from(i).unwrap())
        .collect();
    if targets.is_empty() {
        bail!("no bone name starts with {prefix:?}");
    }
    println!("matched {} bone(s) on prefix {prefix:?}", targets.len());

    let model = morphic::model::decode(&bytes).context("decode model")?;
    let vtargets = morphic::model::vertex_targets(&bytes).context("vertex targets")?;

    // Flag per vertex buffer, flattened in the same (mesh, local buffer) order
    // `vertex_targets` walks, so the two line up index-for-index.
    // `skel_joints` keeps each buffer's decoded (skeleton-space) bone indices so
    // the rebind below can locate the palette slot of a chosen bone.
    let mut flags: Vec<Vec<bool>> = Vec::new();
    let mut skel_joints: Vec<Vec<[u16; 4]>> = Vec::new();
    for mesh in &model.meshes {
        let base = flags.len();
        for vb in &mesh.vertex_buffers {
            let n = vb.positions.len();
            let mut f = vec![false; n];
            if !vb.joints.is_empty() && !vb.weights.is_empty() {
                for i in 0..n {
                    let (j4, w4) = (vb.joints[i], vb.weights[i]);
                    let mut w = 0f32;
                    for k in 0..4 {
                        if w4[k] > 0.0 && targets.contains(&j4[k]) {
                            w += w4[k];
                        }
                    }
                    // Full weight only: a vertex blended between the tail and
                    // the body belongs to the seam and must stay where it is.
                    f[i] = w >= 0.999;
                }
            }
            flags.push(f);
            skel_joints.push(vb.joints.clone());
        }
        // Refuse if any triangle straddles the split: that means the flagged
        // geometry is welded to the rest and collapsing would tear the mesh.
        for prim in &mesh.primitives {
            let f = &flags[base + prim.vertex_buffer];
            for tri in prim.indices.chunks_exact(3) {
                let c = tri.iter().filter(|&&v| f[v as usize]).count();
                if c != 0 && c != 3 {
                    bail!(
                        "triangle straddles the split in prim [{}]: flagged \
                         geometry is welded to the rest and cannot be collapsed \
                         without tearing",
                        prim.material
                    );
                }
            }
        }
    }
    if flags.len() != vtargets.len() {
        bail!(
            "decoded {} vertex buffer(s) but {} vertex target(s): cannot map \
             flags to block indices",
            flags.len(),
            vtargets.len()
        );
    }

    // Splice each affected buffer: collapse flagged verts onto the flagged
    // vertex nearest the body (max X = the tail root here), so anything the
    // rasterizer might still do with a degenerate triangle happens buried in
    // the mesh rather than out in the open.
    let mut out_bytes = bytes.clone();
    let mut total = 0usize;
    // Bind the collapsed cloud to the FIRST matched bone (the tail root): it is
    // the most body-anchored of the set, so the degenerate point stays buried at
    // the hip instead of being flung around by the cloth sim on the tip bones.
    let root_bone = targets[0];
    for ((t, f), sj) in vtargets.iter().zip(&flags).zip(&skel_joints) {
        if f.len() != t.vertex_count {
            bail!(
                "vertex-buffer order mismatch: block {} expects {} verts, \
                 decoded buffer has {}",
                t.block_index,
                t.vertex_count,
                f.len()
            );
        }
        let count = f.iter().filter(|x| **x).count();
        if count == 0 {
            continue;
        }
        if !t.editable {
            bail!(
                "buffer at block {} ({}) carries flagged geometry but is not \
                 displacement-editable",
                t.block_index,
                t.mesh_name
            );
        }
        let mut positions = morphic::model::read_vertex_positions(&out_bytes, t.block_index)
            .with_context(|| format!("read positions block {}", t.block_index))?;
        if positions.len() != f.len() {
            bail!(
                "buffer/flag length mismatch at block {} ({} vs {})",
                t.block_index,
                positions.len(),
                f.len()
            );
        }
        let anchor = (0..positions.len())
            .filter(|&i| f[i])
            .max_by(|&x, &y| positions[x][0].total_cmp(&positions[y][0]))
            .map(|i| positions[i])
            .expect("at least one flagged vertex");
        for i in 0..positions.len() {
            if f[i] {
                positions[i] = anchor;
            }
        }
        out_bytes = morphic::model::replace_vertex_positions(&out_bytes, t.block_index, &positions)
            .with_context(|| format!("splice block {}", t.block_index))?;

        // Coincident is not enough: each collapsed vertex still rides its own
        // bone, so as soon as those bones move independently (an animation, or
        // a cloth sim on the very bones we stripped) the vertices separate and
        // the "removed" geometry reappears as a moving wisp. Point all four
        // influences of every collapsed vertex at ONE shared bone so they hold
        // a single transform under every pose and stay zero-area forever. The
        // weights lane is untouched and still sums to 1.
        let mut joints = morphic::model::read_blend_indices(&out_bytes, t.block_index)
            .with_context(|| format!("read blend indices block {}", t.block_index))?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "block {} has flagged verts but no BLENDINDICES",
                    t.block_index
                )
            })?;
        if joints.len() != positions.len() {
            bail!("blend-index count mismatch at block {}", t.block_index);
        }
        // Find the mesh-local palette slot for `root_bone` by looking up a
        // vertex/slot whose decoded skeleton index is that bone; the on-disk
        // lane at the same position holds the local index. Guaranteed to be a
        // valid palette entry because it is already in use. Falls back to slot 0
        // of a collapsed vertex, which is likewise always a valid index.
        let pin = (0..sj.len())
            .filter(|&i| f[i])
            .find_map(|i| {
                (0..4)
                    .find(|&k| sj[i][k] == root_bone)
                    .map(|k| joints[i][k])
            })
            .unwrap_or_else(|| {
                let i = f.iter().position(|x| *x).expect("flagged vertex");
                joints[i][0]
            });
        for i in 0..joints.len() {
            if f[i] {
                joints[i] = [pin; 4];
            }
        }
        out_bytes = morphic::model::replace_blend_indices(&out_bytes, t.block_index, &joints)
            .with_context(|| format!("rebind block {}", t.block_index))?;

        println!(
            "  block {:>2} ({}): collapsed {count} / {} verts to {anchor:?}, \
             rebound to palette slot {pin}",
            t.block_index,
            t.mesh_name,
            positions.len()
        );
        total += count;
    }
    if total == 0 {
        bail!("nothing flagged: no vertex is fully weighted to {prefix:?}");
    }

    vpkmerge_core::pack(
        &[(entry.as_str(), out_bytes.as_slice())],
        Path::new(out_vpk),
    )
    .context("pack override vpk")?;
    println!("collapsed {total} vertices; wrote {out_vpk}");
    Ok(())
}
