fn main() {
    let b = std::fs::read(&std::env::args().nth(1).unwrap()).unwrap();
    let t = morphic::decode_kv3_resource(&b).unwrap();
    if let morphic::kv3::Value::Object(f) = &t {
        for (k, v) in f {
            let kind = match v {
                morphic::kv3::Value::Array(a) => format!("array[{}]", a.len()),
                morphic::kv3::Value::Object(_) => "object".into(),
                morphic::kv3::Value::String(s) => format!("str[{}]", s.len()),
                _ => "scalar".into(),
            };
            println!("{k}\t{kind}");
        }
    }
}
