use anyhow::{Result, ensure};
use std::{
    path::Path,
    process::{Command, Stdio},
};
pub async fn run(dir: &Path, release: &Path) -> Result<()> {
    let config = super::config::load(dir)?;
    ensure!(
        release.canonicalize()?.parent() == Some(dir.canonicalize()?.join("releases").as_path()),
        "unexpected release path"
    );
    let m: super::install::Manifest =
        serde_json::from_slice(&std::fs::read(release.join("release.json"))?)?;
    ensure!(
        m.instance_id == config.instance_id && m.data_dir == dir && m.release_dir == release,
        "tunnel release identity mismatch"
    );
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("tunnel.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|e| anyhow::anyhow!("local tunnel lock: {e}"))?;
    let log_path = dir.join("tunnel.log");
    if std::fs::metadata(&log_path).is_ok_and(|m| m.len() > 8 * 1024 * 1024) {
        let previous = dir.join("tunnel.log.1");
        if previous.exists() {
            std::fs::remove_file(&previous)?;
        }
        std::fs::rename(&log_path, previous)?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let mut command = Command::new(&m.tunnel);
    super::install::sanitized(&mut command);
    command
        .args(["run", "--profile-file"])
        .arg(release.join("profiles/local.yaml"))
        .args([
            "--open-web-ui=false",
            "--mcp.startup-wait-timeout",
            "20s",
            "--log.format",
            "json",
        ])
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = tokio::process::Command::from(command)
        .kill_on_drop(true)
        .spawn()?;
    #[cfg(windows)]
    let _tree = crate::jobs::WindowsJob::assign_handle(
        child
            .raw_handle()
            .ok_or_else(|| anyhow::anyhow!("tunnel process handle unavailable"))? as _,
    )?;
    crate::runtime::private_write(
        &release.join("tunnel-state.json"),
        &serde_json::to_vec(
            &serde_json::json!({"release_dir":release,"tunnel_id":m.tunnel_id,"pid":child.id(),"host_pid":std::process::id()}),
        )?,
    )?;
    let term = async {
        #[cfg(unix)]
        {
            if let Ok(mut signal) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                signal.recv().await;
                return;
            }
        }
        std::future::pending::<()>().await;
    };
    let status = tokio::select! {
        result=child.wait()=>Some(result?),
        _=tokio::signal::ctrl_c()=>None,
        _=term=>None,
    };
    if status.is_none() {
        child.kill().await?;
        child.wait().await?;
    }
    std::fs::remove_file(release.join("tunnel-state.json"))?;
    if let Some(status) = status {
        anyhow::bail!("tunnel-client stopped: {status}");
    }
    Ok(())
}
