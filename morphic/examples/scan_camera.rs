//! Scan a .vmdl_c for the distinctive CitadelCameraSettings offset floats
//! (-35.36 side, 110.24 back, 87.36 height, 56.6 crouch, 75.92 aim) as f32/f64
//! and report which block (by FOURCC) each hit lands in. These are the
//! over-the-shoulder camera values; if vanilla bakes them in a binary block
//! that hat-man lacks, that block IS the camera.
//! Usage: scan_camera <file.vmdl_c>
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

    let vals: &[(&str, f64)] = &[
        ("side -35.36", -35.36),
        ("back 110.24", 110.24),
        ("backaim 75.92", 75.92),
        ("height 87.36", 87.36),
        ("crouch 56.6", 56.6),
    ];
    for (name, v) in vals {
        let f32p = (*v as f32).to_le_bytes().to_vec();
        let f64p = v.to_le_bytes().to_vec();
        let h32 = find_all(&bytes, &f32p);
        let h64 = find_all(&bytes, &f64p);
        let b32: Vec<String> = h32.iter().take(8).map(|&o| block_of(&res, o)).collect();
        let b64: Vec<String> = h64.iter().take(8).map(|&o| block_of(&res, o)).collect();
        println!(
            "{name}: f32 {} {b32:?} | f64 {} {b64:?}",
            h32.len(),
            h64.len()
        );
    }
}
