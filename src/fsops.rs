//! Filesystem tools. Writes are atomic, follow symlinks to their target and keep the
//! target's existing permission bits; new files get the process umask (normally 0644).
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "__pycache__",
    ".cache",
];
const MAX_EDIT_FILE: u64 = 8 * 1024 * 1024;
const MAX_GREP_FILE: u64 = 4 * 1024 * 1024;
const MAX_WALK: usize = 200_000;

fn mtime(m: &std::fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
fn kind(t: std::fs::FileType) -> &'static str {
    if t.is_symlink() {
        "symlink"
    } else if t.is_dir() {
        "directory"
    } else if t.is_file() {
        "file"
    } else {
        "other"
    }
}
#[cfg(unix)]
fn mode(m: &std::fs::Metadata) -> Value {
    use std::os::unix::fs::PermissionsExt;
    json!(format!("{:04o}", m.permissions().mode() & 0o7777))
}
#[cfg(not(unix))]
fn mode(m: &std::fs::Metadata) -> Value {
    json!(if m.permissions().readonly() {
        "readonly"
    } else {
        "writable"
    })
}

pub fn list(path: &Path) -> Result<Value> {
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in
        std::fs::read_dir(path).with_context(|| format!("cannot list {}", path.display()))?
    {
        if entries.len() >= 1000 {
            truncated = true;
            break;
        }
        let entry = entry?;
        let t = entry.file_type()?;
        let m = entry.metadata().ok();
        entries.push(
            json!({"name":entry.file_name().to_string_lossy(),"type":kind(t),
            "size":m.as_ref().map(|m|m.len()),"modified_unix":m.as_ref().map(mtime)}),
        );
    }
    entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(json!({"path":path,"entries":entries,"truncated":truncated}))
}

pub fn info(path: &Path) -> Result<Value> {
    let Ok(l) = std::fs::symlink_metadata(path) else {
        return Ok(json!({"path":path,"exists":false}));
    };
    let mut v = json!({"path":path,"exists":true,"type":kind(l.file_type()),"size":l.len(),
        "mode":mode(&l),"modified_unix":mtime(&l)});
    if l.file_type().is_symlink() {
        v["target"] = json!(std::fs::read_link(path)?);
        if let Ok(m) = std::fs::metadata(path) {
            v["target_type"] = json!(kind(m.file_type()));
            v["size"] = json!(m.len());
        }
    }
    if let Ok(m) = std::fs::metadata(path)
        && m.is_file()
        && m.len() <= MAX_EDIT_FILE
    {
        let bytes = std::fs::read(path)?;
        let text = !bytes.iter().take(8192).any(|b| *b == 0) && std::str::from_utf8(&bytes).is_ok();
        v["text"] = json!(text);
        if text {
            v["lines"] = json!(
                bytes.iter().filter(|b| **b == b'\n').count()
                    + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"))
            );
        }
    }
    Ok(v)
}

pub fn read_many(paths: &[PathBuf], limit: u64) -> Value {
    let files: Vec<Value> = paths
        .iter()
        .map(|p| match crate::tools::read_utf8(p, 0, limit) {
            Ok(mut v) => {
                v["path"] = json!(p);
                v
            }
            Err(e) => json!({"path":p,"error":e.to_string()}),
        })
        .collect();
    json!({"files":files})
}

/// Writes atomically through a sibling temporary file. A symlinked path writes to the
/// link target, so dotfile managers keep working; existing permission bits survive.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<Value> {
    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path)
            .with_context(|| format!("dangling symlink {}", path.display()))?,
        _ => path.to_path_buf(),
    };
    let existing = std::fs::metadata(&target).ok();
    if let Some(m) = &existing {
        ensure!(m.is_file(), "{} is not a regular file", target.display());
    }
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    ensure!(
        parent.is_dir(),
        "parent directory {} does not exist",
        parent.display()
    );
    let name = target
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid path"))?;
    let tmp = parent.join(format!(
        ".{}.shellmcp-{}",
        name.to_string_lossy(),
        uuid::Uuid::new_v4().simple()
    ));
    let result = (|| -> Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if let Some(m) = &existing {
            std::fs::set_permissions(&tmp, m.permissions())?;
        }
        std::fs::rename(&tmp, &target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("cannot write {}", target.display()))?;
    Ok(json!({"path":target,"bytes_written":bytes.len(),"created":existing.is_none()}))
}

