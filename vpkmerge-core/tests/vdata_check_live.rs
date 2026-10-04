//! Live test for `check_vdata` against the real game pak.
//!
//! Gated on `DEADLOCK_PAK` pointing at `citadel/pak01_dir.vpk`; CI skips.
//!
//! ```sh
//! DEADLOCK_PAK=~/.../Deadlock/game/citadel/pak01_dir.vpk \
//!   cargo test -p vpkmerge-core --test vdata_check_live -- --nocapture
//! ```

use morphic::kv3::Value;
use vpkmerge_core::VdataStatus;

const HEROES: &str = "scripts/heroes.vdata_c";

fn first_leaf(value: &Value, path: &str) -> Option<(String, Value)> {
    match value {
        Value::Object(pairs) => pairs.iter().find_map(|(k, v)| {
            first_leaf(
                v,
                &if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}/{k}")
                },
            )
        }),
        Value::Int(n) => Some((path.to_string(), Value::Int(n.wrapping_add(1)))),
        _ => None,
    }
}

fn set_at(value: &mut Value, path: &str, new: Value) {
    let mut cur = value;
    for seg in path.split('/') {
        let Value::Object(pairs) = cur else {
            panic!("not an object at {seg}")
        };
        cur = &mut pairs.iter_mut().find(|(k, _)| k == seg).unwrap().1;
    }
    *cur = new;
}

#[test]
fn heroes_vdata_against_live_pak() {
    let Ok(pak) = std::env::var("DEADLOCK_PAK") else {
        eprintln!("DEADLOCK_PAK not set; skipping live vdata check test");
        return;
    };
    let original = vpkmerge_core::read_vpk_entry(&pak, HEROES).expect("read heroes.vdata_c");
    let dir = tempfile::tempdir().unwrap();

    let same = dir.path().join("same_dir.vpk");
    vpkmerge_core::pack(&[(HEROES, original.as_slice())], &same).unwrap();
    let reports = vpkmerge_core::check_vdata(&same, &pak).unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].status, VdataStatus::Current);

    let mut tree = morphic::decode_kv3_resource(&original).unwrap();
    let Value::Object(pairs) = &mut tree else {
        panic!("root not an object")
    };
    let removed = pairs.pop().expect("non-empty root").0;
    let (leaf, new) = first_leaf(&tree, "").expect("an int leaf");
    set_at(&mut tree, &leaf, new);
    let edited = morphic::encode_kv3_resource(&original, &tree).unwrap();

    let stale = dir.path().join("stale_dir.vpk");
    vpkmerge_core::pack(
        &[
            (HEROES, edited.as_slice()),
            ("panorama/image_compiler.vdata_c", b"junk".as_slice()),
        ],
        &stale,
    )
    .unwrap();
    let reports = vpkmerge_core::check_vdata(&stale, &pak).unwrap();
    assert_eq!(reports.len(), 2);
    let compiler = reports
        .iter()
        .find(|r| r.entry.starts_with("panorama"))
        .unwrap();
    assert_eq!(compiler.status, VdataStatus::NotInGame);
    let heroes = reports.iter().find(|r| r.entry == HEROES).unwrap();
    eprintln!("{heroes:#?}");
    assert_eq!(heroes.status, VdataStatus::Outdated);
    assert_eq!(heroes.missing, [removed]);
    assert!(heroes.changed.contains(&leaf), "{leaf} not in changed");
}
