//! Build an isolated repair-demo installation from local game resources.
//! Usage: `cargo run -p vpkmerge-core --example compatibility_fixture -- GAME_PAK OUTPUT_ROOT`
use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result};
use morphic::{
    kv3::{self, Seg, Value},
    resource::Resource,
};

fn replace(bytes: &[u8], value: &Value, flags: &kv3::ValueFlags) -> Result<Vec<u8>> {
    let container = Resource::parse(bytes)?;
    let format = kv3::Format::from_payload(container.data_block()?)?;
    Ok(container.rebuild_with_data(&kv3::encode_flagged(value, &format, flags))?)
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let game = PathBuf::from(args.next().context("game pak")?);
    let root = PathBuf::from(args.next().context("output root")?);
    anyhow::ensure!(!root.exists(), "Use a new output directory");
    let citadel = root.join("game/citadel");
    std::fs::create_dir_all(citadel.join("addons"))?;
    let model_path = "models/heroes_wip/werewolf/werewolf.vmdl_c";
    let model = vpkmerge_core::read_vpk_entry(&game, model_path)?;
    let data = Resource::parse(&model)?;
    let (mut value, mut flags) = kv3::decode_flagged(data.data_block()?)?;
    let mut base = BTreeMap::new();
    for field in ["m_vecNmSkeletonRefs", "m_animGraph2Refs"] {
        let refs = value.get(field).context("current animation references")?;
        let mut refs = refs.clone();
        refs.for_each_string_mut(&mut |path| {
            if path.ends_with(".vnmskel") || path.ends_with(".vnmgraph") {
                let compiled = format!("{path}_c");
                base.insert(
                    compiled.clone(),
                    vpkmerge_core::read_vpk_entry(&game, &compiled),
                );
            }
        });
        *value.get_mut(field).context("current bindings")? = Value::Array(vec![]);
        flags.retain(|path, _| !path.starts_with(&[Seg::Key(field.into())]));
    }
    let text = value
        .get_mut("m_modelInfo")
        .and_then(|v| v.get_mut("m_keyValueText"))
        .context("Model metadata")?;
    let mut metadata = text.as_str().context("Model metadata text")?.to_owned();
    let start = metadata
        .find("\tTaggedSoundSettings_t")
        .context("Sound binding")?;
    let end = start + metadata[start..].find("\n\t}").context("Sound block end")? + 4;
    for reference in metadata[start..end].split("resource:\"").skip(1) {
        let path = reference.split('"').next().context("Tagged-sound path")?;
        let compiled = format!("{path}_c");
        base.insert(
            compiled.clone(),
            vpkmerge_core::read_vpk_entry(&game, &compiled),
        );
    }
    metadata.replace_range(start..end, "");
    *text = Value::String(metadata);
    let old_model = replace(&model, &value, &flags)?;
    let mut files: Vec<_> = base
        .into_iter()
        .map(|(p, bytes)| bytes.map(|b| (p, b)))
        .collect::<Result<_>>()?;
    files.push((model_path.into(), model));
    let donor = vpkmerge_core::read_vpk_entry(&game, "scripts/abilities.vdata_c")?;
    let ability = |name: &str| {
        (
            name.into(),
            Value::Object(vec![("damage".into(), Value::Int(42))]),
        )
    };
    let current = Value::Object(vec![ability("ability_existing"), ability("ability_new")]);
    let old = Value::Object(vec![ability("ability_existing")]);
    let current = replace(&donor, &current, &kv3::ValueFlags::new())?;
    let old = replace(&donor, &old, &kv3::ValueFlags::new())?;
    files.push(("scripts/abilities.vdata_c".into(), current));
    let loc = "resource/localization/citadel_heroes/citadel_heroes_english.txt";
    let old_loc =
        br#""lang" { "Language" "english" "Tokens" { "ability_existing" "Authored name" } }"#;
    let new_loc = br#""lang" { "Language" "english" "Tokens" { "ability_existing" "Current name" "ability_new" "New ability" } }"#;
    files.push((loc.into(), new_loc.to_vec()));
    let refs: Vec<_> = files
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    vpkmerge_core::pack(&refs, citadel.join("pak01_dir.vpk"))?;
    vpkmerge_core::pack(
        &[
            (model_path, &old_model),
            ("scripts/abilities.vdata_c", &old),
            (loc, old_loc),
            (
                "readme.txt",
                b"Synthetic compatibility fixture. Never install in the real game.",
            ),
        ],
        citadel.join("addons/pak01_dir.vpk"),
    )?;
    std::fs::write(citadel.join("gameinfo.gi"), "\"GameInfo\" { \"game\" \"Citadel\" \"FileSystem\" { \"SearchPaths\" { \"Game\" \"citadel\" } } }")?;
    println!("{}", root.display());
    Ok(())
}
