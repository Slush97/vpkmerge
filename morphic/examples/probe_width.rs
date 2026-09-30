//! Probe whether m_modelSkeleton's float-ish per-bone fields are stored as DOUBLE
//! (b8) or FLOAT (b4) on disk, by attempting an identity patch with each primitive.
//! Usage: probe_width <file.vmdl_c>
use morphic::kv3::Seg;

fn p(parts: &[&str], idx: &[usize]) -> Vec<Seg> {
    let mut v: Vec<Seg> = parts.iter().map(|s| Seg::Key((*s).to_string())).collect();
    for &i in idx {
        v.push(Seg::Index(i));
    }
    v
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).expect("read");

    // pos component [bone 0][x]
    let pos = p(&["m_modelSkeleton", "m_bonePosParent"], &[0, 0]);
    let scale = p(&["m_modelSkeleton", "m_boneScaleParent"], &[0]);
    let sphere = p(&["m_modelSkeleton", "m_boneSphere"], &[0]);
    let rot = p(&["m_modelSkeleton", "m_boneRotParent"], &[0, 0]);

    for (name, path) in [
        ("m_bonePosParent[0][0]", &pos),
        ("m_boneRotParent[0][0]", &rot),
        ("m_boneScaleParent[0]", &scale),
        ("m_boneSphere[0]", &sphere),
    ] {
        let d = morphic::patch_kv3_resource_doubles(&bytes, &[(path.clone(), 1.0)]);
        let f = morphic::patch_kv3_resource_floats(&bytes, &[(path.clone(), 1.0)]);
        println!(
            "{name}: doubles={} floats={}",
            match &d {
                Ok(_) => "OK(DOUBLE/b8)".to_string(),
                Err(e) => format!("ERR({e:?})"),
            },
            match &f {
                Ok(_) => "OK(FLOAT/b4)".to_string(),
                Err(e) => format!("ERR({e:?})"),
            },
        );
    }
}
