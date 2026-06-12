//! Binary KV3 decode + round-trip against a real Deadlock soundevents file.
//!
//! `gigawatt.vsndevts_c` is committed under `fixtures/kv3/`. It is Valve's
//! shipped v5/LZ4 file, so decoding it exercises the v5 two-buffer reader and
//! LZ4 path. Re-encoding (uncompressed v4) and decoding again must reproduce the
//! exact same tree, which exercises the v4 single-buffer reader and the writer.

use morphic::kv3::Value;

fn fixture() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/kv3/gigawatt.vsndevts_c"
    );
    std::fs::read(path).expect("read gigawatt fixture")
}

#[test]
fn decodes_gigawatt_v5_lz4() {
    let bytes = fixture();
    let root = morphic::decode_kv3_resource(&bytes).expect("decode");

    let Value::Object(events) = &root else {
        panic!("root is not an object");
    };
    // 44 hero soundevents (the survey counted 44 hero vsndevts_c entries; this
    // file holds gigawatt's events).
    assert_eq!(events.len(), 44, "unexpected top-level event count");

    // A known event with the documented shape: a vsnd_files array and a base.
    let fire = root.get("Seven.Wpn.Fire").expect("Seven.Wpn.Fire event");
    assert_eq!(
        fire.get("base").and_then(Value::as_str),
        Some("Base.Weapon.Pistol")
    );

    let vsnd = fire
        .get("vsnd_files")
        .and_then(Value::as_array)
        .expect("vsnd_files array");
    assert_eq!(vsnd.len(), 7);
    assert_eq!(
        vsnd[0].as_str(),
        Some("sounds/weapons/gigawatt/gigawatt_weapon_fire_01.vsnd")
    );

    // volume is present and numeric.
    assert!(fire.get("volume").and_then(Value::as_f64).is_some());
}

#[test]
fn round_trips_through_uncompressed_v4() {
    let bytes = fixture();
    let original = morphic::decode_kv3_resource(&bytes).expect("decode original");

    // Re-encode into a full resource (uncompressed v4 DATA + original RED2),
    // then decode again. The tree must be identical.
    let reencoded = morphic::encode_kv3_resource(&bytes, &original).expect("encode");
    let roundtripped = morphic::decode_kv3_resource(&reencoded).expect("decode reencoded");

    assert_eq!(original, roundtripped, "tree changed across encode/decode");

    // The re-emitted file is a valid container the resource parser accepts, and
    // (being uncompressed) is larger than Valve's LZ4 original.
    assert!(
        reencoded.len() > bytes.len(),
        "uncompressed re-encode should be larger than the LZ4 original"
    );
}

#[test]
fn modify_then_round_trip() {
    let bytes = fixture();
    let mut root = morphic::decode_kv3_resource(&bytes).expect("decode");

    // Phase 2: swap a vsnd path and change a volume.
    root.for_each_string_mut(&mut |s| {
        if s == "sounds/weapons/gigawatt/gigawatt_weapon_fire_01.vsnd" {
            *s = "sounds/custom/my_fire.vsnd".to_string();
        }
    });
    if let Some(vol) = root
        .get_mut("Seven.Wpn.Fire")
        .and_then(|e| e.get_mut("volume"))
    {
        *vol = Value::Double(0.25);
    }

    let reencoded = morphic::encode_kv3_resource(&bytes, &root).expect("encode");
    let back = morphic::decode_kv3_resource(&reencoded).expect("decode");

    let fire = back.get("Seven.Wpn.Fire").unwrap();
    assert_eq!(
        fire.get("vsnd_files").and_then(Value::as_array).unwrap()[0].as_str(),
        Some("sounds/custom/my_fire.vsnd")
    );
    assert_eq!(fire.get("volume").and_then(Value::as_f64), Some(0.25));
}

