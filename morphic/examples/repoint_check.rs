//! Live check for `morphic::repoint_resource_refs` over a real pak: repoints one
//! referenced resource of each sampled file to a made-up path and checks the
//! result against the original (decoded tree, `RERL`, untouched blocks).
//!
//! Every `stride`-th entry of `.vpcf_c` / `.vmat_c` / `.vmdl_c` / `.vnmgraph_c` is
//! checked, plus every entry containing one of the `substr` arguments. Outputs for
//! the substring matches are written to `<out_dir>/<path with / as __>` so they can be fed
//! to the strict loader (`morphic-oracle validate`).
//!
//! Usage: cargo run --release -p morphic --example repoint_check -- <pak01_dir.vpk> <out_dir> <stride> [substr...]

use std::collections::BTreeMap;
use std::path::Path;

use morphic::kv3::{decode_lossless, Node, Val, Value};
use morphic::resource::{read_rerl, resource_id, Resource};

fn kv3_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|c| kv3_strings(c, out)),
        Value::Object(o) => o.iter().for_each(|(_, c)| kv3_strings(c, out)),
        _ => {}
    }
}

fn subst_val(v: &mut Val, old: &str, new: &str) {
    match v {
        Val::String(s) if s == old => *s = new.to_string(),
        Val::Array(items) => items.iter_mut().for_each(|n| subst(n, old, new)),
        Val::Typed { items, .. } => items.iter_mut().for_each(|i| subst_val(i, old, new)),
        Val::Object(m) => m.iter_mut().for_each(|(_, n)| subst(n, old, new)),
        _ => {}
    }
}

fn subst(n: &mut Node, old: &str, new: &str) {
    subst_val(&mut n.val, old, new);
}

fn made_up(old: &str) -> String {
    let (stem, ext) = old.rsplit_once('.').unwrap_or((old, ""));
    let base = stem.rsplit('/').next().unwrap_or(stem);
    format!("zz_repoint/{base}.{ext}")
}

fn check(bytes: &[u8]) -> Result<Option<(Vec<u8>, String, String, usize)>, String> {
    let res = Resource::parse(bytes).map_err(|e| format!("parse: {e}"))?;
    let Some(rerl) = res.find_block(*b"RERL") else {
        return Ok(None);
    };
    let refs = read_rerl(rerl).map_err(|e| e.to_string())?;
    let tree = morphic::decode_kv3_resource(bytes).map_err(|e| format!("decode: {e}"))?;
    let mut strings = Vec::new();
    kv3_strings(&tree, &mut strings);
    let Some((_, old)) = refs.iter().find(|(_, n)| strings.contains(n)) else {
        return Ok(None);
    };
    let new = made_up(old);
    let (out, count) = morphic::repoint_resource_refs(bytes, &[(old.clone(), new.clone())])
        .map_err(|e| format!("repoint: {e}"))?;

    let want_count = strings.iter().filter(|s| *s == old).count();
    if count != want_count {
        return Err(format!("count {count} != {want_count}"));
    }
    let mut want_tree = tree.clone();
    want_tree.for_each_string_mut(&mut |s| {
        if s == old {
            *s = new.clone();
        }
    });
    let got_tree = morphic::decode_kv3_resource(&out).map_err(|e| format!("re-decode: {e}"))?;
    if format!("{got_tree:?}") != format!("{want_tree:?}") {
        return Err("decoded tree differs beyond the redirect".into());
    }

    let mut want_doc = decode_lossless(res.data_block().map_err(|e| e.to_string())?)
        .map_err(|e| format!("lossless: {e}"))?;
    subst(&mut want_doc.root, old, &new);
    let out_data = Resource::parse(&out)
        .and_then(|r| r.data_block().map(<[u8]>::to_vec))
        .map_err(|e| e.to_string())?;
    let got_doc = decode_lossless(&out_data).map_err(|e| format!("lossless out: {e}"))?;
    if format!("{:?}", got_doc.root) != format!("{:?}", want_doc.root) {
        return Err("wire tags/flags/framing differ beyond the redirect".into());
    }

    let out_res = Resource::parse(&out).map_err(|e| format!("re-parse: {e}"))?;
    let mut want_refs: Vec<(u64, String)> = refs
        .iter()
        .filter(|(_, n)| n != old)
        .cloned()
        .chain([(resource_id(&new), new.clone())])
        .collect();
    want_refs.sort_by(|a, b| a.1.cmp(&b.1));
    let got_refs =
        read_rerl(out_res.find_block(*b"RERL").ok_or("RERL lost")?).map_err(|e| e.to_string())?;
    if got_refs != want_refs {
        return Err(format!("RERL {got_refs:?} != {want_refs:?}"));
    }

    if res.blocks().len() != out_res.blocks().len() {
        return Err("block count changed".into());
    }
    for (i, b) in res.blocks().iter().enumerate() {
        if &b.kind != b"DATA" && &b.kind != b"RERL" {
            if out_res.get_block_by_index(i) != res.get_block_by_index(i) {
                return Err(format!(
                    "block {} changed",
                    String::from_utf8_lossy(&b.kind)
                ));
            }
        }
    }
    Ok(Some((out, old.clone(), new, count)))
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(Path::new(&a[1])).unwrap();
    let out_dir = Path::new(&a[2]);
    let stride: usize = a[3].parse().unwrap();
    let needles = &a[4..];
    std::fs::create_dir_all(out_dir).unwrap();

    let mut paths: Vec<String> = vpk
        .file_paths()
        .filter(|p| {
            [".vpcf_c", ".vmat_c", ".vmdl_c", ".vnmgraph_c"]
                .iter()
                .any(|e| p.ends_with(e))
        })
        .cloned()
        .collect();
    paths.sort();

    let mut ok: BTreeMap<String, usize> = BTreeMap::new();
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut failed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (n, p) in paths.iter().enumerate() {
        let wanted = needles.iter().any(|s| p.contains(s.as_str()));
        if !wanted && n % stride != 0 {
            continue;
        }
        let ext = p.rsplit('.').next().unwrap().to_string();
        let bytes = vpk.get_file(p).and_then(|mut f| f.read_all()).unwrap();
        let kv3 = Resource::parse(&bytes)
            .ok()
            .and_then(|r| r.data_block().ok())
            .map_or(String::new(), |d| {
                let ver = u32::from_le_bytes(d[0..4].try_into().unwrap()) & 0xFF;
                let blobs = i32::from_le_bytes(d[56..60].try_into().unwrap());
                format!("v{ver}{}", if blobs > 0 { "+blob" } else { "" })
            });
        match check(&bytes) {
            Ok(Some((out, old, new, count))) => {
                *ok.entry(format!("{ext} {kv3}")).or_default() += 1;
                if wanted {
                    std::fs::write(out_dir.join(p.replace('/', "__")), &out).unwrap();
                    println!(
                        "{p} [{kv3}]: {old} -> {new} ({count} field(s)), {} -> {} bytes",
                        bytes.len(),
                        out.len()
                    );
                }
            }
            Ok(None) => *skipped.entry(format!("{ext} {kv3}")).or_default() += 1,
            Err(e) => failed
                .entry(format!("{ext} {kv3}: {e}"))
                .or_default()
                .push(p.clone()),
        }
    }
    println!("-- ok");
    for (k, v) in &ok {
        println!("{v:6}  {k}");
    }
    println!("-- skipped (no RERL name that DATA also holds)");
    for (k, v) in &skipped {
        println!("{v:6}  {k}");
    }
    println!("-- failed");
    for (k, v) in &failed {
        println!("{:6}  {k}   e.g. {}", v.len(), v[0]);
    }
}
