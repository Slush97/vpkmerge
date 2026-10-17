use super::*;

fn object(pairs: &[(&str, Value)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), v.clone()))
            .collect(),
    )
}

fn resource(value: &Value, flags: &ValueFlags) -> Vec<u8> {
    let payload = kv3::encode_flagged(value, &kv3::Format([0; 16]), flags);
    let mut bytes = vec![0; 28];
    bytes[4..6].copy_from_slice(&12u16.to_le_bytes());
    bytes[8..12].copy_from_slice(&8u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&1u32.to_le_bytes());
    bytes[16..20].copy_from_slice(b"DATA");
    bytes[20..24].copy_from_slice(&8u32.to_le_bytes());
    bytes[24..28].copy_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
    bytes.extend(payload);
    let len = u32::try_from(bytes.len()).unwrap();
    bytes[..4].copy_from_slice(&len.to_le_bytes());
    bytes
}

#[test]
fn missing_data_is_added_without_changing_authored_values_arrays_or_resource_flags() -> Result<()> {
    let folder = tempfile::tempdir()?;
    let base_path = folder.path().join("base_dir.vpk");
    let mod_path = folder.path().join("mod_dir.vpk");
    let output = folder.path().join("fixed_dir.vpk");
    let entry = "scripts/abilities.vdata_c";
    let authored = object(&[(
        "old",
        object(&[
            ("damage", Value::Int(999)),
            ("effects", Value::Array(vec![Value::String("mine".into())])),
        ]),
    )]);
    let base = object(&[
        (
            "old",
            object(&[
                ("damage", Value::Int(12)),
                ("effects", Value::Array(vec![])),
                ("newField", Value::Bool(true)),
            ]),
        ),
        (
            "new",
            object(&[("model", Value::String("models/new.vmdl".into()))]),
        ),
    ]);
    let flag_path = vec![Seg::Key("new".into()), Seg::Key("model".into())];
    let mut flags = ValueFlags::new();
    flags.insert(flag_path.clone(), 1);
    let base = resource(&base, &flags);
    let original = resource(&authored, &ValueFlags::new());
    crate::pack(&[(entry, &base)], &base_path)?;
    crate::pack(
        &[(entry, &original), ("keep.bin", b"authored bytes")],
        &mod_path,
    )?;
    let before = std::fs::read(&mod_path)?;
    let mut repairer = CompatibilityRepairer::new(&base_path)?;
    let preview = repairer.check(&mod_path, false)?;
    assert_eq!(preview.len(), 1);
    assert_eq!(preview[0].changes, 2);
    repairer.heal(&mod_path, &output, false)?;
    assert_eq!(std::fs::read(&mod_path)?, before);
    let bytes = crate::read_vpk_entry(&output, entry)?;
    let result = Document::read(&bytes)?;
    assert_eq!(
        result.value.get("old").unwrap().get("damage"),
        Some(&Value::Int(999))
    );
    assert_eq!(
        result.value.get("old").unwrap().get("effects"),
        authored.get("old").unwrap().get("effects")
    );
    assert_eq!(result.flags.get(&flag_path), Some(&1));
    assert_eq!(
        crate::read_vpk_entry(&output, "keep.bin")?,
        b"authored bytes"
    );
    assert!(repairer.check(&output, false)?.is_empty());
    assert!(repairer.heal(&mod_path, &mod_path, false).is_err());
    Ok(())
}

