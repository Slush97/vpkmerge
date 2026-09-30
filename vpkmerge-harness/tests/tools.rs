use serde_json::json;
use vpkmerge_harness::{execute, init_project, Project, ToolCall};

#[test]
fn project_creation_preserves_existing_work() {
    let dir = tempfile::tempdir().unwrap();
    let path = init_project(dir.path(), "My mod".into()).unwrap();
    let original = std::fs::read(&path).unwrap();
    assert!(init_project(dir.path(), "Replacement".into()).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let mut project = Project::load(&path).unwrap();
    project.schema_version = 99;
    assert!(execute(&project, ToolCall::GetCapabilities {}).is_err());
}

#[test]
fn inventory_pages_and_conflicts_use_real_archives() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = init_project(dir.path(), "Fixture".into()).unwrap();
    let project = Project::load(&manifest).unwrap();
    let a = dir.path().join("a_dir.vpk");
    let b = dir.path().join("b_dir.vpk");
    vpkmerge_core::pack(&[("z.txt", b"z"), ("shared.txt", b"old")], &a).unwrap();
    vpkmerge_core::pack(&[("shared.txt", b"new")], &b).unwrap();
    let result = execute(
        &project,
        ToolCall::InspectMod {
            path: a.clone(),
            offset: 0,
            limit: 1,
        },
    )
    .unwrap();
    assert_eq!(result["entries"], json!(["shared.txt"]));
    assert_eq!(result["next_offset"], 1);
    let result = execute(
        &project,
        ToolCall::InspectMod {
            path: a.clone(),
            offset: 1,
            limit: 1,
        },
    )
    .unwrap();
    assert_eq!(result["entries"], json!(["z.txt"]));
    assert!(result["next_offset"].is_null());
    assert!(execute(
        &project,
        ToolCall::InspectMod {
            path: a.clone(),
            offset: 0,
            limit: 0
        }
    )
    .is_err());
    let result = execute(
        &project,
        ToolCall::DetectConflicts {
            inputs: vec![a, b],
            offset: 0,
            limit: 10,
        },
    )
    .unwrap();
    assert_eq!(
        result["conflicts"],
        json!([{"path": "shared.txt", "owner_indices": [0, 1]}])
    );
}

#[test]
fn cli_rejects_unknown_tools_with_structured_error() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let dir = tempfile::tempdir().unwrap();
    let manifest = init_project(dir.path(), "Fixture".into()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_vpkmerge-harness"))
        .arg("call")
        .arg(manifest)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"tool":"install_mod","arguments":{}}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], false);
    assert!(body["error"].as_str().unwrap().contains("unknown variant"));
}
