//! Decode any block (by FOURCC) as KV3 and pretty-print it (or just its
//! top-level keys / a chosen field).
//! Usage: dump_block_kv3 <file.vmdl_c> <FOURCC> [field]
use morphic::resource::Resource;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let res = Resource::parse(&bytes).expect("parse");
    let mut k = [0u8; 4];
    k.copy_from_slice(a[2].as_bytes());
    let payload = res.find_block(k).expect("block not found");
    let tree = morphic::kv3::decode(payload).expect("kv3 decode");
    match a.get(3) {
        Some(field) => match tree.get(field) {
            Some(v) => println!("{field} = {v:#?}"),
            None => println!("{field}: <absent>"),
        },
        None => {
            if let Some(obj) = tree.as_object() {
                for (key, v) in obj {
                    let kind = match v {
                        morphic::kv3::Value::Array(arr) => format!("array[{}]", arr.len()),
                        morphic::kv3::Value::Object(_) => "object".into(),
                        morphic::kv3::Value::String(s) => format!("str({s:?})"),
                        other => format!("{other:?}").chars().take(50).collect(),
                    };
                    println!("{key}\t{kind}");
                }
            } else {
                println!("{tree:#?}");
            }
        }
    }
}
