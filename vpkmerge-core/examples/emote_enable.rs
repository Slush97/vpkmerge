//! Put the unreleased `cosmetic_ability_emote` into every hero's
//! `ESlot_Cosmetic_1` (the "CosmeticMenu" keybind, which ships bound to the
//! hero-vote poster), packing the patched `scripts/heroes.vdata_c` as an addon.
//! Server-authoritative: only takes effect on a local server.
//!
//! Usage: cargo run --release -p vpkmerge-core --example emote_enable -- <pak01_dir.vpk> <out_dir.vpk>

use morphic::kv3::{Seg, Value};

const ENTRY: &str = "scripts/heroes.vdata_c";
const EMOTE: &str = "cosmetic_ability_emote";

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let bytes = vpkmerge_core::read_vpk_entry(&a[1], ENTRY)?;
    let doc = morphic::decode_kv3_resource(&bytes)?;

    let mut edits = Vec::new();
    for (hero, v) in doc.as_object().unwrap_or_default() {
        let slot = v
            .get("m_mapBoundAbilities")
            .and_then(|m| m.get("ESlot_Cosmetic_1"));
        if let Some(Value::String(old)) = slot {
            println!("{hero}: {old} -> {EMOTE}");
            edits.push((
                vec![
                    Seg::Key(hero.clone()),
                    Seg::Key("m_mapBoundAbilities".into()),
                    Seg::Key("ESlot_Cosmetic_1".into()),
                ],
                EMOTE.to_string(),
            ));
        }
    }

    let patched = morphic::patch_kv3_resource_strings_adding(&bytes, &edits)?;
    let check = morphic::decode_kv3_resource(&patched)?;
    let n = check
        .as_object()
        .unwrap_or_default()
        .iter()
        .filter(|(_, v)| {
            matches!(
                v.get("m_mapBoundAbilities").and_then(|m| m.get("ESlot_Cosmetic_1")),
                Some(Value::String(s)) if s == EMOTE
            )
        })
        .count();
    anyhow::ensure!(n == edits.len(), "verify: {n}/{} slots patched", edits.len());

    vpkmerge_core::pack(&[(ENTRY, patched.as_slice())], &a[2])?;
    println!(
        "{n} heroes patched, {} -> {} bytes, wrote {}",
        bytes.len(),
        patched.len(),
        a[2]
    );
    Ok(())
}
