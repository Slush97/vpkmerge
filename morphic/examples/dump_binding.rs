//! Dump m_modelSkeleton bone names + m_remappingTable compactly for one model.
//! Usage: dump_binding <file.vmdl_c>

fn main() {
    let path = std::env::args().nth(1).expect("file");
    let bytes = std::fs::read(&path).expect("read");
    let tree = morphic::decode_kv3_resource(&bytes).expect("decode");
    let sk = tree.get("m_modelSkeleton").expect("skel");
    let names: Vec<String> = sk
        .get("m_boneName")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap_or("?").to_string())
        .collect();
    println!("bones={}", names.len());
    print!("first 12: ");
    for n in names.iter().take(12) {
        print!("{n} ");
    }
    println!();
    if let Some(rt) = tree.get("m_remappingTable") {
        let r: Vec<i64> = rt
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_int().unwrap_or(-1))
            .collect();
        println!("remap len={}", r.len());
        print!("remap first 16: ");
        for v in r.iter().take(16) {
            print!("{v} ");
        }
        println!();
        // print the names the first 16 remap entries point to
        print!("remap[0..16] -> bone names: ");
        for v in r.iter().take(16) {
            let n = if *v >= 0 && (*v as usize) < names.len() {
                &names[*v as usize]
            } else {
                "?"
            };
            print!("{n} ");
        }
        println!();
    }
    if let Some(st) = tree.get("m_remappingTableStarts") {
        println!("starts: {st:?}");
    }
}
