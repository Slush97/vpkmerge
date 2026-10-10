//! Catalog compatibility across optional caption resources and VO contexts.

use std::path::Path;

use morphic::kv3::Value;
use vpkmerge_core::catalog::{build_voiceline_index, caption_hash, ENGLISH_CAPTIONS_ENTRY};
use vpkmerge_core::catalog_cache::{BuildFingerprint, CatalogCache, CACHE_SCHEMA_VERSION};
use vpkmerge_core::pack;

const HERO_ENTRY: &str = "soundevents/vo/generated_vo_hero_atlas.vsndevts_c";
const CLIP: &str = "sounds/vo/atlas/atlas_self_death_01.vsnd";

fn soundevents(events: &[(&str, Option<&str>)]) -> Vec<u8> {
    // Reuse the committed resource envelope; the test owns every event value.
    let template = include_bytes!("../../morphic/fixtures/kv3/gigawatt.vsndevts_c");
    let root = Value::Object(
        events
            .iter()
            .map(|(name, context)| {
                let mut fields = vec![
                    (
                        "vsnd_files".to_owned(),
                        Value::Array(vec![Value::String(CLIP.to_owned())]),
                    ),
                    ("vsnd_duration".to_owned(), Value::Double(1.5)),
                ];
                if let Some(context) = context {
                    fields.push((
                        "context_name".to_owned(),
                        Value::String((*context).to_owned()),
                    ));
                }
                ((*name).to_owned(), Value::Object(fields))
            })
            .collect(),
    );
    morphic::encode_kv3_resource(template, &root).unwrap()
}

fn caption_resource(event: &str, caption: &str) -> Vec<u8> {
    let text: Vec<u8> = caption
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut bytes = b"VCCD".to_vec();
    for value in [2u32, 1, 8192, 1, 36, caption_hash(event), 0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&u16::try_from(text.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&text);
    bytes
}

#[test]
fn missing_captions_preserves_events_and_normalizes_contexts() {
    let dir = tempfile::tempdir().unwrap();
    let pak = dir.path().join("voices_dir.vpk");
    let bytes = soundevents(&[
        ("atlas_self_death_01_hero_3d", Some("hero_atlas")),
        ("atlas_self_death_02_hero_3d", Some("atlas")),
        ("atlas_self_death_03_hero_3d", None),
        ("atlas_self_death_04_hero_3d", Some("")),
        ("atlas_self_death_05_hero_3d", Some("hero_")),
        ("base", None),
    ]);
    pack(&[(HERO_ENTRY, &bytes)], &pak).unwrap();

    let rows = build_voiceline_index(&pak).unwrap();
    assert_eq!(rows.len(), 5);
    for row in rows {
        assert_eq!(row.hero.as_deref(), Some("atlas"));
        assert_eq!(row.label, "self death");
        assert_eq!(row.caption, None);
        assert_eq!(row.vsnd, [CLIP]);
        assert_eq!(row.duration, Some(1.5));
    }
}

#[test]
fn nonhero_contexts_and_contextless_announcers_remain_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let pak = dir.path().join("voices_dir.vpk");
    let bytes = soundevents(&[
        ("round_start_01", None),
        ("patron_round_start_01", Some("patron")),
    ]);
    pack(&[("soundevents/vo/announcer.vsndevts_c", &bytes)], &pak).unwrap();

    let rows = build_voiceline_index(&pak).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].hero, None);
    assert_eq!(rows[1].hero.as_deref(), Some("patron"));
    assert!(rows.iter().all(|row| row.label == "round start"));
}

#[test]
fn present_captions_still_enrich_voice_events() {
    let dir = tempfile::tempdir().unwrap();
    let pak = dir.path().join("voices_dir.vpk");
    let event = "atlas_self_death_01_hero_3d";
    let voices = soundevents(&[(event, Some("hero_atlas"))]);
    let captions = caption_resource(event, "Not like this!");
    pack(
        &[(HERO_ENTRY, &voices), (ENGLISH_CAPTIONS_ENTRY, &captions)],
        &pak,
    )
    .unwrap();

    let rows = build_voiceline_index(&pak).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].caption.as_deref(), Some("Not like this!"));
}

