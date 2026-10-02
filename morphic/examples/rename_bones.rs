//! Rename bones in a `.vmdl_c` everywhere the model refers to them: every KV3 block
//! (DATA skeleton, MDAT mesh skeletons, PHYS FeModel ctrl names, embedded anim
//! bone lists) gets each exact-match string redirected, and every integer equal
//! to the old name's string token (murmur2, seed 0x31415926, lowercased; e.g.
//! `m_CtrlHash`) rewritten to the new name's token. Strings are appended to each
//! block's table (`set_strings_adding`); all other bytes are preserved.
//!
//! Why: the runtime maps the NM (animgraph) skeleton onto model bones by name. A
//! community skin whose skinning or cloth anchors sit on a stock bone (Midnight
//! Mina's `skirt_*`) inherits stock keyframes authored for a different mesh;
//! renaming detaches the bone from the NM skeleton so it FK-rides its parent at
//! its bind-relative transform instead.
//!
//! Usage: rename_bones <in.vmdl_c> <out.vmdl_c> <old=new> [<old=new>...]
use morphic::kv3::{self, Seg, Value};
use morphic::resource::Resource;
use morphic::vfx_expr::murmur2;

fn token(name: &str) -> u64 {
    u64::from(murmur2(name.to_lowercase().as_bytes(), 0x3141_5926))
}

fn walk(
    v: &Value,
    path: &mut Vec<Seg>,
    map: &[(String, String)],
    strs: &mut Vec<(Vec<Seg>, String)>,
    hashes: &mut Vec<(Vec<Seg>, i64)>,
) {
    match v {
        Value::String(s) => {
            if let Some((_, new)) = map.iter().find(|(old, _)| old == s) {
                strs.push((path.clone(), new.clone()));
            }
        }
        Value::UInt(_) | Value::Int(_) => {
            let n = match v {
                Value::UInt(u) => *u,
                Value::Int(i) => u64::try_from(*i).unwrap_or(u64::MAX),
                _ => unreachable!(),
            };
            if let Some((_, new)) = map.iter().find(|(old, _)| token(old) == n) {
                hashes.push((path.clone(), i64::try_from(token(new)).unwrap()));
            }
        }
        Value::Array(items) => {
            for (i, x) in items.iter().enumerate() {
                path.push(Seg::Index(i));
                walk(x, path, map, strs, hashes);
                path.pop();
            }
        }
        Value::Object(pairs) => {
            for (k, x) in pairs {
                path.push(Seg::Key(k.clone()));
                walk(x, path, map, strs, hashes);
                path.pop();
            }
        }
        _ => {}
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    assert!(
        a.len() >= 4,
        "usage: rename_bones <in.vmdl_c> <out.vmdl_c> <old=new>..."
    );
    let map: Vec<(String, String)> = a[3..]
        .iter()
        .map(|p| {
            let (o, n) = p.split_once('=').expect("old=new");
            (o.to_owned(), n.to_owned())
        })
        .collect();
    let mut bytes = std::fs::read(&a[1]).expect("read input");
    let count = Resource::parse(&bytes).expect("parse").blocks().len();
    for bi in 0..count {
        let res = Resource::parse(&bytes).expect("parse");
        let kind = String::from_utf8_lossy(&res.blocks()[bi].kind).into_owned();
        let raw = res.get_block_by_index(bi).expect("block");
        let Ok(tree) = kv3::decode(raw) else { continue };
        let (mut strs, mut hashes) = (Vec::new(), Vec::new());
        walk(&tree, &mut Vec::new(), &map, &mut strs, &mut hashes);
        if strs.is_empty() && hashes.is_empty() {
            continue;
        }
        let mut fields: Vec<String> = strs
            .iter()
            .map(|(p, _)| p)
            .chain(hashes.iter().map(|(p, _)| p))
            .map(|p| {
                p.iter()
                    .filter_map(|s| match s {
                        Seg::Key(k) => Some(k.as_str()),
                        Seg::Index(_) => None,
                    })
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .collect();
        fields.sort();
        fields.dedup();
        println!(
            "block {bi} {kind}: {} strings, {} tokens  [{}]",
            strs.len(),
            hashes.len(),
            fields.join(", ")
        );
        let mut payload = if strs.is_empty() {
            raw.to_vec()
        } else {
            kv3::set_strings_adding(raw, &strs).expect("set strings")
        };
        if !hashes.is_empty() {
            payload = kv3::set_scalars(&payload, &hashes).expect("set tokens");
        }
        bytes = res.rebuild_with_block(bi, &payload).expect("rebuild");
    }
    std::fs::write(&a[2], &bytes).expect("write output");
    println!("wrote {} ({} bytes)", a[2], bytes.len());
}
