//! Print the byte tail of a resource's KV3 DATA block (blob frame region).
//! usage: blobtail <file.vmat_c>
fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("file");
    let bytes = std::fs::read(path)?;
    let b = morphic::kv3_resource_data_block(&bytes)?;
    let comp1 = i32_at(&b, 76) as usize;
    let comp2 = i32_at(&b, 84) as usize;
    println!(
        "len={} comp={} blocks={} sizeBlobs={} tableBytes={} comp1={} comp2={}",
        b.len(),
        i32_at(&b, 20),
        i32_at(&b, 56),
        i32_at(&b, 60),
        i32_at(&b, 68),
        comp1,
        comp2
    );
    let frames_start = 120 + comp1 + comp2;
    println!(
        "frames region ({} bytes): {:02x?}",
        b.len() - frames_start,
        &b[frames_start..]
    );
    Ok(())
}
