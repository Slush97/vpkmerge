//! Exercise roster schema compatibility through a real KV3 resource and VPK.

use std::path::Path;

use morphic::kv3::Value;
use vpkmerge_core::localization::{build_hero_roster, HeroInfo};

fn object(fields: &[(&str, Value)]) -> Value {
    Value::Object(
        fields
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect(),
    )
}

fn state(value: &str) -> Value {
    Value::String(format!("EHeroDevState_{value}"))
}

fn fixture(dir: &Path, root: &Value) -> std::path::PathBuf {
    // The existing fixture supplies a valid resource envelope, not roster data.
    let template = include_bytes!("../../morphic/fixtures/kv3/gigawatt.vsndevts_c");
    let bytes = morphic::encode_kv3_resource(template, root).unwrap();
    let pak = dir.join("heroes_dir.vpk");
    vpkmerge_core::pack(&[("scripts/heroes.vdata_c", &bytes)], &pak).unwrap();
    pak
}

fn hero(codename: &str, selectable: bool, in_development: bool, disabled: bool) -> HeroInfo {
    HeroInfo {
        codename: codename.to_owned(),
        name: codename.to_owned(),
        selectable,
        in_development,
        disabled,
    }
}

#[test]
fn legacy_roster_preserves_availability_and_localized_names() {
    let dir = tempfile::tempdir().unwrap();
    let root = object(&[
        (
            "hero_yamato",
            object(&[
                ("m_bPlayerSelectable", Value::Bool(false)),
                ("m_bInDevelopment", Value::Bool(true)),
                ("m_bDisabled", Value::Bool(true)),
            ]),
        ),
        (
            "hero_atlas",
            object(&[("m_bPlayerSelectable", Value::Bool(true))]),
        ),
    ]);
    let pak = fixture(dir.path(), &root);
    let loc = dir.path().join("citadel_gc_hero_names");
    std::fs::create_dir_all(&loc).unwrap();
    std::fs::write(
        loc.join("citadel_gc_hero_names_english.txt"),
        r#""lang" { "Tokens" { "hero_atlas:n" "Abrams" } }"#,
    )
    .unwrap();

    let roster = build_hero_roster(&pak, Some(dir.path()), "english").unwrap();
    let mut atlas = hero("atlas", true, false, false);
    atlas.name = "Abrams".to_owned();
    assert_eq!(roster, vec![atlas, hero("yamato", false, true, true)]);
}

#[test]
fn current_roster_uses_development_state_and_preserves_explicit_flags() {
    let dir = tempfile::tempdir().unwrap();
    // These fields mirror the installed October 2026 game. Release and
    // PreRelease do not necessarily agree with m_bInDevelopment.
    let root = object(&[
        (
            "hero_deadpack",
            object(&[
                ("m_HeroID", Value::Int(78)),
                ("m_eHeroDevelopmentState", state("PreRelease")),
                ("m_bInDevelopment", Value::Bool(false)),
                ("m_bPlayerSelectable", Value::Bool(true)),
            ]),
        ),
        (
            "hero_baba",
            object(&[
                ("m_HeroID", Value::Int(88)),
                ("m_eHeroDevelopmentState", state("Release")),
                ("m_bInDevelopment", Value::Bool(true)),
            ]),
        ),
        (
            "hero_atlas",
            object(&[
                ("m_HeroID", Value::Int(6)),
                ("m_eHeroDevelopmentState", state("Release")),
                ("m_bPlayerSelectable", Value::Bool(false)),
            ]),
        ),
        (
            "hero_kali",
            object(&[
                ("m_HeroID", Value::Int(21)),
                ("m_bDisabled", Value::Bool(true)),
                ("m_bInDevelopment", Value::Bool(false)),
            ]),
        ),
        (
            "hero_disabled_release",
            object(&[
                ("m_HeroID", Value::Int(99)),
                ("m_eHeroDevelopmentState", state("Release")),
                ("m_bDisabled", Value::Bool(true)),
            ]),
        ),
        (
            "hero_prerelease_without_flag",
            object(&[
                ("m_HeroID", Value::Int(100)),
                ("m_eHeroDevelopmentState", state("PreRelease")),
            ]),
        ),
    ]);
    let pak = fixture(dir.path(), &root);

    assert_eq!(
        build_hero_roster(&pak, None, "english").unwrap(),
        vec![
            hero("atlas", true, false, false),
            hero("baba", true, true, false),
            hero("deadpack", false, false, false),
            hero("disabled_release", true, false, true),
            hero("kali", false, false, true),
            hero("prerelease_without_flag", false, true, false),
        ]
    );
}

#[test]
fn helpers_and_malformed_nodes_do_not_become_selectable_heroes() {
    let dir = tempfile::tempdir().unwrap();
    let root = object(&[
        (
            "hero_base",
            object(&[
                ("m_HeroID", Value::Int(1)),
                ("m_eHeroDevelopmentState", state("Release")),
            ]),
        ),
        (
            "hero_targetdummy",
            object(&[
                ("m_HeroID", Value::Int(55)),
                ("m_eHeroDevelopmentState", state("DebugOnly")),
            ]),
        ),
        (
            "hero_other_debug_rig",
            object(&[
                ("m_HeroID", Value::Int(101)),
                ("m_eHeroDevelopmentState", state("DebugOnly")),
                ("m_bPlayerSelectable", Value::Bool(true)),
            ]),
        ),
        (
            "hero_template",
            object(&[("m_eHeroDevelopmentState", state("Release"))]),
        ),
        (
            "hero_zero_id",
            object(&[
                ("m_HeroID", Value::Int(0)),
                ("m_eHeroDevelopmentState", state("Release")),
            ]),
        ),
        ("hero_scalar", Value::Bool(true)),
        (
            "hero_bad_legacy_flag",
            object(&[("m_bPlayerSelectable", Value::String("true".to_owned()))]),
        ),
        (
            "hero_unknown_state",
            object(&[
                ("m_HeroID", Value::Int(102)),
                ("m_eHeroDevelopmentState", state("FutureState")),
                ("m_bPlayerSelectable", Value::Bool(true)),
            ]),
        ),
        (
            "hero_malformed_state",
            object(&[
                ("m_HeroID", Value::Int(103)),
                ("m_eHeroDevelopmentState", Value::Int(1)),
                ("m_bPlayerSelectable", Value::Bool(true)),
            ]),
        ),
        ("generic_data_type", object(&[("m_HeroID", Value::Int(1))])),
    ]);
    let pak = fixture(dir.path(), &root);

    assert_eq!(
        build_hero_roster(&pak, None, "english").unwrap(),
        vec![
            hero("malformed_state", false, false, false),
            hero("unknown_state", false, false, false),
        ]
    );
}

#[test]
fn malformed_roster_root_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let pak = fixture(dir.path(), &Value::Array(vec![]));
    let error = build_hero_roster(&pak, None, "english").unwrap_err();
    assert!(error.to_string().contains("root is not an object"));
}
