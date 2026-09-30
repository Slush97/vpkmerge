//! Set `m_modelSkeleton.m_nFlag` for named bones in a `.vmdl_c`, in place
//! (every other byte preserved via `patch_kv3_resource_scalars`).
//!
//! Why: FeModel STATIC (kinematic anchor) nodes read their transform from the
//! animated skeleton. A custom anchor bone carrying the cloth flag (0x400000)
//! is excluded from FK compose and left for the cloth system, which never
//! writes static nodes: the anchor gets no live transform and the whole cloth
//! chain hangs in stale bind space (Baroness Mina v6 ears/purse-charm).
//! Valve's own hierarchy-driven anchors are UNFLAGGED (astro `scarf_0/1` =
//! 0x3cc8) while only simulated nodes carry the flag. This tool applies that
//! authoring: clear (or set) exact flag values per bone name.
//!
//! Usage: set_bone_flags <in.vmdl_c> <out.vmdl_c> <bone=flag> [<bone=flag>...]
//!   flag accepts decimal or 0x hex.
use morphic::kv3::{Seg, Value};

fn skeleton_flags(bytes: &[u8]) -> (Vec<String>, Vec<i64>) {
    let tree = morphic::decode_kv3_resource(bytes).expect("decode DATA");
    let skel = tree.get("m_modelSkeleton").expect("m_modelSkeleton");
    let names = skel
        .get("m_boneName")
        .and_then(Value::as_array)
        .expect("m_boneName")
        .iter()
        .map(|v| v.as_str().expect("bone name str").to_owned())
        .collect::<Vec<_>>();
    let flags = skel
        .get("m_nFlag")
        .and_then(Value::as_array)
        .expect("m_nFlag")
        .iter()
        .map(|v| {
            v.as_int()
                .or_else(|| v.as_uint().and_then(|u| i64::try_from(u).ok()))
                .expect("flag int")
        })
        .collect::<Vec<_>>();
    assert_eq!(names.len(), flags.len(), "skeleton arrays length mismatch");
    (names, flags)
}

fn parse_flag(s: &str) -> i64 {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).expect("hex flag")
    } else {
        s.parse().expect("decimal flag")
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!("usage: set_bone_flags <in.vmdl_c> <out.vmdl_c> <bone=flag> [<bone=flag>...]");
        std::process::exit(2);
    }
    let in_bytes = std::fs::read(&a[1]).expect("read in");
    let wanted: Vec<(String, i64)> = a[3..]
        .iter()
        .map(|kv| {
            let (name, flag) = kv.split_once('=').expect("bone=flag");
            (name.to_owned(), parse_flag(flag))
        })
        .collect();

    let (names, flags) = skeleton_flags(&in_bytes);
    let mut edits: Vec<(Vec<Seg>, i64)> = Vec::new();
    for (bone, want) in &wanted {
        let i = names
            .iter()
            .position(|n| n == bone)
            .unwrap_or_else(|| panic!("bone {bone} not in skeleton"));
        println!("{bone}: 0x{:x} -> 0x{want:x}", flags[i]);
        if flags[i] != *want {
            edits.push((
                vec![
                    Seg::Key("m_modelSkeleton".into()),
                    Seg::Key("m_nFlag".into()),
                    Seg::Index(i),
                ],
                *want,
            ));
        }
    }
    let patched = morphic::patch_kv3_resource_scalars(&in_bytes, &edits).expect("patch m_nFlag");

    // Self-check: re-decode, confirm the targeted bones took and nothing else moved.
    let (chk_names, chk_flags) = skeleton_flags(&patched);
    assert_eq!(chk_names, names, "bone names changed");
    for (i, name) in names.iter().enumerate() {
        let want = wanted
            .iter()
            .find(|(b, _)| b == name)
            .map_or(flags[i], |(_, f)| *f);
        assert_eq!(
            chk_flags[i], want,
            "bone {name}: got 0x{:x} want 0x{want:x}",
            chk_flags[i]
        );
    }

    std::fs::write(&a[2], &patched).expect("write out");
    println!(
        "wrote {} ({} bytes, was {}); {} edit(s)",
        a[2],
        patched.len(),
        in_bytes.len(),
        edits.len()
    );
}
