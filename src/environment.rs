//! Login-environment snapshot. A systemd/launchd service starts with a minimal
//! environment, so commands would miss PATH entries from ~/.profile (cargo, nvm, ...).
//! The snapshot is taken from the user's login shell and cached for an hour.
use anyhow::Result;
use std::{collections::BTreeMap, path::Path, process::Stdio};

const MARKER: &str = "__SHELLMCP_ENV__";
const TTL_S: u64 = 3600;
const DROP: &[&str] = &["PWD", "OLDPWD", "SHLVL", "_", "PS1", "TERM_SESSION_ID"];

/// Shell used for shell_exec on Unix: bash when present (models usually write Bash),
/// otherwise POSIX sh.
pub fn exec_shell() -> &'static str {
    if cfg!(windows) {
        "powershell.exe"
    } else if Path::new("/bin/bash").exists() {
        "/bin/bash"
    } else {
        "/bin/sh"
    }
}

pub fn login_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| s.starts_with('/') && Path::new(s).exists())
        .unwrap_or_else(|| "/bin/sh".into())
}

/// Returns the cached login environment, refreshing it when stale. Failures fall
/// back to the service environment so a broken profile never blocks execution.
pub fn login_env(dir: &Path) -> BTreeMap<String, String> {
    if cfg!(windows) {
        return std::env::vars().collect();
    }
    let cache = dir.join("login-env.json");
    if let Ok(bytes) = std::fs::read(&cache)
        && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && crate::runtime::now().saturating_sub(v["taken_at"].as_u64().unwrap_or(0)) < TTL_S
        && let Ok(env) = serde_json::from_value::<BTreeMap<String, String>>(v["env"].clone())
    {
        return env;
    }
    match snapshot() {
        Ok(env) => {
            let v = serde_json::json!({"taken_at":crate::runtime::now(),"shell":login_shell(),"env":env});
            let _ = crate::runtime::private_write(&cache, v.to_string().as_bytes());
            env
        }
        Err(e) => {
            tracing::warn!(error=%e, "login environment snapshot failed");
            std::env::vars().collect()
        }
    }
}

fn snapshot() -> Result<BTreeMap<String, String>> {
    let shell = login_shell();
    let mut child = std::process::Command::new(&shell)
        .args(["-l", "-c", &format!("printf '\\0{MARKER}\\0'; env -0")])
        .current_dir(crate::runtime::home_dir())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut out = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut buf);
        buf
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("login shell {shell} did not finish within 10s");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let bytes = reader.join().map_err(|_| anyhow::anyhow!("reader panic"))?;
    parse(&bytes)
}

pub fn parse(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    let marker = format!("\0{MARKER}\0");
    let text = String::from_utf8_lossy(bytes);
    let start = text
        .find(&marker)
        .ok_or_else(|| anyhow::anyhow!("environment marker missing"))?;
    let mut env = BTreeMap::new();
    for item in text[start + marker.len()..].split('\0') {
        if let Some((k, v)) = item.split_once('=')
            && !k.is_empty()
            && !DROP.contains(&k)
        {
            env.insert(k.to_string(), v.to_string());
        }
    }
    anyhow::ensure!(env.contains_key("PATH"), "login environment has no PATH");
    Ok(env)
}
