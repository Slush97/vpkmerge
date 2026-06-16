use morphic::kv3::Value;
fn u32_at(d: &[u8], o: usize) -> usize {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap()) as usize
}
fn ctrl(d: &[u8]) -> Vec<u8> {
    let bo = u32_at(d, 8);
    let bc = u32_at(d, 12);
    let base = 8 + bo;
    for i in 0..bc {
        let off = base + i * 12;
        if &d[off..off + 4] == b"CTRL" {
            let s = off + 4 + u32_at(d, off + 4);
            let sz = u32_at(d, off + 8);
            return d[s..s + sz].to_vec();
        }
    }
    vec![]
}
fn walk(v: &Value, out: &mut Vec<bool>) {
    match v {
        Value::Object(p) => {
            for (k, c) in p {
                if k == "m_bMeshoptCompressed" {
                    if let Some(b) = c.as_bool() {
                        out.push(b);
                    }
                }
                walk(c, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|c| walk(c, out)),
        _ => {}
    }
}
fn main() {
    for p in std::env::args().skip(1) {
        let d = std::fs::read(&p).unwrap();
        let c = ctrl(&d);
        let t = morphic::kv3::decode(&c).unwrap();
        let mut f = vec![];
        walk(&t, &mut f);
        println!("{p}: m_bMeshoptCompressed flags = {:?}", f);
    }
}