#[test]
fn malformed_present_captions_remain_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let pak = dir.path().join("voices_dir.vpk");
    let voices = soundevents(&[("atlas_self_death_01_hero_3d", Some("hero_atlas"))]);
    pack(
        &[(HERO_ENTRY, &voices), (ENGLISH_CAPTIONS_ENTRY, b"not VCCD")],
        &pak,
    )
    .unwrap();

    let error = build_voiceline_index(&pak).unwrap_err();
    let diagnostic = format!("{error:#}");
    assert!(diagnostic.contains(ENGLISH_CAPTIONS_ENTRY));
    assert!(diagnostic.contains("bad magic"));
}

/// A valid v1 directory with a caption entry whose external chunk is missing.
fn write_missing_chunk_pak(path: &Path) {
    let mut tree =
        b"dat\0resource/localization/citadel_generated_vo\0citadel_generated_vo_english\0".to_vec();
    tree.extend_from_slice(&0u32.to_le_bytes()); // CRC
    tree.extend_from_slice(&0u16.to_le_bytes()); // no preload
    tree.extend_from_slice(&0u16.to_le_bytes()); // external archive 000
    tree.extend_from_slice(&0u32.to_le_bytes()); // offset
    tree.extend_from_slice(&24u32.to_le_bytes()); // caption header length
    tree.extend_from_slice(&0xffffu16.to_le_bytes());
    tree.extend_from_slice(&[0, 0, 0]);
    let mut bytes = Vec::new();
    for value in [0x55aa_1234u32, 1, u32::try_from(tree.len()).unwrap()] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&tree);
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn unreadable_present_captions_remain_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let pak = dir.path().join("voices_dir.vpk");
    write_missing_chunk_pak(&pak);

    let vpk = valve_pak::open(&pak).unwrap();
    assert!(vpk
        .file_paths()
        .any(|entry| entry == ENGLISH_CAPTIONS_ENTRY));
    let error = build_voiceline_index(&pak).unwrap_err();
    let diagnostic = format!("{error:#}");
    // get_file opens the external chunk before read_all starts.
    assert!(
        diagnostic.contains(&format!("locating {ENGLISH_CAPTIONS_ENTRY}")),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("Failed to open VPK archive"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("voices_000.vpk"), "{diagnostic}");
}

#[test]
fn old_voice_cache_rebuilds_normalized_rows_then_hits() {
    let dir = tempfile::tempdir().unwrap();
    let pak = dir.path().join("voices_dir.vpk");
    let voices = soundevents(&[("atlas_self_death_01_hero_3d", Some("hero_atlas"))]);
    pack(&[(HERO_ENTRY, &voices)], &pak).unwrap();
    let cache_dir = dir.path().join("cache");
    std::fs::create_dir(&cache_dir).unwrap();
    let fingerprint = BuildFingerprint::for_vpk(&pak).unwrap();
    let stale = serde_json::json!({
        "schema": 1,
        "kind": "voiceline",
        "fingerprint": fingerprint,
        "items": [{
            "event": "atlas_self_death_01_hero_3d",
            "hero": "hero_atlas",
            "label": "atlas self death",
            "vsnd": [CLIP],
            "duration": 1.5,
            "caption": null,
        }],
    });
    std::fs::write(
        cache_dir.join("voiceline.json"),
        serde_json::to_vec(&stale).unwrap(),
    )
    .unwrap();

    let cache = CatalogCache::new(cache_dir);
    let (rebuilt, hit) = cache.voicelines_cached(&pak).unwrap();
    assert!(!hit);
    assert_eq!(rebuilt[0].hero.as_deref(), Some("atlas"));
    assert_eq!(rebuilt[0].label, "self death");
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(cache.dir().join("voiceline.json")).unwrap())
            .unwrap();
    assert_eq!(stored["schema"], CACHE_SCHEMA_VERSION);
    let (cached, hit) = cache.voicelines_cached(&pak).unwrap();
    assert!(hit);
    assert_eq!(cached, rebuilt);
}
