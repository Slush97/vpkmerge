//! Isolation test: decode a model's DATA tree and re-encode it with NO edits,
//! then write it back out. If the engine rejects this (ERROR model) it proves
//! morphic's full model-DATA re-encode is not engine-valid, independent of any
//! content change -- so skeleton/remap edits must be done by in-place patching,
//! not encode_kv3_resource.
//! Usage: reencode_unchanged <in.vmdl_c> <out.vmdl_c>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let tree = morphic::decode_kv3_resource(&bytes).expect("decode");
    let out = morphic::encode_kv3_resource(&bytes, &tree).expect("encode");
    std::fs::write(&a[2], &out).expect("write");
    println!(
        "re-encoded UNCHANGED: {} -> {} bytes -> {}",
        bytes.len(),
        out.len(),
        a[2]
    );
}
