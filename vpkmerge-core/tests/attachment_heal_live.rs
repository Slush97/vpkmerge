//! Live test for `AttachmentHealer` against the real game pak.
//!
//! Gated on `DEADLOCK_PAK` pointing at `citadel/pak01_dir.vpk`; CI skips.
//!
//! ```sh
//! DEADLOCK_PAK=~/.../Deadlock/game/citadel/pak01_dir.vpk \
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
    let healed = healer.heal(&stale, &healed_path).unwrap();
    assert_eq!(healed.len(), 1);
    assert!(healed[0].blocks > 0);
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
