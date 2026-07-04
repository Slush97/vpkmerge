//! End-to-end regression tests for the six empirically-verified review
//! scenarios from PR #40's review: each one runs the built `vpkmerge` binary
//! the way a caller (Grimoire, a shell user) would and asserts on the actual
//! output VPK plus the printed report. Fixtures are packed with
//! `vpkmerge_core::pack`, so the tests carry no binary blobs and run on every
//! CI OS.

use anyhow::Result;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::tempdir;

fn vpkmerge(args: &[&str]) -> Result<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_vpkmerge"))
        .args(args)
        .output()?)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("tempdir paths are valid UTF-8")
}

/// Review scenario 1: a lone typed flag must be rejected, never minting an
/// addoninfo.txt with empty title/author over existing provenance.
#[test]
fn metadata_lone_typed_flag_does_not_clobber_existing_addoninfo() -> Result<()> {
    let tmp = tempdir()?;
    let input = tmp.path().join("in_dir.vpk");
    let existing_info: &[u8] =
        b"\"AddonInfo\"\n{\n\taddontitle \"Real Title\"\n\taddonauthor \"Real Author\"\n}\n";
    vpkmerge_core::pack(
        &[
            ("addoninfo.txt", existing_info),
            ("materials/foo.txt", b"x"),
        ],
        &input,
    )?;
    let output = tmp.path().join("out_dir.vpk");

    let out = vpkmerge(&[
        "metadata",
        "--vpk",
        path_str(&input),
        "--build-date",
        "2026-07-04",
        "--output",
        path_str(&output),
    ])?;

    assert!(!out.status.success(), "lone --build-date must be refused");
    let err = stderr(&out);
    assert!(err.contains("--title"), "stderr should name --title: {err}");
    assert!(!output.exists(), "no output VPK may be written on refusal");
    Ok(())
}

/// Review scenario 2: a backslash spelling of an input-owned path is the same
/// entry, so the report must say 3 entries / 1 override, not 4 / 0.
#[test]
fn merge_extra_backslash_collision_reports_honest_counts() -> Result<()> {
    let tmp = tempdir()?;
    let a = tmp.path().join("a_dir.vpk");
    let b = tmp.path().join("b_dir.vpk");
    vpkmerge_core::pack(
        &[
            ("only_a/file.txt", b"a-only"),
            ("shared/file.txt", b"from a"),
        ],
        &a,
    )?;
    vpkmerge_core::pack(&[("only_b/file.txt", b"b-only")], &b)?;
    let extra_src = tmp.path().join("extra.txt");
    std::fs::write(&extra_src, b"from extra")?;
    let output = tmp.path().join("merged_dir.vpk");

    let out = vpkmerge(&[
        path_str(&output),
        path_str(&a),
        path_str(&b),
        "--extra-file",
        &format!("shared\\file.txt={}", path_str(&extra_src)),
    ])?;

    assert!(out.status.success(), "merge failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("3 entries, 1 paths overridden"),
        "got: {text}"
    );
    assert_eq!(
        vpkmerge_core::read_vpk_entry(&output, "shared/file.txt")?,
        b"from extra"
    );
    Ok(())
}

/// Review scenario 3: --strict must refuse an extra-file collision instead of
/// silently letting the extra overwrite the merged entry.
#[test]
fn merge_strict_refuses_extra_collision() -> Result<()> {
    let tmp = tempdir()?;
    let a = tmp.path().join("a_dir.vpk");
    let b = tmp.path().join("b_dir.vpk");
    vpkmerge_core::pack(&[("shared/file.txt", b"from a")], &a)?;
    vpkmerge_core::pack(&[("only_b/file.txt", b"b-only")], &b)?;
    let extra_src = tmp.path().join("extra.txt");
    std::fs::write(&extra_src, b"from extra")?;
    let output = tmp.path().join("merged_dir.vpk");

    let out = vpkmerge(&[
        path_str(&output),
        path_str(&a),
        path_str(&b),
        "--strict",
        "--extra-file",
        &format!("shared/file.txt={}", path_str(&extra_src)),
    ])?;

    assert!(!out.status.success(), "strict must refuse the collision");
    let err = stderr(&out);
    assert!(err.contains("unresolved path conflict"), "got: {err}");
    Ok(())
}

