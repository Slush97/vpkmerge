//! Rewire a hero's `emote_01` animgraph slot (build 6722) in
//! `hero_cosmetic.vnmgraph+<variant>.vnmgraph_c`.
//!
//! Each hero gets a clip pool `[level, up, down]`. The first clip replaces
//! whatever Valve bound to the slot (for most heroes a "spicy idle" instead of
//! their emote clip). Extra clips are chosen by where the player is looking when
//! the emote starts (up to five bands): `look_pitch` (degrees, -80 down .. +80 up) ->
//! `CNmFloatRemapNode` -90..90 -> 0..180 -> weighted
//! `CNmParameterizedClipSelectorNode` (the trooper flinch recipe, but keyed on
//! aim: `random_seed` is not re-rolled per emote in game, so a random pick
//! always lands on the same clip). New resource refs also get a RERL precache
//! entry (id = MurmurHash64B of the path, seed 0xEDABCDEF). Clip skeletons were
//! checked to match each graph's.
//!
//! Usage: cargo run --release -p vpkmerge-core --example emote_rewire -- <pak01_dir.vpk> <out_dir.vpk>

use anyhow::{bail, Context, Result};
use morphic::kv3::{Seg, Value};
use morphic::resource::Resource;

const POOLS: &[(&str, &[&str])] = &[
    (
        "inferno",
        &["models/heroes_wip/inferno/clips/hero_emote_idle_01.vnmclip"],
    ),
    (
        "punkgoat",
        &["models/heroes_wip/punkgoat/clips/hero_emote_idle_01.vnmclip"],
    ),
    (
        "fencer",
        &["models/heroes_wip/fencer/clips/ui_hero_emote.vnmclip"],
    ),
    (
        "frank",
        &["models/heroes_wip/frank/clips/ui_hero_emote.vnmclip"],
    ),
    (
        "drifter",
        &["models/heroes_wip/drifter/clips/ui_hero_emote_01.vnmclip"],
    ),
    (
        "pocket",
        &["models/heroes_staging/synth/clips/ui_hero_emote.vnmclip"],
    ),
    (
        "lash",
        &[
            "models/heroes_staging/lash_v2/clips/ui_hero_emote_02.vnmclip",
            "models/heroes_staging/lash_v2/clips/hero_emote_01.vnmclip",
            "models/heroes_staging/lash_v2/clips/ui_hero_emote.vnmclip",
            "models/heroes_staging/lash_v2/clips/out_of_combat_idle.vnmclip",
            "models/heroes_staging/lash_v2/clips/sleep_idle.vnmclip",
        ],
    ),
    (
        "vampirebat",
        &[
            "models/heroes_wip/vampirebat/clips/here_emote_idle_01.vnmclip",
            "models/heroes_wip/vampirebat/clips/ui_hero_spicy_idle.vnmclip",
            "models/heroes_wip/vampirebat/clips/vampirebat_shopping.vnmclip",
            "models/heroes_wip/vampirebat/clips/vampirebat_ui_matchmaking.vnmclip",
        ],
    ),
];

/// Shipped graph whose nodes serve as templates for the remap + selector.
const TEMPLATE_GRAPH: &str = "animgraphs/animgraph2/npc_units/troopers/trooper.vnmgraph_c";

/// Selector options in ascending-pitch order (as pool indices, pool order
/// `[level, up, down, far up, far down]`) and their weights over the remapped
/// 0..180 range (weight 1 = 1 degree). Bands split at +/-15 and +/-45 degrees.
fn pitch_layout(pool_len: usize) -> Result<(Vec<usize>, Vec<u64>)> {
    Ok(match pool_len {
        2 => (vec![0, 1], vec![105, 75]),
        3 => (vec![2, 0, 1], vec![75, 30, 75]),
        4 => (vec![2, 0, 1, 3], vec![75, 30, 30, 45]),
        5 => (vec![4, 2, 0, 1, 3], vec![45, 30, 30, 30, 45]),
        n => bail!("pool of {n} clips has no pitch layout"),
    })
}

fn murmur64b(data: &[u8], seed: u64) -> u64 {
    const M: u32 = 0x5bd1_e995;
    let mix = |k: u32| {
        let k = k.wrapping_mul(M);
        (k ^ (k >> 24)).wrapping_mul(M)
    };
    let word = |c: &[u8]| u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
    #[allow(clippy::cast_possible_truncation)]
    let (mut h1, mut h2) = ((seed as u32) ^ data.len() as u32, (seed >> 32) as u32);
    let mut rest = data;
    while rest.len() >= 8 {
        h1 = h1.wrapping_mul(M) ^ mix(word(&rest[..4]));
        h2 = h2.wrapping_mul(M) ^ mix(word(&rest[4..8]));
        rest = &rest[8..];
    }
    if rest.len() >= 4 {
        h1 = h1.wrapping_mul(M) ^ mix(word(&rest[..4]));
        rest = &rest[4..];
    }
    if !rest.is_empty() {
        for (i, b) in rest.iter().enumerate() {
            h2 ^= u32::from(*b) << (8 * i);
        }
        h2 = h2.wrapping_mul(M);
    }
    h1 = (h1 ^ (h2 >> 18)).wrapping_mul(M);
    h2 = (h2 ^ (h1 >> 22)).wrapping_mul(M);
    h1 = (h1 ^ (h2 >> 17)).wrapping_mul(M);
    h2 = (h2 ^ (h1 >> 19)).wrapping_mul(M);
    (u64::from(h1) << 32) | u64::from(h2)
}

