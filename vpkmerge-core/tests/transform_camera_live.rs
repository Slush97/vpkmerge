//! Optional real-model coverage. Set `DEADLOCK_PAK`, `CAMERA_REPAIR_SILVER_MOD` and
//! `CAMERA_REPAIR_CALICO_MOD` to read local archives without changing them.
use morphic::kv3::{Seg, Value};
use morphic::resource::Resource;
use vpkmerge_core::{AttachmentHealer, AttachmentStatus};

const SILVER: [&str; 2] = [
    "models/heroes_wip/werewolf/werewolf.vmdl_c",
    "models/heroes_wip/werewolf/transform/werewolf_transform.vmdl_c",
];
const CALICO: [&str; 2] = [
    "models/heroes_staging/nano/nano_v2/nano.vmdl_c",
    "models/abilities/nano_catform_model.vmdl_c",
];

fn root(att: &Value) -> bool {
    att.get("value")
        .and_then(|v| v.get("m_influenceNames"))
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .and_then(Value::as_str)
        == Some("root_motion")
}

fn mesh_blocks(bytes: &[u8]) -> Vec<usize> {
    Resource::parse(bytes)
        .unwrap()
        .blocks()
        .iter()
        .enumerate()
        .filter(|(_, b)| &b.kind == b"MDAT")
        .map(|(i, _)| i)
        .collect()
}

fn legacy_model(bytes: &[u8], stock: &[u8]) -> Vec<u8> {
    let base = Resource::parse(stock).unwrap();
    let base_mesh =
        morphic::kv3::decode(base.get_block_by_index(mesh_blocks(stock)[0]).unwrap()).unwrap();
    let base_atts = base_mesh
        .get("m_attachments")
        .and_then(Value::as_array)
        .unwrap();
    let mut output = bytes.to_vec();
    for block in mesh_blocks(bytes) {
        let resource = Resource::parse(&output).unwrap();
        let raw = resource.get_block_by_index(block).unwrap();
        let tree = morphic::kv3::decode(raw).unwrap();
        let Some(atts) = tree.get("m_attachments").and_then(Value::as_array) else {
            continue;
        };
        let mut edits = Vec::new();
        for (index, att) in atts.iter().enumerate() {
            if !root(att)
                || !base_atts
                    .iter()
                    .any(|b| root(b) && b.get("key") == att.get("key"))
            {
                continue;
            }
            for field in ["m_vInfluenceOffsets", "m_vInfluenceRotations"] {
                let values: Vec<f64> = att
                    .get("value")
                    .unwrap()
                    .get(field)
                    .and_then(Value::as_array)
                    .and_then(|v| v.first())
                    .and_then(Value::as_array)
                    .unwrap()
                    .iter()
                    .map(|v| v.as_f64().unwrap())
                    .collect();
                let old = if values.len() == 3 {
                    vec![values[1], values[2], values[0]]
                } else {
                    let [x, y, z, w] = [values[0], values[1], values[2], values[3]];
                    vec![
                        (x - w - z + y) * 0.5,
                        (y + z - w - x) * 0.5,
                        (z - y + x - w) * 0.5,
                        (w + x + y + z) * 0.5,
                    ]
                };
                for (component, value) in old.into_iter().enumerate() {
                    edits.push((
                        vec![
                            Seg::Key("m_attachments".into()),
                            Seg::Index(index),
                            Seg::Key("value".into()),
                            Seg::Key(field.into()),
                            Seg::Index(0),
                            Seg::Index(component),
                        ],
                        value,
                    ));
                }
            }
        }
        if !edits.is_empty() {
            let patched = morphic::kv3::set_reals(raw, &edits).unwrap();
            output = resource.rebuild_with_block(block, &patched).unwrap();
        }
    }
    output
}

