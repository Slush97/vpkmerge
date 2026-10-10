// Cross-roster skeleton compatibility matrix.
//
// Every Deadlock hero rides a `gen_man`-derived core humanoid rig, but the fit
// varies: wraith is a 116/116 drop-in donor for the hat-man (gen_man) mesh swap,
// inferno shares only 83/116 with a 15 deg bind drift (the "claw" disaster). This
// tool quantifies that for the whole roster so we know which heroes a gen_man-rigged
// mesh can drop onto with NO rebind, which need a small remap, and which are hard.
//
// It also computes:
//   - the universal bone core (intersection of bone names across all heroes), i.e.
//     the safe bone set a custom mesh can weight to and swap onto ANY hero, and
//   - skeleton families: connected components of heroes that are mutually drop-in
//     compatible (shared >=90% names, bind-rotation delta < 3 deg).
//
// Usage:
//   cargo run --release -p vpkmerge-core --example hero_skeleton_matrix -- <pak01_dir.vpk> [--ref ENTRY] [--json]

use morphic::kv3::Value;
use std::collections::{BTreeMap, BTreeSet};

const GEN_MAN: &str = "models/npc/ghosts_active/gen_man_ghost.vmdl_c";

#[derive(Clone)]
struct Bone {
    parent: Option<usize>,
    pos: [f64; 3],
    rot: [f64; 4],
}

struct Skel {
    names: Vec<String>,
    bones: Vec<Bone>,
    index: BTreeMap<String, usize>,
    world_rot: Vec<[f64; 4]>, // accumulated down the hierarchy (deformation orientation)
    height: f64,              // pelvis->head world distance (scale-calibration hint)
}

impl Skel {
    fn parent_name(&self, i: usize) -> &str {
        match self.bones[i].parent {
            Some(p) if p < self.names.len() => &self.names[p],
            _ => "<root>",
        }
    }
}

fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    // (x,y,z,w)
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

fn quat_rot_vec(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let u = [q[0], q[1], q[2]];
    let s = q[3];
    let uv = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let uuv = [
        u[1] * uv[2] - u[2] * uv[1],
        u[2] * uv[0] - u[0] * uv[2],
        u[0] * uv[1] - u[1] * uv[0],
    ];
    [
        v[0] + 2.0 * (s * uv[0] + uuv[0]),
        v[1] + 2.0 * (s * uv[1] + uuv[1]),
        v[2] + 2.0 * (s * uv[2] + uuv[2]),
    ]
}

/// Helper/non-deforming bones whose bind orientation is irrelevant to a mesh swap
/// (IK solver targets, leaf tips like `hand_end_L`/`finger_index_end_R`, attachment
/// bones, motion root). These carry no skin weight, so their bind delta is noise.
fn is_deform(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    !(name == "root_motion"
        || lower.ends_with("_iktarget")
        || lower.ends_with("_offset")
        || lower.contains("_end")
        || lower.ends_with("_hlpr")
        || lower.starts_with("object_hand")
        || lower.starts_with("weapon_hand")
        || lower.starts_with("weaponhand")
        || lower.starts_with("weapon_target")
        || lower.starts_with("attach"))
}

fn arr_f(v: &Value) -> [f64; 3] {
    let a = v.as_array().unwrap_or(&[]);
    let g = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(f64::NAN);
    [g(0), g(1), g(2)]
}
fn arr_q(v: &Value) -> [f64; 4] {
    let a = v.as_array().unwrap_or(&[]);
    let g = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(f64::NAN);
    [g(0), g(1), g(2), g(3)]
}

