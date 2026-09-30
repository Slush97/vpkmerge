//! Yamato per-ability VFX recolor: abilities 1-3 (Power Slash, Flying Strike,
//! Crimson Slash) to one color, the ultimate (Shadow Transformation) to another.
//!
//! `recolor-hero` lands one hue on the whole recipe; this splits the same recipe
//! by ability slot. Slots are resolved from `scripts/abilities.vdata_c` roots
//! (`citadel_ability_power_slash` / `_flying_strike` / `_healing_slash` /
//! `_infinity_slash`, bound to Yamato in `heroes.vdata_c`), then closed over each
//! particle's `m_ChildRef` tree, with a filename-keyword fallback for particles
//! the game spawns from code. Anything that matches no slot (primary tracer,
//! alt-fire explosive dart, legacy unused systems) is left vanilla.
//!
//! ```text
//! # dry run: prints the grouping + source colors, writes nothing
//! cargo run --release -p vpkmerge-core --example yamato_ability_colors -- <pak01_dir.vpk>
//! # bake (then re-reads the addon and prints each slot's resulting colors)
//! ... -- <pak01_dir.vpk> --out <out_dir.vpk> [--abilities H[,S[,V]]] [--ult H[,S[,V]]]
//!        [--preview-dir DIR]
//! ```
//!
//! Colors are `recolor-hero` semantics: absolute hue, saturation/value scales.
//! Defaults: abilities 215,1.2,1.0 (blue), ult 43,0.65,1.15 (light gold).

use anyhow::{bail, Context, Result};
use morphic::kv3::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use vpkmerge_core::{
    recipe_for, recolor_particle_bytes, recolor_texture_hue, recolor_texture_preview_png, Recolor,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Slot {
    PowerSlash,
    FlyingStrike,
    CrimsonSlash,
    Ult,
}

impl Slot {
    const ALL: [Slot; 4] = [
        Slot::PowerSlash,
        Slot::FlyingStrike,
        Slot::CrimsonSlash,
        Slot::Ult,
    ];

    fn label(self) -> &'static str {
        match self {
            Slot::PowerSlash => "1 Power Slash",
            Slot::FlyingStrike => "2 Flying Strike",
            Slot::CrimsonSlash => "3 Crimson Slash",
            Slot::Ult => "4 Shadow Transformation",
        }
    }

    fn is_ult(self) -> bool {
        self == Slot::Ult
    }
}

/// Particle roots per slot, verbatim from `scripts/abilities.vdata_c`.
const ROOTS: &[(Slot, &str)] = &[
    (
        Slot::PowerSlash,
        "particles/abilities/yamato/yamato_power_slash_impact.vpcf_c",
    ),
    (
        Slot::PowerSlash,
        "particles/abilities/yamato/yamato_power_slash_charge.vpcf_c",
    ),
    (
        Slot::PowerSlash,
        "particles/abilities/yamato/yamato_power_slash_pulse.vpcf_c",
    ),
    (
        Slot::PowerSlash,
        "particles/abilities/yamato/yamato_blade_dash.vpcf_c",
    ),
    (
        Slot::PowerSlash,
        "particles/abilities/yamato/yamato_blade_dash_full.vpcf_c",
    ),
    (
        Slot::FlyingStrike,
        "particles/abilities/yamato/yamato_flying_strike_start.vpcf_c",
    ),
    (
        Slot::FlyingStrike,
        "particles/abilities/yamato/yamato_flying_strike_impact.vpcf_c",
    ),
    (
        Slot::FlyingStrike,
        "particles/abilities/yamato/yamato_flying_strike_hit.vpcf_c",
    ),
    (
        Slot::FlyingStrike,
        "particles/abilities/yamato/yamato_flying_strike_grapple_return.vpcf_c",
    ),
    (
        Slot::FlyingStrike,
        "particles/abilities/yamato/yamato_flying_strike_rope_tracer.vpcf_c",
    ),
    (
        Slot::FlyingStrike,
        "particles/abilities/yamato/yamato_flying_strike_preview.vpcf_c",
    ),
    (
        Slot::CrimsonSlash,
        "particles/abilities/yamato/yamato_crimson_slash_hit.vpcf_c",
    ),
    (
        Slot::CrimsonSlash,
        "particles/abilities/yamato/yamato_crimson_slash_blade_glow.vpcf_c",
    ),
    (
        Slot::CrimsonSlash,
        "particles/abilities/yamato/yamato_crimson_slash.vpcf_c",
    ),
    (
        Slot::CrimsonSlash,
        "particles/abilities/yamato/yamato_crimson_slash_preview.vpcf_c",
    ),
    (
        Slot::Ult,
        "particles/abilities/yamato/yamato_infinity_slash_invincible_buff_timer.vpcf_c",
    ),
    (
        Slot::Ult,
        "particles/abilities/yamato/yamato_infinity_slash_start.vpcf_c",
    ),
    (
        Slot::Ult,
        "particles/abilities/yamato/yamato_infinity_slash_seppuku.vpcf_c",
    ),
    (
        Slot::Ult,
        "particles/abilities/yamato/yamato_shadow_form_buff_timer.vpcf_c",
    ),
    (
        Slot::Ult,
        "particles/status_fx/status_fx_yamato_shadow_form.vpcf_c",
    ),
];

