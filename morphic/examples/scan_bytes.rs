//! Scan a file for byte patterns and report which block (by FOURCC) each hit
//! falls in. Used to locate the baked camera struct (distinctive values:
//! 1920x1080 int pair, FOV floats 90/35/25).
//! Usage: scan_bytes <file.vmdl_c>
use morphic::resource::Resource;

fn block_of(res: &Resource, off: usize) -> String {
    for (i, b) in res.blocks().iter().enumerate() {
        let s = b.offset as usize;
        let e = s + b.size as usize;
        if off >= s && off < e {
            return format!("#{i} {} (+{})", String::from_utf8_lossy(&b.kind), off - s);
        }
    }
    "??".into()
}

fn find_all(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() || needle.len() > hay.len() {
        return out;
    }
    for i in 0..=hay.len() - needle.len() {
        if &hay[i..i + needle.len()] == needle {
            out.push(i);
        }
    }
    out
}

fn main() {
    let path = std::env::args().nth(1).expect("file");
    let bytes = std::fs::read(&path).expect("read");
    let res = Resource::parse(&bytes).expect("parse");

    let patterns: &[(&str, Vec<u8>)] = &[
        (
            "1920|1080 int32 pair",
            [1920u32.to_le_bytes(), 1080u32.to_le_bytes()].concat(),
        ),
        (
            "512|384 int32 pair",
            [512u32.to_le_bytes(), 384u32.to_le_bytes()].concat(),
        ),
        ("fov 90.0 f32", 90.0f32.to_le_bytes().to_vec()),
        ("fov 35.0 f32", 35.0f32.to_le_bytes().to_vec()),
        ("fov 25.0 f32", 25.0f32.to_le_bytes().to_vec()),
        ("1920 f32", 1920.0f32.to_le_bytes().to_vec()),
    ];
    for (name, pat) in patterns {
        let hits = find_all(&bytes, pat);
        print!("{name}: {} hits", hits.len());
        let blocks: Vec<String> = hits.iter().take(12).map(|&o| block_of(&res, o)).collect();
        println!(" -> {blocks:?}");
    }
}
