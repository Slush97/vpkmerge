// Print the KV3 paths under the first block of a given FOURCC whose last key
// matches a filter, with their decoded values (truncated).
// usage: block_paths <file.vmdl_c> <FOURCC> <key-substring>
use morphic::kv3::Value;
use morphic::resource::Resource;

fn walk(v: &Value, path: &mut Vec<String>, filter: &str) {
    match v {
        Value::Object(f) => {
            for (k, c) in f {
                path.push(k.clone());
                if k.contains(filter) {
                    let s = format!("{c:?}").replace(['\n', ' '], "");
                    println!("{} = {}", path.join("/"), &s[..s.len().min(300)]);
                } else {
                    walk(c, path, filter);
                }
                path.pop();
            }
        }
        Value::Array(a) => {
            for (i, c) in a.iter().enumerate() {
                path.push(format!("#{i}"));
                walk(c, path, filter);
                path.pop();
            }
        }
        _ => {}
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).unwrap();
    let res = Resource::parse(&bytes).unwrap();
    let (i, _) = res
        .blocks()
        .iter()
        .enumerate()
        .find(|(_, b)| b.kind == a[2].as_bytes())
        .expect("no such block");
    let tree = morphic::kv3::decode(res.get_block_by_index(i).unwrap()).unwrap();
    walk(&tree, &mut Vec::new(), &a[3]);
}
