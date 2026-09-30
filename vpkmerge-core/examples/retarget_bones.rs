//! Throwaway: list a hero rig's non-finger bones with parent + bind angle, and
//! diff the core bone set across heroes to confirm the shared gen_man naming.
//!
//! Usage: retarget_bones <pak01_dir.vpk> <codename>...

use morphic::model::{Mat4, Skeleton, Vec3};

fn origin(m: &Mat4) -> Vec3 {
    m.transform_point(Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    })
}

fn is_finger(n: &str) -> bool {
    let n = n.to_lowercase();
    ["finger", "thumb", "index", "middle", "ring", "pinky"]
        .iter()
        .any(|f| n.contains(f))
}

fn core_bones(pak: &str, codename: &str) -> anyhow::Result<(Skeleton, String)> {
    let entry = vpkmerge_core::hero_model_entry(pak, None, codename)?;
    let bytes = vpkmerge_core::read_vpk_entry(pak, &entry)?;
    let tree = morphic::decode_kv3_resource(&bytes)?;
    Ok((Skeleton::from_model_data(&tree)?, entry))
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let pak = &args[1];
    let heroes = &args[2..];

    let mut sets: Vec<(String, std::collections::BTreeSet<String>)> = Vec::new();

    for codename in heroes {
        let (skel, entry) = core_bones(pak, codename)?;
        println!(
            "\n===== {codename}  ({entry})  {} bones =====",
            skel.bones.len()
        );
        let mut set = std::collections::BTreeSet::new();
        for (i, b) in skel.bones.iter().enumerate() {
            if is_finger(&b.name) {
                continue;
            }
            set.insert(b.name.clone());
            let parent = b
                .parent
                .map_or("-".to_string(), |p| skel.bones[p].name.clone());
            let o = origin(&b.global_bind);
            println!("  [{i:3}] {:28} parent {:24} z {:7.2}", b.name, parent, o.z);
        }
        sets.push((codename.clone(), set));
    }

    if sets.len() > 1 {
        println!("\n===== cross-hero core-bone diff =====");
        let (first_name, first) = &sets[0];
        for (name, s) in &sets[1..] {
            let missing: Vec<&String> = first.difference(s).collect();
            let extra: Vec<&String> = s.difference(first).collect();
            println!(
                "\n{name} vs {first_name}: shared {}, missing {}, extra {}",
                first.intersection(s).count(),
                missing.len(),
                extra.len()
            );
            if !missing.is_empty() {
                println!("  missing: {missing:?}");
            }
            if !extra.is_empty() {
                println!("  extra:   {extra:?}");
            }
        }
    }

    Ok(())
}