fn resource_id(path: &str) -> u64 {
    murmur64b(path.as_bytes(), 0xEDAB_CDEF)
}

fn u32_at(b: &[u8], o: usize) -> usize {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize
}

fn read_rerl(block: &[u8]) -> Vec<(u64, String)> {
    let entries = u32_at(block, 0);
    (0..u32_at(block, 4))
        .map(|i| {
            let e = entries + i * 16;
            let id = u64::from_le_bytes(block[e..e + 8].try_into().unwrap());
            let s = e + 8 + u32_at(block, e + 8);
            let end = s + block[s..].iter().position(|&c| c == 0).unwrap();
            (id, String::from_utf8_lossy(&block[s..end]).into_owned())
        })
        .collect()
}

fn write_rerl(refs: &[(u64, String)]) -> Vec<u8> {
    const ENTRIES_AT: usize = 16;
    let mut out = vec![0u8; ENTRIES_AT + refs.len() * 16];
    out[0..4].copy_from_slice(&u32::try_from(ENTRIES_AT).unwrap().to_le_bytes());
    out[4..8].copy_from_slice(&u32::try_from(refs.len()).unwrap().to_le_bytes());
    for (i, (id, name)) in refs.iter().enumerate() {
        let e = ENTRIES_AT + i * 16;
        out[e..e + 8].copy_from_slice(&id.to_le_bytes());
        let rel = u32::try_from(out.len() - (e + 8)).unwrap();
        out[e + 8..e + 12].copy_from_slice(&rel.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.push(0);
    }
    out
}

fn int(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Int(i) => Some(*i),
        Value::UInt(u) => i64::try_from(*u).ok(),
        _ => None,
    }
}

fn arr<'a>(doc: &'a Value, key: &str) -> Result<&'a [Value]> {
    match doc.get(key) {
        Some(Value::Array(a)) => Ok(a),
        _ => bail!("no {key}"),
    }
}

fn with(mut node: Value, key: &str, v: Value) -> Result<Value> {
    let Value::Object(fields) = &mut node else {
        bail!("node is not an object")
    };
    let slot = fields
        .iter_mut()
        .find(|(k, _)| k == key)
        .with_context(|| format!("no field {key}"))?;
    slot.1 = v;
    Ok(node)
}

/// (`m_nodes` index of the `emote_01` selector, option position, clip node index).
fn emote_option(doc: &Value) -> Result<(usize, usize, usize)> {
    let nodes = arr(doc, "m_nodes")?;
    for (sel, n) in nodes.iter().enumerate() {
        let (Some(Value::Array(ids)), Some(Value::Array(opts))) =
            (n.get("m_optionIDs"), n.get("m_optionNodeIndices"))
        else {
            continue;
        };
        if let Some(i) = ids.iter().position(|s| s.as_str() == Some("emote_01")) {
            let clip = usize::try_from(int(opts.get(i)).context("option idx")?)?;
            return Ok((sel, i, clip));
        }
    }
    bail!("no emote_01 selector")
}

fn slot_of(doc: &Value, clip_node: usize) -> Result<usize> {
    let slot = int(arr(doc, "m_nodes")?[clip_node].get("m_nDataSlotIdx")).context("slot")?;
    usize::try_from(slot).context("emote_01 clip node is unbound")
}

fn class_template(doc: &Value, class: &str) -> Result<Value> {
    arr(doc, "m_nodes")?
        .iter()
        .find(|n| n.get("_class").and_then(Value::as_str) == Some(class))
        .cloned()
        .with_context(|| format!("template graph has no {class}"))
}

fn node_path(doc: &Value, name: &str) -> Result<i64> {
    let i = arr(doc, "m_nodePaths")?
        .iter()
        .position(|p| p.as_str() == Some(name))
        .with_context(|| format!("no node {name}"))?;
    Ok(i64::try_from(i)?)
}

