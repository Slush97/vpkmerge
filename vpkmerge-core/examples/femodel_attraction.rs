// Stiffen a model's cloth chain by rewriting per-node animation attraction in
// PHYS.m_pFeModel.m_NodeIntegrator, byte-faithfully (float lanes only), and pack
// the patched .vmdl_c into an addon VPK.
//
// Each rule is `SUBSTR=FORCE,VERTEX`: every node whose name contains SUBSTR gets
// those two attraction values. Later rules win.
//
// usage: cargo run --release -p vpkmerge-core --example femodel_attraction -- \
//          <mod_dir.vpk> <entry.vmdl_c> <out_dir.vpk> <SUBSTR=F,V>...
use morphic::kv3::{Seg, Value};
use morphic::resource::Resource;

fn fe_path(root: &Value) -> Option<(Vec<Seg>, &Value)> {
    for key in ["m_pFeModel", "m_feModel"] {
        if let Some(fe) = root.get(key) {
            return Some((vec![Seg::Key(key.into())], fe));
        }
    }
    let parts = root.get("m_parts")?.as_array()?;
    for (i, p) in parts.iter().enumerate() {
        for key in ["m_pFeModel", "m_feModel"] {
            if let Some(fe) = p.get(key) {
                return Some((
                    vec![Seg::Key("m_parts".into()), Seg::Index(i), Seg::Key(key.into())],
                    fe,
                ));
            }
        }
    }
    None
}

fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (vpk, entry, out) = (&a[0], &a[1], &a[2]);
    let rules: Vec<(String, f32, f32)> = a[3..]
        .iter()
        .map(|r| {
            let (k, v) = r.split_once('=').expect("SUBSTR=F,V");
            let (f, vv) = v.split_once(',').expect("F,V");
            (k.to_string(), f.parse().unwrap(), vv.parse().unwrap())
        })
        .collect();

    let bytes = vpkmerge_core::read_vpk_entry(vpk, entry)?;
    let res = Resource::parse(&bytes)?;
    let idx = res
        .blocks()
        .iter()
        .position(|b| &b.kind == b"PHYS")
        .expect("no PHYS block");
    let b = &res.blocks()[idx];
    let phys = &bytes[b.offset as usize..(b.offset + b.size) as usize];
    let tree = morphic::kv3::decode(phys)?;
    let (base, fe) = fe_path(&tree).expect("no FeModel");
    let names = fe.get("m_CtrlName").and_then(Value::as_array).expect("m_CtrlName");

    let mut edits = Vec::new();
    for (i, n) in names.iter().enumerate() {
        let Value::String(name) = n else { continue };
        let Some((_, f, v)) = rules.iter().rev().find(|(k, _, _)| name.contains(k.as_str())) else {
            continue;
        };
        for (field, val) in [("flAnimationForceAttraction", *f), ("flAnimationVertexAttraction", *v)] {
            let mut p = base.clone();
            p.extend([Seg::Key("m_NodeIntegrator".into()), Seg::Index(i), Seg::Key(field.into())]);
            edits.push((p, f64::from(val)));
        }
        eprintln!("{name}: force {f} vertex {v}");
    }
    // FeModel scalars are stored as doubles; set_doubles also keeps PHYS's blob framing.
    let new_phys = morphic::kv3::set_doubles(phys, &edits)?;
    let new_bytes = res.rebuild_with_block(idx, &new_phys)?;
    vpkmerge_core::pack(&[(entry.as_str(), new_bytes.as_slice())], out)?;
    eprintln!("patched {} values -> {out}", edits.len());
    Ok(())
}
