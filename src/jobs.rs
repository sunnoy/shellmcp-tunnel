use crate::{invocation::Invocation, store::Store};
use anyhow::Result;
#[cfg(windows)]
use anyhow::ensure;
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
};
const MAX_LOG: u64 = 8 * 1024 * 1024;
fn job_dir(dir: &Path, id: &str) -> Result<std::path::PathBuf> {
    uuid::Uuid::parse_str(id)?;
    Ok(dir.join("jobs").join(id))
}
pub async fn start(dir: &Path, e: &Invocation, timeout: u64) -> Result<serde_json::Value> {
    garbage_collect(dir)?;
    let id = uuid::Uuid::new_v4().to_string();
    let jd = job_dir(dir, &id)?;
    crate::runtime::secure_dir(&jd)?;
    let request = serde_json::json!({"job_id":id,"invocation":e});
    crate::runtime::private_write(&jd.join("request.json"), &serde_json::to_vec(&request)?)?;
    let mut store = Store::open(dir)?;
    if let Err(err) = store.new_job(&id, &e.invocation_id) {
        std::fs::remove_dir_all(&jd)?;
        return Err(err);
    }
    if let Err(err) = crate::local::admission::bind_job(dir, &id, &request) {
        store.job_done(&id, -1)?;
        return Err(err);
    }
    let mut cmd = tokio::process::Command::new(std::env::current_exe()?);
    cmd.arg("local-job-supervisor")
        .arg("--job-id")
        .arg(&id)
        .arg("--data-dir")
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000 | 0x00000200);
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => {
            store.job_done(&id, -1)?;
            return Err(err.into());
        }
    };
    wait_for_job(dir, &id, store, timeout, child).await
}

