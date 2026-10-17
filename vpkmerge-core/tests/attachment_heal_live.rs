//! Live test for `AttachmentHealer` against the real game pak.
//!
//! Gated on `DEADLOCK_PAK` pointing at `citadel/pak01_dir.vpk`; CI skips.
//! `ATTACHMENT_HEAL_MOD` adds a real stale mod to heal (read only), e.g. one
//! whose model stores its attachment vectors as f64 instead of the game's f32.
//!
//! ```sh
//! DEADLOCK_PAK=~/.../Deadlock/game/citadel/pak01_dir.vpk \
//! ATTACHMENT_HEAL_MOD=~/.../citadel/addons/.disabled/after_school_mina_dir.vpk \
//!   cargo test -p vpkmerge-core --test attachment_heal_live -- --nocapture
//! ```

use morphic::kv3::{Seg, Value};
use morphic::resource::Resource;
use vpkmerge_core::{AttachmentHealer, AttachmentStatus};

const MODEL: &str = "models/heroes_wip/werewolf/werewolf.vmdl_c";
const OLD_FRAME: [f32; 4] = [-0.5, -0.5, -0.5, 0.5];

/// The game's model with every `root_motion` attachment turned back into the
/// pre-update frame, the way a model swap built before it ships.
fn reframe(model: &[u8]) -> Vec<u8> {
    let mdats: Vec<usize> = Resource::parse(model)
        .unwrap()
        .blocks()
        .iter()
        .enumerate()
        .filter(|(_, b)| &b.kind == b"MDAT")
        .map(|(i, _)| i)
        .collect();
    let mut out = model.to_vec();
    for index in mdats {
        let res = Resource::parse(&out).unwrap();
        let raw = res.get_block_by_index(index).unwrap();
        let tree = morphic::kv3::decode(raw).unwrap();
        let Some(atts) = tree.get("m_attachments").and_then(Value::as_array) else {
            continue;
        };
        let mut edits = Vec::new();
        for (i, att) in atts.iter().enumerate() {
            let value = att.get("value").unwrap();
            let parent = value
                .get("m_influenceNames")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .and_then(Value::as_str);
            if parent != Some("root_motion") {
                continue;
            }
            for (c, v) in OLD_FRAME.iter().enumerate() {
                edits.push((
                    vec![
                        Seg::Key("m_attachments".into()),
                        Seg::Index(i),
                        Seg::Key("value".into()),
                        Seg::Key("m_vInfluenceRotations".into()),
                        Seg::Index(0),
                        Seg::Index(c),
                    ],
                    *v,
                ));
            }
        }
        let patched = morphic::kv3::set_floats(raw, &edits).unwrap();
        out = res.rebuild_with_block(index, &patched).unwrap();
    }
    out
}

#[test]
fn heals_a_reframed_live_hero_model() {
    let Ok(pak) = std::env::var("DEADLOCK_PAK") else {
        eprintln!("DEADLOCK_PAK not set; skipping live attachment heal test");
        return;
    };
    let original = vpkmerge_core::read_vpk_entry(&pak, MODEL).expect("read hero model");
    let dir = tempfile::tempdir().unwrap();
    let mut healer = AttachmentHealer::new(&pak);

    let same = dir.path().join("same_dir.vpk");
    vpkmerge_core::pack(&[(MODEL, original.as_slice())], &same).unwrap();
    let reports = healer.check(&same).unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].status, AttachmentStatus::Current);

    let stale = dir.path().join("stale_dir.vpk");
    let reframed = reframe(&original);
    vpkmerge_core::pack(
        &[
            (MODEL, reframed.as_slice()),
            ("readme.txt", b"kept".as_slice()),
        ],
        &stale,
    )
    .unwrap();
    let reports = healer.check(&stale).unwrap();
    eprintln!("{reports:#?}");
    assert_eq!(reports[0].status, AttachmentStatus::Stale);
    assert!(reports[0].stale.iter().any(|n| n == "standing_pivot"));

    let healed_path = dir.path().join("healed_dir.vpk");
    let models = healer.heal(&stale, &healed_path).unwrap();
    assert_eq!(models.len(), 1);
    assert!(models[0].blocks > 0);
    let reports = healer.check(&healed_path).unwrap();
    assert_eq!(reports[0].status, AttachmentStatus::Current);
    assert_eq!(
        vpkmerge_core::read_vpk_entry(&healed_path, "readme.txt").unwrap(),
        b"kept"
    );

    let again = dir.path().join("again_dir.vpk");
    assert!(healer.heal(&healed_path, &again).unwrap().is_empty());
    assert!(!again.exists());
}

fn is_root_motion(att: &Value) -> bool {
    att.get("value")
        .and_then(|v| v.get("m_influenceNames"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        == Some("root_motion")
}

type Vectors = Vec<Vec<f64>>;

fn vectors(att: &Value, field: &str) -> Vectors {
    att.get("value")
        .and_then(|v| v.get(field))
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|v| {
            v.as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_f64().unwrap())
                .collect()
        })
        .collect()
}

