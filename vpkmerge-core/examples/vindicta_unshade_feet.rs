// Removes the dark baked shading on Vindicta's feet/ankles and packs the result
// as an addon VPK. Works on Valve's stock model and on skin mods that ship their
// own copy of it.
//
// Why this is a vertex-color edit and not a texture edit: her legs, feet and hands
// are drawn by `vindicta_head.vmat`, whose `g_tColor` is a flat 4x4 white swatch.
// The material sets `F_VERTEX_COLOR` with `g_fVertexColorStrength1 = 1`, so the
// entire visible color of that geometry is the mesh's baked per-vertex `COLOR`. In
// Valve's mesh the foot region is a flat `RGB(42,49,64)` (L 0.19) across 796
// vertices with a ramp up to the shin's `RGB(94,122,162)` (L 0.47); that step is
// the dark ankle/foot read. Skin mods inherit it (`ghost_bride_vindicta` ships
// `RGB(42,48,63)` on 737 foot vertices).
//
// Selection is by **position, within the buffers a vertex-colored material draws**:
//   * only buffers whose drawing material sets `F_VERTEX_COLOR` are eligible, so
//     the dress/props/hair buffers (which also carry COLOR on some skins) are never
//     touched;
//   * within those, only vertices at or below `--z-limit`;
//   * and only vertices *darker* than the target, so nothing is ever darkened.
// Keying on the color value instead does not generalize: `ghost_bride_vindicta`
// reuses its dark foot value up at z 48, so a value-keyed selector silently caught
// 97 of ~840 foot vertices.
//
// Positions, normals, UVs and skin weights are byte-preserved; only the COLOR lane
// is rewritten. Every LOD buffer is handled, else the feet snap dark at distance.
// A buffer with nothing selected is left alone (any write would convert a meshopt
// buffer to uncompressed and inflate the file for no change).
//
// usage:
//   cargo run --example vindicta_unshade_feet -- <vpk> <out_dir.vpk>
//         [--base <pak01_dir.vpk>] [--z-limit N] [--target R,G,B]
//         [--repack | --model-only] [--dry-run]

use std::collections::BTreeSet;

use anyhow::{Context, Result};

/// Vertices at or below this height are "lower leg + foot". Valve's ramp finishes
/// by z ~12 (the shin's brightest band is z 10..12), so 13 covers the clamp and its
/// blend without reaching the thigh gradient above.
const DEFAULT_Z_LIMIT: f32 = 13.0;

/// Below this luminance a foot vertex counts as "still dark" in the post-check.
const DARK_L: f32 = 0.30;

fn quant(c: [f32; 4]) -> [u8; 3] {
    [
        (c[0] * 255.0).round() as u8,
        (c[1] * 255.0).round() as u8,
        (c[2] * 255.0).round() as u8,
    ]
}

fn luma(c: [u8; 3]) -> f32 {
    (0.2126 * f32::from(c[0]) + 0.7152 * f32::from(c[1]) + 0.0722 * f32::from(c[2])) / 255.0
}