#[test]
fn set_scalars_patches_a_v4_block_in_place() {
    use morphic::kv3::Seg;

    // `encode_kv3_resource` emits an uncompressed **v4** DATA block, so this builds
    // a real v4 block to exercise the v4 single-buffer scalar patch (OBJECT member
    // counts from the b4 lane, INT64 from b8, inline strings/types) end to end. The
    // base-game particles that need this are also v4; this is the committed
    // regression for that path (their in-game recolor is the live proof).
    let template = fixture(); // reused only for a valid resource envelope / RED2
    let tree = Value::Object(vec![
        ("m_nFirst".to_string(), Value::Int(100)),
        (
            "m_sKeep".to_string(),
            Value::String("unchanged".to_string()),
        ),
        ("m_nSecond".to_string(), Value::Int(7)),
    ]);
    let v4 = morphic::encode_kv3_resource(&template, &tree).expect("encode v4 resource");

    // Patch one scalar in place; the block stays v4, everything else byte-faithful.
    let patched = morphic::patch_kv3_resource_scalars(
        &v4,
        &[(vec![Seg::Key("m_nSecond".to_string())], 4242)],
    )
    .expect("v4 scalar patch");

    let root = morphic::decode_kv3_resource(&patched).expect("decode patched v4");
    assert_eq!(root.get("m_nSecond").and_then(Value::as_int), Some(4242));
    // Siblings are untouched (the patch is surgical, not a re-encode).
    assert_eq!(root.get("m_nFirst").and_then(Value::as_int), Some(100));
    assert_eq!(
        root.get("m_sKeep").and_then(Value::as_str),
        Some("unchanged")
    );

    // A path that resolves to no integer scalar is an error, not a silent no-op.
    assert!(morphic::patch_kv3_resource_scalars(
        &v4,
        &[(vec![Seg::Key("m_sKeep".to_string())], 1)],
    )
    .is_err());
}

#[test]
fn set_doubles_patches_a_tint_vector_in_place() {
    use morphic::kv3::Seg;

    // The material g_vColorTint shape: an RGBA f64 vector. Patch two channels in
    // place (the double sibling of the scalar patch); the third is untouched and the
    // tagless 1.0 alpha is not patchable.
    let template = fixture();
    let tree = Value::Object(vec![(
        "m_value".to_string(),
        Value::Array(vec![
            Value::Double(0.9),
            Value::Double(0.5),
            Value::Double(0.2),
            Value::Double(1.0),
        ]),
    )]);
    let v = morphic::encode_kv3_resource(&template, &tree).expect("encode");

    let key = |i: usize| vec![Seg::Key("m_value".to_string()), Seg::Index(i)];
    let patched = morphic::patch_kv3_resource_doubles(&v, &[(key(0), 0.123), (key(2), 0.456)])
        .expect("patch");
    let root = morphic::decode_kv3_resource(&patched).expect("decode patched");
    let arr = root
        .get("m_value")
        .and_then(Value::as_array)
        .expect("m_value");
    assert!((arr[0].as_f64().unwrap() - 0.123).abs() < 1e-9);
    assert!(
        (arr[1].as_f64().unwrap() - 0.5).abs() < 1e-9,
        "untouched channel preserved"
    );
    assert!((arr[2].as_f64().unwrap() - 0.456).abs() < 1e-9);

    // The 1.0 alpha is a tagless DOUBLE_ONE (no stored bytes), so it is not a
    // patchable target.
    assert!(morphic::patch_kv3_resource_doubles(&v, &[(key(3), 0.5)]).is_err());
}

#[test]
fn patch_doubles_retints_a_real_blobbed_material() {
    use morphic::kv3::Seg;

    // `necro_hands.vmat_c` is a real Deadlock material whose DATA block is KV3 v5,
    // LZ4-compressed, AND carries a binary-blob section (countBlocks = 1). This is
    // the shape that cannot be re-emitted uncompressed without the engine misreading
    // its blob framing, so `patch_kv3_resource_doubles` must retint it while keeping
    // it compressed. End-to-end (full resource envelope) exercise of the path the
    // hero ability-VFX recolor drives. The in-game gate (the hand reads recolored,
    // not red/wireframe) is documented in docs/spike-blobbed-vmat-recolor.md.
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/material/necro_hands.vmat_c"
    ))
    .expect("read necro_hands fixture");

    let tree = morphic::decode_kv3_resource(&bytes).expect("decode material");
    let params = tree
        .get("m_vectorParams")
        .and_then(Value::as_array)
        .expect("m_vectorParams");
    // Find the first color/self-illum tint param.
    let pi = params
        .iter()
        .position(|p| {
            p.get("m_name").and_then(Value::as_str).is_some_and(|name| {
                name.starts_with("g_vColorTint") || name.starts_with("g_vSelfIllumTint")
            })
        })
        .expect("a tint param");

    let path = vec![
        Seg::Key("m_vectorParams".to_string()),
        Seg::Index(pi),
        Seg::Key("m_value".to_string()),
        Seg::Index(0),
    ];
    let patched = morphic::patch_kv3_resource_doubles(&bytes, &[(path, 0.321)]).expect("retint");

    // The whole file still parses, and the channel reads back the new value.
    let back = morphic::decode_kv3_resource(&patched).expect("decode retinted");
    let ch = back
        .get("m_vectorParams")
        .and_then(Value::as_array)
        .and_then(|a| a.get(pi))
        .and_then(|p| p.get("m_value"))
        .and_then(Value::as_array)
        .expect("m_value");
    assert!(
        (ch[0].as_f64().unwrap() - 0.321).abs() < 1e-9,
        "channel retinted"
    );
    // Every other field is identical (binary blob included): edit the source tree's
    // one channel and require the full trees to match.
    let mut expect = tree.clone();
    if let Some(Value::Array(ps)) = expect.get_mut("m_vectorParams") {
        if let Some(Value::Array(c)) = ps[pi].get_mut("m_value") {
            c[0] = Value::Double(0.321);
        }
    }
    assert_eq!(back, expect, "only the targeted channel changed");
}