/// Filename keywords for particles spawned from code (no vdata root) and for
/// textures. Checked against the file stem.
const KEYWORDS: &[(Slot, &[&str])] = &[
    (Slot::PowerSlash, &["power_slash", "blade_dash"]),
    (Slot::FlyingStrike, &["flying_strike"]),
    (Slot::CrimsonSlash, &["crimson_slash", "healing_slash"]),
    (
        Slot::Ult,
        &[
            "infinity_slash",
            "shadow_form",
            "shadow_redemption",
            "yamoto_shadow_shape",
            "status_fx_yamato",
        ],
    ),
];

fn keyword_slot(entry: &str) -> Option<Slot> {
    let stem = entry.rsplit('/').next().unwrap_or(entry);
    KEYWORDS
        .iter()
        .find(|(_, kws)| kws.iter().any(|k| stem.contains(k)))
        .map(|(s, _)| *s)
}

fn parse_color(spec: &str) -> Result<Recolor> {
    let parts: Vec<f64> = spec
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .with_context(|| format!("bad color {spec:?}, want H[,S[,V]]"))?;
    match parts.as_slice() {
        [h] => Ok(Recolor::hue(*h)),
        [h, s] => Ok(Recolor::new(*h, *s, 1.0)),
        [h, s, v] => Ok(Recolor::new(*h, *s, *v)),
        _ => bail!("bad color {spec:?}, want H[,S[,V]]"),
    }
}

struct Vpks(Vec<valve_pak::VPK>);

impl Vpks {
    fn read(&self, entry: &str) -> Option<Vec<u8>> {
        self.0
            .iter()
            .find_map(|v| v.get_file(entry).ok()?.read_all().ok())
    }

    fn list(&self, prefixes: &[String], suffix: &str) -> Vec<String> {
        let mut seen = BTreeSet::new();
        for v in &self.0 {
            for p in v.file_paths() {
                if p.ends_with(suffix) && prefixes.iter().any(|pre| p.starts_with(pre.as_str())) {
                    seen.insert(p.clone());
                }
            }
        }
        seen.into_iter().collect()
    }
}

/// Every `*.vpcf` string in the tree (child refs), as `.vpcf_c` entry paths.
fn child_refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(pairs) => pairs.iter().for_each(|(_, c)| child_refs(c, out)),
        Value::Array(items) => items.iter().for_each(|c| child_refs(c, out)),
        Value::String(s) if s.ends_with(".vpcf") => out.push(format!("{s}_c")),
        _ => {}
    }
}

/// Source color stats, mirroring the recolor's field selection (arrays under a
/// key containing "color"/"tint"): count, circular-mean hue of chromatic
/// entries, mean saturation, mean value.
#[derive(Default)]
struct ColorStats {
    n: usize,
    hx: f64,
    hy: f64,
    chroma_n: usize,
    s: f64,
    v: f64,
}

impl ColorStats {
    fn add(&mut self, rgb: [f64; 3]) {
        let max = rgb[0].max(rgb[1]).max(rgb[2]);
        let min = rgb[0].min(rgb[1]).min(rgb[2]);
        if max < 0.02 {
            return; // black literal defaults carry no color
        }
        let d = max - min;
        let s = d / max;
        self.n += 1;
        self.s += s;
        self.v += max;
        if s > 0.15 {
            let h = if max == rgb[0] {
                60.0 * ((rgb[1] - rgb[2]) / d).rem_euclid(6.0)
            } else if max == rgb[1] {
                60.0 * ((rgb[2] - rgb[0]) / d + 2.0)
            } else {
                60.0 * ((rgb[0] - rgb[1]) / d + 4.0)
            };
            self.hx += h.to_radians().cos();
            self.hy += h.to_radians().sin();
            self.chroma_n += 1;
        }
    }

