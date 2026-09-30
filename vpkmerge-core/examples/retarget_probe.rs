//! Throwaway: measure a Deadlock hero rig's bind pose against Mixamo's convention
//! so the retargeter's rest-pose correction can be scoped from real numbers.
//!
//! Usage: retarget_probe <pak01_dir.vpk> <codename>

use morphic::model::{Mat4, Skeleton, Vec3};

/// Mixamo's fixed humanoid rig (the bones that carry motion). Suffix after the
/// `mixamorig:` prefix, in Mixamo's own naming.
const MIXAMO_CORE: &[&str] = &[
    "Hips",
    "Spine",
    "Spine1",
    "Spine2",
    "Neck",
    "Head",
    "LeftShoulder",
    "LeftArm",
    "LeftForeArm",
    "LeftHand",
    "RightShoulder",
    "RightArm",
    "RightForeArm",
    "RightHand",
    "LeftUpLeg",
    "LeftLeg",
    "LeftFoot",
    "LeftToeBase",
    "RightUpLeg",
    "RightLeg",
    "RightFoot",
    "RightToeBase",
];

fn origin(m: &Mat4) -> Vec3 {
    m.transform_point(Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    })
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    Vec3 {
        x: a.x - b.x,
        y: a.y - b.y,
        z: a.z - b.z,
    }
}

fn len(v: Vec3) -> f32 {
    (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let pak = &args[1];
    let codename = &args[2];

    let entry = vpkmerge_core::hero_model_entry(pak, None, codename)?;
    println!("model entry: {entry}");
    let bytes = vpkmerge_core::read_vpk_entry(pak, &entry)?;
    let tree = morphic::decode_kv3_resource(&bytes)?;
    let skel = Skeleton::from_model_data(&tree)?;
    println!("bones: {}\n", skel.bones.len());

    // Child lookup so we can point each bone at its successor.
    let mut child_of: Vec<Option<usize>> = vec![None; skel.bones.len()];
    for (i, b) in skel.bones.iter().enumerate() {
        if let Some(p) = b.parent {
            if child_of[p].is_none() {
                child_of[p] = Some(i);
            }
        }
    }

    // ---- 1. Name-shape survey: what families of bones exist? ----
    let mut fam: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for b in &skel.bones {
        let n = b.name.to_lowercase();
        let k = if n.contains("ik") {
            "ik"
        } else if n.contains("offset") {
            "offset"
        } else if n.contains("cloth") || n.contains("jiggle") || n.contains("dyn") {
            "cloth/dyn"
        } else if n.contains("weapon") || n.contains("gun") || n.contains("prop") {
            "weapon/prop"
        } else if n.contains("attach") || n.starts_with("a_") {
            "attachment"
        } else if n.contains("finger")
            || n.contains("thumb")
            || n.contains("index")
            || n.contains("middle")
            || n.contains("ring")
            || n.contains("pinky")
        {
            "finger"
        } else if n.contains("face") || n.contains("jaw") || n.contains("eye") || n.contains("brow")
        {
            "face"
        } else {
            "core/other"
        };
        *fam.entry(k).or_default() += 1;
    }
    println!("== bone families ==");
    for (k, v) in &fam {
        println!("  {k:14} {v}");
    }

    // ---- 2. Bind-pose geometry: A-pose or T-pose? ----
    // Source is Z-up. For each arm/leg bone, report the direction to its child and
    // the angle off horizontal (0 deg = straight out = T-pose arm).
    println!("\n== limb bind directions (angle off horizontal, +down) ==");
    let want = [
        "arm", "shoulder", "clavicle", "elbow", "hand", "leg", "thigh", "knee", "ankle", "foot",
    ];
    for (i, b) in skel.bones.iter().enumerate() {
        let n = b.name.to_lowercase();
        if !want.iter().any(|w| n.contains(w)) {
            continue;
        }
        if n.contains("ik") || n.contains("offset") || n.contains("finger") {
            continue;
        }
        let Some(c) = child_of[i] else { continue };
        let d = sub(origin(&skel.bones[c].global_bind), origin(&b.global_bind));
        let l = len(d);
        if l < 1e-4 {
            continue;
        }
        let horiz = (d.x * d.x + d.y * d.y).sqrt();
        let angle = (-d.z).atan2(horiz).to_degrees();
        println!(
            "  {:28} len {:7.2}  dir ({:6.2},{:6.2},{:6.2})  {:+6.1} deg",
            b.name,
            l,
            d.x / l,
            d.y / l,
            d.z / l,
            angle
        );
    }

    // ---- 3. Scale reference: hip height + total height ----
    let mut zmin = f32::INFINITY;
    let mut zmax = f32::NEG_INFINITY;
    for b in &skel.bones {
        let o = origin(&b.global_bind);
        zmin = zmin.min(o.z);
        zmax = zmax.max(o.z);
    }
    let hips = skel
        .bones
        .iter()
        .find(|b| {
            let n = b.name.to_lowercase();
            n.contains("pelvis") || n.contains("hip") || n == "root"
        })
        .map(|b| origin(&b.global_bind).z);
    println!("\n== scale reference ==");
    println!(
        "  bone z-extent: {zmin:.2} .. {zmax:.2}  (span {:.2})",
        zmax - zmin
    );
    if let Some(h) = hips {
        println!("  hip height:    {h:.2}");
        println!("  Mixamo hip ~1.0 (m) -> translation scale factor ~{h:.1}x");
    }

    // ---- 4. Mixamo mapping coverage: can each Mixamo bone find a home? ----
    println!("\n== Mixamo core-bone mapping candidates ==");
    let names: Vec<String> = skel.bones.iter().map(|b| b.name.to_lowercase()).collect();
    let mut hit = 0;
    for m in MIXAMO_CORE {
        let ml = m.to_lowercase();
        // Crude token match, just to see whether an obvious counterpart exists.
        let side = if ml.starts_with("left") {
            Some("l")
        } else if ml.starts_with("right") {
            Some("r")
        } else {
            None
        };
        let stem = ml.trim_start_matches("left").trim_start_matches("right");
        let cand: Vec<&String> = names
            .iter()
            .enumerate()
            .filter(|(i, n)| {
                let core = !n.contains("ik") && !n.contains("offset");
                let stem_hit = match stem {
                    "arm" => n.contains("arm") && !n.contains("fore"),
                    "forearm" => n.contains("forearm") || n.contains("lowerarm"),
                    "upleg" => {
                        n.contains("thigh") || n.contains("upperleg") || n.contains("leg_up")
                    }
                    "leg" => n.contains("calf") || n.contains("lowerleg") || n.contains("knee"),
                    "toebase" => n.contains("toe"),
                    s => n.contains(s),
                };
                let side_hit = side.is_none_or(|s| {
                    n.ends_with(&format!("_{s}"))
                        || n.contains(&format!("_{s}_"))
                        || n.contains(if s == "l" { "left" } else { "right" })
                });
                let _ = i;
                core && stem_hit && side_hit
            })
            .map(|(_, n)| n)
            .collect();
        if cand.is_empty() {
            println!("  {m:16} -> (none)");
        } else {
            hit += 1;
            let show: Vec<&str> = cand.iter().take(3).map(|s| s.as_str()).collect();
            println!("  {m:16} -> {}", show.join(", "));
        }
    }
    println!("\n  auto-matched {hit}/{} core bones", MIXAMO_CORE.len());

    Ok(())
}
