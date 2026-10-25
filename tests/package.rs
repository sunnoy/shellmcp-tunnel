use sha2::{Digest, Sha256};
use std::{path::Path, process::Command};

fn package(out: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_package-local"));
    command.args([
        "--base-url",
        "https://example.invalid/releases/download/v0.3.0",
    ]);
    command.arg("--out").arg(out);
    command
}

#[test]
fn release_urls_resolve_to_flat_assets_with_verified_checksums() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("native");
    std::fs::write(&source, b"native artifact fixture").unwrap();
    let out = temp.path().join("release");
    let mut command = package(&out);
    for platform in [
        "linux-amd64",
        "linux-arm64",
        "darwin-amd64",
        "darwin-arm64",
        "windows-amd64",
        "windows-arm64",
    ] {
        let argument = format!("{platform}={}", source.display());
        command.arg("--artifact").arg(&argument);
        if platform.starts_with("windows") {
            command.arg("--host-artifact").arg(&argument);
        }
    }
    assert!(command.output().unwrap().status.success());
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(out.join("artifacts.json")).unwrap()).unwrap();
    let mut count = 0;
    for (platform, components) in manifest.as_object().unwrap() {
        assert!(components.get("agent").is_some());
        assert_eq!(
            components.get("host").is_some(),
            platform.starts_with("windows")
        );
        for component in components.as_object().unwrap().values() {
            let url = component["url"].as_str().unwrap();
            let filename = url
                .strip_prefix("https://example.invalid/releases/download/v0.3.0/")
                .unwrap();
            assert!(
                !filename.contains('/'),
                "GitHub release assets must be flat"
            );
            let bytes = std::fs::read(out.join(filename)).unwrap();
            assert_eq!(component["sha256"], hex::encode(Sha256::digest(&bytes)));
            let script = if platform.starts_with("windows") {
                "install-local.ps1"
            } else {
                "install-local.sh"
            };
            let script = std::fs::read_to_string(out.join(script)).unwrap();
            assert!(script.contains(url));
            assert!(script.contains(component["sha256"].as_str().unwrap()));
            assert!(!script.contains("@ARTIFACTS@"));
            count += 1;
        }
    }
    assert_eq!(count, 8);
}

#[test]
fn missing_windows_host_is_rejected_before_writing_a_release() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("agent.exe");
    std::fs::write(&source, b"agent without host").unwrap();
    let out = temp.path().join("release");
    let result = package(&out)
        .arg("--artifact")
        .arg(format!("windows-amd64={}", source.display()))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Windows host missing"));
    assert!(!out.exists());
}