#[test]
fn localization_preserves_custom_text_comments_hints_and_encoding() -> Result<()> {
    let source = "\"lang\" { \"Language\" \"english\" \"Tokens\" { // author\n\"old:n\" \"Custom \\\"name\\\"\" } }";
    let base = "\"lang\" { \"Language\" \"english\" \"Tokens\" { \"OLD\" \"Vanilla\" \"ability_new\" \"New ability\" } }";
    let (out, added) = localization::repair(source.as_bytes(), base.as_bytes())?.unwrap();
    assert_eq!(added, 1);
    let result = String::from_utf8(out.clone())?;
    assert!(result.contains("// author\n\"old:n\" \"Custom \\\"name\\\"\""));
    assert!(result.contains("\"ability_new\" \"New ability\""));
    assert!(!result.contains("Vanilla"));
    assert!(localization::repair(&out, base.as_bytes())?.is_none());
    let utf16: Vec<_> = [0xff, 0xfe]
        .into_iter()
        .chain(source.encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    assert!(localization::repair(&utf16, base.as_bytes())?
        .unwrap()
        .0
        .starts_with(&[0xff, 0xfe]));
    assert!(localization::repair(b"\"lang\" { \"Tokens\" { \"bad\" } }", base.as_bytes()).is_err());
    assert!(localization::repair(
        b"\"lang\" { \"Language\" \"english\" \"Tokens\" { \"a\" \"1\" \"A\" \"2\" } }",
        base.as_bytes()
    )
    .is_err());
    Ok(())
}

#[test]
fn loose_localization_updates_override_the_game_pak() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let entry = "resource/localization/citadel_heroes/citadel_heroes_english.txt";
    let text = |token: &str| {
        format!("\"lang\" {{ \"Language\" \"english\" \"Tokens\" {{ \"{token}\" \"value\" }} }}")
    };
    let base = dir.path().join("base_dir.vpk");
    let input = dir.path().join("mod_dir.vpk");
    crate::pack(&[(entry, text("packed").as_bytes())], &base)?;
    crate::pack(&[(entry, text("authored").as_bytes())], &input)?;
    let loose = dir.path().join(entry);
    std::fs::create_dir_all(loose.parent().unwrap())?;
    std::fs::write(loose, text("loose"))?;
    let output = dir.path().join("out_dir.vpk");
    CompatibilityRepairer::new(&base)?.heal(&input, &output, false)?;
    let result = String::from_utf8(crate::read_vpk_entry(output, entry)?)?;
    assert!(result.contains("authored") && result.contains("loose") && !result.contains("packed"));
    Ok(())
}

#[test]
fn duplicate_keys_and_streamed_tails_are_declined() -> Result<()> {
    let value = object(&[("a", Value::Int(1)), ("A", Value::Int(2))]);
    assert!(Document::read(&resource(&value, &ValueFlags::new())).is_err());
    let mut bytes = resource(&object(&[("a", Value::Int(1))]), &ValueFlags::new());
    bytes.extend(b"streamed");
    assert!(Document::read(&bytes)?.write(&bytes).is_err());
    Ok(())
}

fn sample_skeleton(extra: bool) -> Value {
    let pose = Value::Array(
        vec![0., 0., 0., 1., 0., 0., 0., 1.]
            .into_iter()
            .map(Value::Double)
            .collect(),
    );
    let count = if extra { 3 } else { 2 };
    object(&[
        (
            "m_boneIDs",
            Value::Array(
                ["root", "arm", "new_leaf"]
                    .iter()
                    .take(count)
                    .map(|s| Value::String((*s).into()))
                    .collect(),
            ),
        ),
        (
            "m_parentIndices",
            Value::Array(
                vec![Value::Int(-1), Value::Int(0), Value::Int(0)]
                    .into_iter()
                    .take(count)
                    .collect(),
            ),
        ),
        (
            "m_parentSpaceReferencePose",
            Value::Array(vec![pose.clone(); count]),
        ),
        ("m_modelSpaceReferencePose", Value::Array(vec![pose; count])),
        ("m_numBonesToSampleAtLowLOD", Value::Int(2)),
        ("m_maskDefinitions", Value::Array(vec![])),
        ("m_secondarySkeletons", Value::Array(vec![])),
    ])
}

