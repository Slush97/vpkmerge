use anyhow::Result;
use std::process::Command;

fn texture() -> Vec<u8> {
    let mut bytes = vec![0; 68];
    for (offset, value) in [(0, 68u32), (8, 8), (12, 1), (20, 8), (24, 40)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[4] = 12;
    bytes[6] = 1;
    bytes[16..20].copy_from_slice(b"DATA");
    bytes[28] = 1;
    bytes[48] = 8;
    bytes[50] = 4;
    bytes[52] = 1;
    bytes[54] = 4;
    bytes[55] = 3;
    for (level, pixels) in [(2, 2), (1, 8), (0, 32)] {
        bytes.extend_from_slice(&[level, 37, 123, 13].repeat(pixels));
    }
    bytes
}

#[test]
fn mips_flag_preserves_default_files_and_cleans_up_exports() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let pak = tmp.path().join("test_dir.vpk");
    let out_dir = tmp.path().join("dump");
    let texture = texture();
    vpkmerge_core::pack(&[("materials/test.vtex_c", texture.as_slice())], &pak)?;
    let dump = |mips: bool| -> Result<()> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vpkmerge"));
        command
            .args(["panorama", "dump", "--vpk"])
            .arg(&pak)
            .arg("--out-dir")
            .arg(&out_dir);
        if mips {
            command.arg("--mips");
        }
        let output = command.output()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    dump(false)?;
    let png_path = out_dir.join("materials/test.png");
    let raw_path = out_dir.join("_raw/materials/test.vtex_c");
    let manifest_path = out_dir.join("_manifest.json");
    let png = std::fs::read(&png_path)?;
    let manifest = std::fs::read(&manifest_path)?;
    assert_eq!(std::fs::read(&raw_path)?, texture);
    assert!(!out_dir.join("materials/test.mips.json").exists());
    dump(true)?;
    assert_eq!(std::fs::read(&png_path)?, png);
    assert_eq!(std::fs::read(&raw_path)?, texture);
    let sidecar: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out_dir.join("materials/test.mips.json"))?)?;
    assert_eq!(sidecar["level_count"], 3);
    assert_eq!(sidecar["format"], "RGBA8888");
    assert_eq!(sidecar["one_level"], false);
    assert_eq!(sidecar["levels"][2]["width"], 2);
    assert_eq!(sidecar["levels"][2]["height"], 1);
    for mip in 0..3 {
        assert!(out_dir
            .join(format!("materials/test.mip{mip}.png"))
            .exists());
    }
    assert!(!out_dir.join("materials/test.mip3.png").exists());
    dump(false)?;
    assert_eq!(std::fs::read(&manifest_path)?, manifest);
    assert_eq!(std::fs::read(&png_path)?, png);
    assert!(!out_dir.join("materials/test.mips.json").exists());
    assert!(!out_dir.join("materials/test.mip0.png").exists());
    Ok(())
}

#[test]
fn mips_flag_marks_no_lod_as_one_level() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let pak = tmp.path().join("test_dir.vpk");
    let out_dir = tmp.path().join("dump");
    let mut texture = texture();
    texture[30] = 8;
    texture[55] = 1;
    // Keep only the 8x4 mip, which is stored last in the synthetic resource.
    texture.drain(68..108);
    vpkmerge_core::pack(&[("materials/test.vtex_c", texture.as_slice())], &pak)?;
    let output = Command::new(env!("CARGO_BIN_EXE_vpkmerge"))
        .args(["panorama", "dump", "--mips", "--no-raw", "--vpk"])
        .arg(&pak)
        .arg("--out-dir")
        .arg(&out_dir)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let sidecar: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out_dir.join("materials/test.mips.json"))?)?;
    assert_eq!(sidecar["level_count"], 1);
    assert_eq!(sidecar["one_level"], true);
    assert_eq!(sidecar["no_lod"], true);
    assert!(out_dir.join("materials/test.mip0.png").exists());
    assert!(!out_dir.join("materials/test.mip1.png").exists());
    assert!(!out_dir.join("_raw").exists());
    Ok(())
}