fn load_skel(bytes: &[u8]) -> Option<Skel> {
    let tree = morphic::decode_kv3_resource(bytes).ok()?;
    let sk = tree.get("m_modelSkeleton")?;
    let names: Vec<String> = sk
        .get("m_boneName")?
        .as_array()?
        .iter()
        .map(|x| x.as_str().unwrap_or("?").to_string())
        .collect();
    let parents: Vec<i64> = sk
        .get("m_nParent")?
        .as_array()?
        .iter()
        .map(|x| x.as_int().unwrap_or(-1))
        .collect();
    let pos = sk.get("m_bonePosParent")?.as_array()?;
    let rot = sk.get("m_boneRotParent")?.as_array()?;
    if names.len() != parents.len() || names.len() != pos.len() || names.len() != rot.len() {
        return None;
    }
    let mut bones = Vec::with_capacity(names.len());
    let mut index = BTreeMap::new();
    for i in 0..names.len() {
        let p = parents[i];
        bones.push(Bone {
            parent: (p >= 0 && (p as usize) < names.len()).then_some(p as usize),
            pos: arr_f(&pos[i]),
            rot: arr_q(&rot[i]),
        });
        index.insert(names[i].clone(), i);
    }
    // Accumulate world-space bind transforms (parents precede children in Source skeletons).
    let mut world_rot = vec![[0.0; 4]; names.len()];
    let mut world_pos = vec![[0.0; 3]; names.len()];
    for i in 0..names.len() {
        match bones[i].parent {
            Some(p) if p < i => {
                world_rot[i] = quat_mul(world_rot[p], bones[i].rot);
                let r = quat_rot_vec(world_rot[p], bones[i].pos);
                world_pos[i] = [
                    world_pos[p][0] + r[0],
                    world_pos[p][1] + r[1],
                    world_pos[p][2] + r[2],
                ];
            }
            _ => {
                world_rot[i] = bones[i].rot;
                world_pos[i] = bones[i].pos;
            }
        }
    }
    let height = match (index.get("pelvis"), index.get("head")) {
        (Some(&p), Some(&h)) => dist(world_pos[p], world_pos[h]),
        _ => 0.0,
    };
    Some(Skel {
        names,
        bones,
        index,
        world_rot,
        height,
    })
}

/// Angle between two unit quaternions, in degrees.
fn quat_angle_deg(a: [f64; 4], b: [f64; 4]) -> f64 {
    let dot = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]).abs();
    2.0 * dot.min(1.0).acos() * 180.0 / std::f64::consts::PI
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

struct Cmp {
    shared: usize,
    missing: Vec<String>,        // ref bones absent in target
    missing_deform: Vec<String>, // ref DEFORM bones absent (-> undriven verts; the real gap)
    worst: Vec<(String, f64)>,   // deform bones with the largest WORLD bind-rot delta (deg)
    parent_diff: usize,
    rot_mean: f64, // parent-relative, all shared bones (legacy; matches prior memory numbers)
    rot_max: f64,
    pos_mean: f64,
    wrot_mean: f64, // WORLD-space, deform bones only (the true mesh-swap signal)
    wrot_max: f64,
    deform_shared: usize,
}

/// Compare `tgt` against reference `r`, keyed by bone name.
fn compare(r: &Skel, tgt: &Skel) -> Cmp {
    let mut missing = Vec::new();
    let mut missing_deform = Vec::new();
    let mut shared = 0usize;
    let mut parent_diff = 0usize;
    let (mut rot_sum, mut rot_max) = (0.0, 0.0_f64);
    let mut pos_sum = 0.0;
    let (mut wrot_sum, mut wrot_max) = (0.0, 0.0_f64);
    let mut deform_shared = 0usize;
    let mut per_bone: Vec<(String, f64)> = Vec::new();
    for (i, name) in r.names.iter().enumerate() {
        let Some(&j) = tgt.index.get(name) else {
            missing.push(name.clone());
            if is_deform(name) {
                missing_deform.push(name.clone());
            }
            continue;
        };
        shared += 1;
        if r.parent_name(i) != tgt.parent_name(j) {
            parent_diff += 1;
        }
        let dr = quat_angle_deg(r.bones[i].rot, tgt.bones[j].rot);
        let dpos = dist(r.bones[i].pos, tgt.bones[j].pos);
        rot_sum += dr;
        rot_max = rot_max.max(dr);
        pos_sum += dpos;
        if is_deform(name) {
            let wdr = quat_angle_deg(r.world_rot[i], tgt.world_rot[j]);
            wrot_sum += wdr;
            wrot_max = wrot_max.max(wdr);
            deform_shared += 1;
            per_bone.push((name.clone(), wdr));
        }
    }
    per_bone.sort_by(|a, b| b.1.total_cmp(&a.1));
    per_bone.truncate(6);
    let n = shared.max(1) as f64;
    let dn = deform_shared.max(1) as f64;
    Cmp {
        shared,
        missing,
        missing_deform,
        worst: per_bone,
        parent_diff,
        rot_mean: rot_sum / n,
        rot_max,
        pos_mean: pos_sum / n,
        wrot_mean: wrot_sum / dn,
        wrot_max,
        deform_shared,
    }
}

