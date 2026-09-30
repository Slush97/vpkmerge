// Throwaway: print each block's fourcc, size and KV3 container header bytes.
// usage: cargo run -p morphic --example scratch_block_header -- <file.vmdl_c>
use morphic::resource::Resource;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let res = Resource::parse(&bytes).expect("parse");
    for (i, block) in res.blocks().iter().enumerate() {
        let Some(payload) = res.get_block_by_index(i) else {
            continue;
        };
        let fourcc = String::from_utf8_lossy(&block.kind).to_string();
        let head: Vec<String> = payload.iter().take(8).map(|b| format!("{b:02x}")).collect();
        // KV3 v5 header: magic(4) then format guid(16) then u32 compression method etc.
        let ver = if payload.len() > 4 { payload[3] } else { 0 };
        println!(
            "block {i:2} {fourcc} len={:<9} magic={} kv3ver={}",
            payload.len(),
            head.join(" "),
            ver as char
        );
    }
}