fn flag_value(s: &str, args: &[String]) -> Option<String> {
    args.iter()
        .position(|a| a == s)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Block indices of the vertex buffers drawn by a material that sets
/// `F_VERTEX_COLOR`, i.e. the ones whose baked color is actually rendered.
fn vertex_colored_blocks(
    vpk_path: &str,
    base: Option<&str>,
    model_bytes: &[u8],
) -> Result<BTreeSet<usize>> {
    let model = morphic::model::decode(model_bytes).context("decoding model")?;
    let targets = morphic::model::vertex_targets(model_bytes)?;

    // Buffers are addressed by global block index; the decoded model exposes them
    // per mesh, in the same order vertex_targets lists them for that mesh.
    let mut per_mesh: std::collections::HashMap<usize, Vec<usize>> =
        std::collections::HashMap::new();
    for t in &targets {
        per_mesh
            .entry(t.mesh_index)
            .or_default()
            .push(t.block_index);
    }

    let mut uses_vc: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    let mut out = BTreeSet::new();
    for mesh in &model.meshes {
        let Some(blocks) = per_mesh.get(&mesh.mesh_index) else {
            continue;
        };
        for prim in &mesh.primitives {
            let entry = if prim.material.ends_with("_c") {
                prim.material.clone()
            } else {
                format!("{}_c", prim.material)
            };
            let vc = *uses_vc.entry(entry.clone()).or_insert_with(|| {
                let t = vpkmerge_core::vmat_style::VmatTargets::Entries(vec![entry.clone()]);
                // A skin's own material first, then the base pak it inherits from.
                let mut mats = vpkmerge_core::vmat_style::list_materials(vpk_path, None, &t)
                    .unwrap_or_default();
                if mats.is_empty() {
                    if let Some(b) = base {
                        mats = vpkmerge_core::vmat_style::list_materials(b, None, &t)
                            .unwrap_or_default();
                    }
                }
                mats.first().is_some_and(|m| {
                    m.flags
                        .iter()
                        .any(|(f, v)| f == "F_VERTEX_COLOR" && *v != 0)
                })
            });
            if vc {
                if let Some(&block) = blocks.get(prim.vertex_buffer) {
                    out.insert(block);
                }
            }
        }
    }

    // `decode()` only exposes LOD0 meshes, so the material scan above can only see
    // LOD0 buffers. The lower LODs carry their own copy of the same COLOR data and
    // are drawn by the same materials, so extend eligibility to the LOD siblings by
    // base mesh name ("body" -> "body_lod1"/"body_lod2"/...). Skipping them leaves
    // the feet dark at distance.
    let base_of = |name: &str| -> String {
        name.rsplit_once("_lod")
            .filter(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            .map_or_else(|| name.to_string(), |(b, _)| b.to_string())
    };
    let eligible_bases: BTreeSet<String> = targets
        .iter()
        .filter(|t| out.contains(&t.block_index))
        .map(|t| base_of(&t.mesh_name))
        .collect();
    for t in &targets {
        if t.has_color && eligible_bases.contains(&base_of(&t.mesh_name)) {
            out.insert(t.block_index);
        }
    }
    Ok(out)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        anyhow::bail!(
            "usage: vindicta_unshade_feet <vpk> <out_dir.vpk> [--base <pak01_dir.vpk>] \
             [--z-limit N] [--target R,G,B] [--repack|--model-only] [--dry-run]"
        );
    }
    let vpk_path = args[0].clone();
    let out_vpk = args[1].clone();
    let base = flag_value("--base", &args);
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let z_limit: f32 = flag_value("--z-limit", &args)
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_Z_LIMIT);
    let target_arg: Option<[u8; 3]> = flag_value("--target", &args).and_then(|s| {
        let v: Vec<u8> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        (v.len() == 3).then(|| [v[0], v[1], v[2]])
    });

    let entry = vpkmerge_core::hero_model_entry(&vpk_path, None, "hornet")
        .context("resolving Vindicta's body model in the given VPK")?;
    let original = vpkmerge_core::read_vpk_entry(&vpk_path, &entry)
        .with_context(|| format!("reading {entry}"))?;
    println!("model: {entry}  ({} bytes)", original.len());
    println!("z limit: {z_limit}");

    let eligible = vertex_colored_blocks(&vpk_path, base.as_deref(), &original)?;
    anyhow::ensure!(
        !eligible.is_empty(),
        "no vertex buffer is drawn by an F_VERTEX_COLOR material: nothing this tool can fix \
         (pass --base <pak01_dir.vpk> if this skin inherits its materials from the base pak)"
    );
    println!(
        "eligible buffers (drawn by an F_VERTEX_COLOR material): {:?}\n",
        eligible
    );

    // Target tone is resolved **per buffer**, from that buffer's own shin band. A
    // global target is wrong on skins where several buffers set F_VERTEX_COLOR: on
    // `ghost_bride_vindicta` 18 of 19 buffers qualify and most carry a neutral white
    // COLOR lane, so a whole-model maximum resolves to pure white and would leave
    // her feet glowing instead of skin-toned.
    let mut bytes = original.clone();
    let mut total = 0usize;
    for &block in &eligible {
        let Some(colors) = morphic::model::read_vertex_colors(&bytes, block)? else {
            continue;
        };
        let positions = morphic::model::read_vertex_positions(&bytes, block)?;

        // Only touch a buffer that actually exhibits the defect: dark vertices in
        // the foot region. This keeps the edit off legitimately-shaded low geometry
        // (a dress hem, boot fringe) that merely happens to sit below the z limit.
        let has_dark_foot = positions
            .iter()
            .zip(&colors)
            .any(|(p, c)| p[2] <= 6.5 && luma(quant(*c)) < DARK_L);
        if !has_dark_foot {
            continue;
        }

        let target = match target_arg {
            Some(t) => t,
            None => {
                let mut best = [0u8; 3];
                let mut best_l = -1.0f32;
                for (p, c) in positions.iter().zip(&colors) {
                    if p[2] < z_limit || p[2] > 24.0 {
                        continue; // shin/calf band, below the skirt hem
                    }
                    let q = quant(*c);
                    if luma(q) > best_l {
                        best_l = luma(q);
                        best = q;
                    }
                }
                anyhow::ensure!(
                    best_l > 0.0,
                    "block {block}: no vertices in z {z_limit}..24 to sample a target tone from; \
                     pass --target R,G,B"
                );
                best
            }
        };
        let target_l = luma(target);
        let tf = [
            f32::from(target[0]) / 255.0,
            f32::from(target[1]) / 255.0,
            f32::from(target[2]) / 255.0,
        ];

        let (new_bytes, changed) =
            morphic::model::recolor_vertex_buffer_at(&bytes, block, |_i, p, c| {
                // Below the limit, and darker than where we are taking it: lift.
                if p[2] <= z_limit && luma(quant(c)) < target_l {
                    [tf[0], tf[1], tf[2], c[3]]
                } else {
                    c
                }
            })
            .with_context(|| format!("rewriting COLOR of block {block}"))?;
        println!(
            "block {block:>3}: {changed} vertices lifted to RGB({}, {}, {}) L={target_l:.4}",
            target[0], target[1], target[2]
        );
        bytes = new_bytes;
        total += changed;
    }

    println!("\n{total} vertices recolored");
    println!("model bytes: {} -> {}", original.len(), bytes.len());
    anyhow::ensure!(
        total > 0,
        "nothing was recolored: no dark skin below the z limit"
    );

    // Post-condition: verify rather than trust. An under-applied edit would ship a
    // mod that looks unchanged in game.
    let mut dark_left = 0usize;
    let mut worst = f32::INFINITY;
    for &block in &eligible {
        let Some(colors) = morphic::model::read_vertex_colors(&bytes, block)? else {
            continue;
        };
        let positions = morphic::model::read_vertex_positions(&bytes, block)?;
        for (p, c) in positions.iter().zip(&colors) {
            if p[2] > 6.5 {
                continue; // the foot/ankle region proper
            }
            let l = luma(quant(*c));
            if l < DARK_L {
                dark_left += 1;
                worst = worst.min(l);
            }
        }
    }
    anyhow::ensure!(
        dark_left == 0,
        "{dark_left} foot vertices are still dark (darkest L={worst:.4}) after the edit"
    );
    println!("post-check: no foot vertex below z 6.5 is left dark");

    if dry_run {
        println!("\n--dry-run: nothing written");
        return Ok(());
    }

    // A skin mod ships its own `hornet.vmdl_c`, so a model-only override cannot
    // stack with it: whichever pak loads first supplies the whole file and the
    // other's model is discarded. Repacking the skin's full entry set with the
    // patched model makes the output a drop-in *replacement* for that skin instead
    // of a competitor. Against the base pak that would mean rewriting the game, so
    // there we emit the model alone.
    let src = valve_pak::open(&vpk_path).with_context(|| format!("reopening {vpk_path}"))?;
    let entry_count = src.file_paths().count();
    let force_model_only = args.iter().any(|a| a == "--model-only");
    let force_repack = args.iter().any(|a| a == "--repack");
    let repack = force_repack || (!force_model_only && entry_count <= 5000);

    if !repack {
        println!("\ninput has {entry_count} entries: emitting the model alone");
        vpkmerge_core::pack(&[(entry.as_str(), bytes.as_slice())], &out_vpk)
            .with_context(|| format!("packing {out_vpk}"))?;
        println!("wrote {out_vpk}  (overrides {entry})");
        return Ok(());
    }

    println!(
        "\ninput has {entry_count} entries: repacking all of them with the patched model \
         (drop-in replacement for this skin)"
    );
    let mut owned: Vec<(String, Vec<u8>)> = Vec::with_capacity(entry_count);
    for path in src.file_paths() {
        if path == &entry {
            continue; // replaced below
        }
        owned.push((
            path.clone(),
            vpkmerge_core::read_vpk_entry(&vpk_path, path)
                .with_context(|| format!("reading {path}"))?,
        ));
    }
    owned.push((entry.clone(), bytes));
    let refs: Vec<(&str, &[u8])> = owned
        .iter()
        .map(|(p, d)| (p.as_str(), d.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &out_vpk).with_context(|| format!("packing {out_vpk}"))?;
    println!("wrote {out_vpk}  ({} entries, model patched)", refs.len());
    Ok(())
}