// Classify on the WORLD-space deform-bone delta (the true mesh-swap signal) + the count
// of missing DEFORM bones (undriven verts). The parent-relative number and raw missing
// count are inflated by pelvis re-parenting, IK targets, and weightless leaf tips.
fn classify(c: &Cmp) -> &'static str {
    let md = c.missing_deform.len();
    if md == 0 && c.wrot_mean < 1.0 {
        "DROP-IN" // wraith: full superset of gen_man deform bones, identical pose
    } else if md <= 4 && c.wrot_mean < 5.0 {
        "EASY" // identical rest pose, only cosmetic deform bones short
    } else if md <= 25 && c.wrot_mean < 25.0 {
        "REMAP" // inferno: weight-remap + IK handling needed
    } else {
        "HARD"
    }
}

fn model_entry(model: &str) -> String {
    if model.ends_with(".vmdl") {
        format!("{model}_c")
    } else if model.ends_with(".vmdl_c") {
        model.to_string()
    } else {
        format!("{model}.vmdl_c")
    }
}

fn codename(model: &str) -> String {
    for m in [
        "models/heroes_staging/",
        "models/heroes_wip/",
        "models/heroes/",
    ] {
        if let Some(rest) = model.strip_prefix(m) {
            return rest.split('/').next().unwrap_or("").to_string();
        }
    }
    model.rsplit('/').next().unwrap_or(model).to_string()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: hero_skeleton_matrix <pak01_dir.vpk> [--ref ENTRY] [--json]");
        std::process::exit(2);
    }
    let pak = &args[1];
    let json = args.iter().any(|a| a == "--json");
    let ref_entry = args
        .windows(2)
        .find(|w| w[0] == "--ref")
        .map_or(GEN_MAN.to_string(), |w| w[1].clone());

    let vpk = valve_pak::open(pak)?;
    let read = |entry: &str| -> Option<Vec<u8>> { vpk.get_file(entry).ok()?.read_all().ok() };

    // Reference rig.
    let ref_bytes =
        read(&ref_entry).ok_or_else(|| anyhow::anyhow!("ref {ref_entry} not in pak"))?;
    let reference =
        load_skel(&ref_bytes).ok_or_else(|| anyhow::anyhow!("ref skeleton decode failed"))?;
    let ref_total = reference.names.len();

    // Roster: every selectable-ish hero record with a model path.
    let hv = read("scripts/heroes.vdata_c").ok_or_else(|| anyhow::anyhow!("no heroes.vdata_c"))?;
    let root = morphic::decode_kv3_resource(&hv)?;
    let Value::Object(top) = root else {
        anyhow::bail!("heroes.vdata_c root not object");
    };

    struct Row {
        code: String,
        record: String,
        selectable: bool,
        skel: Option<Skel>,
        model: String,
        err: Option<String>,
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut seen_models: BTreeSet<String> = BTreeSet::new();
    for (record, value) in &top {
        if !record.starts_with("hero_") || record == "hero_base" || record == "hero_testhero" {
            continue;
        }
        let Value::Object(obj) = value else { continue };
        let get = |k: &str| obj.iter().find(|(n, _)| n == k).map(|(_, v)| v);
        let model = get("m_strModelName").and_then(Value::as_str).unwrap_or("");
        if model.is_empty() {
            continue;
        }
        // Dedup heroes that share a model file (won't happen often).
        if !seen_models.insert(model.to_string()) {
            continue;
        }
        let selectable = matches!(get("m_bPlayerSelectable"), Some(Value::Bool(true)));
        let entry = model_entry(model);
        let (skel, err) = match read(&entry) {
            Some(b) => match load_skel(&b) {
                Some(s) => (Some(s), None),
                None => (None, Some("skeleton decode failed".to_string())),
            },
            None => (None, Some("model not in pak".to_string())),
        };
        rows.push(Row {
            code: codename(model),
            record: record.clone(),
            selectable,
            skel,
            model: model.to_string(),
            err,
        });
    }
    rows.sort_by(|a, b| a.code.cmp(&b.code));

    // --- Universal bone core: intersection of names across all decoded skeletons.
    let mut core: Option<BTreeSet<String>> = None;
    let mut decoded = 0usize;
    for r in &rows {
        if let Some(s) = &r.skel {
            decoded += 1;
            let set: BTreeSet<String> = s.names.iter().cloned().collect();
            core = Some(match core {
                None => set,
                Some(c) => c.intersection(&set).cloned().collect(),
            });
        }
    }
    let core = core.unwrap_or_default();
    let ref_names: BTreeSet<String> = reference.names.iter().cloned().collect();
    let core_in_ref: usize = core.intersection(&ref_names).count();

    if json {
        let mut items = Vec::new();
        for r in &rows {
            let body = if let Some(s) = &r.skel {
                let c = compare(&reference, s);
                format!(
                    "\"bones\":{},\"height\":{:.2},\"shared\":{},\"missing\":{},\"missingDeform\":{},\"deformShared\":{},\"parentDiff\":{},\"wRotMeanDeg\":{:.3},\"wRotMaxDeg\":{:.3},\"rotMeanDeg\":{:.3},\"rotMaxDeg\":{:.3},\"posMean\":{:.3},\"class\":\"{}\",\"missingDeformBones\":{:?},\"missingBones\":{:?}",
                    s.names.len(),
                    s.height,
                    c.shared,
                    ref_total - c.shared,
                    c.missing_deform.len(),
                    c.deform_shared,
                    c.parent_diff,
                    c.wrot_mean,
                    c.wrot_max,
                    c.rot_mean,
                    c.rot_max,
                    c.pos_mean,
                    classify(&c),
                    c.missing_deform,
                    c.missing,
                )
            } else {
                format!("\"error\":{:?}", r.err.as_deref().unwrap_or("?"))
            };
            items.push(format!(
                "{{\"code\":{:?},\"record\":{:?},\"selectable\":{},\"model\":{:?},{}}}",
                r.code, r.record, r.selectable, r.model, body
            ));
        }
        println!(
            "{{\"ref\":{:?},\"refBones\":{},\"decoded\":{},\"universalCore\":{},\"universalCoreInRef\":{},\"heroes\":[{}]}}",
            ref_entry,
            ref_total,
            decoded,
            core.len(),
            core_in_ref,
            items.join(",")
        );
        return Ok(());
    }

    println!(
        "Reference rig: {ref_entry}  ({ref_total} bones)\nRoster models decoded: {decoded}/{}\n",
        rows.len()
    );
    println!(
        "Columns: shar=shared bone NAMES /{ref_total}, mDef=missing DEFORM bones (undriven verts),"
    );
    println!(
        "wRotMean/Max=WORLD-space bind-rot delta over DEFORM bones in deg (the true swap signal),"
    );
    println!(
        "ht=skeleton pelvis->head height (gen_man ref = {:.1}; scale-calibration hint).\n",
        reference.height
    );
    println!(
        "{:<14} {:>3} {:>4} {:>5} {:>4} {:>8} {:>8} {:>6}  {:<8}",
        "code", "sel", "bone", "shar", "mDef", "wRotMean", "wRotMax", "ht", "class"
    );
    println!("{}", "-".repeat(82));
    // Order: DROP-IN, EASY, REMAP, HARD, then errors.
    let order = |c: &str| match c {
        "DROP-IN" => 0,
        "EASY" => 1,
        "REMAP" => 2,
        "HARD" => 3,
        _ => 4,
    };
    let mut printable: Vec<(&Row, Option<Cmp>)> = rows
        .iter()
        .map(|r| (r, r.skel.as_ref().map(|s| compare(&reference, s))))
        .collect();
    printable.sort_by(|a, b| {
        let ca = a.1.as_ref().map_or("ZZ", |c| classify(c));
        let cb = b.1.as_ref().map_or("ZZ", |c| classify(c));
        order(ca)
            .cmp(&order(cb))
            .then_with(|| {
                a.1.as_ref()
                    .map(|c| c.wrot_mean)
                    .unwrap_or(f64::MAX)
                    .total_cmp(&b.1.as_ref().map(|c| c.wrot_mean).unwrap_or(f64::MAX))
            })
            .then_with(|| a.0.code.cmp(&b.0.code))
    });
    let mut class_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (r, c) in &printable {
        match c {
            Some(c) => {
                let cl = classify(c);
                *class_counts.entry(cl).or_default() += 1;
                println!(
                    "{:<14} {:>3} {:>4} {:>5} {:>4} {:>7.2}d {:>7.2}d {:>6.1}  {:<8}",
                    r.code,
                    if r.selectable { "y" } else { "." },
                    r.skel.as_ref().unwrap().names.len(),
                    c.shared,
                    c.missing_deform.len(),
                    c.wrot_mean,
                    c.wrot_max,
                    r.skel.as_ref().unwrap().height,
                    cl,
                );
            }
            None => {
                *class_counts.entry("ERR").or_default() += 1;
                println!(
                    "{:<14} {:>3} {:>4} {:>5} {:>4} {:>8} {:>8} {:>6}  {}",
                    r.code,
                    if r.selectable { "y" } else { "." },
                    "-",
                    "-",
                    "-",
                    "-",
                    "-",
                    "-",
                    r.err.as_deref().unwrap_or("?")
                );
            }
        }
    }

    println!("\nClass tally: {class_counts:?}");
    println!(
        "Universal bone core (intersection across {decoded} hero skeletons): {} bones; {} of them are in the gen_man ref.",
        core.len(),
        core_in_ref
    );

    // Which gen_man bones are present in EVERY hero (safe donor-weight targets).
    let universal_ref: Vec<&String> = reference
        .names
        .iter()
        .filter(|n| core.contains(*n))
        .collect();
    let ref_not_universal: Vec<&String> = reference
        .names
        .iter()
        .filter(|n| !core.contains(*n))
        .collect();
    println!(
        "gen_man bones present in ALL heroes ({}/{ref_total}) -- the 'weight to these and you can swap onto anyone' set:\n  {:?}",
        universal_ref.len(),
        universal_ref
    );
    println!(
        "NOT universal ({}, mostly fingers/twists/IK/eyes/jaw/weapon): {:?}",
        ref_not_universal.len(),
        ref_not_universal
    );

    // Near-miss diagnostics: the closest non-drop-in heroes and their worst DEFORM bones.
    println!("\nNear-miss heroes (closest to drop-in after wraith) and their worst-offending deform bones (world-space):");
    let mut near: Vec<(&Row, Cmp)> = rows
        .iter()
        .filter_map(|r| r.skel.as_ref().map(|s| (r, compare(&reference, s))))
        .filter(|(r, _)| r.code != "wraith")
        .collect();
    near.sort_by(|a, b| a.1.wrot_mean.total_cmp(&b.1.wrot_mean));
    for (r, c) in near.iter().take(8) {
        let worst: Vec<String> = c
            .worst
            .iter()
            .filter(|(_, d)| *d > 1.0)
            .map(|(n, d)| format!("{n}={d:.0}deg"))
            .collect();
        println!(
            "  {:<14} shared {}/{ref_total} (deform {}), wRotMean {:.2}deg; worst: {}",
            r.code,
            c.shared,
            c.deform_shared,
            c.wrot_mean,
            if worst.is_empty() {
                "(all <1deg)".to_string()
            } else {
                worst.join(", ")
            }
        );
    }

    // --- Skeleton families: union-find over mutually drop-in-compatible heroes.
    let decoded_rows: Vec<&Row> = rows.iter().filter(|r| r.skel.is_some()).collect();
    let n = decoded_rows.len();
    let mut uf: Vec<usize> = (0..n).collect();
    fn find(uf: &mut [usize], x: usize) -> usize {
        let mut x = x;
        while uf[x] != x {
            uf[x] = uf[uf[x]];
            x = uf[x];
        }
        x
    }
    for i in 0..n {
        for j in (i + 1)..n {
            let a = decoded_rows[i].skel.as_ref().unwrap();
            let b = decoded_rows[j].skel.as_ref().unwrap();
            let c = compare(a, b);
            let smaller = a.names.len().min(b.names.len());
            let share_frac = c.shared as f64 / smaller.max(1) as f64;
            if share_frac >= 0.90 && c.wrot_mean < 3.0 {
                let (ra, rb) = (find(&mut uf, i), find(&mut uf, j));
                uf[ra] = rb;
            }
        }
    }
    let mut fams: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for i in 0..n {
        let root = find(&mut uf, i);
        fams.entry(root)
            .or_default()
            .push(decoded_rows[i].code.clone());
    }
    let mut fam_list: Vec<Vec<String>> = fams.into_values().collect();
    fam_list.sort_by_key(|v| std::cmp::Reverse(v.len()));
    println!("\nSkeleton families (mutually >=90% shared names, bind-rot < 3deg):");
    for (i, f) in fam_list.iter().enumerate() {
        println!("  family {i} ({} heroes): {}", f.len(), f.join(", "));
    }

    Ok(())
}
