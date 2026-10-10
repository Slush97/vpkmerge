fn main() {
    let b = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let t = morphic::decode_kv3_resource(&b).unwrap();
    if let morphic::kv3::Value::Object(o) = &t {
        for (k, _) in o {
            println!("{k}");
        }
    }
}