fn rewire(
    pak: &str,
    variant: &str,
    pool: &[&str],
    remap_t: &Value,
    selector_t: &Value,
) -> Result<Vec<u8>> {
    let entry = format!("animgraphs/animgraph2/hero/hero_cosmetic.vnmgraph+{variant}.vnmgraph_c");
    let mut bytes = vpkmerge_core::read_vpk_entry(pak, &entry)?;
    let doc = morphic::decode_kv3_resource(&bytes)?;
    let (sel, opt, clip_node) = emote_option(&doc)?;
    let slot = slot_of(&doc, clip_node)?;
    let old = arr(&doc, "m_resources")?[slot]
        .as_str()
        .context("resource")?
        .to_string();
    let mut rerl_edit: Vec<(Option<String>, String)> = Vec::new();

    if old != pool[0] {
        let path = vec![Seg::Key("m_resources".into()), Seg::Index(slot)];
        bytes = morphic::patch_kv3_resource_strings_adding(&bytes, &[(path, pool[0].to_string())])?;
        rerl_edit.push((Some(old.clone()), pool[0].to_string()));
    }

    if pool.len() > 1 {
        let res_base = arr(&doc, "m_resources")?.len();
        let node_base = arr(&doc, "m_nodes")?.len();
        let base_path = arr(&doc, "m_nodePaths")?[clip_node]
            .as_str()
            .context("path")?
            .to_string();
        let n = pool.len() - 1;
        let (order, weights) = pitch_layout(pool.len())?;
        let clip_node_for = |p: usize| if p == 0 { clip_node } else { node_base + 1 + p };
        let range = |a: f64, b: f64| {
            Value::Object(vec![
                ("m_flBegin".into(), Value::Double(a)),
                ("m_flEnd".into(), Value::Double(b)),
            ])
        };

        let remap = with(
            remap_t.clone(),
            "m_nInputValueNodeIdx",
            Value::Int(node_path(&doc, "look_pitch")?),
        )?;
        let remap = with(
            with(remap, "m_inputRange", range(-90.0, 90.0))?,
            "m_outputRange",
            range(0.0, 180.0),
        )?;
        let options = order
            .iter()
            .map(|&p| Value::Int(i64::try_from(clip_node_for(p)).unwrap()))
            .collect();
        let selector = with(
            selector_t.clone(),
            "m_optionNodeIndices",
            Value::Array(options),
        )?;
        let selector = with(
            selector,
            "m_optionWeights",
            Value::Array(weights.into_iter().map(Value::UInt).collect()),
        )?;
        let selector = with(
            selector,
            "m_parameterNodeIdx",
            Value::Int(i64::try_from(node_base)?),
        )?;
        let mut new_nodes = vec![
            (format!("{base_path}/By Pitch/Float Remap"), remap),
            (
                format!("{base_path}/By Pitch/Parameterized Clip Selector"),
                selector,
            ),
        ];
        let clip_t = arr(&doc, "m_nodes")?[clip_node].clone();
        for k in 0..n {
            new_nodes.push((
                format!("{base_path}/By Pitch/Clip {}", k + 2),
                with(
                    clip_t.clone(),
                    "m_nDataSlotIdx",
                    Value::Int(i64::try_from(res_base + k)?),
                )?,
            ));
        }

        for (k, clip) in pool[1..].iter().enumerate() {
            let at = [Seg::Key("m_resources".into())];
            bytes = morphic::patch_kv3_resource_array_insert(
                &bytes,
                &at,
                res_base + k,
                &Value::String((*clip).into()),
            )?;
            rerl_edit.push((None, (*clip).to_string()));
        }
        for (k, (path, node)) in new_nodes.into_iter().enumerate() {
            // Cloned templates keep their source index; the runtime places each
            // node instance by m_nNodeIdx, so a stale one crashes on graph load.
            let node = with(
                node,
                "m_nNodeIdx",
                Value::Int(i64::try_from(node_base + k)?),
            )?;
            bytes = morphic::patch_kv3_resource_array_insert(
                &bytes,
                &[Seg::Key("m_nodes".into())],
                node_base + k,
                &node,
            )?;
            bytes = morphic::patch_kv3_resource_array_insert(
                &bytes,
                &[Seg::Key("m_nodePaths".into())],
                node_base + k,
                &Value::String(path),
            )?;
        }
        let redirect = vec![
            Seg::Key("m_nodes".into()),
            Seg::Index(sel),
            Seg::Key("m_optionNodeIndices".into()),
            Seg::Index(opt),
        ];
        bytes = morphic::patch_kv3_resource_scalars(
            &bytes,
            &[(redirect, i64::try_from(node_base + 1)?)],
        )?;
    }

    let r = Resource::parse(&bytes)?;
    let rerl_idx = r
        .blocks()
        .iter()
        .position(|b| &b.kind == b"RERL")
        .context("no RERL")?;
    let mut refs = read_rerl(r.get_block_by_index(rerl_idx).context("RERL bytes")?);
    for (old, new) in &rerl_edit {
        let entry = (resource_id(new), new.clone());
        match old {
            Some(old) => {
                let hit = refs
                    .iter_mut()
                    .find(|(_, n)| n == old)
                    .context("old clip not in RERL")?;
                anyhow::ensure!(
                    hit.0 == resource_id(old),
                    "{variant}: RERL hash check failed"
                );
                *hit = entry;
            }
            None => refs.push(entry),
        }
    }
    let out = r.rebuild_with_block(rerl_idx, &write_rerl(&refs))?;
    verify(&out, variant, pool, &doc)?;
    Ok(out)
}