#[test]
fn skeleton_layout_repairs_are_bounded_by_pose_hierarchy_masks_and_animation_providers(
) -> Result<()> {
    let dir = tempfile::tempdir()?;
    let base = dir.path().join("base_dir.vpk");
    let input = dir.path().join("skin_dir.vpk");
    let output = dir.path().join("out_dir.vpk");
    let entry = "models/robot/robot.vnmskel_c";
    let current = resource(&sample_skeleton(true), &ValueFlags::new());
    let old = resource(&sample_skeleton(false), &ValueFlags::new());
    crate::pack(&[(entry, &current)], &base)?;
    crate::pack(&[(entry, &old), ("skin.bin", b"geometry")], &input)?;
    let mut repairer = CompatibilityRepairer::new(&base)?;
    assert!(matches!(
        repairer.check(&input, false)?[0].status,
        RepairStatus::Repairable
    ));
    assert!(matches!(
        repairer.check(&input, true)?[0].status,
        RepairStatus::Unsupported
    ));
    repairer.heal(&input, &output, false)?;
    assert_eq!(crate::read_vpk_entry(&output, entry)?, current);
    assert_eq!(crate::read_vpk_entry(&output, "skin.bin")?, b"geometry");
    let mut authored = sample_skeleton(false);
    if let Value::Array(poses) = authored.get_mut("m_parentSpaceReferencePose").unwrap() {
        if let Value::Array(values) = &mut poses[1] {
            values[4] = Value::Double(4.0);
        }
    }
    let bytes = resource(&authored, &ValueFlags::new());
    let custom = dir.path().join("custom_dir.vpk");
    crate::pack(&[(entry, &bytes)], &custom)?;
    assert!(matches!(
        repairer.check(&custom, false)?[0].status,
        RepairStatus::Unsupported
    ));
    let declined = dir.path().join("declined_dir.vpk");
    repairer.heal(&custom, &declined, false)?;
    assert!(!declined.exists());
    Ok(())
}

#[test]
fn installed_game_repairs_preserve_real_model_geometry_and_kv3_flags() -> Result<()> {
    let Some(game) = std::env::var_os("DEADLOCK_PAK") else {
        return Ok(());
    };
    let game = PathBuf::from(game);
    let model = "models/heroes_wip/werewolf/werewolf.vmdl_c";
    let bytes = crate::read_vpk_entry(&game, model)?;
    let mut document = Document::read(&bytes)?;
    for key in ["m_animGraph2Refs", "m_vecNmSkeletonRefs"] {
        *document
            .value
            .get_mut(key)
            .context("Missing current binding")? = Value::Array(vec![]);
        document
            .flags
            .retain(|path, _| !path.starts_with(&[Seg::Key(key.into())]));
    }
    let old = document.write(&bytes)?;
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("old_dir.vpk");
    let output = dir.path().join("fixed_dir.vpk");
    crate::pack(&[(model, &old), ("skin.txt", b"authored")], &input)?;
    let before = std::fs::read(&input)?;
    let mut repairer = CompatibilityRepairer::new(&game)?;
    let reports = repairer.check(&input, false)?;
    assert_eq!(reports.len(), 1);
    assert!(
        matches!(reports[0].status, RepairStatus::Repairable),
        "{reports:?}"
    );
    repairer.heal(&input, &output, false)?;
    assert_eq!(std::fs::read(&input)?, before);
    let actual = crate::read_vpk_entry(&output, model)?;
    assert_eq!(
        Document::read(&actual)?.value,
        Document::read(&bytes)?.value
    );
    assert_eq!(
        Document::read(&actual)?.flags,
        Document::read(&bytes)?.flags
    );
    let original = Resource::parse(&bytes)?;
    let container = Resource::parse(&actual)?;
    for (index, block) in original.blocks().iter().enumerate() {
        if block.kind != *b"DATA" {
            assert_eq!(
                original.get_block_by_index(index),
                container.get_block_by_index(index)
            );
        }
    }
    assert!(repairer.check(&output, false)?.is_empty());
    Ok(())
}
