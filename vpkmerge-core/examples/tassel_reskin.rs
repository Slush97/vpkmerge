//! Re-skin Holliday's boot fringe from the shin bone (`leg_lower_*`) onto the
//! tassel bones (`flaps_0_*`) that the FeModel cloth rig drives. This is the
//! MISSING HALF of `tassel_phys.rs`: that wires the sim to drive `flaps_0_*`, but
//! no vertex was bound to those bones, so nothing moved. Here we rebind the fringe.
//!
//! Selection (principled): a vertex is fringe iff (a) it belongs to a SMALL
//! connected component (the fringe is dozens of little quad-strip cards; the solid
//! boot/calf is one big island), (b) it is currently weighted to `leg_lower_L/R`,
//! and (c) the *nearest bone* to its model-space position is `flaps_0_L/R` (within
//! a sanity radius). Only the `leg_lower` influence lane is repointed; weights and
//! all other attributes are preserved. Re-skins LOD0 body only (block 0).
//!
//! Outputs (verify BEFORE the slow in-game test): /tmp/astro_reskin.vmdl_c and
//! /tmp/astro_reskin.glb (import to Blender, the `flaps_0_L` vgroup is now the
//! fringe). Pairs with `tassel_phys.rs --mode both` for the actual swinging addon.
//!
//! Usage: cargo run -p vpkmerge-core --example tassel_reskin -- <pak.vpk> [radius]

use morphic::model::{decode, decode_skeleton, invert_remap, remap_table, reskin_vertex_buffer};

const ENTRY: &str = "models/heroes_staging/astro/astro.vmdl_c";

fn blocks(b: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let bo = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
    let c = u32::from_le_bytes(b[12..16].try_into().unwrap()) as usize;
    let base = 8 + bo;
    (0..c)
        .map(|i| {
            let e = base + i * 12;
            let mut k = [0u8; 4];
            k.copy_from_slice(&b[e..e + 4]);
            let r = u32::from_le_bytes(b[e + 4..e + 8].try_into().unwrap()) as usize;
            let l = u32::from_le_bytes(b[e + 8..e + 12].try_into().unwrap()) as usize;
            (k, (e + 4) + r, l)
        })
        .collect()
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

fn main() -> anyhow::Result<()> {
    let pak = std::env::args().nth(1).expect("pak.vpk");
    let radius: f32 = std::env::args().nth(2).map_or(10.0, |s| s.parse().unwrap());
    let mut model = vpkmerge_core::read_vpk_entry(&pak, ENTRY)?;

    let sk = decode_skeleton(&model).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let names: Vec<&str> = sk.bones.iter().map(|b| b.name.as_str()).collect();
    let bidx = |n: &str| names.iter().position(|x| *x == n).unwrap();
    let bone_pos: Vec<[f32; 3]> = sk
        .bones
        .iter()
        .map(|b| {
            [
                b.global_bind.m[12],
                b.global_bind.m[13],
                b.global_bind.m[14],
            ]
        })
        .collect();
    let flap_l = bidx("flaps_0_L");
    let flap_r = bidx("flaps_0_R");
    let ll_l = bidx("leg_lower_L");
    let ll_r = bidx("leg_lower_R");
    let r2 = radius * radius;

    // nearest bone to a model-space point (argmin over all bone heads)
    let nearest = |p: [f32; 3]| -> usize {
        let mut best = 0usize;
        let mut bd = f32::MAX;
        for (i, bp) in bone_pos.iter().enumerate() {
            let d = dist2(p, *bp);
            if d < bd {
                bd = d;
                best = i;
            }
        }
        best
    };

    // DATA block -> per-mesh remap table
    let model_for_data = model.clone();
    let (_, off, len) = blocks(&model_for_data)
        .into_iter()
        .find(|(k, _, _)| k == b"DATA")
        .unwrap();
    let data = morphic::kv3::decode(&model_for_data[off..off + len])
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;

    // connected components of the LOD0 body so we can pick out the fringe cards
    // (small islands) vs the solid boot/calf (one big island).
    let m0 = decode(&model).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let body0 = m0
        .meshes
        .iter()
        .find(|p| p.name == "body")
        .ok_or_else(|| anyhow::anyhow!("no LOD0 body mesh"))?;
    let nverts = body0.vertex_buffers[0].positions.len();
    let mut parent: Vec<usize> = (0..nverts).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    for prim in &body0.primitives {
        if prim.vertex_buffer != 0 {
            continue;
        }
        for tri in prim.indices.chunks_exact(3) {
            let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let (ra, rb, rc) = (
                find(&mut parent, a),
                find(&mut parent, b),
                find(&mut parent, c),
            );
            parent[rb] = ra;
            parent[rc] = ra;
        }
    }
    let mut comp_size = vec![0usize; nverts];
    for v in 0..nverts {
        let r = find(&mut parent, v);
        comp_size[r] += 1;
    }
    let small: Vec<bool> = (0..nverts)
        .map(|v| {
            let r = find(&mut parent, v);
            comp_size[r] <= 200
        })
        .collect();
    println!(
        "LOD0 body: {nverts} verts, {} in small components (<=200)",
        small.iter().filter(|b| **b).count()
    );

    // resolve local palette slots for the LOD0 body mesh
    let rt = remap_table(&data, body0.mesh_index)
        .ok_or_else(|| anyhow::anyhow!("no remap for body mesh"))?;
    let inv = invert_remap(&rt);
    let local = |model_bone: usize| inv.get(&model_bone).copied();
    let (Some(ll_l_loc), Some(ll_r_loc), Some(fl_l_loc), Some(fl_r_loc)) =
        (local(ll_l), local(ll_r), local(flap_l), local(flap_r))
    else {
        anyhow::bail!("body mesh missing a needed bone in palette");
    };

    let (new_bytes, changed) = reskin_vertex_buffer(&model, 0, |i, pos, mut j, w| {
        if !small[i] {
            return j;
        }
        let nb = nearest(pos);
        if nb == flap_l && dist2(pos, bone_pos[flap_l]) <= r2 {
            for k in 0..4 {
                if j[k] == ll_l_loc && w[k] > 0.0 {
                    j[k] = fl_l_loc;
                }
            }
        }
        if nb == flap_r && dist2(pos, bone_pos[flap_r]) <= r2 {
            for k in 0..4 {
                if j[k] == ll_r_loc && w[k] > 0.0 {
                    j[k] = fl_r_loc;
                }
            }
        }
        j
    })
    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    println!("LOD0 body: {changed} verts re-skinned");
    model = new_bytes;

    // verify: count flap-weighted verts after the edit
    let m = decode(&model).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    for (nm, bi) in [("flaps_0_L", flap_l as u16), ("flaps_0_R", flap_r as u16)] {
        let mut c = 0usize;
        for mesh in &m.meshes {
            for vb in &mesh.vertex_buffers {
                for (j4, w4) in vb.joints.iter().zip(&vb.weights) {
                    for k in 0..4 {
                        if j4[k] == bi && w4[k] > 0.0 {
                            c += 1;
                        }
                    }
                }
            }
        }
        println!("  post-reskin {nm} (#{bi}) now drives {c} verts");
    }

    std::fs::write("/tmp/astro_reskin.vmdl_c", &model)?;
    let glb = morphic::model::to_glb(&m).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    std::fs::write("/tmp/astro_reskin.glb", &glb)?;
    println!("wrote /tmp/astro_reskin.vmdl_c and /tmp/astro_reskin.glb");
    Ok(())
}
