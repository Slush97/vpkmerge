// Throwaway: bake an edited loose NM clip onto a modded model as an animated GLB,
// and report how far the prop actually travels across the clip.
// usage: cargo run -p morphic --example scratch_rem_idle_preview --
//          <mod vpk> <base vpk> <model entry> <clip entry> <nmskel entry> <out.glb>
use std::path::Path;

fn read(vpk: &valve_pak::VPK, entry: &str) -> Option<Vec<u8>> {
    vpk.get_file(entry).ok().and_then(|mut f| f.read_all().ok())
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let modded = valve_pak::open(Path::new(&a[1])).unwrap();
    let base = valve_pak::open(Path::new(&a[2])).unwrap();
    let get = |entry: &str| read(&modded, entry).or_else(|| read(&base, entry)).unwrap();

    let model_bytes = get(&a[3]);
    // Optional 7th arg "stock": force the clip to come from the base pak, so the
    // same modded model can be previewed against Valve's unedited motion.
    let stock = a.get(7).map(String::as_str) == Some("stock");
    let clip_bytes = if stock {
        read(&base, &a[4]).unwrap()
    } else {
        get(&a[4])
    };
    let skel_bytes = get(&a[5]);

    let mut model = morphic::model::decode(&model_bytes).unwrap();
    let clip = morphic::model::decode_nm_clip(&clip_bytes).unwrap();
    let nm = morphic::model::decode_nm_skeleton(&skel_bytes).unwrap();

    // How much does each spine bone actually rotate over the clip?
    for name in [
        "flip_a_page_0",
        "flip_a_page_1",
        "flip_a_page_2",
        "flip_a_page_3",
    ] {
        let Some(i) = nm.bone_names.iter().position(|b| b == name) else {
            continue;
        };
        let t = &clip.tracks[i];
        match &t.rotations {
            None => println!("{name:<16} STATIC"),
            Some(r) => {
                let q0 = r[0];
                let mut max = 0f32;
                for q in r {
                    let dot = (q0.x * q.x + q0.y * q.y + q0.z * q.z + q0.w * q.w)
                        .abs()
                        .min(1.0);
                    max = max.max(2.0 * dot.acos().to_degrees());
                }
                println!("{name:<16} animated, {} frames, max {max:.2} deg", r.len());
            }
        }
    }

    let baked = morphic::model::nm_clip_to_clip(&clip, &nm, &model.skeleton, "idle");
    model.animations = vec![baked];
    let glb = morphic::model::to_glb(&model).unwrap();
    std::fs::write(&a[6], glb).unwrap();
    println!("wrote {}", a[6]);
}
