//! Print the DATA-block KV3 version + blob flag of a compiled resource.
fn main() {
    let path = std::env::args().nth(1).expect("file arg");
    let bytes = std::fs::read(&path).expect("read");
    let data = morphic::kv3_resource_data_block(&bytes).expect("data block");
    let ver = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) & 0xFF;
    let has_blobs = morphic::kv3_resource_has_blobs(&bytes).expect("blobs");
    println!(
        "DATA kv3 version={ver} has_blobs={has_blobs} block_len={}",
        data.len()
    );
}