pub fn edit(path: &Path, old: &str, new: &str, all: bool) -> Result<Value> {
    let m = std::fs::metadata(path).with_context(|| format!("cannot open {}", path.display()))?;
    ensure!(
        m.is_file() && m.len() <= MAX_EDIT_FILE,
        "not a regular text file up to 8MiB"
    );
    let text = String::from_utf8(std::fs::read(path)?).context("file is not valid UTF-8")?;
    let count = text.matches(old).count();
    if count == 0 {
        bail!(
            "old_string not found in {}.{}",
            path.display(),
            nearest(&text, old)
        );
    }
    ensure!(
        all || count == 1,
        "old_string occurs {count} times; add surrounding context or set replace_all"
    );
    let updated = if all {
        text.replace(old, new)
    } else {
        text.replacen(old, new, 1)
    };
    let written = write_file(path, updated.as_bytes())?;
    let diff = similar::TextDiff::from_lines(&text, &updated)
        .unified_diff()
        .context_radius(2)
        .to_string();
    let shown: String = diff.chars().take(8000).collect();
    Ok(
        json!({"path":written["path"],"replacements":if all{count}else{1},
        "diff":shown,"diff_truncated":shown.len()<diff.len()}),
    )
}

fn nearest(text: &str, old: &str) -> String {
    let first = old
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(old)
        .trim();
    let mut best = (0.0f32, 0usize, "");
    for (i, line) in text.lines().enumerate().take(100_000) {
        let r = similar::TextDiff::from_chars(first, line.trim()).ratio();
        if r > best.0 {
            best = (r, i + 1, line);
        }
    }
    if best.0 < 0.5 {
        return " No similar line found; re-read the file.".into();
    }
    let line: String = best.2.chars().take(200).collect();
    format!(
        " Closest line {} ({:.0}% similar): {line:?}. Re-read the file and copy exact text, including whitespace.",
        best.1,
        best.0 * 100.0
    )
}

pub fn mkdir(path: &Path) -> Result<Value> {
    let existed = path.is_dir();
    std::fs::create_dir_all(path).with_context(|| format!("cannot create {}", path.display()))?;
    Ok(json!({"path":path,"created":!existed}))
}

