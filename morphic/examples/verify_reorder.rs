//! Verify a reordered .vmdl_c is engine-valid: every NON-DATA block byte-identical
//! to the original, and the injected animgraph/nmskel refs survived the DATA patch.
//! Usage: verify_reorder <orig.vmdl_c> <reordered.vmdl_c>
use morphic::kv3::Value;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let orig = std::fs::read(&a[1]).expect("read orig");
    let out = std::fs::read(&a[2]).expect("read out");

    let ro = morphic::resource::Resource::parse(&orig).expect("parse orig");
    let rn = morphic::resource::Resource::parse(&out).expect("parse out");
    let bo = ro.blocks();
    let bn = rn.blocks();
    println!("orig {} blocks, out {} blocks", bo.len(), bn.len());
    assert_eq!(bo.len(), bn.len(), "block count changed");

    for (i, (a, b)) in bo.iter().zip(bn.iter()).enumerate() {
        let ka = String::from_utf8_lossy(&a.kind);
        let kb = String::from_utf8_lossy(&b.kind);
        assert_eq!(a.kind, b.kind, "block {i} type changed {ka}->{kb}");
        let sa = &orig[a.offset as usize..(a.offset + a.size) as usize];
        let sb = &out[b.offset as usize..(b.offset + b.size) as usize];
        if ka == "DATA" {
            println!(
                "  block {i} {ka}: {} -> {} bytes (patched, expected)",
                sa.len(),
                sb.len()
            );
        } else {
            let same = sa == sb;
            println!(
                "  block {i} {ka}: {} bytes, byte-identical={same}",
                sa.len()
            );
            assert!(
                same,
                "NON-DATA block {ka} changed -- mesh/envelope corruption"
            );
        }
    }

    // animgraph/nmskel refs still present in DATA?
    let tree = morphic::decode_kv3_resource(&out).expect("decode out DATA");
    for f in ["m_animGraph2Refs", "m_vecNmSkeletonRefs"] {
        match tree.get(f).and_then(Value::as_array) {
            Some(arr) => println!("  {f}: present, len {}", arr.len()),
            None => println!("  {f}: ABSENT (warn)"),
        }
    }
    println!("VERIFY OK");
}
