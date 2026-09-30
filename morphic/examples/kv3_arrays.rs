//! Show how arrays are framed on the wire (generic / typed / byte-length /
//! auxiliary-buffer, element type, length), which the folded `Value` view hides.
//!
//! Usage:
//!   kv3_arrays <vpk> <entry>          every array in every KV3 block of one entry
//!   kv3_arrays <vpk> .<ext> <KEY>     histogram of how root member KEY is framed
//!                                     across every entry with that extension
use morphic::kv3::{Node, Val};
use std::collections::BTreeMap;

fn walk(path: &str, tag: u8, v: &Val, aux: bool) {
    match v {
        Val::Array(items) => {
            println!("{path}: tag {tag} generic len {} aux={aux}", items.len());
            for (i, n) in items.iter().enumerate() {
                walk(&format!("{path}[{i}]"), n.tag, &n.val, aux);
            }
        }
        Val::Typed {
            sub,
            sub_flag,
            items,
        } => {
            println!(
                "{path}: tag {tag} sub {sub} flag {sub_flag:?} len {} aux={aux}",
                items.len()
            );
            let aux = aux ^ (tag == 25);
            for (i, it) in items.iter().enumerate() {
                walk(&format!("{path}[{i}]"), *sub, it, aux);
            }
        }
        Val::Object(m) => {
            for (k, n) in m {
                walk(&format!("{path}/{k}"), n.tag, &n.val, aux);
            }
        }
        _ => {}
    }
}

fn framing(n: &Node) -> String {
    match &n.val {
        Val::Typed { sub, items, .. } => {
            format!("tag {} sub {sub} len>255={}", n.tag, items.len() > 255)
        }
        Val::Array(items) => format!("tag {} generic empty={}", n.tag, items.is_empty()),
        _ => format!("tag {}", n.tag),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(&a[1]).expect("open vpk");
    let read = |entry: &str| vpk.get_file(entry).and_then(|mut f| f.read_all());

    if a[2].starts_with('.') {
        let mut hist = BTreeMap::<String, usize>::new();
        let files: Vec<&String> = vpk.file_paths().filter(|p| p.ends_with(&a[2])).collect();
        for f in files {
            let Ok(bytes) = read(f) else { continue };
            let Ok(data) = morphic::kv3_resource_data_block(&bytes) else {
                continue;
            };
            let Ok(doc) = morphic::kv3::decode_lossless(&data) else {
                continue;
            };
            if let Some(n) = doc.root.get(&a[3]) {
                *hist.entry(framing(n)).or_default() += 1;
            }
        }
        for (k, v) in hist {
            println!("{v:6} {k}");
        }
        return;
    }

    let bytes = read(&a[2]).expect("read entry");
    let res = morphic::resource::Resource::parse(&bytes).expect("parse resource");
    for (i, meta) in res.blocks().iter().enumerate() {
        let Some(b) = res.get_block_by_index(i) else {
            continue;
        };
        let Ok(doc) = morphic::kv3::decode_lossless(b) else {
            continue;
        };
        println!("== {}", String::from_utf8_lossy(&meta.kind));
        walk("", doc.root.tag, &doc.root.val, false);
    }
}
