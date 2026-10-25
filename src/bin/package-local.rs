//! Offline packaging only. Does not upload artifacts or modify services.
use anyhow::{Result, ensure};
use clap::Parser;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::PathBuf};
#[derive(Parser)]
struct Args {
    #[arg(long)]
    out: PathBuf,
    #[arg(long)]
    base_url: String,
    /// Repeat platform=path, e.g. linux-amd64=target/release/shellmcp-agent.
    #[arg(long, required = true)]
    artifact: Vec<String>,
    /// Every Windows agent must be paired with its GUI host.
    #[arg(long)]
    host_artifact: Vec<String>,
}
fn parse(values: Vec<String>) -> Result<BTreeMap<String, PathBuf>> {
    let mut map = BTreeMap::new();
    for value in values {
        let (key, path) = value
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("expected platform=path"))?;
        ensure!(
            map.insert(key.into(), path.into()).is_none(),
            "duplicate platform"
        );
    }
    Ok(map)
}
fn main() -> Result<()> {
    let args = Args::parse();
    let base = args.base_url.trim_end_matches('/');
    let url = url::Url::parse(base)?;
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && !base.contains(['\'', '"', '\n', '\r', '`', '$']),
        "use a plain HTTPS release directory URL"
    );
    let agents = parse(args.artifact)?;
    let hosts = parse(args.host_artifact)?;
    ensure!(!args.out.exists(), "output directory must be new");
    for (platform, path) in &agents {
        ensure!(
            matches!(
                platform.as_str(),
                "linux-amd64"
                    | "linux-arm64"
                    | "darwin-amd64"
                    | "darwin-arm64"
                    | "windows-amd64"
                    | "windows-arm64"
            ),
            "unsupported platform"
        );
        ensure!(path.metadata()?.len() > 0, "empty agent artifact");
        if platform.starts_with("windows") {
            ensure!(
                hosts
                    .get(platform)
                    .is_some_and(|p| p.metadata().is_ok_and(|m| m.len() > 0)),
                "Windows host missing"
            );
        }
    }
    let mut shell = String::new();
    let mut windows = serde_json::Map::new();
    let mut manifest = serde_json::Map::new();
    for (platform, source) in &agents {
        let dir = args.out.join(platform);
        std::fs::create_dir_all(&dir)?;
        let windows_target = platform.starts_with("windows");
        let filename = if windows_target {
            "shellmcp-agent.exe"
        } else {
            "shellmcp-agent"
        };
        let bytes = std::fs::read(source)?;
        let hash = hex::encode(Sha256::digest(&bytes));
        std::fs::write(dir.join(filename), bytes)?;
        let agent =
            serde_json::json!({"url":format!("{base}/{platform}/{filename}"),"sha256":hash});
        if windows_target {
            let bytes = std::fs::read(&hosts[platform])?;
            let hash = hex::encode(Sha256::digest(&bytes));
            std::fs::write(dir.join("shellmcp-host.exe"), bytes)?;
            windows.insert(platform.clone(),serde_json::json!({"agent":agent,"host":{"url":format!("{base}/{platform}/shellmcp-host.exe"),"sha256":hash}}));
        } else {
            let uname = match platform.as_str() {
                "linux-amd64" => "Linux-x86_64",
                "linux-arm64" => "Linux-aarch64",
                "darwin-amd64" => "Darwin-x86_64",
                _ => "Darwin-arm64",
            };
            shell.push_str(&format!(
                "  {uname}) agent_url='{base}/{platform}/{filename}'; agent_sha='{hash}' ;;\n"
            ));
        }
        manifest.insert(platform.clone(), agent);
    }
    let sh = include_str!("../../scripts/install-local.sh").replace("# @ARTIFACTS@", &shell);
    let ps = include_str!("../../scripts/install-local.ps1")
        .replace("@ARTIFACTS@", &serde_json::to_string(&windows)?);
    std::fs::write(args.out.join("install-local.sh"), sh)?;
    std::fs::write(args.out.join("install-local.ps1"), format!("\u{feff}{ps}"))?;
    std::fs::write(
        args.out.join("artifacts.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    println!(
        "Packaged {} platforms in {}. Nothing published.",
        agents.len(),
        args.out.display()
    );
    Ok(())
}
