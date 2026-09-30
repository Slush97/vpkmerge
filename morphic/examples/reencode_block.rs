//! Isolation test for a NON-DATA block: decode one block (by FOURCC) as KV3,
//! re-encode it with morphic's block writer (v4 uncompressed), and splice it
//! back via rebuild_with_block. Used to learn whether a full re-encode of a
//! given block (e.g. CTRL) is engine-valid, the way reencode_unchanged tested
//! the DATA block.
//!
//! Usage: reencode_block <in.vmdl_c> <FOURCC> <out.vmdl_c>
use morphic::kv3::{decode, encode, Format};
use morphic::resource::Resource;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let mut k = [0u8; 4];
    k.copy_from_slice(a[2].as_bytes());

    let res = Resource::parse(&bytes).expect("parse");
    let idx = res
        .blocks()
        .iter()
        .position(|b| b.kind == k)
        .expect("block FOURCC not found");
    let payload = res.get_block_by_index(idx).expect("payload");

    let tree = decode(payload).expect("decode block");
    let fmt = Format::from_payload(payload).expect("format guid");
    let reenc = encode(&tree, &fmt);

    // sanity: re-decode the freshly encoded block and confirm tree-equality.
    let back = decode(&reenc).expect("re-decode block");
    let eq = format!("{tree:?}") == format!("{back:?}");
    println!(
        "block {} (decl#{idx}) {} -> {} bytes, tree-equal={eq}",
        a[2],
        payload.len(),
        reenc.len()
    );

    let out = res.rebuild_with_block(idx, &reenc).expect("rebuild");
    // re-parse the whole resource to confirm the table is consistent.
    let res2 = Resource::parse(&out).expect("re-parse rebuilt");
    assert_eq!(
        res2.blocks().len(),
        res.blocks().len(),
        "block count changed"
    );
    println!(
        "rebuilt resource: {} -> {} bytes, {} blocks",
        bytes.len(),
        out.len(),
        res2.blocks().len()
    );

    std::fs::write(&a[3], &out).expect("write");
    println!("wrote {}", a[3]);
}
