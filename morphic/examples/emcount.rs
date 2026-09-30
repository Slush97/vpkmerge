use morphic::kv3::Value;
use morphic::resource::Resource;
fn main() {
    let b = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    let res = Resource::parse(&b).unwrap();
    let ctrl = res.find_block(*b"CTRL").unwrap();
    let t = morphic::kv3::decode(ctrl).unwrap();
    let em = t.get("embedded_meshes").and_then(Value::as_array).unwrap();
    println!("embedded_meshes: {} meshes", em.len());
    for (i, m) in em.iter().enumerate() {
        let name = m.get("m_Name").and_then(Value::as_str).unwrap_or("?");
        let db = m.get("m_nDataBlock").and_then(Value::as_int).unwrap_or(-9);
        let vbs = m
            .get("m_vertexBuffers")
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0);
        let ib = m
            .get("m_indexBuffers")
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0);
        let vb_idx: Vec<i64> = m
            .get("m_vertexBuffers")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("m_nBlockIndex").and_then(Value::as_int))
                    .collect()
            })
            .unwrap_or_default();
        println!(
            "  [{i}] {name:30} dataBlock={db} vtxBufs={vbs}{:?} idxBufs={ib}",
            vb_idx
        );
    }
    // also show embedded_animation/physics/distancefield block refs
    for k in [
        "embedded_animation",
        "embedded_physics",
        "embedded_distancefield",
    ] {
        if let Some(o) = t.get(k) {
            println!("  {k} = {o:?}");
        }
    }
}
