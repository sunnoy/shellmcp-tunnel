//! Local owner policy and audit log. The policy file lives in the agent data dir,
//! which filesystem tools refuse to touch, so remote tools cannot relax it through fs
//! tools; in read_only mode no tool can write anywhere.
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const AUDIT_MAX: u64 = 10 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Full,
    ReadOnly,
    Disabled,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Policy {
    pub mode: Mode,
    #[serde(default)]
    pub deny_tools: Vec<String>,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            mode: Mode::Full,
            deny_tools: vec![],
        }
    }
}

pub fn load(dir: &Path) -> Policy {
    std::fs::read(dir.join("policy.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
pub fn load_checked(dir: &Path) -> Result<Policy> {
    match std::fs::read(dir.join("policy.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Policy::default()),
        Err(e) => Err(e.into()),
    }
}
pub fn save(dir: &Path, p: &Policy) -> Result<()> {
    for t in &p.deny_tools {
        ensure!(crate::catalogue::tool(t).is_some(), "unknown tool {t}");
    }
    crate::runtime::private_write(&dir.join("policy.json"), &serde_json::to_vec_pretty(p)?)
}
/// Tools allowed under the local policy for discovery and environment reporting.
pub fn allowed_tools(p: &Policy) -> Vec<String> {
    crate::catalogue::catalogue()["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["name"].as_str())
        .filter(|n| check(p, n).is_ok())
        .map(String::from)
        .collect()
}
pub fn check(p: &Policy, tool: &str) -> Result<()> {
    if p.deny_tools.iter().any(|t| t == tool) {
        bail!("tool {tool} is disabled by the host owner's local policy");
    }
    match p.mode {
        Mode::Full => Ok(()),
        Mode::Disabled => bail!("remote execution is disabled on this host by its owner"),
        Mode::ReadOnly => {
            let control = matches!(tool, "get_environment" | "job_list" | "job_output");
            ensure!(
                control || crate::catalogue::read_only(tool),
                "tool {tool} is blocked: this host is in read-only mode"
            );
            Ok(())
        }
    }
}

/// Rejects filesystem tool paths inside the agent data directory (device token,
/// policy, state). Symlinks are resolved through the nearest existing ancestor.
pub fn guard_path(dir: &Path, path: &Path) -> Result<()> {
    let data = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut probe: PathBuf = path.to_path_buf();
    let mut tail = Vec::new();
    let resolved = loop {
        if let Ok(c) = std::fs::canonicalize(&probe) {
            break tail
                .iter()
                .rev()
                .fold(c, |acc: PathBuf, t: &std::ffi::OsString| acc.join(t));
        }
        // file_name() is None for `..`. Peeling path components keeps walking
        // to a canonical ancestor, including macOS /var -> /private/var aliases.
        match probe.components().next_back() {
            Some(std::path::Component::Normal(name)) => tail.push(name.to_os_string()),
            Some(std::path::Component::ParentDir) => tail.push("..".into()),
            Some(std::path::Component::CurDir) => {}
            _ => bail!("cannot resolve filesystem tool path"),
        }
        ensure!(probe.pop(), "cannot resolve filesystem tool path");
    };
    ensure!(
        !resolved.starts_with(&data),
        "access to the agent data directory is not allowed through remote tools"
    );
    Ok(())
}

/// Appends one JSON line per invocation; rotates to audit.jsonl.1 past 10MiB.
pub fn audit(dir: &Path, entry: serde_json::Value) {
    use std::io::Write;
    let path = dir.join("audit.jsonl");
    if std::fs::metadata(&path)
        .map(|m| m.len() > AUDIT_MAX)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, dir.join("audit.jsonl.1"));
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    if let Ok(mut f) = opts.open(&path) {
        let _ = writeln!(f, "{entry}");
    }
}

pub fn tail_audit(dir: &Path, n: usize) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(dir.join("audit.jsonl")).unwrap_or_default();
    let lines: Vec<String> = text.lines().map(String::from).collect();
    Ok(lines[lines.len().saturating_sub(n)..].to_vec())
}
