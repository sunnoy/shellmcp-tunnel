use anyhow::{Result, ensure};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
pub const VERSION: &str = "v0.0.16";
pub fn asset() -> Result<(String, &'static str)> {
    let (os, arch, hash) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => (
            "linux",
            "amd64",
            "d60cdba019bce451bcc3a15478cc5b9cb11270b049f5b56ea39a80b517f8b117",
        ),
        ("linux", "aarch64") => (
            "linux",
            "arm64",
            "963d0384aaa7c798778479c45673f9051a4039dd891aef8f8978d2a1a628b74f",
        ),
        ("macos", "x86_64") => (
            "darwin",
            "amd64",
            "57b3dd73f2d042c7aeb5664681f7538078359f55538b9e048640f1df71a0c83f",
        ),
        ("macos", "aarch64") => (
            "darwin",
            "arm64",
            "a160820d45089b5253d8d671fe2a689bde0f73d78c135d1497a7b33f28a3ac62",
        ),
        ("windows", "x86_64") => (
            "windows",
            "amd64",
            "edef7241b0c647fcb30f1a80ff376b6b25c51927960f257a3f01e21b17c2aba6",
        ),
        ("windows", "aarch64") => (
            "windows",
            "arm64",
            "ecd748288b9bd9cc8f5a963855eb143f4788b831475156703d0bafdcd2dcb149",
        ),
        _ => anyhow::bail!("unsupported tunnel platform"),
    };
    Ok((format!("tunnel-client-{VERSION}-{os}-{arch}.zip"), hash))
}
pub fn executable(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}
pub fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    let _ = path;
    Ok(())
}
pub async fn install(dir: &Path, archive: Option<&Path>) -> Result<PathBuf> {
    let (name, expected) = asset()?;
    let bytes = if let Some(path) = archive {
        std::fs::read(path)?
    } else {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()?;
        let mut response = client
            .get(format!(
                "https://github.com/openai/tunnel-client/releases/download/{VERSION}/{name}"
            ))
            .send()
            .await?
            .error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 512 * 1024 * 1024,
                "tunnel archive exceeds size limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        bytes
    };
    ensure!(
        hex::encode(Sha256::digest(&bytes)) == expected,
        "official tunnel archive SHA256 mismatch"
    );
    crate::runtime::secure_dir(dir)?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut total = 0;
    for i in 0..zip.len() {
        let mut item = zip.by_index(i)?;
        let relative = item
            .enclosed_name()
            .ok_or_else(|| anyhow::anyhow!("unsafe archive path"))?;
        ensure!(
            item.unix_mode().unwrap_or(0) & 0o170000 != 0o120000,
            "archive symlink rejected"
        );
        total += item.size();
        ensure!(
            total <= 1024 * 1024 * 1024,
            "expanded archive exceeds size limit"
        );
        let path = dir.join(relative);
        if item.is_dir() {
            crate::runtime::secure_dir(&path)?;
        } else {
            crate::runtime::secure_dir(path.parent().unwrap())?;
            let mut file = std::fs::File::create(&path)?;
            std::io::copy(&mut item, &mut file)?;
            make_executable(&path)?;
        }
    }
    let name = format!("tunnel-client{}", std::env::consts::EXE_SUFFIX);
    let paths = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && e.file_name() == name.as_str())
        .map(|e| e.into_path())
        .collect::<Vec<_>>();
    ensure!(
        paths.len() == 1,
        "expected exactly one official tunnel executable"
    );
    Ok(paths[0].clone())
}
