// Dump a model's skeleton (name, parent index, parent-relative position + rotation
// quaternion) as JSON, straight from morphic's decode of the compiled `.vmdl_c`.
// These are Source model-space, parent-relative transforms -- exactly what a
// ModelDoc `Skeleton` node's `origin` / `angles` need (angles = quat -> QAngle).
//
// usage: cargo run --release -p vpkmerge-core --example dump_skeleton -- <vpk> <entry>

use vpkmerge_core::read_vpk_entry;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: dump_skeleton <vpk> <entry>");
        std::process::exit(2);
    }
    let bytes = read_vpk_entry(&args[1], &args[2])?;
    let skel = morphic::model::decode_skeleton(&bytes)?;
    let mut out = String::from("[");
    for (i, b) in skel.bones.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let parent = b.parent.map_or(-1i64, |p| p as i64);
        out.push_str(&format!(
            "{{\"name\":\"{}\",\"parent\":{},\"pos\":[{},{},{}],\"quat\":[{},{},{},{}]}}",
            b.name,
            parent,
            b.position.x,
            b.position.y,
            b.position.z,
            b.rotation.x,
            b.rotation.y,
            b.rotation.z,
            b.rotation.w,
        ));
    }
    out.push(']');
    println!("{out}");
    Ok(())
}
