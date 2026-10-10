//! Verify morphic re-encodes a resource's DATA block faithfully to the tree:
//! decode -> encode_kv3_resource -> decode, assert the two trees are equal.
//! Usage: kv3_roundtrip_check <file.vmdl_c>
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let bytes = std::fs::read(&path).expect("read");
    let tree = morphic::decode_kv3_resource(&bytes).expect("decode1");
    let re = morphic::encode_kv3_resource(&bytes, &tree).expect("encode");
    let tree2 = morphic::decode_kv3_resource(&re).expect("decode2");
    if tree == tree2 {
        println!(
            "TREE-FAITHFUL: decode->encode->decode equal (orig DATA file {} -> {} bytes)",
            bytes.len(),
            re.len()
        );
    } else {
        println!("MISMATCH: re-encoded tree differs from original tree");
        // find first differing top-level key
        if let (morphic::kv3::Value::Object(a), morphic::kv3::Value::Object(b)) = (&tree, &tree2) {
            for ((ka, va), (kb, vb)) in a.iter().zip(b.iter()) {
                if ka != kb || va != vb {
                    println!("  first diff key: {ka} vs {kb} (equal_val={})", va == vb);
                    break;
                }
            }
        }
    }
}
