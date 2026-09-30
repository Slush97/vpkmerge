//! Throwaway: dump m_modelSkeleton per-bone name/parent/flag/pos/rot as JSON.
//! Usage: dump_skel_info <file.vmdl_c>
use morphic::kv3::Value;

fn nums(v: &Value) -> Vec<f64> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| match x {
                    Value::Double(d) => *d,
                    Value::Int(i) => *i as f64,
                    Value::UInt(u) => *u as f64,
                    _ => f64::NAN,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn int(v: &Value) -> i64 {
    match v {
        Value::Int(i) => *i,
        Value::UInt(u) => i64::try_from(*u).unwrap_or(-1),
        _ => -1,
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let tree = morphic::decode_kv3_resource(&bytes).expect("decode");
    let skel = tree.get("m_modelSkeleton").expect("m_modelSkeleton");
    let names = skel.get("m_boneName").and_then(Value::as_array).unwrap();
    let parents = skel.get("m_nParent").and_then(Value::as_array).unwrap();
    let flags = skel.get("m_nFlag").and_then(Value::as_array).unwrap();
    let pos = skel
        .get("m_bonePosParent")
        .and_then(Value::as_array)
        .unwrap();
    let rot = skel
        .get("m_boneRotParent")
        .and_then(Value::as_array)
        .unwrap();
    println!("[");
    for i in 0..names.len() {
        let comma = if i + 1 == names.len() { "" } else { "," };
        println!(
            "{{\"i\":{},\"name\":{:?},\"parent\":{},\"flag\":{},\"pos\":{:?},\"rot\":{:?}}}{}",
            i,
            names[i].as_str().unwrap_or(""),
            int(&parents[i]),
            int(&flags[i]),
            nums(&pos[i]),
            nums(&rot[i]),
            comma
        );
    }
    println!("]");
}
