use anyhow::Result;
fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let b = std::fs::read(&a[1])?;
    let r = morphic::resource::Resource::parse(&b)?;
    for block in r.blocks() {
        println!("{} {}", String::from_utf8_lossy(&block.kind), block.size);
        if &block.kind == b"PHYS" || &block.kind == b"DATA" {
            let tree = morphic::kv3::decode(r.find_block(block.kind).unwrap())?;
            println!("{tree:#?}");
        }
    }
    Ok(())
}
