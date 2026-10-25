use super::install::Manifest;
use anyhow::{Result, ensure};
use std::time::Duration;
fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(6))
        .build()?)
}
pub async fn agent(m: &Manifest) -> Result<serde_json::Value> {
    let c = super::config::load(&m.data_dir)?;
    let value: serde_json::Value = client()?
        .get(format!("http://{}/readyz", c.listen))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    ensure!(
        value["ready"] == true
            && value["mode"] == "local"
            && value["instance_id"] == m.instance_id
            && value["version"] == m.version
            && value["release_dir"] == m.release_dir.to_string_lossy().as_ref(),
        "MCP readiness identity mismatch"
    );
    Ok(value)
}
pub async fn tunnel(m: &Manifest) -> Result<serde_json::Value> {
    let http = client()?;
    let ready = http
        .get(format!("http://{}/readyz", m.health))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    ensure!(ready.trim() == "ready", "tunnel is not ready");
    let value: serde_json::Value = http
        .get(format!("http://{}/health?details=true", m.health))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let control = &value["components"]["control-plane"];
    ensure!(
        control["status"] == "ok"
            && control["details"]["last_success"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
            && control["details"]["consecutive_failures"] == 0,
        "tunnel has not successfully polled OpenAI"
    );
    ensure!(
        value["components"]["mcp"]["details"]["startup_probe"]["state"] == "succeeded",
        "tunnel MCP startup probe did not succeed"
    );
    // Identity comes from the supervisor holding the private release lock, plus its PID file.
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(m.release_dir.join("tunnel-state.json"))?)?;
    ensure!(
        state["release_dir"] == m.release_dir.to_string_lossy().as_ref()
            && state["tunnel_id"] == m.tunnel_id,
        "tunnel identity mismatch"
    );
    Ok(
        serde_json::json!({"ready":true,"tunnel_id":m.tunnel_id,"last_success":control["details"]["last_success"],"pid":state["pid"]}),
    )
}
pub async fn wait(m: &Manifest, with_tunnel: bool, budget: Duration) -> Result<()> {
    let until = tokio::time::Instant::now() + budget;
    loop {
        let result = async {
            agent(m).await?;
            if with_tunnel {
                tunnel(m).await?;
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        match result {
            Ok(()) => return Ok(()),
            Err(e) if tokio::time::Instant::now() >= until => return Err(e),
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
pub async fn wait_stopped(m: &Manifest) -> Result<()> {
    let c = super::config::load(&m.data_dir)?;
    for i in 0..50 {
        let stopped = tokio::net::TcpStream::connect(c.listen).await.is_err()
            && tokio::net::TcpStream::connect(m.health).await.is_err();
        let unlocked = ["local.lock", "tunnel.lock"].iter().all(|name| {
            let file = match std::fs::OpenOptions::new()
                .write(true)
                .open(m.data_dir.join(name))
            {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
                Err(_) => return false,
            };
            fs2::FileExt::try_lock_exclusive(&file).is_ok()
        });
        if stopped && unlocked {
            return Ok(());
        }
        ensure!(
            i < 49,
            "listeners or service locks remained after service removal"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}
pub async fn doctor(dir: &std::path::Path) -> Result<serde_json::Value> {
    let m = super::install::load(dir)?;
    Ok(serde_json::json!({"agent":agent(&m).await?,"tunnel":tunnel(&m).await?}))
}