/// Every `root_motion` attachment's offsets and rotations in `mdat`, by name.
fn root_vectors(mdat: &Value) -> Vec<(String, Vectors, Vectors)> {
    mdat.get("m_attachments")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter(|a| is_root_motion(a))
        .map(|a| {
            (
                a.get("key").and_then(Value::as_str).unwrap().to_string(),
                vectors(a, "m_vInfluenceOffsets"),
                vectors(a, "m_vInfluenceRotations"),
            )
        })
        .collect()
}

/// `mdat` with every `root_motion` attachment's offsets and rotations nulled.
fn without_root_vectors(mut mdat: Value) -> Value {
    if let Some(Value::Array(atts)) = mdat.get_mut("m_attachments") {
        for att in atts.iter_mut().filter(|a| is_root_motion(a)) {
            let value = att.get_mut("value").unwrap();
            for field in ["m_vInfluenceOffsets", "m_vInfluenceRotations"] {
                *value.get_mut(field).unwrap() = Value::Null;
            }
        }
    }
    mdat
}

/// The healed model differs from the mod's only in its `MDAT` blocks, and
/// those only in the `root_motion` vectors, which now hold the game's values.
fn assert_only_attachments_changed(entry: &str, original: &[u8], healed: &[u8], game: &[u8]) {
    let game_res = Resource::parse(game).unwrap();
    let game_atts = (0..game_res.blocks().len())
        .filter(|&i| &game_res.blocks()[i].kind == b"MDAT")
        .map(|i| morphic::kv3::decode(game_res.get_block_by_index(i).unwrap()).unwrap())
        .find(|mdat| mdat.get("m_attachments").is_some())
        .map(|mdat| root_vectors(&mdat))
        .unwrap();

    let (before, after) = (
        Resource::parse(original).unwrap(),
        Resource::parse(healed).unwrap(),
    );
    assert_eq!(before.blocks().len(), after.blocks().len(), "{entry}");
    for (i, block) in before.blocks().iter().enumerate() {
        assert_eq!(block.kind, after.blocks()[i].kind, "{entry} block {i}");
        let (old, new) = (
            before.get_block_by_index(i).unwrap(),
            after.get_block_by_index(i).unwrap(),
        );
        if &block.kind != b"MDAT" {
            assert_eq!(old, new, "{entry} block {i} changed");
            continue;
        }
        let (old, new) = (
            morphic::kv3::decode(old).unwrap(),
            morphic::kv3::decode(new).unwrap(),
        );
        for (name, offsets, rotations) in root_vectors(&new) {
            let Some((_, game_off, game_rot)) = game_atts.iter().find(|(n, _, _)| *n == name)
            else {
                continue;
            };
            for (ours, theirs) in [(&offsets, game_off), (&rotations, game_rot)] {
                for (mine, game) in ours.iter().zip(theirs) {
                    assert_eq!(mine, game, "{entry} block {i} {name}");
                }
            }
        }
        assert_eq!(
            without_root_vectors(old),
            without_root_vectors(new),
            "{entry} block {i}: more than the root_motion vectors changed"
        );
    }
}

#[test]
fn heals_a_real_stale_mod() {
    let (Ok(pak), Ok(mod_path)) = (
        std::env::var("DEADLOCK_PAK"),
        std::env::var("ATTACHMENT_HEAL_MOD"),
    ) else {
        eprintln!("DEADLOCK_PAK or ATTACHMENT_HEAL_MOD not set; skipping");
        return;
    };
    let mut healer = AttachmentHealer::new(&pak);
    let stale: Vec<String> = healer
        .check(&mod_path)
        .unwrap()
        .into_iter()
        .filter(|r| r.status == AttachmentStatus::Stale)
        .map(|r| r.entry)
        .collect();
    assert!(!stale.is_empty(), "{mod_path} ships no stale model");

    let dir = tempfile::tempdir().unwrap();
    let healed_path = dir.path().join("healed_dir.vpk");
    let models = healer.heal(&mod_path, &healed_path).unwrap();
    eprintln!("{models:#?}");
    let entries: Vec<&str> = models.iter().map(|m| m.entry.as_str()).collect();
    assert_eq!(entries, stale);
    for report in healer.check(&healed_path).unwrap() {
        assert_ne!(report.status, AttachmentStatus::Stale, "{}", report.entry);
    }

    let (before, after) = (
        valve_pak::open(&mod_path).unwrap(),
        valve_pak::open(&healed_path).unwrap(),
    );
    let mut paths: Vec<String> = before.file_paths().cloned().collect();
    let mut healed_paths: Vec<String> = after.file_paths().cloned().collect();
    paths.sort();
    healed_paths.sort();
    assert_eq!(paths, healed_paths);
    for path in &paths {
        let old = before.get_file(path).unwrap().read_all().unwrap();
        let new = after.get_file(path).unwrap().read_all().unwrap();
        if stale.contains(path) {
            let game = vpkmerge_core::read_vpk_entry(&pak, path).unwrap();
            assert_only_attachments_changed(path, &old, &new, &game);
        } else {
            assert!(old == new, "{path} changed");
        }
    }
}