pub fn rename(source: &Path, destination: &Path) -> Result<Value> {
    ensure!(
        std::fs::symlink_metadata(source).is_ok(),
        "source {} does not exist",
        source.display()
    );
    ensure!(
        std::fs::symlink_metadata(destination).is_err(),
        "destination {} exists; refusing to replace it",
        destination.display()
    );
    std::fs::rename(source, destination).with_context(|| {
        format!(
            "cannot move {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(json!({"source":source,"destination":destination}))
}

/// Translates a glob (`*`, `**`, `?`, `[..]`, `{a,b}`) into an anchored regex over
/// `/`-separated relative paths. A pattern without `/` matches the file name anywhere.
pub fn glob_regex(pattern: &str) -> Result<regex::Regex> {
    let p = if pattern.contains('/') {
        pattern.trim_start_matches("./").to_string()
    } else {
        format!("**/{pattern}")
    };
    let mut out = String::from("^");
    let chars: Vec<char> = p.chars().collect();
    let (mut i, mut brace) = (0, 0);
    while i < chars.len() {
        let c = chars[i];
        match c {
            '*' if chars.get(i + 1) == Some(&'*') => {
                if chars.get(i + 2) == Some(&'/') {
                    out.push_str("(?:.*/)?");
                    i += 1;
                } else {
                    out.push_str(".*");
                }
                i += 1;
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            '[' => {
                let end = chars[i..].iter().position(|c| *c == ']').map(|n| n + i);
                let Some(end) = end else {
                    bail!("unclosed [ in glob")
                };
                let body: String = chars[i + 1..end].iter().collect();
                let body = body
                    .strip_prefix('!')
                    .map(|b| format!("^{b}"))
                    .unwrap_or(body);
                out.push('[');
                out.push_str(&body.replace('\\', "\\\\"));
                out.push(']');
                i = end;
            }
            '{' => {
                brace += 1;
                out.push_str("(?:");
            }
            '}' if brace > 0 => {
                brace -= 1;
                out.push(')');
            }
            ',' if brace > 0 => out.push('|'),
            c => out.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    ensure!(brace == 0, "unclosed {{ in glob");
    out.push('$');
    Ok(regex::Regex::new(&out)?)
}

fn walk(root: &Path) -> impl Iterator<Item = walkdir::DirEntry> {
    walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !(e.file_type().is_dir()
                    && SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
        })
        .filter_map(|e| e.ok())
        .take(MAX_WALK)
}
fn relative(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn glob(root: &Path, pattern: &str, max: usize) -> Result<Value> {
    ensure!(root.is_dir(), "{} is not a directory", root.display());
    let re = glob_regex(pattern)?;
    let mut found = Vec::new();
    let mut truncated = false;
    for e in walk(root) {
        if e.depth() == 0 {
            continue;
        }
        let rel = relative(root, e.path());
        if re.is_match(&rel) {
            if found.len() >= max {
                truncated = true;
                break;
            }
            found.push((mtime(&e.metadata()?), rel, e.file_type().is_dir()));
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let matches: Vec<Value> = found
        .into_iter()
        .map(|(t, p, d)| json!({"path":p,"directory":d,"modified_unix":t}))
        .collect();
    Ok(json!({"root":root,"matches":matches,"truncated":truncated,"skipped_dirs":SKIP_DIRS}))
}

pub fn grep(
    root: &Path,
    pattern: &str,
    filter: Option<&str>,
    ignore_case: bool,
    max: usize,
) -> Result<Value> {
    let re = regex::RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .size_limit(1 << 22)
        .build()
        .context("invalid regular expression")?;
    let filter = filter.map(glob_regex).transpose()?;
    let mut matches = Vec::new();
    let (mut files, mut truncated) = (0usize, false);
    let single = root.is_file();
    let entries: Box<dyn Iterator<Item = PathBuf>> = if single {
        Box::new(std::iter::once(root.to_path_buf()))
    } else {
        ensure!(root.is_dir(), "{} does not exist", root.display());
        Box::new(
            walk(root)
                .filter(|e| e.file_type().is_file())
                .map(|e| e.into_path()),
        )
    };
    'files: for path in entries {
        let rel = if single {
            path.to_string_lossy().into_owned()
        } else {
            relative(root, &path)
        };
        if let Some(f) = &filter
            && !f.is_match(&rel)
        {
            continue;
        }
        let Ok(m) = std::fs::metadata(&path) else {
            continue;
        };
        if m.len() > MAX_GREP_FILE {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if bytes.iter().take(8192).any(|b| *b == 0) {
            continue;
        }
        files += 1;
        let text = String::from_utf8_lossy(&bytes);
        for (n, line) in text.lines().enumerate() {
            if re.is_match(line) {
                if matches.len() >= max {
                    truncated = true;
                    break 'files;
                }
                let shown: String = line.chars().take(300).collect();
                matches.push(json!({"path":rel,"line":n+1,"text":shown}));
            }
        }
    }
    Ok(json!({"root":root,"matches":matches,"files_searched":files,"truncated":truncated}))
}