/// Review scenario 4: --verbose must print an override line for an extra file
/// replacing a merged entry, like any other override.
#[test]
fn merge_verbose_prints_extra_override_line() -> Result<()> {
    let tmp = tempdir()?;
    let a = tmp.path().join("a_dir.vpk");
    let b = tmp.path().join("b_dir.vpk");
    vpkmerge_core::pack(&[("shared/file.txt", b"from a")], &a)?;
    vpkmerge_core::pack(&[("only_b/file.txt", b"b-only")], &b)?;
    let extra_src = tmp.path().join("extra.txt");
    std::fs::write(&extra_src, b"from extra")?;
    let output = tmp.path().join("merged_dir.vpk");

    let out = vpkmerge(&[
        path_str(&output),
        path_str(&a),
        path_str(&b),
        "--verbose",
        "--extra-file",
        &format!("shared/file.txt={}", path_str(&extra_src)),
    ])?;

    assert!(out.status.success(), "merge failed: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("override: shared/file.txt <- --extra-file"),
        "got: {text}"
    );
    Ok(())
}

/// Review scenario 5: an extras-only metadata run must report each embedded
/// file exactly once and count entries from the VPK actually written.
#[test]
fn metadata_extras_only_reports_files_once() -> Result<()> {
    let tmp = tempdir()?;
    let input = tmp.path().join("in_dir.vpk");
    vpkmerge_core::pack(&[("materials/foo.txt", b"x")], &input)?;
    let extra_src = tmp.path().join("g.json");
    std::fs::write(&extra_src, br#"{"format":"grimoire-embedded-metadata"}"#)?;
    let output = tmp.path().join("out_dir.vpk");

    let out = vpkmerge(&[
        "metadata",
        "--vpk",
        path_str(&input),
        "--extra-file",
        &format!("g.json={}", path_str(&extra_src)),
        "--output",
        path_str(&output),
    ])?;

    assert!(out.status.success(), "metadata failed: {}", stderr(&out));
    let text = stdout(&out);
    assert_eq!(
        text.matches("g.json").count(),
        1,
        "embedded file must be reported exactly once: {text}"
    );
    assert!(text.contains("2 entries"), "got: {text}");
    assert!(text.contains("originals preserved"), "got: {text}");
    Ok(())
}

/// Review scenario 6: --drop-entry ./x must actually remove x, and the report
/// must describe the removal truthfully (no bogus 'originals preserved').
#[test]
fn metadata_drop_entry_dot_spelling_removes_and_reports_truthfully() -> Result<()> {
    let tmp = tempdir()?;
    let input = tmp.path().join("in_dir.vpk");
    vpkmerge_core::pack(
        &[
            ("grimoire_meta.json", b"legacy sidecar".as_slice()),
            ("materials/foo.txt", b"x"),
        ],
        &input,
    )?;
    let extra_src = tmp.path().join("new.json");
    std::fs::write(&extra_src, br#"{"schemaVersion":1}"#)?;
    let output = tmp.path().join("out_dir.vpk");

    let out = vpkmerge(&[
        "metadata",
        "--vpk",
        path_str(&input),
        "--drop-entry",
        "./grimoire_meta.json",
        "--extra-file",
        &format!("new.json={}", path_str(&extra_src)),
        "--output",
        path_str(&output),
    ])?;

    assert!(out.status.success(), "metadata failed: {}", stderr(&out));
    assert!(
        vpkmerge_core::read_vpk_entry(&output, "grimoire_meta.json").is_err(),
        "dropped entry must not survive in the output"
    );
    let text = stdout(&out);
    assert!(
        text.contains("dropped 1 entry: grimoire_meta.json"),
        "got: {text}"
    );
    assert!(!text.contains("originals preserved"), "got: {text}");
    assert!(text.contains("2 entries"), "got: {text}");
    Ok(())
}
