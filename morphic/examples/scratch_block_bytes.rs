// Throwaway: hexdump the first N bytes of a block by index.
// usage: cargo run -p morphic --example scratch_block_bytes -- <file.vmdl_c> <index> [n]
use morphic::resource::Resource;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");
    let idx: usize = a[2].parse().expect("index");
    let n: usize = a.get(3).map_or(48, |s| s.parse().unwrap());
    let res = Resource::parse(&bytes).expect("parse");
    let payload = res.get_block_by_index(idx).expect("block");
    println!("len={}", payload.len());
    for (i, chunk) in payload
        .iter()
        .take(n)
        .collect::<Vec<_>>()
        .chunks(16)
        .enumerate()
    {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        println!("  {:04x}  {}", i * 16, hex.join(" "));
    }
}
