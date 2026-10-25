use super::{
    config, download,
    services::{self, Definition},
};
use anyhow::{Context, Result, ensure};
use clap::Args;
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
#[derive(Args)]
pub struct Options {
    #[arg(long,default_value_os_t=config::default_dir())]
    pub data_dir: PathBuf,
    #[arg(long)]
    pub tunnel_id: String,
    #[arg(long, conflicts_with = "key_env")]
    pub key_file: Option<PathBuf>,
    #[arg(long, conflicts_with = "key_file")]
    pub key_env: Option<String>,
    #[arg(long, default_value = "127.0.0.1:18700")]
    pub listen: SocketAddr,
    #[arg(long, default_value = "127.0.0.1:18710")]
    pub health: SocketAddr,
    /// Offline cache of the exact pinned official archive; digest verification remains mandatory.
    #[arg(long)]
    pub tunnel_archive: Option<PathBuf>,
    #[arg(long)]
    pub dry_run: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub instance_id: String,
    pub data_dir: PathBuf,
    pub release_dir: PathBuf,
    pub version: String,
    pub tunnel: PathBuf,
    pub tunnel_id: String,
    pub health: SocketAddr,
    pub services: Vec<Definition>,
}
pub fn load(dir: &Path) -> Result<Manifest> {
    let dir = dir.canonicalize()?;
    config::load(&dir)?;
    config::private_path(&dir.join("install.json"), false)?;
    let m: Manifest = serde_json::from_slice(&std::fs::read(dir.join("install.json"))?)?;
    validate(&dir, &m)?;
    Ok(m)
}
fn validate(dir: &Path, m: &Manifest) -> Result<()> {
    ensure!(
        m.schema == 1
            && m.data_dir == dir.canonicalize()?
            && m.instance_id == config::load(dir)?.instance_id,
        "installation identity mismatch"
    );
    ensure!(
        m.release_dir.parent() == Some(dir.join("releases").as_path()),
        "invalid release directory"
    );
    uuid::Uuid::parse_str(
        m.release_dir
            .file_name()
            .unwrap()
            .to_str()
            .unwrap_or_default(),
    )?;
    ensure!(m.tunnel.starts_with(&m.release_dir), "invalid tunnel path");
    let expected = services::definitions(dir, &m.release_dir, &m.instance_id)?;
    ensure!(
        serde_json::to_value(&expected)? == serde_json::to_value(&m.services)?,
        "invalid service manifest"
    );
    Ok(())
}
fn lock(dir: &Path) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("install.lock"))?;
    fs2::FileExt::try_lock_exclusive(&file).context("another installer is running")?;
    Ok(file)
}
fn key(options: &Options, old: Option<&Manifest>) -> Result<Vec<u8>> {
    let bytes = if let Some(path) = &options.key_file {
        std::fs::read(path)?
    } else if let Some(name) = &options.key_env {
        std::env::var(name)
            .context("key environment variable is unavailable")?
            .into_bytes()
    } else if let Some(m) = old {
        std::fs::read(m.release_dir.join("runtime.key"))?
    } else {
        anyhow::bail!("provide --key-file or --key-env (variable name, never the secret itself)")
    };
    let value = std::str::from_utf8(&bytes)?.trim();
    ensure!(
        !value.is_empty() && !value.contains(char::is_whitespace),
        "invalid runtime key file"
    );
    Ok(value.as_bytes().to_vec())
}
pub(crate) fn sanitized(command: &mut Command) {
    // Explicit allowlist: tunnel configuration and keys cannot leak from a login environment.
    let vars = std::env::vars_os()
        .filter(|(k, _)| {
            matches!(
                k.to_string_lossy().as_ref(),
                "PATH"
                    | "HOME"
                    | "USER"
                    | "USERPROFILE"
                    | "LOCALAPPDATA"
                    | "APPDATA"
                    | "SYSTEMROOT"
                    | "SystemRoot"
                    | "WINDIR"
                    | "TEMP"
                    | "TMP"
                    | "TMPDIR"
                    | "SSL_CERT_FILE"
                    | "SSL_CERT_DIR"
                    | "HTTP_PROXY"
                    | "HTTPS_PROXY"
                    | "ALL_PROXY"
                    | "NO_PROXY"
                    | "http_proxy"
                    | "https_proxy"
                    | "all_proxy"
                    | "no_proxy"
            )
        })
        .collect::<Vec<_>>();
    command.env_clear().envs(vars);
}
pub async fn install(o: Options) -> Result<()> {
    ensure!(
        o.tunnel_id.starts_with("tunnel_")
            && o.tunnel_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "invalid tunnel ID"
    );
    let mut health = config::LocalConfig {
        config_version: 1,
        instance_id: uuid::Uuid::new_v4().to_string(),
        listen: o.health,
        max_wait_s: 30,
    };
    config::validate(&health)?;
    health.listen = o.listen;
    config::validate(&health)?;
    ensure!(o.health != o.listen, "health and MCP ports must differ");
    config::check_dir(&o.data_dir)?;
    download::asset()?;
    if o.dry_run {
        println!(
            "{}",
            serde_json::json!({"action":"install","data_dir":o.data_dir,"listen":o.listen,"health":o.health,"tunnel_id":o.tunnel_id,"tunnel_version":download::VERSION})
        );
        return Ok(());
    }
    crate::runtime::secure_dir(&o.data_dir)?;
    let dir = o.data_dir.canonicalize()?;
    let _lock = lock(&dir)?;
    ensure!(
        !dir.join("transaction.json").exists(),
        "unfinished installation; use local recover first"
    );
    let old = if dir.join("install.json").exists() {
        Some(load(&dir)?)
    } else {
        None
    };
    let key = key(&o, old.as_ref())?;
    let config = config::init(&dir, o.listen)?;
    let release = dir.join("releases").join(uuid::Uuid::new_v4().to_string());
    let defs = services::definitions(&dir, &release, &config.instance_id)?;
    services::preflight(&defs, old.as_ref().map_or(&[], |m| m.services.as_slice()))?;
    if let Some(m) = &old {
        ensure!(
            m.health == o.health,
            "changing the health port requires uninstall first"
        );
        super::diagnostics::agent(m)
            .await
            .context("existing agent not healthy; repair it before upgrading")?;
    } else {
        for addr in [o.listen, o.health] {
            std::net::TcpListener::bind(addr)
                .with_context(|| format!("port {addr} is already occupied"))?;
        }
    }
    crate::runtime::secure_dir(&release)?;
    let prepared = prepare(&dir, &release, &o, key, &config, defs).await;
    let manifest = match prepared {
        Ok(m) => m,
        Err(e) => {
            std::fs::remove_dir_all(&release)?;
            return Err(e);
        }
    };
    // Journal remains if killed midway; never silently treat a partial install as committed.
    ensure!(
        !dir.join("transaction.json").exists(),
        "unfinished installation; use local recover first"
    );
    crate::runtime::private_write(
        &dir.join("transaction.json"),
        &serde_json::to_vec(&manifest)?,
    )?;
    let result = activate(&manifest, old.as_ref()).await;
    if let Err(error) = result {
        let recovery = rollback(&manifest, old.as_ref()).await;
        if let Err(recovery) = recovery {
            anyhow::bail!(
                "installation failed: {error:#}; rollback failed: {recovery:#}; run local recover"
            );
        }
        std::fs::remove_file(dir.join("transaction.json"))?;
        anyhow::bail!("installation failed and previous services restored: {error:#}");
    }
    crate::runtime::private_write(
        &dir.join("install.json"),
        &serde_json::to_vec_pretty(&manifest)?,
    )?;
    std::fs::remove_file(dir.join("transaction.json"))?;
    println!(
        "{}",
        serde_json::json!({"installed":true,"version":manifest.version,"data_dir":dir,"tunnel_id":manifest.tunnel_id,"mcp_url":format!("http://{}/mcp",config.listen),"services":manifest.services.iter().map(|d|&d.name).collect::<Vec<_>>(),"chatgpt":"Select Tunnel, this tunnel ID, and No Auth; client connection still needs verification."})
    );
    Ok(())
}
async fn prepare(
    dir: &Path,
    release: &Path,
    o: &Options,
    key: Vec<u8>,
    config: &config::LocalConfig,
    defs: Vec<Definition>,
) -> Result<Manifest> {
    let source = std::env::current_exe()?;
    let agent = download::executable(release, "shellmcp-agent");
    std::fs::copy(&source, &agent)?;
    download::make_executable(&agent)?;
    #[cfg(windows)]
    {
        let host = download::executable(source.parent().unwrap(), "shellmcp-host");
        ensure!(
            host.exists(),
            "shellmcp-host.exe must accompany the agent executable"
        );
        std::fs::copy(host, download::executable(release, "shellmcp-host"))?;
    }
    let tunnel = download::install(&release.join("tunnel"), o.tunnel_archive.as_deref()).await?;
    crate::runtime::private_write(&release.join("runtime.key"), &key)?;
    crate::runtime::secure_dir(&release.join("profiles"))?;
    let mut init = Command::new(&tunnel);
    sanitized(&mut init);
    init.args([
        "init",
        "--sample",
        "sample_mcp_remote_no_auth",
        "--profile",
        "local",
        "--profile-dir",
    ])
    .arg(release.join("profiles"))
    .args([
        "--tunnel-id",
        &o.tunnel_id,
        "--mcp-server-url",
        &format!("http://{}/mcp", o.listen),
        "--health-listen-addr",
        &o.health.to_string(),
        "--control-plane-api-key-ref",
        &format!("file:{}", release.join("runtime.key").display()),
    ]);
    services::checked(&mut init)?;
    let m = Manifest {
        schema: 1,
        instance_id: config.instance_id.clone(),
        data_dir: dir.into(),
        release_dir: release.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        tunnel,
        tunnel_id: o.tunnel_id.clone(),
        health: o.health,
        services: defs,
    };
    crate::runtime::private_write(&release.join("release.json"), &serde_json::to_vec(&m)?)?;
    Ok(m)
}
async fn activate(m: &Manifest, old: Option<&Manifest>) -> Result<()> {
    if let Some(old) = old {
        for d in old.services.iter().rev() {
            services::stop(d)?;
        }
        super::diagnostics::wait_stopped(old).await?;
    }
    services::install(&m.services[0])?;
    super::diagnostics::wait(m, false, Duration::from_secs(20)).await?;
    services::install(&m.services[1])?;
    super::diagnostics::wait(m, true, Duration::from_secs(75)).await?;
    Ok(())
}
async fn rollback(m: &Manifest, old: Option<&Manifest>) -> Result<()> {
    let mut failures = vec![];
    for d in m.services.iter().rev().filter(|d| d.path.exists()) {
        if let Err(e) = services::remove(d) {
            failures.push(e.to_string());
        }
    }
    // Task Scheduler can report Ready before the terminated process releases its locks.
    super::diagnostics::wait_stopped(m).await?;
    ensure!(failures.is_empty(), "{}", failures.join("; "));
    if let Some(old) = old {
        activate(old, None).await?;
    }
    Ok(())
}
pub async fn recover(dir: &Path) -> Result<()> {
    let dir = dir.canonicalize()?;
    config::load(&dir)?;
    let _lock = lock(&dir)?;
    let m: Manifest = serde_json::from_slice(&std::fs::read(dir.join("transaction.json"))?)?;
    validate(&dir, &m)?;
    let old = if dir.join("install.json").exists() {
        Some(load(&dir)?)
    } else {
        None
    };
    rollback(&m, old.as_ref()).await?;
    std::fs::remove_file(dir.join("transaction.json"))?;
    println!("installation recovered");
    Ok(())
}
pub async fn uninstall(dir: &Path, purge: bool) -> Result<()> {
    let dir = dir.canonicalize()?;
    ensure!(
        !(cfg!(windows) && purge && std::env::current_exe()?.starts_with(&dir)),
        "run uninstall --purge from an executable outside the installation directory on Windows"
    );
    let _lock = lock(&dir)?;
    let m = load(&dir)?;
    ensure!(
        !dir.join("transaction.json").exists(),
        "run local recover before uninstall"
    );
    ensure!(
        crate::store::Store::open(&dir)?.active_job_count()? == 0,
        "running jobs exist; finish or cancel them before uninstall"
    );
    services::preflight(&m.services, &m.services)?;
    for d in m.services.iter().rev() {
        services::remove(d)?;
    }
    super::diagnostics::wait_stopped(&m).await?;
    std::fs::remove_file(dir.join("install.json"))?;
    if purge {
        drop(_lock);
        std::fs::remove_dir_all(&dir)?;
    }
    println!(
        "{}",
        serde_json::json!({"uninstalled":true,"data_preserved":!purge})
    );
    Ok(())
}
