// Throwaway: per-bone motion range of an NM clip, so you can tell which bones a
// stock clip actually swings. Prints max rotation swing (deg, vs frame 0) and max
// translation drift for bones whose name contains any filter substring.
// usage: cargo run -p morphic --example nm_track_motion -- <vpk> <skel.vnmskel_c> <clip.vnmclip_c> [substr...]
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let vpk = valve_pak::open(Path::new(&a[1])).unwrap();
    let read = |e: &str| vpk.get_file(e).unwrap().read_all().unwrap();
    let nm = morphic::model::decode_nm_skeleton(&read(&a[2])).unwrap();
    let clip = morphic::model::decode_nm_clip(&read(&a[3])).unwrap();
    let filters = &a[4..];
    println!(
        "frames {} duration {:.3}s additive {}",
        clip.frame_count, clip.duration, clip.additive
    );
    for (i, t) in clip.tracks.iter().enumerate() {
        let name = nm.bone_names.get(i).map_or("?", String::as_str);
        if !filters.is_empty() && !filters.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        let rot = t.rotations.as_ref().map_or(0.0, |r| {
            r.iter()
                .map(|q| {
                    let d = (q.x * r[0].x + q.y * r[0].y + q.z * r[0].z + q.w * r[0].w)
                        .abs()
                        .min(1.0);
                    2.0 * d.acos().to_degrees()
                })
                .fold(0.0f32, f32::max)
        });
        let tr = t.translations.as_ref().map_or(0.0, |v| {
            v.iter()
                .map(|p| {
                    ((p.x - v[0].x).powi(2) + (p.y - v[0].y).powi(2) + (p.z - v[0].z).powi(2))
                        .sqrt()
                })
                .fold(0.0f32, f32::max)
        });
        println!(
            "{i:4} {name:24} rot_swing {rot:6.1} deg  trans_drift {tr:6.2}  animated_rot {}",
            t.rotations.is_some()
        );
    }
}
