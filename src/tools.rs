use crate::invocation::Invocation;
use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};
fn number(a: &serde_json::Value, name: &str, default: u64, max: u64) -> Result<u64> {
    let n = match a.get(name) {
        Some(v) => v
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("{name} must be nonnegative integer"))?,
        None => default,
    };
    ensure!(n <= max, "{name} exceeds {max}");
    Ok(n)
}
fn string<'a>(a: &'a serde_json::Value, name: &str) -> Result<&'a str> {
    a.get(name)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing string {name}"))
}
pub async fn execute_invocation(dir: &Path, e: &Invocation) -> Result<serde_json::Value> {
    let started = std::time::Instant::now();
    let outcome = run(dir, e).await;
    let (result, error) = match outcome {
        Ok(v) => {
            let failed =
                e.tool == "shell_exec" && v["job"]["exit_code"].as_i64().is_some_and(|n| n != 0);
            (
                crate::invocation::result(serde_json::to_string(&v)?, failed),
                None,
            )
        }
        Err(err) => (
            crate::invocation::result(err.to_string(), true),
            Some(err.to_string()),
        ),
    };
    use sha2::Digest;
    crate::policy::audit(
        dir,
        serde_json::json!({"ts":crate::runtime::now(),"invocation_id":e.invocation_id,"tool":e.tool,
            "arguments_sha256":hex::encode(sha2::Sha256::digest(e.arguments_json.as_bytes())),
            "error":error,"duration_ms":started.elapsed().as_millis() as u64}),
    );
    Ok(result)
}

fn path(dir: &Path, a: &serde_json::Value, name: &str) -> Result<PathBuf> {
    let p = crate::runtime::resolve(string(a, name)?);
    crate::policy::guard_path(dir, &p)?;
    Ok(p)
}

async fn run(dir: &Path, e: &Invocation) -> Result<serde_json::Value> {
    let a: serde_json::Value = serde_json::from_str(&e.arguments_json)?;
    crate::catalogue::validate(&e.tool, &a)?;
    let policy = crate::policy::load_checked(dir)?;
    crate::policy::check(&policy, &e.tool)?;
    let max = |d: u64| number(&a, "max_results", d, 1000).map(|n| n as usize);
    let root = || -> Result<PathBuf> {
        match a.get("path") {
            Some(_) => path(dir, &a, "path"),
            None => Ok(crate::runtime::home_dir()),
        }
    };
    Ok(match e.tool.as_str() {
        "get_environment" => serde_json::json!({
            "hostname":crate::runtime::hostname(),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,
            "version":env!("CARGO_PKG_VERSION"),"mode":"local",
            "shell":crate::environment::exec_shell(),"login_shell":crate::environment::login_shell(),
            "home":crate::runtime::home_dir(),"user":std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default(),
            "policy":policy,"allowed_tools":crate::policy::allowed_tools(&policy),
            "path_rules":"Relative paths and ~ resolve against home. The agent data directory is not accessible."}),
        "fs_list" => crate::fsops::list(&path(dir, &a, "path")?)?,
        "fs_read" => {
            let p = path(dir, &a, "path")?;
            let mut v = read_utf8(
                &p,
                number(&a, "offset", 0, i64::MAX as u64)?,
                number(&a, "limit", 65536, 65536)?,
            )?;
            v["path"] = serde_json::json!(p);
            v
        }
        "fs_read_many" => {
            let mut paths = Vec::new();
            for v in a["paths"].as_array().into_iter().flatten() {
                let p = crate::runtime::resolve(v.as_str().unwrap_or_default());
                crate::policy::guard_path(dir, &p)?;
                paths.push(p);
            }
            crate::fsops::read_many(&paths, number(&a, "limit", 16384, 32768)?)
        }
        "fs_info" => crate::fsops::info(&path(dir, &a, "path")?)?,
        "fs_glob" => crate::fsops::glob(&root()?, string(&a, "pattern")?, max(200)?)?,
        "fs_grep" => crate::fsops::grep(
            &root()?,
            string(&a, "pattern")?,
            a["glob"].as_str(),
            a["ignore_case"].as_bool().unwrap_or(false),
            max(200)?,
        )?,
        "fs_write" => {
            crate::fsops::write_file(&path(dir, &a, "path")?, string(&a, "content")?.as_bytes())?
        }
        "fs_edit" => crate::fsops::edit(
            &path(dir, &a, "path")?,
            string(&a, "old_string")?,
            string(&a, "new_string")?,
            a["replace_all"].as_bool().unwrap_or(false),
        )?,
        "fs_move" => {
            crate::fsops::rename(&path(dir, &a, "source")?, &path(dir, &a, "destination")?)?
        }
        "fs_mkdir" => crate::fsops::mkdir(&path(dir, &a, "path")?)?,
        "shell_exec" => {
            if a.get("cwd").is_some() {
                let cwd = path(dir, &a, "cwd")?;
                ensure!(cwd.is_dir(), "cwd {} is not a directory", cwd.display());
            }
            let timeout = number(&a, "timeout_s", 30, 30)?;
            ensure!(timeout >= 1, "timeout_s must be >=1");
            crate::jobs::start(dir, e, timeout).await?
        }
        "job_list" => serde_json::json!({"jobs":crate::store::Store::open(dir)?.list_jobs()?}),
        "job_output" => crate::jobs::output(
            dir,
            string(&a, "job_id")?,
            number(&a, "offset", 0, i64::MAX as u64)?,
            number(&a, "limit", 65536, 65536)?,
        )?,
        "job_kill" => crate::jobs::kill(dir, string(&a, "job_id")?)?,
        _ => anyhow::bail!("unknown tool"),
    })
}

pub fn read_utf8(path: &Path, offset: u64, limit: u64) -> Result<serde_json::Value> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    ensure!(file.metadata()?.is_file(), "not a regular file");
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(limit + 3).read_to_end(&mut bytes)?;
    let mut start = 0;
    while start < bytes.len() && (bytes[start] & 0xc0) == 0x80 {
        start += 1;
    }
    ensure!(start <= 3, "invalid UTF-8 file");
    let mut end = (limit as usize).min(bytes.len()).max(start);
    while end < bytes.len() && (bytes[end] & 0xc0) == 0x80 {
        end += 1;
    }
    let content = std::str::from_utf8(&bytes[start..end])?;
    // Byte offsets advance to whole UTF-8 characters. A page can extend by at most 3
    // bytes so pagination never loses or replaces a multibyte character.
    Ok(
        serde_json::json!({"content":content,"offset":offset+start as u64,"next_offset":offset+end as u64}),
    )
}