    fn summary(&self) -> String {
        if self.n == 0 {
            return "no visible color fields".to_string();
        }
        #[allow(clippy::cast_precision_loss)]
        let n = self.n as f64;
        let hue = if self.chroma_n == 0 {
            "n/a".to_string()
        } else {
            format!(
                "{:.0}",
                self.hy.atan2(self.hx).to_degrees().rem_euclid(360.0)
            )
        };
        format!(
            "{} fields, mean hue {hue} ({} chromatic), mean sat {:.2}, mean val {:.2}",
            self.n,
            self.chroma_n,
            self.s / n,
            self.v / n
        )
    }
}

fn as_rgb(v: &Value) -> Option<[f64; 3]> {
    let Value::Array(items) = v else { return None };
    if items.len() != 3 && items.len() != 4 {
        return None;
    }
    let mut out = [0.0; 3];
    for (i, it) in items.iter().take(3).enumerate() {
        let n = match it {
            Value::Int(n) if (0..=255).contains(n) => *n,
            Value::UInt(u) if *u <= 255 => i64::try_from(*u).ok()?,
            _ => return None,
        };
        #[allow(clippy::cast_precision_loss)]
        {
            out[i] = n as f64 / 255.0;
        }
    }
    Some(out)
}

fn color_stats(v: &Value, colorish: bool, stats: &mut ColorStats) {
    if colorish {
        if let Some(rgb) = as_rgb(v) {
            stats.add(rgb);
            return;
        }
    }
    match v {
        Value::Object(pairs) => {
            for (k, c) in pairs {
                let kl = k.to_lowercase();
                color_stats(c, kl.contains("color") || kl.contains("tint"), stats);
            }
        }
        Value::Array(items) => items.iter().for_each(|c| color_stats(c, colorish, stats)),
        _ => {}
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut vpk_paths = Vec::new();
    let mut out: Option<String> = None;
    let mut abilities = Recolor::new(215.0, 1.2, 1.0);
    let mut ult = Recolor::new(43.0, 0.65, 1.15);
    let mut preview_dir: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--base" => vpk_paths.push(val()?.clone()),
            "--out" => out = Some(val()?.clone()),
            "--abilities" => abilities = parse_color(val()?)?,
            "--ult" => ult = parse_color(val()?)?,
            "--preview-dir" => preview_dir = Some(val()?.clone()),
            _ => vpk_paths.insert(0, a.clone()),
        }
    }
    if vpk_paths.is_empty() {
        bail!("usage: yamato_ability_colors <pak01_dir.vpk> [--base VPK] [--out OUT_dir.vpk] [--abilities H,S,V] [--ult H,S,V] [--preview-dir DIR]");
    }
    let vpks = Vpks(
        vpk_paths
            .iter()
            .map(|p| valve_pak::open(p).with_context(|| format!("opening {p}")))
            .collect::<Result<_>>()?,
    );
    let recipe = recipe_for("yamato").context("yamato recipe")?;

    // Decode every recipe particle once: child graph + source color stats.
    let particles = vpks.list(&recipe.particle_prefixes, ".vpcf_c");
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut trees: BTreeMap<String, Value> = BTreeMap::new();
    for entry in &particles {
        let bytes = vpks
            .read(entry)
            .with_context(|| format!("reading {entry}"))?;
        let tree = morphic::decode_kv3_resource(&bytes)
            .map_err(|e| anyhow::anyhow!("decoding {entry}: {e}"))?;
        let mut refs = Vec::new();
        child_refs(&tree, &mut refs);
        children.insert(entry.clone(), refs);
        trees.insert(entry.clone(), tree);
    }

    // Assign slots: BFS from vdata roots over child refs, then keyword fallback.
    let known: BTreeSet<&String> = particles.iter().collect();
    let mut slot_of: BTreeMap<String, Slot> = BTreeMap::new();
    let mut clashes: Vec<String> = Vec::new();
    for &(slot, root) in ROOTS {
        if !known.contains(&root.to_string()) {
            bail!("vdata root {root} is not in the VPK (game update drift?)");
        }
        let mut queue = VecDeque::from([root.to_string()]);
        while let Some(e) = queue.pop_front() {
            if !known.contains(&e) {
                continue; // a shared child outside Yamato's prefixes: never touched
            }
            match slot_of.get(&e) {
                Some(&s) if s == slot => continue,
                Some(&s) => {
                    if s.is_ult() != slot.is_ult() {
                        clashes.push(format!("{e}: {} vs {}", s.label(), slot.label()));
                    }
                    continue;
                }
                None => {}
            }
            slot_of.insert(e.clone(), slot);
            queue.extend(children.get(&e).cloned().unwrap_or_default());
        }
    }
    for entry in &particles {
        if !slot_of.contains_key(entry) {
            if let Some(slot) = keyword_slot(entry) {
                slot_of.insert(entry.clone(), slot);
            }
        }
    }
    if !clashes.is_empty() {
        bail!(
            "particles reached from both an ability and the ult (would get two colors):\n  {}",
            clashes.join("\n  ")
        );
    }

    let textures: Vec<(Slot, &String)> = recipe
        .texture_entries
        .iter()
        .map(|t| {
            keyword_slot(t)
                .map(|s| (s, t))
                .with_context(|| format!("recipe texture {t} matches no ability slot"))
        })
        .collect::<Result<_>>()?;

    for slot in Slot::ALL {
        let members: Vec<&String> = slot_of
            .iter()
            .filter(|(_, &s)| s == slot)
            .map(|(e, _)| e)
            .collect();
        let mut stats = ColorStats::default();
        for e in &members {
            color_stats(&trees[*e], false, &mut stats);
        }
        let tex = textures.iter().filter(|(s, _)| *s == slot).count();
        println!(
            "{:<24} {:>3} particles, {tex} textures | source: {}",
            slot.label(),
            members.len(),
            stats.summary()
        );
    }
    let untouched: Vec<&String> = particles
        .iter()
        .filter(|e| !slot_of.contains_key(*e))
        .collect();
    println!(
        "{:<24} {:>3} particles (left vanilla)",
        "unassigned",
        untouched.len()
    );
    for e in &untouched {
        println!("    {e}");
    }

    let Some(out) = out else {
        println!("\ndry run: pass --out <OUT_dir.vpk> to bake");
        return Ok(());
    };

    let color_for = |slot: Slot| if slot.is_ult() { ult } else { abilities };
    let mut packed: Vec<(String, Vec<u8>)> = Vec::new();
    let mut per_slot: BTreeMap<Slot, (usize, usize, usize)> = BTreeMap::new();
    for (entry, &slot) in &slot_of {
        let bytes = vpks.read(entry).expect("listed entry readable");
        let counts = per_slot.entry(slot).or_default();
        match recolor_particle_bytes(&bytes, color_for(slot)) {
            Ok(Some(new)) => {
                packed.push((entry.clone(), new));
                counts.0 += 1;
            }
            Ok(None) => counts.1 += 1,
            Err(e) => {
                counts.2 += 1;
                eprintln!("  note: skipping {entry} (left vanilla): {e:#}");
            }
        }
    }
    for &(slot, entry) in &textures {
        let bytes = vpks
            .read(entry)
            .with_context(|| format!("recipe texture {entry} missing (recipe drift?)"))?;
        let recolor = color_for(slot);
        packed.push((entry.clone(), recolor_texture_hue(&bytes, recolor)?));
        if let Some(dir) = &preview_dir {
            std::fs::create_dir_all(dir)?;
            let stem = entry
                .rsplit('/')
                .next()
                .unwrap_or(entry)
                .trim_end_matches(".vtex_c");
            std::fs::write(
                format!("{dir}/{stem}.png"),
                recolor_texture_preview_png(&bytes, recolor)?,
            )?;
        }
    }
    for slot in Slot::ALL {
        let (done, colorless, skipped) = per_slot.get(&slot).copied().unwrap_or_default();
        let c = color_for(slot);
        println!(
            "{:<24} hue {:.0} sat x{:.2} val x{:.2}: {done} particles recolored, {colorless} color-free, {skipped} unpatchable",
            slot.label(),
            c.hue,
            c.saturation,
            c.value
        );
    }

    let refs: Vec<(&str, &[u8])> = packed
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, &out).with_context(|| format!("packing {out}"))?;
    println!("packed {} entries into {out}", packed.len());

    // Verify: re-read the addon and report each slot's resulting colors.
    let baked = Vpks(vec![
        valve_pak::open(&out).with_context(|| format!("reopening {out}"))?
    ]);
    for slot in Slot::ALL {
        let mut stats = ColorStats::default();
        for (entry, _) in slot_of.iter().filter(|(_, &s)| s == slot) {
            if let Some(bytes) = baked.read(entry) {
                let tree = morphic::decode_kv3_resource(&bytes)
                    .map_err(|e| anyhow::anyhow!("re-decoding baked {entry}: {e}"))?;
                color_stats(&tree, false, &mut stats);
            }
        }
        println!("verify {:<17} {}", slot.label(), stats.summary());
    }
    Ok(())
}
