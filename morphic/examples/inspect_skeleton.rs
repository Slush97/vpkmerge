//! Inspect the on-disk structure of m_modelSkeleton's per-bone arrays so the
//! in-place reorder knows exactly which lanes/widths each field occupies.
//! Usage: inspect_skeleton <file.vmdl_c>
use morphic::kv3::Value;

fn shape(v: &Value) -> String {
    match v {
        Value::Null => "Null".into(),
        Value::Bool(_) => "Bool".into(),
        Value::Int(x) => format!("Int({x})"),
        Value::UInt(x) => format!("UInt({x})"),
        Value::Double(x) => format!("Double({x})"),
        Value::String(s) => format!("String({s:?})"),
        Value::Array(a) => format!("Array[{}]", a.len()),
        Value::Object(o) => format!("Object{{{}}}", o.len()),
        _ => "<other>".into(),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let tree = morphic::decode_kv3_resource(&bytes).expect("decode");
    let skel = tree.get("m_modelSkeleton").expect("m_modelSkeleton");
    println!("== m_modelSkeleton keys ==");
    if let Some(obj) = skel.as_object() {
        for (k, v) in obj {
            println!("  {k} = {}", shape(v));
        }
    }
    for field in [
        "m_boneName",
        "m_nParent",
        "m_bonePosParent",
        "m_boneRotParent",
        "m_nFlag",
        "m_boneScaleParent",
    ] {
        if let Some(arr) = skel.get(field).and_then(Value::as_array) {
            println!("\n== {field} (len {}) ==", arr.len());
            for (i, e) in arr.iter().take(2).enumerate() {
                print!("  [{i}] = {}", shape(e));
                if let Some(inner) = e.as_array() {
                    let kinds: Vec<String> = inner.iter().map(shape).collect();
                    print!("  -> [{}]", kinds.join(", "));
                }
                println!();
            }
        }
    }
    // remapping table value sample
    if let Some(rt) = tree.get("m_remappingTable").and_then(Value::as_array) {
        println!("\n== m_remappingTable (len {}) ==", rt.len());
        let sample: Vec<String> = rt.iter().take(6).map(shape).collect();
        println!("  first 6: {}", sample.join(", "));
    }
}
