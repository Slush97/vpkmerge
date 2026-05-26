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
