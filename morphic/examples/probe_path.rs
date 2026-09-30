//! Decode any block (by FOURCC) as KV3 and pretty-print the value at a nested
//! path. Path segments separated by '/'; a numeric segment indexes an array,
//! anything else keys an object. With no path, lists top-level keys+shapes.
//! A trailing "?" segment prints shapes of the children instead of full values.
//!
//! Usage: probe_path <file.vmdl_c> <FOURCC> [seg/seg/seg] [--shape]
use morphic::kv3::Value;
use morphic::resource::Resource;

fn shape(v: &Value) -> String {
    match v {
        Value::Array(a) => format!("array[{}]", a.len()),
        Value::Object(o) => format!("object{{{}}}", o.len()),
        Value::Int(x) => format!("Int({x})"),
        Value::UInt(x) => format!("UInt({x})"),
        Value::Double(x) => format!("Double({x})"),
        Value::String(s) => format!("String({s:?})"),
        Value::Binary(b) => format!("Binary[{}]", b.len()),
        other => format!("{other:?}").chars().take(60).collect(),
    }
}

fn children(v: &Value) {
    match v {
        Value::Object(o) => {
            for (k, c) in o {
                println!("  {k}\t{}", shape(c));
            }
        }
        Value::Array(a) => {
            for (i, c) in a.iter().enumerate() {
                println!("  [{i}]\t{}", shape(c));
            }
        }
        other => println!("  (leaf) {}", shape(other)),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let res = Resource::parse(&bytes).expect("parse");
    let (fourcc, occ) = match a[2].split_once(':') {
        Some((f, n)) => (f, n.parse::<usize>().expect("occurrence index")),
        None => (a[2].as_str(), 0),
    };
    let mut k = [0u8; 4];
    k.copy_from_slice(fourcc.as_bytes());
    let payload = res
        .blocks()
        .iter()
        .filter(|b| b.kind == k)
        .nth(occ)
        .map(|b| &bytes[b.offset as usize..(b.offset + b.size) as usize])
        .expect("block not found");
    let tree = morphic::kv3::decode(payload).expect("kv3 decode");

    let shape_only = a.iter().any(|s| s == "--shape");
    let path = a.get(3).filter(|s| !s.starts_with("--"));

    let mut cur = &tree;
    if let Some(p) = path {
        for seg in p.split('/') {
            if seg.is_empty() {
                continue;
            }
            cur = if let Ok(idx) = seg.parse::<usize>() {
                cur.as_array()
                    .and_then(|ar| ar.get(idx))
                    .unwrap_or_else(|| panic!("no index {seg}"))
            } else {
                cur.get(seg).unwrap_or_else(|| panic!("no key {seg}"))
            };
        }
    }

    if shape_only {
        println!("{}:", path.map(String::as_str).unwrap_or("<root>"));
        children(cur);
    } else if matches!(cur, Value::Array(_) | Value::Object(_)) {
        println!("{:#?}", cur);
    } else {
        println!("{}", shape(cur));
    }
}
