//! Regression: a v2 VPK whose header declares a 0-byte MD5 (self-hashes)
//! section and a 0-byte signature ends right after its embedded data. Such
//! paks are valid (the engine loads them; several `GameBanana` mods ship this
//! way) but `valve_pak` 0.1.0 unconditionally read 48 checksum bytes and failed
//! with "failed to fill whole buffer". The vendored patch reads the section
//! only when the header declares it.

use std::path::Path;

use vpkmerge_core::{
    detect_conflicts, inspect, merge, pack, read_vpk_entry, split, CollisionPolicy, MergeOptions,
    PathPredicate, SplitOptions, SplitOutput,
};

const EMBEDDED: u16 = 0x7fff;

/// Hand-assemble a single-file (`_dir.vpk`, all data embedded) v2 VPK with
/// `self_hashes_length = 0`, `signature_length = 0` and nothing after the data.
fn write_checksumless_vpk(path: &Path, files: &[(&str, &str, &str, &[u8])]) {
    let mut tree = Vec::new();
    let mut data = Vec::new();
    let mut exts: Vec<&str> = files.iter().map(|f| f.0).collect();
    exts.dedup();
    for ext in exts {
        tree.extend_from_slice(ext.as_bytes());
        tree.push(0);
        let mut dirs: Vec<&str> = files.iter().filter(|f| f.0 == ext).map(|f| f.1).collect();
        dirs.dedup();
        for dir in dirs {
            tree.extend_from_slice(dir.as_bytes());
            tree.push(0);
            for (_, _, name, bytes) in files.iter().filter(|f| f.0 == ext && f.1 == dir) {
                tree.extend_from_slice(name.as_bytes());
                tree.push(0);
                tree.extend_from_slice(&0u32.to_le_bytes());
                tree.extend_from_slice(&0u16.to_le_bytes());
                tree.extend_from_slice(&EMBEDDED.to_le_bytes());
                tree.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
                tree.extend_from_slice(&u32::try_from(bytes.len()).unwrap().to_le_bytes());
                tree.extend_from_slice(&0xffffu16.to_le_bytes());
                data.extend_from_slice(bytes);
            }
            tree.push(0);
        }
        tree.push(0);
    }
    tree.push(0);

    let mut out = Vec::new();
    out.extend_from_slice(&0x55aa_1234u32.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&u32::try_from(tree.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // archive MD5 section
    out.extend_from_slice(&0u32.to_le_bytes()); // self-hashes (MD5) section
    out.extend_from_slice(&0u32.to_le_bytes()); // signature section
    out.extend_from_slice(&tree);
    out.extend_from_slice(&data);
    std::fs::write(path, out).unwrap();
}

const HELLO: &[u8] = b"hello from a checksumless pak";
const STYLE: &[u8] = b"body { color: red; }";

fn fixture(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("nochecksum_dir.vpk");
    write_checksumless_vpk(
        &path,
        &[
            ("txt", "a/b", "hello", HELLO),
            ("txt", " ", "root", b"root"),
            ("vcss_c", "panorama/styles", "hud", STYLE),
        ],
    );
    path
}

#[test]
fn opens_and_lists() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    let info = inspect(&path).unwrap();
    let mut paths = info.file_paths;
    paths.sort();
    assert_eq!(
        paths,
        ["a/b/hello.txt", "panorama/styles/hud.vcss_c", "root.txt"]
    );
}

#[test]
fn extracts_entries() {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture(dir.path());
    assert_eq!(read_vpk_entry(&path, "a/b/hello.txt").unwrap(), HELLO);
    assert_eq!(
        read_vpk_entry(&path, "panorama/styles/hud.vcss_c").unwrap(),
        STYLE
    );
    assert_eq!(read_vpk_entry(&path, "root.txt").unwrap(), b"root");
}

#[test]
fn merges_with_a_checksummed_pak() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let normal = dir.path().join("normal_dir.vpk");
    pack(
        &[("a/b/hello.txt", b"overridden"), ("other.txt", b"other")],
        &normal,
    )
    .unwrap();

    let conflicts = detect_conflicts(&[&bare, &normal]).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, "a/b/hello.txt");

    let out = dir.path().join("merged_dir.vpk");
    let report = merge(&[&normal, &bare], &out, &MergeOptions::default()).unwrap();
    assert_eq!(report.total_entries, 4);
    assert_eq!(report.overridden_paths, 1);
    assert_eq!(read_vpk_entry(&out, "a/b/hello.txt").unwrap(), HELLO);
    assert_eq!(read_vpk_entry(&out, "other.txt").unwrap(), b"other");
    assert_eq!(
        read_vpk_entry(&out, "panorama/styles/hud.vcss_c").unwrap(),
        STYLE
    );

    let strict = MergeOptions {
        collision_policy: CollisionPolicy::Error,
        ..MergeOptions::default()
    };
    assert!(merge(&[&normal, &bare], dir.path().join("s_dir.vpk"), &strict).is_err());
}

#[test]
fn merging_bare_and_padded_variants_yields_identical_content() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let padded = dir.path().join("padded_dir.vpk");
    let mut bytes = std::fs::read(&bare).unwrap();
    bytes.extend_from_slice(&[0u8; 48]);
    std::fs::write(&padded, bytes).unwrap();

    let other = dir.path().join("other_dir.vpk");
    pack(&[("other.txt", b"other")], &other).unwrap();

    let out_bare = dir.path().join("mb_dir.vpk");
    let out_padded = dir.path().join("mp_dir.vpk");
    merge(&[&other, &bare], &out_bare, &MergeOptions::default()).unwrap();
    merge(&[&other, &padded], &out_padded, &MergeOptions::default()).unwrap();

    let mut a = inspect(&out_bare).unwrap().file_paths;
    let mut b = inspect(&out_padded).unwrap().file_paths;
    a.sort();
    b.sort();
    assert_eq!(a, b);
    for entry in &a {
        assert_eq!(
            read_vpk_entry(&out_bare, entry).unwrap(),
            read_vpk_entry(&out_padded, entry).unwrap(),
            "{entry}"
        );
    }
}

#[test]
fn splits() {
    let dir = tempfile::tempdir().unwrap();
    let bare = fixture(dir.path());
    let out = dir.path().join("styles_dir.vpk");
    split(
        &bare,
        &[SplitOutput {
            path: out.clone(),
            predicate: PathPredicate::AnyPrefix(vec!["panorama/".into()]),
        }],
        &SplitOptions::default(),
    )
    .unwrap();
    assert_eq!(
        inspect(&out).unwrap().file_paths,
        ["panorama/styles/hud.vcss_c"]
    );
}