async fn wait_for_job(
    dir: &Path,
    id: &str,
    store: Store,
    timeout: u64,
    mut child: tokio::process::Child,
) -> Result<serde_json::Value> {
    // The supervisor outlives this request on completion, timeout, error or
    // cancellation. Transfer ownership before polling so every path reaps it
    // without blocking the runtime or killing a detached background job.
    let (exit_tx, mut exit_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let status = child.wait().await;
        if let Err(err) = &status {
            tracing::warn!(error=%err, "failed to reap job supervisor");
        }
        let _ = exit_tx.send(status);
    });
    let until = std::time::Instant::now() + std::time::Duration::from_secs(timeout);
    loop {
        let status = match exit_rx.try_recv() {
            Ok(status) => Some(status?),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => None,
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                anyhow::bail!("job supervisor reaper stopped before reporting exit");
            }
        };
        // Read completion after observing exit: the supervisor may have committed
        // its result since the previous poll. Never overwrite that result with -1.
        let job = store.job(id)?;
        if job["state"] == "completed" {
            let output = output(dir, id, 0, 65536)?;
            return Ok(serde_json::json!({"job":job,"output":output}));
        }
        if let Some(status) = status {
            store.job_done(id, -1)?;
            anyhow::bail!("job supervisor exited before completion: {status}");
        }
        if std::time::Instant::now() >= until {
            return Ok(
                serde_json::json!({"job":job,"background":true,"message":"Wait timeout elapsed; execution continues. Use job_output or job_kill."}),
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

pub(crate) fn execute_command(dir: &Path, id: &str, a: &serde_json::Value) -> Result<()> {
    let jd = job_dir(dir, id)?;
    let command = a["command"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing command"))?;
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("powershell.exe");
        c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        c
    } else {
        let mut c = Command::new(crate::environment::exec_shell());
        c.args(["-c", command]);
        c.env_clear().envs(crate::environment::login_env(dir));
        c
    };
    cmd.current_dir(
        a["cwd"]
            .as_str()
            .map(crate::runtime::resolve)
            .unwrap_or_else(crate::runtime::home_dir),
    );
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut child = cmd.spawn()?;
    #[cfg(windows)]
    let _job = WindowsJob::assign(&child)?;
    let store = Store::open(dir)?;
    store.job_started(id, child.id())?;
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let count = std::sync::Arc::new(std::sync::Mutex::new(0u64));
    let file = std::sync::Arc::new(std::sync::Mutex::new(std::fs::File::create(
        jd.join("output.log"),
    )?));
    let a = drain(out, file.clone(), count.clone());
    let b = drain(err, file, count);
    loop {
        if jd.join("cancel").exists() {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            #[cfg(windows)]
            {
                child.kill()?;
            }
        }
        if let Some(exit) = child.try_wait()? {
            // Descendants holding inherited pipes must not keep a finished job forever.
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            #[cfg(windows)]
            drop(_job);
            a.join()
                .map_err(|_| anyhow::anyhow!("output drain panic"))??;
            b.join()
                .map_err(|_| anyhow::anyhow!("output drain panic"))??;
            store.job_done(id, exit.code().unwrap_or(-1))?;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Ok(())
}
fn drain<R: Read + Send + 'static>(
    mut pipe: R,
    file: std::sync::Arc<std::sync::Mutex<std::fs::File>>,
    count: std::sync::Arc<std::sync::Mutex<u64>>,
) -> std::thread::JoinHandle<Result<()>> {
    std::thread::spawn(move || {
        let mut buf = [0; 8192];
        loop {
            let n = pipe.read(&mut buf)?;
            if n == 0 {
                break;
            }
            let mut count = count.lock().unwrap();
            let keep = (MAX_LOG.saturating_sub(*count) as usize).min(n);
            if keep > 0 {
                file.lock().unwrap().write_all(&buf[..keep])?;
                *count += keep as u64;
            }
        }
        Ok(())
    })
}
pub fn output(dir: &Path, id: &str, offset: u64, limit: u64) -> Result<serde_json::Value> {
    let store = Store::open(dir)?;
    let job = store.job(id)?;
    use std::io::{Seek, SeekFrom};
    let path = job_dir(dir, id)?.join("output.log");
    let mut text = Vec::new();
    let mut total = 0;
    if path.exists() {
        let mut file = std::fs::File::open(path)?;
        total = file.metadata()?.len();
        file.seek(SeekFrom::Start(offset))?;
        file.take(limit.min(65536)).read_to_end(&mut text)?;
    }
    Ok(
        serde_json::json!({"job":job,"output":String::from_utf8_lossy(&text),"offset":offset,"next_offset":offset+text.len() as u64,"total_bytes":total,"truncated":total>=MAX_LOG}),
    )
}
pub fn kill(dir: &Path, id: &str) -> Result<serde_json::Value> {
    let job = Store::open(dir)?.job(id)?;
    if job["state"] != "completed" {
        crate::runtime::private_write(&job_dir(dir, id)?.join("cancel"), b"cancel")?;
    }
    Ok(serde_json::json!({"job_id":id,"cancel_requested":true}))
}
#[cfg(windows)]
pub(crate) struct WindowsJob(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl WindowsJob {
    fn assign(child: &std::process::Child) -> Result<Self> {
        use std::os::windows::io::AsRawHandle;
        Self::assign_handle(child.as_raw_handle() as _)
    }
    pub(crate) fn assign_handle(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            ensure!(!h.is_null(), "CreateJobObject failed");
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
                || AssignProcessToJobObject(h, handle) == 0
            {
                windows_sys::Win32::Foundation::CloseHandle(h);
                anyhow::bail!("cannot contain Windows process tree");
            }
            Ok(Self(h))
        }
    }
}
#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

pub fn garbage_collect(dir: &Path) -> Result<()> {
    let store = Store::open(dir)?;
    let jobs = store.completed_jobs()?;
    let mut bytes = 0u64;
    for entry in std::fs::read_dir(dir.join("jobs"))
        .into_iter()
        .flatten()
        .flatten()
    {
        if let Ok(m) = std::fs::metadata(entry.path().join("output.log")) {
            bytes += m.len();
        }
    }
    let mut count = jobs.len();
    for (id, created) in jobs {
        if crate::runtime::now().saturating_sub(created) > 7 * 86400
            || bytes > 256 * 1024 * 1024
            || count > 500
        {
            let path = job_dir(dir, &id)?;
            let size = std::fs::metadata(path.join("output.log"))
                .map(|m| m.len())
                .unwrap_or(0);
            if path.exists() {
                std::fs::remove_dir_all(path)?;
            }
            store.delete_job(&id)?;
            bytes = bytes.saturating_sub(size);
            count = count.saturating_sub(1);
        }
    }
    Ok(())
}