#[test]
fn decodes_a_two_blob_material() {
    // `necro_picker_hand_effect.vmat_c` carries TWO binary blobs (its
    // `m_dynamicParams` expressions), framed one-per-blob. The blob-frame decoder
    // used to assume each frame fills the whole remaining region and rejected this
    // with "blob frame: expected 12, got 6"; it must now decode both short frames.
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/material/necro_picker_hand_effect.vmat_c"
    ))
    .expect("read 2-blob fixture");

    let tree = morphic::decode_kv3_resource(&bytes).expect("decode 2-blob material");
    // Sanity: it parsed as a real material (its self-illum tint is present), and the
    // two dynamic-param expressions decoded as non-empty binary blobs.
    assert!(tree
        .get("m_vectorParams")
        .and_then(Value::as_array)
        .is_some_and(|p| p.iter().any(|param| param
            .get("m_name")
            .and_then(Value::as_str)
            .is_some_and(|n| n.starts_with("g_vSelfIllumTint")))));
    let blobs: Vec<&[u8]> = collect_binaries(&tree);
    assert_eq!(blobs.len(), 2, "two dynamic-param blobs");
    assert!(blobs.iter().all(|b| !b.is_empty()), "blobs are non-empty");
}

/// Every `Value::Binary` in the tree, in document order.
fn collect_binaries(value: &Value) -> Vec<&[u8]> {
    fn walk<'a>(v: &'a Value, out: &mut Vec<&'a [u8]>) {
        match v {
            Value::Binary(b) => out.push(b.as_slice()),
            Value::Array(items) => items.iter().for_each(|x| walk(x, out)),
            Value::Object(pairs) => pairs.iter().for_each(|(_, x)| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, &mut out);
    out
}

#[test]
fn inserts_a_blob_into_a_two_blob_material() {
    // Insert a third dynamic param (a binary expression blob) at index 0 of the
    // 2-blob fixture's m_dynamicParams. The result must stay v5 + LZ4 with a
    // 3-blob section (the engine misreads uncompressed blob framing), the new
    // blob must land FIRST in document order, and both original blobs must
    // survive byte-identically.
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/material/necro_picker_hand_effect.vmat_c"
    ))
    .expect("read 2-blob fixture");
    let before = morphic::decode_kv3_resource(&bytes).expect("decode");
    let before_blobs: Vec<Vec<u8>> = collect_binaries(&before)
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect();

    let new_code: Vec<u8> = vec![0x07, 0x00, 0x00, 0x80, 0x3F, 0x00]; // return 1.0
    let inserted = Value::Object(vec![
        (
            "m_name".to_string(),
            Value::String("g_flSelfIllumScale1".to_string()),
        ),
        ("m_value".to_string(), Value::Binary(new_code.clone())),
    ]);
    let patched = morphic::patch_kv3_resource_array_insert(
        &bytes,
        &[morphic::kv3::Seg::Key("m_dynamicParams".to_string())],
        0,
        &inserted,
    )
    .expect("blob-bearing insert");

    let data = morphic::kv3_resource_data_block(&patched).expect("data block");
    assert_eq!(data[0], 5, "kv3 version");
    assert_eq!(
        i32::from_le_bytes(data[20..24].try_into().unwrap()),
        1,
        "compressionMethod must stay LZ4"
    );
    assert_eq!(
        i32::from_le_bytes(data[56..60].try_into().unwrap()),
        3,
        "blob count"
    );

    let after = morphic::decode_kv3_resource(&patched).expect("re-decode");
    let params = after
        .get("m_dynamicParams")
        .and_then(Value::as_array)
        .expect("dynamic params");
    assert_eq!(params.len(), 3);
    assert_eq!(
        params[0].get("m_name").and_then(Value::as_str),
        Some("g_flSelfIllumScale1")
    );
    let after_blobs: Vec<Vec<u8>> = collect_binaries(&after)
        .into_iter()
        .map(<[u8]>::to_vec)
        .collect();
    assert_eq!(after_blobs.len(), 3);
    assert_eq!(after_blobs[0], new_code, "new blob lands first");
    assert_eq!(
        &after_blobs[1..],
        &before_blobs[..],
        "original blobs survive byte-identically"
    );
}
