//! Regression: some packers write a root-level file's directory as an empty
//! string instead of the usual `" "`. Seen on a community hero skin that packed
//! `tools_thumbnail_cache.sqlite3` next to its model. `valve_pak` 0.1.0 read the
//! empty string as the end of that extension's directory list, took the file
//! name for the next extension and its metadata bytes for a directory name, and
//! failed with "Invalid UTF-8 in cstring", so no command could open the mod.

use std::path::Path;

use vpkmerge_core::{inspect, read_vpk_entry};

const EMBEDDED: u16 = 0x7fff;

/// Hand-assemble a single-file v2 VPK. Each file is `(ext, dir, name, bytes)`
/// and `dir` is written verbatim, so `""` reproduces the odd root spelling.
fn write_vpk(path: &Path, files: &[(&str, &str, &str, &[u8])]) {
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

const MODEL: &[u8] = b"model bytes";
const CACHE: &[u8] = b"thumbnail cache";

#[test]
fn an_empty_first_directory_is_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skin_dir.vpk");
    // The layout of the skin that hit this: the root file comes last.
    write_vpk(
        &path,
        &[
            ("vmdl_c", "models/heroes_wip/ivy", "ivy", MODEL),
            ("sqlite3", "", "tools_thumbnail_cache", CACHE),
        ],
    );

    let mut paths = inspect(&path).unwrap().file_paths;
    paths.sort();
    assert_eq!(
        paths,
        [
            "models/heroes_wip/ivy/ivy.vmdl_c",
            "tools_thumbnail_cache.sqlite3"
        ]
    );
    assert_eq!(
        read_vpk_entry(&path, "tools_thumbnail_cache.sqlite3").unwrap(),
        CACHE
    );
    assert_eq!(
        read_vpk_entry(&path, "models/heroes_wip/ivy/ivy.vmdl_c").unwrap(),
        MODEL
    );
}

#[test]
fn entries_after_an_empty_root_directory_still_parse() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("skin_dir.vpk");
    write_vpk(
        &path,
        &[
            ("txt", "", "readme", b"root"),
            ("txt", "notes", "todo", b"nested"),
            ("vmat_c", "materials", "skin", b"material"),
        ],
    );

    let mut paths = inspect(&path).unwrap().file_paths;
    paths.sort();
    assert_eq!(
        paths,
        ["materials/skin.vmat_c", "notes/todo.txt", "readme.txt"]
    );
    assert_eq!(read_vpk_entry(&path, "readme.txt").unwrap(), b"root");
    assert_eq!(read_vpk_entry(&path, "notes/todo.txt").unwrap(), b"nested");
}
