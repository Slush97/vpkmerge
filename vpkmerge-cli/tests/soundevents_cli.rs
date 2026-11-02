//! JSON compatibility and opt-in KV3 format metadata through the built CLI.

use std::path::Path;
use std::process::{Command, Output};
use vpkmerge_core::SoundEvents;

fn fixture() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../morphic/fixtures/kv3/gigawatt.vsndevts_c"
    ))
}

fn run(extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vpkmerge"))
        .arg("soundevents")
        .arg(fixture())
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn default_stdout_is_byte_identical_to_the_original_json_projection() {
    let se = SoundEvents::from_file(fixture()).unwrap();
    let expected = format!("{}\n", serde_json::to_string_pretty(&se.to_json()).unwrap());
    let output = run(&[]);
    assert!(output.status.success());
    assert_eq!(output.stdout, expected.as_bytes());
}

#[test]
fn with_format_wraps_the_stored_format_and_unchanged_data() {
    let se = SoundEvents::from_file(fixture()).unwrap();
    let original = run(&[]);
    let wrapped = run(&["--with-format"]);
    assert!(wrapped.status.success());
    assert_eq!(wrapped.stderr, original.stderr);
    let json: serde_json::Value = serde_json::from_slice(&wrapped.stdout).unwrap();
    let data: serde_json::Value = serde_json::from_slice(&original.stdout).unwrap();
    let format = se.format().unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "format": {"name": format.name(), "guid": format.guid()},
            "data": data,
        })
    );
}

#[test]
fn with_format_is_only_available_in_json_mode() {
    for flag in ["--encode", "--encode-vpk"] {
        let output = run(&["--with-format", flag, "unused"]);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
    }
}
