//! Exercise the commands Grimoire uses against current-style catalog data.

use std::path::Path;
use std::process::{Command, Output};

use anyhow::Result;
use serde_json::{json, Value};
use vpkmerge_core::soundevents::SoundEvents;

fn resource(root: Value) -> Result<Vec<u8>> {
    let template = include_bytes!("../../morphic/fixtures/kv3/gigawatt.vsndevts_c");
    let mut events = SoundEvents::from_bytes(template.to_vec())?;
    events.replace_from_json(root);
    events.encode()
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vpkmerge"))
        .args(args)
        .output()
        .unwrap()
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn output_json(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn voices() -> Result<Vec<u8>> {
    resource(json!({
        "atlas_self_death_01_hero_3d": {
            "context_name": "hero_atlas",
            "vsnd_files": "sounds/vo/atlas/death.vsnd"
        },
        "yamato_self_death_01_hero_3d": {
            "context_name": "hero_yamato",
            "vsnd_files": ["sounds/vo/yamato/death.vsnd"]
        }
    }))
}

#[test]
fn voice_filter_finds_current_context_without_captions() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let pak = dir.path().join("game_dir.vpk");
    let voices = voices()?;
    vpkmerge_core::pack(
        &[("soundevents/vo/generated_vo_hero_atlas.vsndevts_c", &voices)],
        &pak,
    )?;
    let result = output_json(&run(&[
        "catalog",
        "voiceline",
        "--vpk",
        path(&pak),
        "--hero",
        "atlas",
        "--json",
    ]));
    assert_eq!(result.as_array().unwrap().len(), 1);
    assert_eq!(result[0]["hero"], "atlas");
    assert_eq!(result[0]["label"], "self death");
    assert_eq!(result[0]["vsnd"][0], "sounds/vo/atlas/death.vsnd");
    assert_eq!(result[0]["caption"], Value::Null);
    Ok(())
}

#[test]
fn cache_warms_both_indexes_without_captions_then_reuses_them() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let pak = dir.path().join("game_dir.vpk");
    let cache = dir.path().join("cache");
    let voices = voices()?;
    vpkmerge_core::pack(
        &[
            ("soundevents/vo/generated_vo_hero_atlas.vsndevts_c", &voices),
            ("panorama/images/items/example.vtex_c", b"index-only"),
        ],
        &pak,
    )?;
    let args = [
        "catalog",
        "cache",
        "--vpk",
        path(&pak),
        "--dir",
        path(&cache),
        "--json",
    ];
    let cold = output_json(&run(&args));
    assert_eq!(cold["voiceline"]["count"], 2);
    assert_eq!(cold["texture"]["count"], 1);
    assert_eq!(cold["voiceline"]["cacheHit"], false);
    assert_eq!(cold["texture"]["cacheHit"], false);
    let warm = output_json(&run(&args));
    assert_eq!(warm["fingerprint"], cold["fingerprint"]);
    assert_eq!(warm["voiceline"]["cacheHit"], true);
    assert_eq!(warm["texture"]["cacheHit"], true);
    Ok(())
}

#[test]
fn hero_picker_receives_released_heroes_from_development_state() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let pak = dir.path().join("game_dir.vpk");
    let heroes = resource(json!({
        "hero_base": {"m_HeroID": 0},
        "hero_atlas": {"m_HeroID": 6, "m_eHeroDevelopmentState": "EHeroDevState_Release"},
        "hero_yamato": {"m_HeroID": 27, "m_eHeroDevelopmentState": "EHeroDevState_PreRelease"}
    }))?;
    vpkmerge_core::pack(&[("scripts/heroes.vdata_c", &heroes)], &pak)?;
    let selectable = output_json(&run(&["catalog", "heroes", "--vpk", path(&pak), "--json"]));
    assert_eq!(selectable.as_array().unwrap().len(), 1);
    assert_eq!(selectable[0]["codename"], "atlas");
    assert_eq!(selectable[0]["selectable"], true);
    let all = output_json(&run(&[
        "catalog",
        "heroes",
        "--vpk",
        path(&pak),
        "--all",
        "--json",
    ]));
    assert_eq!(all.as_array().unwrap().len(), 2);
    assert_eq!(all[1]["codename"], "yamato");
    assert_eq!(all[1]["selectable"], false);
    Ok(())
}
