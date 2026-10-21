//! Local (non-CI) check of the `RERL` codec across a whole Deadlock pak. Gated on
//! `MORPHIC_MODEL_VPK` pointing at a `pak01_dir.vpk`; skipped otherwise. Over
//! every compiled resource that carries a `RERL` block it asserts what
//! `morphic::resource::rerl` documents: each id is `resource_id` of its name,
//! entries are sorted by name, and `write_rerl(read_rerl(block))` reproduces the
//! block byte for byte (modulo the entry table starting at 8 or 12).
//!
//! Run with: `MORPHIC_MODEL_VPK=/path/to/pak01_dir.vpk cargo test --release -p
//! morphic --test rerl_local -- --nocapture`.

use morphic::resource::{read_rerl, resource_id, write_rerl, Resource};

#[test]
fn rerl_codec_matches_every_shipped_block() {
    let Ok(vpk_path) = std::env::var("MORPHIC_MODEL_VPK") else {
        eprintln!("MORPHIC_MODEL_VPK not set; skipping local RERL codec check");
        return;
    };
    let vpk = valve_pak::open(&vpk_path).expect("open vpk");
    let mut paths: Vec<String> = vpk
        .file_paths()
        .filter(|p| p.ends_with("_c"))
        .cloned()
        .collect();
    paths.sort();

    let mut checked = 0usize;
    for p in &paths {
        let Ok(bytes) = vpk.get_file(p).and_then(|mut f| f.read_all()) else {
            continue;
        };
        let Some(stored) = Resource::parse(&bytes)
            .ok()
            .and_then(|r| r.find_block(*b"RERL"))
        else {
            continue;
        };
        let refs = read_rerl(stored).unwrap_or_else(|e| panic!("{p}: {e}"));
        for (id, name) in &refs {
            assert_eq!(*id, resource_id(name), "{p}: id of {name}");
        }
        assert!(refs.windows(2).all(|w| w[0].1 < w[1].1), "{p}: order");

        let mut canonical = stored.to_vec();
        if canonical[0] == 12 {
            canonical.drain(8..12);
            canonical[0] = 8;
        }
        assert_eq!(write_rerl(&refs), canonical, "{p}: round trip");
        checked += 1;
    }
    assert!(
        checked > 10_000,
        "expected many RERL blocks, found {checked}"
    );
    eprintln!("RERL codec exact on {checked} blocks");
}
