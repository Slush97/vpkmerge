//! Decode a resource DATA block and pretty-print one root field's Value tree.
//! Usage: dump_field <file.vmdl_c> <key>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let tree = morphic::decode_kv3_resource(&bytes).expect("decode");
    match tree.get(&a[2]) {
        Some(v) => println!("{} = {v:#?}", a[2]),
        None => println!("{} : <absent>", a[2]),
    }
}
