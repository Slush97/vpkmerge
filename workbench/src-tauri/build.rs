fn main() {
    // tauri-build only reports the bundled sidecar as a missing resource path.
    let target = std::env::var("TARGET").unwrap();
    let exe = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let sidecar = format!("binaries/vpkmerge-mcp-{target}{exe}");
    assert!(
        std::path::Path::new(&sidecar).exists(),
        "src-tauri/{sidecar} is missing. Run `pnpm sidecar` to build it."
    );
    tauri_build::build();
}
