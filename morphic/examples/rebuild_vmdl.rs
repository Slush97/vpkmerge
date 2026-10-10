//! Reassemble a .vmdl_c through the structured block writer (full re-layout via
//! rebuild_with_block) and write it out. Engine-valid (align16 layout), used to
//! confirm in-game that the writer preserves the camera bake.
//! Usage: rebuild_vmdl <in.vmdl_c> <out.vmdl_c>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let res = morphic::resource::Resource::parse(&bytes).expect("parse");
    let block0 = res.get_block_by_index(0).expect("block 0").to_vec();
    let rebuilt = res.rebuild_with_block(0, &block0).expect("rebuild");
    std::fs::write(&a[2], &rebuilt).expect("write");
    println!("rebuilt {} -> {} ({} bytes)", a[1], a[2], rebuilt.len());
}
