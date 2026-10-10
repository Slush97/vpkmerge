//! Validates `morphic::model::read_glb_skeleton` against pak-derived truth: export
//! a hero rig to `.glb`, read the rest pose back, and diff every bone's name,
//! parent, and model-space bind against the `.vmdl_c` the export came from.
//!
//! Usage: retarget_glb_skel <pak01_dir.vpk> <codename> <exported.glb>

use morphic::model::{read_glb_skeleton, Mat4, Skeleton, Vec3};

fn origin(m: &Mat4) -> Vec3 {
    m.transform_point(Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    })
}

/// Largest absolute elementwise difference between two transforms.
fn max_elem_diff(a: &Mat4, b: &Mat4) -> f32 {
    a.m.iter()
        .zip(b.m.iter())
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f32::max)
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let (pak, codename, glb_path) = (&args[1], &args[2], &args[3]);

    // Truth: the skeleton the exporter read out of the pak.
    let entry = vpkmerge_core::hero_model_entry(pak, None, codename)?;
    let bytes = vpkmerge_core::read_vpk_entry(pak, &entry)?;
    let truth = Skeleton::from_model_data(&morphic::decode_kv3_resource(&bytes)?)?;

    // Under test: the rest pose read back out of the exported glb.
    let glb = std::fs::read(glb_path)?;
    let read = read_glb_skeleton(&glb)?;

    println!("entry:       {entry}");
    println!("bind source: {:?}", read.bind_source);
    println!(
        "bones:       {} truth / {} read\n",
        truth.bones.len(),
        read.bones.len()
    );

    if truth.bones.len() != read.bones.len() {
        anyhow::bail!("bone count mismatch");
    }

    let mut name_bad = 0usize;
    let mut parent_bad = 0usize;
    let mut worst_bind = 0.0f32;
    let mut worst_bind_bone = String::new();
    let mut worst_pos = 0.0f32;
    let mut worst_pos_bone = String::new();

    for (t, r) in truth.bones.iter().zip(&read.bones) {
        if t.name != r.name {
            if name_bad < 5 {
                println!("  name mismatch: {:?} vs {:?}", t.name, r.name);
            }
            name_bad += 1;
        }
        if t.parent != r.parent {
            if parent_bad < 5 {
                println!(
                    "  parent mismatch on {}: {:?} vs {:?}",
                    t.name, t.parent, r.parent
                );
            }
            parent_bad += 1;
        }

        let d = max_elem_diff(&t.global_bind, &r.global_bind);
        if d > worst_bind {
            worst_bind = d;
            worst_bind_bone.clone_from(&t.name);
        }

        // Positional error in Source units is the one with physical meaning.
        let (a, b) = (origin(&t.global_bind), origin(&r.global_bind));
        let p = ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt();
        if p > worst_pos {
            worst_pos = p;
            worst_pos_bone.clone_from(&t.name);
        }
    }

    println!("name mismatches:   {name_bad}");
    println!("parent mismatches: {parent_bad}");
    println!("worst global_bind element diff: {worst_bind:.2e}  ({worst_bind_bone})");
    println!("worst bone-origin distance:     {worst_pos:.2e} units  ({worst_pos_bone})");

    // Sanity: the arm should read as bound well below horizontal, the property that
    // makes a rest-pose correction necessary in the first place.
    if let (Some(up), Some(lo)) = (read.by_name("arm_upper_L"), read.by_name("arm_lower_L")) {
        let (a, e) = (origin(&up.global_bind), origin(&lo.global_bind));
        let (dx, dy, dz) = (e.x - a.x, e.y - a.y, e.z - a.z);
        let angle = (-dz).atan2((dx * dx + dy * dy).sqrt()).to_degrees();
        println!("\narm_upper_L -> arm_lower_L: {angle:+.1} deg off horizontal");
    }

    let ok = name_bad == 0 && parent_bad == 0 && worst_pos < 1e-2;
    println!("\n{}", if ok { "PASS" } else { "FAIL" });
    if !ok {
        anyhow::bail!("round-trip mismatch");
    }
    Ok(())
}
