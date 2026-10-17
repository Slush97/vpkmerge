use morphic::kv3::{self, Format, Seg, Value};

fn legacy(encoding: &[u8; 16], body: &[u8], compressed: bool) -> Vec<u8> {
    let mut data = vec![0x56, 0x4b, 0x56, 0x03];
    data.extend(encoding);
    data.extend([0x42; 16]);
    if compressed {
        data.extend(u32::try_from(body.len()).unwrap().to_le_bytes());
        data.extend(lz4_flex::block::compress(body));
    } else {
        data.extend(body);
    }
    data
}

#[test]
fn legacy_flags_and_format_survive_conversion_to_versioned_kv3() {
    let raw = [
        0x00, 0x05, 0x86, 0x1b, 0xd8, 0xf7, 0xc1, 0x40, 0xad, 0x82, 0x75, 0xa4, 0x82, 0x67, 0xe7,
        0x14,
    ];
    let lz4 = [
        0x8a, 0x34, 0x47, 0x68, 0xa1, 0x63, 0x5c, 0x4f, 0xa1, 0x97, 0x53, 0x80, 0x6f, 0xd9, 0xb1,
        0x19,
    ];
    let mut body = 2u32.to_le_bytes().to_vec();
    body.extend(b"model\0models/test.vmdl\0");
    body.push(9); // OBJECT, one named Resource string.
    body.extend(1u32.to_le_bytes());
    body.extend(0u32.to_le_bytes());
    body.extend([0x86, 1]);
    body.extend(1u32.to_le_bytes());
    body.extend([0xff; 4]);
    for bytes in [legacy(&raw, &body, false), legacy(&lz4, &body, true)] {
        let format = Format::from_payload(&bytes).unwrap();
        assert_eq!(format.0, [0x42; 16]);
        let (value, flags) = kv3::decode_flagged(&bytes).unwrap();
        assert_eq!(
            value.get("model"),
            Some(&Value::String("models/test.vmdl".into()))
        );
        assert_eq!(flags.get(&vec![Seg::Key("model".into())]), Some(&1));
        let encoded = kv3::encode_flagged(&value, &format, &flags);
        assert_eq!(kv3::decode_flagged(&encoded).unwrap(), (value, flags));
        assert_eq!(Format::from_payload(&encoded).unwrap(), format);
        assert!(kv3::decode_flagged(&bytes[..bytes.len() - 3]).is_err());
    }
}

#[test]
fn nested_array_reference_flags_round_trip_without_affecting_plain_decode() {
    let value = Value::Object(vec![(
        "refs".into(),
        Value::Array(vec![
            Value::String("x.vmdl".into()),
            Value::String("y.vmdl".into()),
        ]),
    )]);
    let mut flags = kv3::ValueFlags::new();
    for index in 0..2 {
        flags.insert(vec![Seg::Key("refs".into()), Seg::Index(index)], 1);
    }
    let encoded = kv3::encode_flagged(&value, &Format([1; 16]), &flags);
    assert_eq!(kv3::decode(&encoded).unwrap(), value);
    assert_eq!(kv3::decode_flagged(&encoded).unwrap(), (value, flags));
}
