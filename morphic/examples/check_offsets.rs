//! Compare original block offsets to what an align16 cumulative writer would
//! produce, to find where the layout diverges.
//! Usage: check_offsets <file.vmdl_c>
fn align16(n: usize) -> usize {
    (n + 15) & !15
}
fn main() {
    let path = std::env::args().nth(1).expect("file arg");
    let bytes = std::fs::read(&path).expect("read");
    let res = morphic::resource::Resource::parse(&bytes).expect("parse");
    let blocks = res.blocks();
    let table_len = blocks.len() * 12;
    let mut cursor = align16(16 + table_len);
    let mut mism = 0;
    for (i, b) in blocks.iter().enumerate() {
        let expected = cursor;
        let actual = b.offset as usize;
        if expected != actual {
            let k = String::from_utf8_lossy(&b.kind);
            println!(
                "MISMATCH decl#{i} {k}: original_off={actual} align16_off={expected} (diff {})",
                actual as i64 - expected as i64
            );
            mism += 1;
        }
        cursor = align16(actual + b.size as usize);
    }
    println!("mismatches={mism} / {}", blocks.len());
}