fn unchanged_except_camera(before: &[u8], after: &[u8]) {
    let old = Resource::parse(before).unwrap();
    let new = Resource::parse(after).unwrap();
    assert_eq!(old.blocks().len(), new.blocks().len());
    for (i, block) in old.blocks().iter().enumerate() {
        assert_eq!(block.kind, new.blocks()[i].kind);
        let (a, b) = (
            old.get_block_by_index(i).unwrap(),
            new.get_block_by_index(i).unwrap(),
        );
        if a == b {
            continue;
        }
        if &block.kind == b"DATA" {
            let mut a = morphic::kv3::decode(a).unwrap();
            let mut b = morphic::kv3::decode(b).unwrap();
            let before = a
                .get("m_modelInfo")
                .unwrap()
                .get("m_keyValueText")
                .unwrap()
                .as_str()
                .unwrap();
            let after = b
                .get("m_modelInfo")
                .unwrap()
                .get("m_keyValueText")
                .unwrap()
                .as_str()
                .unwrap();
            let (prefix, suffix) = before.rsplit_once('}').unwrap();
            assert!(after.starts_with(prefix) && after.ends_with(&format!("}}{suffix}")));
            assert!(after.contains("CitadelCameraSettings_t"));
            for tree in [&mut a, &mut b] {
                *tree
                    .get_mut("m_modelInfo")
                    .unwrap()
                    .get_mut("m_keyValueText")
                    .unwrap() = Value::Null;
            }
            assert_eq!(a, b);
        } else if &block.kind == b"MDAT" {
            let a = morphic::kv3::decode(a).unwrap();
            let b = morphic::kv3::decode(b).unwrap();
            let names: Vec<_> = a
                .get("m_attachments")
                .and_then(Value::as_array)
                .unwrap()
                .iter()
                .map(|v| v.get("key").unwrap().clone())
                .collect();
            let mask = |mut tree: Value| {
                if let Some(Value::Array(atts)) = tree.get_mut("m_attachments") {
                    atts.retain(|v| names.contains(v.get("key").unwrap()));
                    for att in atts.iter_mut().filter(|a| root(a)) {
                        let value = att.get_mut("value").unwrap();
                        *value.get_mut("m_vInfluenceOffsets").unwrap() = Value::Null;
                        *value.get_mut("m_vInfluenceRotations").unwrap() = Value::Null;
                    }
                }
                tree
            };
            assert_eq!(mask(a), mask(b));
        } else {
            panic!("Non-camera block changed: {:?}", block.kind);
        }
    }
}

#[test]
fn repairs_real_partial_and_fully_legacy_transform_models() {
    let Ok(base) = std::env::var("DEADLOCK_PAK") else {
        eprintln!("DEADLOCK_PAK not set; skipping");
        return;
    };
    for (variable, targets) in [
        ("CAMERA_REPAIR_SILVER_MOD", SILVER),
        ("CAMERA_REPAIR_CALICO_MOD", CALICO),
    ] {
        let Ok(path) = std::env::var(variable) else {
            eprintln!("{variable} not set; skipping");
            continue;
        };
        let source: Vec<_> = targets
            .iter()
            .map(|entry| (*entry, vpkmerge_core::read_vpk_entry(&path, entry).unwrap()))
            .collect();
        let stock: Vec<_> = targets
            .iter()
            .map(|entry| vpkmerge_core::read_vpk_entry(&base, entry).unwrap())
            .collect();
        for fully_legacy in [false, true] {
            let fixtures: Vec<_> = source
                .iter()
                .zip(&stock)
                .map(|((entry, bytes), stock)| {
                    (
                        *entry,
                        if fully_legacy {
                            legacy_model(bytes, stock)
                        } else {
                            bytes.clone()
                        },
                    )
                })
                .collect();
            let dir = tempfile::tempdir().unwrap();
            let input = dir.path().join("input_dir.vpk");
            let output = dir.path().join("output_dir.vpk");
            let mut entries: Vec<_> = fixtures.iter().map(|(e, b)| (*e, b.as_slice())).collect();
            entries.push(("sentinel.txt", b"untouched"));
            vpkmerge_core::pack(&entries, &input).unwrap();
            let original_file = std::fs::read(&input).unwrap();
            let mut healer = AttachmentHealer::new(&base);
            let reports = healer.check(&input).unwrap();
            assert!(
                reports
                    .iter()
                    .all(|r| r.status != AttachmentStatus::Undecodable),
                "{reports:#?}"
            );
            let stale: Vec<_> = reports
                .iter()
                .filter(|r| r.status == AttachmentStatus::Stale)
                .collect();
            assert_eq!(
                stale.len(),
                if fully_legacy { 2 } else { 1 },
                "{variable} {reports:#?}"
            );
            let repaired = healer.heal(&input, &output).unwrap();
            assert_eq!(repaired.len(), stale.len());
            assert!(healer
                .check(&output)
                .unwrap()
                .iter()
                .all(|r| r.status == AttachmentStatus::Current));
            assert_eq!(
                vpkmerge_core::read_vpk_entry(&output, "sentinel.txt").unwrap(),
                b"untouched"
            );
            for (entry, before) in &fixtures {
                unchanged_except_camera(
                    before,
                    &vpkmerge_core::read_vpk_entry(&output, entry).unwrap(),
                );
            }
            let again = dir.path().join("again_dir.vpk");
            assert!(healer.heal(&output, &again).unwrap().is_empty());
            assert!(!again.exists());
            assert_eq!(std::fs::read(&input).unwrap(), original_file);
        }
    }
}
