//! Full structural diff of two resources' DATA trees: print every key-path that
//! exists in only one, or whose scalar/shape differs. Arrays are compared by
//! length and element-wise (descending into objects). Used to enumerate EVERY
//! field that differs between vanilla and the RC-built model, so the camera
//! field can't hide.
//! Usage: tree_diff <a.vmdl_c> <b.vmdl_c>
use morphic::kv3::Value;

fn kind(v: &Value) -> String {
    match v {
        Value::Object(_) => "object".into(),
        Value::Array(a) => format!("array[{}]", a.len()),
        Value::String(s) => format!("str({:?})", s.chars().take(40).collect::<String>()),
        Value::Resource(s) => format!("res({:?})", s.chars().take(40).collect::<String>()),
        other => format!("{other:?}").chars().take(50).collect(),
    }
}

fn diff(a: &Value, b: &Value, path: &str, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(fa), Value::Object(fb)) => {
            for (k, va) in fa {
                let p = format!("{path}/{k}");
                match fb.iter().find(|(kk, _)| kk == k) {
                    Some((_, vb)) => diff(va, vb, &p, out),
                    None => out.push(format!("ONLY-A {p} = {}", kind(va))),
                }
            }
            for (k, vb) in fb {
                if !fa.iter().any(|(kk, _)| kk == k) {
                    out.push(format!("ONLY-B {path}/{k} = {}", kind(vb)));
                }
            }
        }
        (Value::Array(aa), Value::Array(ab)) => {
            if aa.len() != ab.len() {
                out.push(format!("LEN   {path}: A={} B={}", aa.len(), ab.len()));
            }
            for (i, (x, y)) in aa.iter().zip(ab.iter()).enumerate() {
                diff(x, y, &format!("{path}[{i}]"), out);
            }
        }
        _ => {
            if a != b {
                out.push(format!("DIFF  {path}: A={} B={}", kind(a), kind(b)));
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ba = std::fs::read(&args[1]).expect("read a");
    let bb = std::fs::read(&args[2]).expect("read b");
    let ta = morphic::decode_kv3_resource(&ba).expect("decode a");
    let tb = morphic::decode_kv3_resource(&bb).expect("decode b");
    let mut out = Vec::new();
    diff(&ta, &tb, "", &mut out);
    // Collapse array-element spam: summarize repeated [N] paths under a root.
    println!("total diff lines: {}", out.len());
    for line in out.iter().take(400) {
        println!("{line}");
    }
    if out.len() > 400 {
        println!("... ({} more)", out.len() - 400);
    }
}