/// Re-decode and walk emote_01 -> (selector ->) clip nodes -> resources; every
/// pooled clip must be reachable and precached, and nothing else may move.
fn verify(out: &[u8], variant: &str, pool: &[&str], before: &Value) -> Result<()> {
    let doc = morphic::decode_kv3_resource(out)?;
    let nodes = arr(&doc, "m_nodes")?;
    let res = arr(&doc, "m_resources")?;
    let (_, _, target) = emote_option(&doc)?;
    let (clip_nodes, expected): (Vec<usize>, Vec<&str>) = match nodes[target]
        .get("m_optionNodeIndices")
    {
        Some(Value::Array(o)) if pool.len() > 1 => {
            let remap =
                usize::try_from(int(nodes[target].get("m_parameterNodeIdx")).context("param")?)?;
            let input = int(nodes[remap].get("m_nInputValueNodeIdx")).context("remap input")?;
            anyhow::ensure!(
                before.get("m_nodePaths").and_then(|p| match p {
                    Value::Array(a) => a.get(usize::try_from(input).ok()?)?.as_str(),
                    _ => None,
                }) == Some("look_pitch")
            );
            let Some(Value::Array(w)) = nodes[target].get("m_optionWeights") else {
                bail!("{variant}: no weights")
            };
            let total: i64 = w.iter().filter_map(|v| int(Some(v))).sum();
            anyhow::ensure!(total == 180, "{variant}: weights sum {total}, want 180");
            let (order, _) = pitch_layout(pool.len())?;
            (
                o.iter()
                    .map(|v| usize::try_from(int(Some(v)).unwrap()).unwrap())
                    .collect(),
                order.iter().map(|&p| pool[p]).collect(),
            )
        }
        _ => (vec![target], pool.to_vec()),
    };
    let got: Vec<&str> = clip_nodes
        .iter()
        .map(|&c| res[slot_of(&doc, c).unwrap()].as_str().unwrap())
        .collect();
    anyhow::ensure!(got == expected, "{variant}: option mismatch {got:?}");
    anyhow::ensure!(
        nodes.len() == arr(&doc, "m_nodePaths")?.len(),
        "node/path count mismatch"
    );
    for (i, n) in nodes.iter().enumerate() {
        anyhow::ensure!(
            int(n.get("m_nNodeIdx")) == Some(i64::try_from(i)?),
            "{variant}: node {i} has m_nNodeIdx {:?}",
            int(n.get("m_nNodeIdx"))
        );
    }
    let before_nodes = arr(before, "m_nodes")?;
    for (i, n) in before_nodes.iter().enumerate() {
        anyhow::ensure!(
            n == &nodes[i] || n.get("m_optionIDs").is_some(),
            "{variant}: node {i} changed unexpectedly"
        );
    }
    let refs = read_rerl(
        Resource::parse(out)?
            .blocks()
            .iter()
            .position(|b| &b.kind == b"RERL")
            .and_then(|i| Resource::parse(out).ok()?.get_block_by_index(i))
            .context("RERL")?,
    );
    for r in res {
        let name = r.as_str().context("res")?;
        anyhow::ensure!(
            refs.iter()
                .any(|(id, n)| n == name && *id == resource_id(name)),
            "{variant}: {name} not precached"
        );
    }
    Ok(())
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let template =
        morphic::decode_kv3_resource(&vpkmerge_core::read_vpk_entry(&a[1], TEMPLATE_GRAPH)?)?;
    let remap_t = class_template(&template, "CNmFloatRemapNode::CDefinition")?;
    let selector_t = class_template(&template, "CNmParameterizedClipSelectorNode::CDefinition")?;

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for (variant, pool) in POOLS {
        let out = rewire(&a[1], variant, pool, &remap_t, &selector_t)?;
        println!("{variant}: {} clip(s) -> {}", pool.len(), pool.join(", "));
        files.push((
            format!("animgraphs/animgraph2/hero/hero_cosmetic.vnmgraph+{variant}.vnmgraph_c"),
            out,
        ));
    }
    let packed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(e, b)| (e.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&packed, &a[2])?;
    println!("wrote {}", a[2]);
    Ok(())
}
