use shellmcp_agent::local::download;

#[tokio::test]
async fn tampered_archive_is_rejected_before_installation() {
    let temp = tempfile::tempdir().unwrap();
    let archive = temp.path().join("tampered.zip");
    std::fs::write(&archive, b"not the pinned official archive").unwrap();
    let destination = temp.path().join("release");
    let error = download::install(&destination, Some(&archive))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("SHA256 mismatch"));
    assert!(!destination.exists());
}

#[tokio::test]
#[ignore = "requires SHELLMCP_TEST_TUNNEL_ARCHIVE pointing to the pinned native archive"]
async fn pinned_archive_extracts_and_runs() {
    let archive = std::path::PathBuf::from(
        std::env::var_os("SHELLMCP_TEST_TUNNEL_ARCHIVE").expect("archive path required"),
    );
    let temp = tempfile::tempdir().unwrap();
    let executable = download::install(&temp.path().join("release"), Some(&archive))
        .await
        .unwrap();
    let output = tokio::process::Command::new(executable)
        .arg("--version")
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains(&download::VERSION[1..]));
}
