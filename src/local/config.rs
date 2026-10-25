use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalConfig {
    pub config_version: u32,
    pub instance_id: String,
    pub listen: SocketAddr,
    pub max_wait_s: u64,
}

pub fn default_dir() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default())
            .join("ShellMCP-Agent-Local")
    } else {
        crate::runtime::home_dir().join(".local/share/shellmcp-agent-local")
    }
}

pub fn validate(config: &LocalConfig) -> Result<()> {
    ensure!(
        config.config_version == 1,
        "unsupported local config version"
    );
    uuid::Uuid::parse_str(&config.instance_id)?;
    ensure!(
        matches!(config.listen.ip(), IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST)
            || matches!(config.listen.ip(), IpAddr::V6(ip) if ip == Ipv6Addr::LOCALHOST),
        "local MCP must bind 127.0.0.1 or ::1"
    );
    ensure!(
        config.listen.port() != 0,
        "configure an explicit nonzero port"
    );
    ensure!(
        (1..=30).contains(&config.max_wait_s),
        "local wait limit must be 1..30 seconds"
    );
    Ok(())
}

pub fn check_dir(dir: &Path) -> Result<()> {
    if dir.exists() {
        private_path(dir, true)?;
    }
    ensure!(
        !dir.join("config.json").exists(),
        "refusing a legacy agent data directory"
    );
    let legacy = if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("ShellMCP-Agent")
    } else {
        crate::runtime::home_dir().join(".local/share/shellmcp-agent")
    };
    if let (Ok(a), Ok(b)) = (dir.canonicalize(), legacy.canonicalize()) {
        ensure!(
            a != b && !a.starts_with(&b),
            "local and legacy data directories must be separate"
        );
    }
    #[cfg(unix)]
    if dir.exists() {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            dir.metadata()?.uid() == unsafe { libc::geteuid() },
            "data directory belongs to another user"
        );
    }
    Ok(())
}

pub(crate) fn private_path(path: &Path, directory: bool) -> Result<()> {
    let m = std::fs::symlink_metadata(path)?;
    ensure!(
        !m.file_type().is_symlink() && m.is_dir() == directory,
        "unexpected path type: {}",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0,
            "path must be private and owned by current user: {}",
            path.display()
        );
    }
    Ok(())
}

pub fn init(dir: &Path, listen: SocketAddr) -> Result<LocalConfig> {
    check_dir(dir)?;
    if dir.join("local.json").exists() {
        let c = load(dir)?;
        ensure!(
            c.listen == listen,
            "existing instance uses {}; change local.json explicitly",
            c.listen
        );
        return Ok(c);
    }
    ensure!(
        !dir.join("state.db").exists(),
        "refusing to adopt an unidentified existing database"
    );
    let c = LocalConfig {
        config_version: 1,
        instance_id: uuid::Uuid::new_v4().to_string(),
        listen,
        max_wait_s: 30,
    };
    validate(&c)?;
    crate::runtime::secure_dir(dir)?;
    crate::runtime::secure_dir(&dir.join("jobs"))?;
    crate::runtime::private_write(&dir.join("local.json"), &serde_json::to_vec_pretty(&c)?)?;
    crate::policy::save(dir, &crate::policy::Policy::default())?;
    crate::store::Store::open(dir)?;
    super::admission::open(dir)?;
    Ok(c)
}

pub fn load(dir: &Path) -> Result<LocalConfig> {
    check_dir(dir)?;
    private_path(&dir.join("local.json"), false)?;
    private_path(&dir.join("jobs"), true)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = dir.join("local.json").metadata()?;
        ensure!(
            m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0,
            "local.json must be owned by the current user and private"
        );
    }
    let c = serde_json::from_slice(&std::fs::read(dir.join("local.json"))?)?;
    validate(&c)?;
    crate::policy::load_checked(dir)?;
    Ok(c)
}
